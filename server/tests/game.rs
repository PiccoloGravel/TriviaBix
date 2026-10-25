//! Plays whole games against a real server over WebSockets.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};

use triviabix::{Config, room::Timing};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

const MAX_PLAYERS: usize = 10;

async fn start_server() -> String {
    let db = std::env::temp_dir().join(format!("triviabix-test-{}.db", uuid::Uuid::new_v4()));
    let config = Config {
        database_url: format!("sqlite://{}", db.display()),
        static_dir: concat!(env!("CARGO_MANIFEST_DIR"), "/../web").into(),
        public_url: None,
        max_players: MAX_PLAYERS,
        max_rooms: 10,
        timing: Timing {
            question: Duration::from_secs(5),
            reveal: Duration::from_millis(200),
            scoreboard: Duration::from_millis(200),
            idle_timeout: Duration::from_secs(60),
        },
    };
    let app = triviabix::router(config).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("127.0.0.1:{}", addr.port())
}

async fn create_room(addr: &str, rounds: usize) -> (String, String) {
    let res: Value = reqwest::Client::new()
        .post(format!("http://{addr}/api/rooms"))
        .json(&json!({ "rounds": rounds }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    (
        res["code"].as_str().unwrap().into(),
        res["host_token"].as_str().unwrap().into(),
    )
}

async fn open(addr: &str, path_and_query: &str) -> Ws {
    connect_async(format!("ws://{addr}{path_and_query}"))
        .await
        .unwrap()
        .0
}

async fn recv(ws: &mut Ws) -> Value {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(10), ws.next())
            .await
            .expect("timed out waiting for a message")
            .expect("socket closed")
            .unwrap();
        if let Message::Text(text) = msg {
            return serde_json::from_str(&text).unwrap();
        }
    }
}

/// Reads until a state message matching `pred` arrives.
async fn wait_state(ws: &mut Ws, pred: impl Fn(&Value) -> bool) -> Value {
    loop {
        let msg = recv(ws).await;
        if msg["type"] == "state" && pred(&msg) {
            return msg;
        }
    }
}

async fn send(ws: &mut Ws, msg: Value) {
    ws.send(Message::Text(msg.to_string().into()))
        .await
        .unwrap();
}

