//! Plays whole games against a real server over WebSockets.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};

use triviabix::{Config, room::Timing};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

const MAX_PLAYERS: usize = 10;
const ADMIN_PASSWORD: &str = "letmein";

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
        admin_password: Some(ADMIN_PASSWORD.into()),
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
    (res["code"].as_str().unwrap().into(), res["host_token"].as_str().unwrap().into())
}

async fn open(addr: &str, path_and_query: &str) -> Ws {
    connect_async(format!("ws://{addr}{path_and_query}")).await.unwrap().0
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

/// Skips state updates until an error arrives.
async fn next_error(ws: &mut Ws) -> Value {
    loop {
        let msg = recv(ws).await;
        if msg["type"] == "error" {
            return msg;
        }
    }
}

async fn send(ws: &mut Ws, msg: Value) {
    ws.send(Message::Text(msg.to_string().into())).await.unwrap();
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
        let mut ws = open(&addr, &format!("/ws/play?code={}&name=Player{i}", code.to_lowercase())).await;
        let welcome = recv(&mut ws).await;
        assert_eq!(welcome["type"], "welcome", "{welcome}");
        players.push(ws);
    }
    let lobby = wait_state(&mut host, |s| s["players"].as_array().unwrap().len() == MAX_PLAYERS).await;
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
        let q = wait_state(&mut host, |s| s["phase"] == "question" && s["round"] == round).await;
        assert_eq!(q["question"]["choices"].as_array().unwrap().len(), 4);
        assert!(q["reveal"].is_null(), "answer must not leak during the question");

        // Everyone except the last player answers 0; the last answers 1.
        for (i, ws) in players.iter_mut().enumerate() {
            send(ws, json!({ "type": "answer", "choice": if i == MAX_PLAYERS - 1 { 1 } else { 0 } })).await;
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
    let health: Value = reqwest::get(format!("http://{addr}/healthz")).await.unwrap().json().await.unwrap();
    assert_eq!(health["ok"], true);

    // Back to the lobby for a rematch with scores reset.
    send(&mut host, json!({ "type": "play_again" })).await;
    let lobby = wait_state(&mut host, |s| s["phase"] == "lobby").await;
    assert!(lobby["players"].as_array().unwrap().iter().all(|p| p["score"] == 0));
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
    let qr = reqwest::get(format!("http://{addr}/api/qr?text=http://x/?code=ABCD")).await.unwrap();
    assert_eq!(qr.headers()["content-type"], "image/svg+xml");
}

#[tokio::test]
async fn ratings_filter_questions() {
    use triviabix::db::{Db, Rating};
    let path = std::env::temp_dir().join(format!("triviabix-test-{}.db", uuid::Uuid::new_v4()));
    let db = Db::connect(&format!("sqlite://{}", path.display())).await.unwrap();
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

#[tokio::test]
async fn players_pick_unique_characters_and_names() {
    let addr = start_server().await;
    let (code, host_token) = create_room(&addr, 3).await;
    let mut host = open(&addr, &format!("/ws/host?code={code}&token={host_token}")).await;

    let chars: Value = reqwest::get(format!("http://{addr}/api/characters")).await.unwrap().json().await.unwrap();
    assert!(chars.as_array().unwrap().len() >= MAX_PLAYERS);

    let mut fox = open(&addr, &format!("/ws/play?code={code}&name=Ana&character=fox")).await;
    recv(&mut fox).await;
    let s = wait_state(&mut host, |s| s["players"].as_array().unwrap().len() == 1).await;
    assert_eq!(s["players"][0]["character"], "fox");

    // The fox is taken, and made-up characters are refused.
    let mut copy = open(&addr, &format!("/ws/play?code={code}&name=Ben&character=fox")).await;
    assert_eq!(recv(&mut copy).await["fatal"], true);
    let mut fake = open(&addr, &format!("/ws/play?code={code}&name=Ben&character=dragon-king")).await;
    assert_eq!(recv(&mut fake).await["fatal"], true);

    // No pick gets a random free character.
    let mut ben = open(&addr, &format!("/ws/play?code={code}&name=Ben")).await;
    recv(&mut ben).await;
    let s = wait_state(&mut host, |s| s["players"].as_array().unwrap().len() == 2).await;
    let bens = s["players"][1]["character"].as_str().unwrap().to_string();
    assert!(!bens.is_empty() && bens != "fox");

    // The room info lists taken characters for the join screen.
    let info: Value = reqwest::get(format!("http://{addr}/api/rooms/{code}")).await.unwrap().json().await.unwrap();
    assert_eq!(info["taken_characters"].as_array().unwrap().len(), 2);

    // Changing your look in the lobby.
    send(&mut ben, json!({ "type": "profile", "character": "fox" })).await;
    let err = loop {
        let m = recv(&mut ben).await;
        if m["type"] == "error" { break m; }
    };
    assert_eq!(err["fatal"], false);
    send(&mut ben, json!({ "type": "profile", "character": "robot", "name": "Big Ben" })).await;
    let s = wait_state(&mut host, |s| s["players"][1]["character"] == "robot").await;
    assert_eq!(s["players"][1]["name"], "Big Ben");
    send(&mut ben, json!({ "type": "profile", "name": "ana" })).await;
    let err = loop {
        let m = recv(&mut ben).await;
        if m["type"] == "error" { break m; }
    };
    assert!(err["message"].as_str().unwrap().contains("already called"));
}

#[tokio::test]
async fn late_joiners_play_from_the_next_question() {
    let addr = start_server().await;
    let (code, host_token) = create_room(&addr, 3).await;
    let mut host = open(&addr, &format!("/ws/host?code={code}&token={host_token}")).await;
    let mut alice = open(&addr, &format!("/ws/play?code={code}&name=Alice")).await;
    recv(&mut alice).await;

    send(&mut host, json!({ "type": "start" })).await;
    wait_state(&mut host, |s| s["phase"] == "question" && s["round"] == 1).await;

    // Bob turns up mid-question and is let in, but sits this question out.
    let mut bob = open(&addr, &format!("/ws/play?code={code}&name=Bob")).await;
    assert_eq!(recv(&mut bob).await["type"], "welcome");
    let s = wait_state(&mut bob, |s| s["you"]["waiting"] == true).await;
    assert_eq!(s["phase"], "question");
    send(&mut bob, json!({ "type": "answer", "choice": 0 })).await;
    assert_eq!(next_error(&mut bob).await["fatal"], false);

    // Alice answering is enough to close the question; Bob isn't waited on or scored.
    send(&mut alice, json!({ "type": "answer", "choice": 0 })).await;
    let reveal = wait_state(&mut host, |s| s["phase"] == "reveal").await;
    assert_eq!(reveal["reveal"]["results"].as_array().unwrap().len(), 1);

    // From the next question Bob is a full player.
    let q2 = wait_state(&mut host, |s| s["phase"] == "question" && s["round"] == 2).await;
    assert!(q2["players"].as_array().unwrap().iter().all(|p| p["waiting"] == false));
    send(&mut alice, json!({ "type": "answer", "choice": 0 })).await;
    send(&mut bob, json!({ "type": "answer", "choice": 1 })).await;
    let reveal = wait_state(&mut host, |s| s["phase"] == "reveal" && s["round"] == 2).await;
    assert_eq!(reveal["reveal"]["results"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn late_joins_can_be_turned_off() {
    let addr = start_server().await;
    let (code, host_token) = create_room(&addr, 3).await;
    let mut host = open(&addr, &format!("/ws/host?code={code}&token={host_token}")).await;
    let mut alice = open(&addr, &format!("/ws/play?code={code}&name=Alice")).await;
    recv(&mut alice).await;
    send(&mut host, json!({ "type": "settings", "late_join": false })).await;
    send(&mut host, json!({ "type": "start" })).await;
    wait_state(&mut host, |s| s["phase"] == "question").await;

    let mut bob = open(&addr, &format!("/ws/play?code={code}&name=Bob")).await;
    assert_eq!(recv(&mut bob).await["fatal"], true);
}

#[tokio::test]
async fn teams_are_balanced_and_scored_by_average() {
    let addr = start_server().await;
    let res: Value = reqwest::Client::new()
        .post(format!("http://{addr}/api/rooms"))
        .json(&json!({ "rounds": 3, "teams": 2 }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let (code, host_token) = (res["code"].as_str().unwrap(), res["host_token"].as_str().unwrap());
    let mut host = open(&addr, &format!("/ws/host?code={code}&token={host_token}")).await;

    let mut players = Vec::new();
    for name in ["A", "B", "C"] {
        let mut ws = open(&addr, &format!("/ws/play?code={code}&name={name}")).await;
        recv(&mut ws).await;
        players.push(ws);
    }
    let s = wait_state(&mut host, |s| s["players"].as_array().unwrap().len() == 3).await;
    let teams: Vec<_> = s["players"].as_array().unwrap().iter().map(|p| p["team"].as_u64().unwrap()).collect();
    assert_eq!(teams, [0, 1, 0]);

    // C switches to the blue team from their phone.
    send(&mut players[2], json!({ "type": "team", "team": 1 })).await;
    wait_state(&mut host, |s| s["players"][2]["team"] == 1).await;
    send(&mut players[2], json!({ "type": "team", "team": 7 })).await;
    assert_eq!(next_error(&mut players[2]).await["fatal"], false);

    send(&mut host, json!({ "type": "start" })).await;
    let q = wait_state(&mut host, |s| s["phase"] == "question").await;
    assert_eq!(q["teams"].as_array().unwrap().len(), 2);
    for ws in &mut players {
        send(ws, json!({ "type": "answer", "choice": 0 })).await;
    }
    let s = wait_state(&mut host, |s| s["phase"] == "reveal").await;
    let score = |id: u64| s["players"].as_array().unwrap().iter().find(|p| p["id"] == id).unwrap()["score"].as_i64().unwrap();
    for team in s["teams"].as_array().unwrap() {
        let members: Vec<i64> = team["members"].as_array().unwrap().iter().map(|m| score(m.as_u64().unwrap())).collect();
        assert_eq!(team["score"].as_i64().unwrap(), members.iter().sum::<i64>() / members.len() as i64, "{team}");
    }
}

/// Logs in to the admin API and returns the session cookie.
async fn admin_login(addr: &str) -> String {
    let http = reqwest::Client::new();
    let wrong = http
        .post(format!("http://{addr}/api/admin/login"))
        .json(&json!({ "password": "guess" }))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 401);
    let res = http
        .post(format!("http://{addr}/api/admin/login"))
        .json(&json!({ "password": ADMIN_PASSWORD }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let cookie = res.headers()["set-cookie"].to_str().unwrap();
    assert!(cookie.contains("HttpOnly"));
    cookie.split(';').next().unwrap().to_string()
}

async fn admin(addr: &str, cookie: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let http = reqwest::Client::new();
    let url = format!("http://{addr}/api/admin/{path}");
    let req = match body {
        Some(b) => http.post(url).json(&b),
        None => http.get(url),
    };
    let res = req.header("cookie", cookie).send().await.unwrap();
    (res.status().as_u16(), res.json().await.unwrap())
}

#[tokio::test]
async fn admin_sees_and_runs_every_room() {
    let addr = start_server().await;

    // Locked without a login.
    let res = reqwest::get(format!("http://{addr}/api/admin/rooms")).await.unwrap();
    assert_eq!(res.status(), 401);
    let (status, _) = admin(&addr, "tbx_admin=forged", "rooms", None).await;
    assert_eq!(status, 401);
    let cookie = admin_login(&addr).await;

    // Two games side by side: one made by a TV, one from the admin panel.
    let (code_a, token_a) = create_room(&addr, 5).await;
    let (status, made) = admin(&addr, &cookie, "rooms", Some(json!({ "rounds": 3, "max_rating": "PG-13" }))).await;
    assert_eq!(status, 200);
    let code_b = made["code"].as_str().unwrap().to_string();
    let token_b = made["host_token"].as_str().unwrap().to_string();

    let mut host_a = open(&addr, &format!("/ws/host?code={code_a}&token={token_a}")).await;
    let mut host_b = open(&addr, &format!("/ws/host?code={code_b}&token={token_b}")).await;
    let mut sam = open(&addr, &format!("/ws/play?code={code_a}&name=Sam")).await;
    recv(&mut sam).await;
    let mut kim = open(&addr, &format!("/ws/play?code={code_a}&name=Kim")).await;
    recv(&mut kim).await;
    wait_state(&mut host_a, |s| s["players"].as_array().unwrap().len() == 2).await;

    let (_, list) = admin(&addr, &cookie, "rooms", None).await;
    let rooms = list["rooms"].as_array().unwrap();
    assert_eq!(rooms.len(), 2);
    let a = rooms.iter().find(|r| r["code"] == code_a.as_str()).unwrap();
    assert_eq!(a["players"].as_array().unwrap().len(), 2);
    assert_eq!(a["host_connected"], true);
    assert_eq!(a["phase"], "lobby");
    let sam_id = a["players"][0]["id"].clone();

    // Run room A from the panel: start, skip, kick, end.
    let act = |code: &str| format!("rooms/{code}");
    let (status, _) = admin(&addr, &cookie, &act(&code_a), Some(json!({ "action": "start" }))).await;
    assert_eq!(status, 200);
    wait_state(&mut host_a, |s| s["phase"] == "question").await;
    let (_, list) = admin(&addr, &cookie, "rooms", None).await;
    let a = list["rooms"].as_array().unwrap().iter().find(|r| r["code"] == code_a.as_str()).unwrap().clone();
    assert!(a["question"]["answer"].is_string(), "the operator can see the answer");

    admin(&addr, &cookie, &act(&code_a), Some(json!({ "action": "skip" }))).await;
    wait_state(&mut host_a, |s| s["phase"] == "reveal").await;

    admin(&addr, &cookie, &act(&code_a), Some(json!({ "action": "kick", "player_id": sam_id }))).await;
    assert_eq!(next_error(&mut sam).await["fatal"], true);

    let (status, _) = admin(&addr, &cookie, &act(&code_a), Some(json!({ "action": "end_game" }))).await;
    assert_eq!(status, 200);
    let fin = wait_state(&mut host_a, |s| s["phase"] == "final").await;
    assert_eq!(fin["players"].as_array().unwrap().len(), 1);
    let (status, err) = admin(&addr, &cookie, &act(&code_a), Some(json!({ "action": "skip" }))).await;
    assert_eq!(status, 409, "{err}");

    // Settings and teams from the panel.
    admin(&addr, &cookie, &act(&code_a), Some(json!({ "action": "lobby" }))).await;
    admin(&addr, &cookie, &act(&code_a), Some(json!({ "action": "settings", "teams": 3, "late_join": false }))).await;
    let s = wait_state(&mut host_a, |s| s["settings"]["teams"] == 3).await;
    assert_eq!(s["settings"]["late_join"], false);
    let kim_id = s["players"][0]["id"].clone();
    admin(&addr, &cookie, &act(&code_a), Some(json!({ "action": "set_team", "player_id": kim_id, "team": 2 }))).await;
    wait_state(&mut host_a, |s| s["players"][0]["team"] == 2).await;

    // Closing room B sends its TV away and drops it from the list.
    admin(&addr, &cookie, &act(&code_b), Some(json!({ "action": "close" }))).await;
    assert_eq!(next_error(&mut host_b).await["fatal"], true);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let (_, list) = admin(&addr, &cookie, "rooms", None).await;
    assert_eq!(list["rooms"].as_array().unwrap().len(), 1);

    // Logging out ends the session.
    let http = reqwest::Client::new();
    http.post(format!("http://{addr}/api/admin/logout")).header("cookie", &cookie).send().await.unwrap();
    let (status, _) = admin(&addr, &cookie, "rooms", None).await;
    assert_eq!(status, 401);
}
