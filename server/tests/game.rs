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
    start_server_with_db().await.0
}

/// Also returns the database URL, for tests that need to control the content.
async fn start_server_with_db() -> (String, String) {
    let db = std::env::temp_dir().join(format!("triviabix-test-{}.db", uuid::Uuid::new_v4()));
    let database_url = format!("sqlite://{}", db.display());
    let config = Config {
        database_url: database_url.clone(),
        static_dir: concat!(env!("CARGO_MANIFEST_DIR"), "/../web").into(),
        public_url: None,
        max_players: MAX_PLAYERS,
        max_rooms: 10,
        timing: Timing {
            question: Duration::from_secs(5),
            hangman: Duration::from_secs(5),
            reveal: Duration::from_millis(200),
            scoreboard: Duration::from_millis(200),
            intro: Duration::from_millis(200),
            talk_cps: 0.0,
            idle_timeout: Duration::from_secs(60),
        },
        host_script: None,
    };
    let app = triviabix::router(config).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("127.0.0.1:{}", addr.port()), database_url)
}

async fn create_room(addr: &str, rounds: usize) -> (String, String) {
    create_room_with(addr, json!({ "rounds": rounds })).await
}

async fn create_room_with(addr: &str, body: Value) -> (String, String) {
    let res: Value = reqwest::Client::new()
        .post(format!("http://{addr}/api/rooms"))
        .json(&body)
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

    // The host opens the show before the first question.
    let intro = wait_state(&mut host, |s| s["phase"] == "intro").await;
    assert!(
        intro["host"]["text"].as_str().unwrap().len() > 10,
        "{intro}"
    );

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
        // The host announces the answer and reacts, with graphics for the TV.
        let answer = reveal["question"]["choices"][correct as usize]
            .as_str()
            .unwrap();
        let said = reveal["host"]["text"].as_str().unwrap();
        assert!(said.contains(answer), "host should say the answer: {said}");
        assert!(!said.contains('{'), "unfilled placeholder: {said}");
        assert!(!reveal["host"]["callouts"].as_array().unwrap().is_empty());
    }

    let fin = wait_state(&mut host, |s| s["phase"] == "final").await;
    assert_eq!(fin["round"], 3);
    assert!(!fin["host"]["text"].as_str().unwrap().is_empty());

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

/// Starts a speed hangman room whose only puzzle is "Hat Trick", with `n` players.
async fn hangman_room(n: usize) -> (Ws, Vec<Ws>) {
    let (addr, database_url) = start_server_with_db().await;
    let pool = sqlx::SqlitePool::connect(&database_url).await.unwrap();
    sqlx::query("DELETE FROM hangman_phrases WHERE phrase <> 'Hat Trick'")
        .execute(&pool)
        .await
        .unwrap();

    let (code, host_token) =
        create_room_with(&addr, json!({ "rounds": 3, "mode": "hangman" })).await;
    let mut host = open(&addr, &format!("/ws/host?code={code}&token={host_token}")).await;
    let mut players = Vec::new();
    for i in 0..n {
        let mut ws = open(&addr, &format!("/ws/play?code={code}&name=P{i}")).await;
        recv(&mut ws).await;
        players.push(ws);
    }
    let lobby = wait_state(&mut host, |s| s["players"].as_array().unwrap().len() == n).await;
    assert_eq!(lobby["settings"]["mode"], "hangman");
    send(&mut host, json!({ "type": "start" })).await;
    (host, players)
}

async fn next_error(ws: &mut Ws) -> Value {
    loop {
        let msg = recv(ws).await;
        if msg["type"] == "error" {
            return msg;
        }
    }
}