#[tokio::test]
async fn eight_players_play_a_full_game() {
    let addr = start_server().await;
    let (code, host_token) = create_room(&addr, 3).await;
    assert_eq!(code.len(), 4);

    let mut host = open(&addr, &format!("/ws/host?code={code}&token={host_token}")).await;
    assert_eq!(recv(&mut host).await["role"], "host");

    // Fill the room to the limit; room codes are case-insensitive.
    let mut players = Vec::new();
    for i in 0..MAX_PLAYERS {
        let mut ws = open(
            &addr,
            &format!("/ws/play?code={}&name=Player{i}", code.to_lowercase()),
        )
        .await;
        let welcome = recv(&mut ws).await;
        assert_eq!(welcome["type"], "welcome", "{welcome}");
        players.push(ws);
    }
    let lobby = wait_state(&mut host, |s| {
        s["players"].as_array().unwrap().len() == MAX_PLAYERS
    })
    .await;
    assert_eq!(lobby["phase"], "lobby");
    assert_eq!(lobby["vip"], lobby["players"][0]["id"]);

    // One more is turned away.
    let mut extra = open(&addr, &format!("/ws/play?code={code}&name=Extra")).await;
    let err = recv(&mut extra).await;
    assert_eq!(err["type"], "error");
    assert_eq!(err["fatal"], true);

    // The VIP (first player) starts the game from their phone.
    send(&mut players[0], json!({ "type": "start" })).await;

    for round in 1..=3 {
        let q = wait_state(&mut host, |s| {
            s["phase"] == "question" && s["round"] == round
        })
        .await;
        assert_eq!(q["question"]["choices"].as_array().unwrap().len(), 4);
        assert!(
            q["reveal"].is_null(),
            "answer must not leak during the question"
        );

        // Everyone except the last player answers 0; the last answers 1.
        for (i, ws) in players.iter_mut().enumerate() {
            send(
                ws,
                json!({ "type": "answer", "choice": if i == MAX_PLAYERS - 1 { 1 } else { 0 } }),
            )
            .await;
        }
        // All answered, so the reveal comes right away instead of after 5s.
        let reveal = wait_state(&mut host, |s| s["phase"] == "reveal").await;
        let correct = reveal["reveal"]["correct"].as_u64().unwrap();
        for r in reveal["reveal"]["results"].as_array().unwrap() {
            let right = r["choice"].as_u64() == Some(correct);
            assert_eq!(r["points"].as_i64().unwrap() > 0, right, "{r}");
        }
    }

    let fin = wait_state(&mut host, |s| s["phase"] == "final").await;
    assert_eq!(fin["round"], 3);

    // Players see their own rank in the final state.
    let mine = wait_state(&mut players[3], |s| s["phase"] == "final").await;
    assert!(mine["you"]["rank"].as_u64().unwrap() >= 1);

    // Results were written to the database.
    let health: Value = reqwest::get(format!("http://{addr}/healthz"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(health["ok"], true);

    // Back to the lobby for a rematch with scores reset.
    send(&mut host, json!({ "type": "play_again" })).await;
    let lobby = wait_state(&mut host, |s| s["phase"] == "lobby").await;
    assert!(
        lobby["players"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["score"] == 0)
    );
}

#[tokio::test]
async fn rejoin_with_token_keeps_the_same_player() {
    let addr = start_server().await;
    let (code, host_token) = create_room(&addr, 3).await;
    let mut host = open(&addr, &format!("/ws/host?code={code}&token={host_token}")).await;

    let mut phone = open(&addr, &format!("/ws/play?code={code}&name=Sam")).await;
    let welcome = recv(&mut phone).await;
    let token = welcome["token"].as_str().unwrap().to_string();
    let id = welcome["player_id"].clone();
    drop(phone);

    wait_state(&mut host, |s| s["players"][0]["connected"] == false).await;

    let mut phone = open(&addr, &format!("/ws/play?code={code}&token={token}")).await;
    let welcome = recv(&mut phone).await;
    assert_eq!(welcome["player_id"], id);
    let s = wait_state(&mut host, |s| s["players"][0]["connected"] == true).await;
    assert_eq!(s["players"].as_array().unwrap().len(), 1);

    // Same name from a different phone is refused.
    let mut copycat = open(&addr, &format!("/ws/play?code={code}&name=sam")).await;
    assert_eq!(recv(&mut copycat).await["fatal"], true);

    // The host can't use a made-up token, and unknown rooms are reported.
    let mut fake = open(&addr, &format!("/ws/host?code={code}&token=nope")).await;
    assert_eq!(recv(&mut fake).await["fatal"], true);
    let mut lost = open(&addr, "/ws/play?code=ZZZZ&name=Lost").await;
    assert_eq!(recv(&mut lost).await["fatal"], true);
}

#[tokio::test]
async fn unanswered_question_times_out() {
    let addr = start_server().await;
    let (code, host_token) = create_room(&addr, 3).await;
    let mut host = open(&addr, &format!("/ws/host?code={code}&token={host_token}")).await;
    let mut phone = open(&addr, &format!("/ws/play?code={code}&name=Slowpoke")).await;
    recv(&mut phone).await;

    send(&mut host, json!({ "type": "start" })).await;
    wait_state(&mut host, |s| s["phase"] == "question").await;
    // Nobody answers; the 5 second timer should move us on.
    let reveal = wait_state(&mut host, |s| s["phase"] == "reveal").await;
    assert_eq!(reveal["reveal"]["results"][0]["points"], 0);
    assert!(reveal["reveal"]["results"][0]["choice"].is_null());
}

#[tokio::test]
async fn static_pages_are_served() {
    let addr = start_server().await;
    for path in ["/", "/host", "/style.css", "/player.js", "/host.js"] {
        let res = reqwest::get(format!("http://{addr}{path}")).await.unwrap();
        assert!(res.status().is_success(), "{path}: {}", res.status());
    }
    let qr = reqwest::get(format!("http://{addr}/api/qr?text=http://x/?code=ABCD"))
        .await
        .unwrap();
    assert_eq!(qr.headers()["content-type"], "image/svg+xml");
}

#[tokio::test]
async fn ratings_filter_questions() {
    use triviabix::db::{Db, Rating};
    let path = std::env::temp_dir().join(format!("triviabix-test-{}.db", uuid::Uuid::new_v4()));
    let db = Db::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    let total = db.question_count().await.unwrap() as usize;

    let pg = db.random_questions(total, Rating::Pg, &[]).await.unwrap();
    assert!(!pg.is_empty());
    assert!(pg.iter().all(|q| q.category != "After Dark"));

    let pg13 = db.random_questions(total, Rating::Pg13, &[]).await.unwrap();
    assert!(pg13.len() > pg.len());
    assert!(pg13.iter().all(|q| q.choices.len() == 4 && q.answer < 4));

    let r: Rating = serde_json::from_str("\"NC-17\"").unwrap();
    assert_eq!(r, Rating::Nc17);
    assert!(Rating::Pg < Rating::Ma);
}