#[tokio::test]
async fn speed_hangman_letters_strikes_and_solve() {
    let (mut host, mut players) = hangman_room(2).await;

    let q = wait_state(&mut host, |s| s["phase"] == "question").await;
    // There was only one puzzle to load, so the game is one round long.
    assert_eq!(q["total_rounds"], 1);
    assert!(q["question"].is_null());
    assert_eq!(
        q["hangman"]["board"], "___ _____",
        "the phrase must not leak"
    );
    assert_eq!(q["hangman"]["category"], "Sports");

    let ids: Vec<Value> = q["players"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].clone())
        .collect();

    // A hit reveals every T and pays per tile.
    send(&mut players[0], json!({ "type": "guess", "letter": "t" })).await;
    let s = wait_state(&mut host, |s| s["hangman"]["hits"] == "T").await;
    assert_eq!(s["hangman"]["board"], "__t T____");
    assert_eq!(s["players"][0]["score"], 150);

    // Guessing again straight away is too fast, and taken letters are refused.
    send(&mut players[0], json!({ "type": "guess", "letter": "z" })).await;
    assert_eq!(next_error(&mut players[0]).await["fatal"], false);
    send(&mut players[1], json!({ "type": "guess", "letter": "T" })).await;
    assert!(
        next_error(&mut players[1]).await["message"]
            .as_str()
            .unwrap()
            .contains("taken")
    );

    // A miss costs points and a strike for the whole room.
    send(&mut players[1], json!({ "type": "guess", "letter": "z" })).await;
    let s = wait_state(&mut host, |s| s["hangman"]["strikes"] == 1).await;
    assert_eq!(s["hangman"]["misses"], "Z");
    assert_eq!(s["players"][1]["score"], -50);

    // A wrong solve is another strike and locks that player out of solving.
    send(
        &mut players[1],
        json!({ "type": "solve", "text": "hat trik" }),
    )
    .await;
    let mine = wait_state(&mut players[1], |s| s["hangman"]["strikes"] == 2).await;
    assert_eq!(mine["you"]["hangman"]["solve_locked"], true);
    send(
        &mut players[1],
        json!({ "type": "solve", "text": "hat trick" }),
    )
    .await;
    assert_eq!(next_error(&mut players[1]).await["fatal"], false);

    // The right answer ends the round at once and shows the phrase.
    send(
        &mut players[0],
        json!({ "type": "solve", "text": "HAT-TRICK" }),
    )
    .await;
    let r = wait_state(&mut host, |s| s["phase"] == "reveal").await;
    assert_eq!(r["hangman"]["outcome"], "solved");
    assert_eq!(r["hangman"]["solved_by"], ids[0]);
    assert_eq!(r["hangman"]["board"], "Hat Trick");
    // 150 for the Ts, then 250 + 50 for each of the 7 blanks + a speed bonus.
    let score = r["players"][0]["score"].as_i64().unwrap();
    assert!(score > 150 + 250 + 7 * 50, "{score}");
    assert_eq!(r["hangman"]["results"][0]["points"], score);

    wait_state(&mut host, |s| s["phase"] == "final").await;
}

#[tokio::test]
async fn speed_hangman_six_misses_hangs_everyone() {
    let (mut host, mut players) = hangman_room(6).await;
    wait_state(&mut host, |s| s["phase"] == "question").await;
    for (ws, letter) in players.iter_mut().zip(["B", "D", "E", "F", "G", "J"]) {
        send(ws, json!({ "type": "guess", "letter": letter })).await;
    }
    let r = wait_state(&mut host, |s| s["phase"] == "reveal").await;
    assert_eq!(r["hangman"]["outcome"], "hanged");
    assert_eq!(r["hangman"]["strikes"], 6);
    assert!(r["hangman"]["solved_by"].is_null());
    assert_eq!(r["hangman"]["board"], "Hat Trick");

    // Trivia answers are refused in a hangman game.
    send(&mut players[0], json!({ "type": "answer", "choice": 0 })).await;
    assert_eq!(next_error(&mut players[0]).await["fatal"], false);
}

#[tokio::test]
async fn players_pick_unique_characters_and_names() {
    let addr = start_server().await;
    let (code, host_token) = create_room(&addr, 3).await;
    let mut host = open(&addr, &format!("/ws/host?code={code}&token={host_token}")).await;

    let chars: Value = reqwest::get(format!("http://{addr}/api/characters"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(chars.as_array().unwrap().len() >= MAX_PLAYERS);

    let mut fox = open(
        &addr,
        &format!("/ws/play?code={code}&name=Ana&character=fox"),
    )
    .await;
    recv(&mut fox).await;
    let s = wait_state(&mut host, |s| s["players"].as_array().unwrap().len() == 1).await;
    assert_eq!(s["players"][0]["character"], "fox");

    // The fox is taken, and made-up characters are refused.
    let mut copy = open(
        &addr,
        &format!("/ws/play?code={code}&name=Ben&character=fox"),
    )
    .await;
    assert_eq!(recv(&mut copy).await["fatal"], true);
    let mut fake = open(
        &addr,
        &format!("/ws/play?code={code}&name=Ben&character=dragon-king"),
    )
    .await;
    assert_eq!(recv(&mut fake).await["fatal"], true);

    // No pick gets a random free character.
    let mut ben = open(&addr, &format!("/ws/play?code={code}&name=Ben")).await;
    recv(&mut ben).await;
    let s = wait_state(&mut host, |s| s["players"].as_array().unwrap().len() == 2).await;
    let bens = s["players"][1]["character"].as_str().unwrap().to_string();
    assert!(!bens.is_empty() && bens != "fox");

    // The room info lists taken characters for the join screen.
    let info: Value = reqwest::get(format!("http://{addr}/api/rooms/{code}"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info["taken_characters"].as_array().unwrap().len(), 2);

    // Changing your look in the lobby.
    send(&mut ben, json!({ "type": "profile", "character": "fox" })).await;
    let err = loop {
        let m = recv(&mut ben).await;
        if m["type"] == "error" {
            break m;
        }
    };
    assert_eq!(err["fatal"], false);
    send(
        &mut ben,
        json!({ "type": "profile", "character": "robot", "name": "Big Ben" }),
    )
    .await;
    let s = wait_state(&mut host, |s| s["players"][1]["character"] == "robot").await;
    assert_eq!(s["players"][1]["name"], "Big Ben");
    send(&mut ben, json!({ "type": "profile", "name": "ana" })).await;
    let err = loop {
        let m = recv(&mut ben).await;
        if m["type"] == "error" {
            break m;
        }
    };
    assert!(err["message"].as_str().unwrap().contains("already called"));
}
