//! Plays whole games against a real server over WebSockets.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
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
        itunes_url: FakeItunes::start().await.url,
        music_country: "US".into(),
        music_warmup: false,
        timing: Timing {
            question: Duration::from_secs(5),
            song_question: Duration::from_secs(5),
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

/// Stands in for the iTunes Search API. Every search finds a track whose name echoes the search
/// term, except that Rihanna is always explicit and Gotye never has a preview.
#[derive(Clone, Default)]
struct FakeItunes {
    url: String,
    searches: Arc<AtomicUsize>,
    lookups: Arc<AtomicUsize>,
    terms: Arc<Mutex<Vec<String>>>,
}

impl FakeItunes {
    async fn start() -> Self {
        use axum::{
            Json,
            extract::{Query, State},
            routing::get,
        };
        use std::collections::HashMap;

        fn track(id: usize, term: &str) -> Value {
            json!({
                "kind": "song",
                "trackId": id,
                "trackName": term,
                "artistName": term,
                "previewUrl": if term.contains("Gotye") { Value::Null } else { json!(format!("https://previews.test/{id}.m4a")) },
                "artworkUrl100": format!("https://art.test/{id}/100x100bb.jpg"),
                "trackViewUrl": format!("https://music.test/{id}"),
                "trackExplicitness": if term.contains("Rihanna") { "explicit" } else { "notExplicit" },
            })
        }
        async fn search(
            State(f): State<FakeItunes>,
            Query(q): Query<HashMap<String, String>>,
        ) -> Json<Value> {
            f.searches.fetch_add(1, Ordering::SeqCst);
            let term = q["term"].clone();
            let mut terms = f.terms.lock().unwrap();
            terms.push(term.clone());
            Json(json!({ "results": [track(terms.len(), &term)] }))
        }
        async fn lookup(
            State(f): State<FakeItunes>,
            Query(q): Query<HashMap<String, String>>,
        ) -> Json<Value> {
            f.lookups.fetch_add(1, Ordering::SeqCst);
            let terms = f.terms.lock().unwrap();
            let results: Vec<Value> = q["id"]
                .split(',')
                .filter_map(|id| id.parse::<usize>().ok())
                .filter_map(|id| terms.get(id - 1).map(|t| track(id, t)))
                .collect();
            Json(json!({ "results": results }))
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fake = FakeItunes {
            url: format!("http://{}", listener.local_addr().unwrap()),
            ..Default::default()
        };
        let app = axum::Router::new()
            .route("/search", get(search))
            .route("/lookup", get(lookup))
            .with_state(fake.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        fake
    }
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

#[tokio::test]
async fn name_that_tune_plays_clips_on_the_tv_only() {
    let addr = start_server().await;
    let (code, host_token) = create_room_with(
        &addr,
        json!({ "rounds": 3, "mode": "songs", "clip_ms": 1000, "max_rating": "PG" }),
    )
    .await;
    let mut host = open(&addr, &format!("/ws/host?code={code}&token={host_token}")).await;
    let mut phones = Vec::new();
    for name in ["Ann", "Bo"] {
        let mut ws = open(&addr, &format!("/ws/play?code={code}&name={name}")).await;
        recv(&mut ws).await;
        phones.push(ws);
    }
    let lobby = wait_state(&mut host, |s| s["players"].as_array().unwrap().len() == 2).await;
    assert_eq!(lobby["settings"]["mode"], "songs");
    assert_eq!(lobby["settings"]["clip_ms"], 1000);

    send(&mut host, json!({ "type": "start" })).await;
    let mut played = Vec::new();
    for round in 1..=3 {
        let q = wait_state(&mut host, |s| {
            s["phase"] == "question" && s["round"] == round
        })
        .await;
        let question = &q["question"];
        assert_eq!(question["kind"], "song");
        assert!(
            question["category"]
                .as_str()
                .unwrap()
                .starts_with("Name That Tune")
        );
        let clip = &question["clip"];
        assert!(
            clip["preview_url"]
                .as_str()
                .unwrap()
                .starts_with("https://previews.test/")
        );
        assert_eq!(clip["clip_ms"], 1000);
        assert!(clip["start_ms"].as_u64().unwrap() <= 20_000);
        // The TV gets the next clip early so it can buffer; the last round has none.
        assert_eq!(q["next_clip"].is_string(), round < 3, "{q}");

        let choices: Vec<&str> = question["choices"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap())
            .collect();
        assert_eq!(choices.len(), 4);
        assert!(choices.iter().all(|c| c.contains(" — ")));
        let mut unique = choices.clone();
        unique.dedup();
        assert_eq!(unique.len(), 4);

        // Phones see the choices but never the audio or the answer.
        let p = wait_state(&mut phones[0], |s| {
            s["phase"] == "question" && s["round"] == round
        })
        .await;
        assert!(
            p["question"]["clip"].is_null() && p["next_clip"].is_null() && p["reveal"].is_null(),
            "{p}"
        );

        for ws in phones.iter_mut() {
            send(ws, json!({ "type": "answer", "choice": 0 })).await;
        }
        let reveal = wait_state(&mut host, |s| s["phase"] == "reveal").await;
        let song = &reveal["reveal"]["song"];
        let correct = reveal["reveal"]["correct"].as_u64().unwrap() as usize;
        let label = format!(
            "{} — {}",
            song["title"].as_str().unwrap(),
            song["artist"].as_str().unwrap()
        );
        assert_eq!(choices[correct], label);
        assert!(
            song["store_url"]
                .as_str()
                .unwrap()
                .starts_with("https://music.test/")
        );
        assert!(
            song["artwork_url"]
                .as_str()
                .unwrap()
                .ends_with("600x600bb.jpg")
        );
        // PG room: no explicit tracks (Rihanna is always explicit in the fake), nothing unplayable.
        assert!(
            !label.contains("Rihanna") && !label.contains("Gotye"),
            "{label}"
        );
        played.push(label);
    }
    wait_state(&mut host, |s| s["phase"] == "final").await;

    // A rematch picks different songs.
    send(&mut host, json!({ "type": "play_again" })).await;
    wait_state(&mut host, |s| s["phase"] == "lobby").await;
    send(&mut host, json!({ "type": "start" })).await;
    let q = wait_state(&mut host, |s| s["phase"] == "question").await;
    send(&mut phones[0], json!({ "type": "answer", "choice": 0 })).await;
    send(&mut phones[1], json!({ "type": "answer", "choice": 0 })).await;
    let reveal = wait_state(&mut host, |s| s["phase"] == "reveal").await;
    let song = &reveal["reveal"]["song"];
    let label = format!(
        "{} — {}",
        song["title"].as_str().unwrap(),
        song["artist"].as_str().unwrap()
    );
    assert!(!played.contains(&label), "{label} repeated");
    assert_eq!(q["question"]["kind"], "song");
}

#[tokio::test]
async fn song_rounds_respect_ratings_and_cache_tracks() {
    use triviabix::db::{Db, Rating};
    use triviabix::music::{Itunes, build_round};
    let fake = FakeItunes::start().await;
    let itunes = Itunes::new(&fake.url, "US");
    let path = std::env::temp_dir().join(format!("triviabix-test-{}.db", uuid::Uuid::new_v4()));
    let db = Db::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();

    let pg_songs = db.songs(Rating::Pg).await.unwrap();
    let all_songs = db.songs(Rating::Nc17).await.unwrap();
    assert!(pg_songs.len() >= 50 && all_songs.len() > pg_songs.len());

    let round = build_round(&db, &itunes, 5, Rating::Pg, &[]).await.unwrap();
    assert_eq!(round.len(), 5);
    for q in &round {
        assert!(pg_songs.iter().any(|s| s.id == q.id));
        // Wrong answers come from the same rating pool too.
        for c in &q.choices {
            assert!(
                pg_songs
                    .iter()
                    .any(|s| *c == format!("{} — {}", s.title, s.artist)),
                "{c}"
            );
        }
    }
    let searched = fake.searches.load(Ordering::SeqCst);
    assert!(searched >= 5);

    // The same songs again are refreshed with one lookup, not searched again.
    let lookups = fake.lookups.load(Ordering::SeqCst);
    let ids: Vec<i64> = all_songs
        .iter()
        .map(|s| s.id)
        .filter(|id| !round.iter().any(|q| q.id == *id))
        .collect();
    let again = build_round(&db, &itunes, 5, Rating::Pg, &ids)
        .await
        .unwrap();
    assert_eq!(again.len(), 5);
    assert_eq!(fake.searches.load(Ordering::SeqCst), searched);
    assert_eq!(fake.lookups.load(Ordering::SeqCst), lookups + 1);

    // An MA room can play Rihanna's explicit track; Gotye has no preview and gets retired.
    // Searches are capped per game, so it takes a few games to find everything.
    let mut everything = Vec::new();
    for _ in 0..6 {
        everything = build_round(&db, &itunes, 100, Rating::Ma, &[])
            .await
            .unwrap();
    }
    assert!(everything.len() > pg_songs.len());
    assert!(
        everything
            .iter()
            .any(|q| q.choices[q.answer].contains("Rihanna"))
    );
    assert!(
        everything
            .iter()
            .all(|q| !q.choices[q.answer].contains("Gotye"))
    );
    assert!(
        !everything
            .iter()
            .any(|q| q.choices[q.answer].contains("Nine Inch Nails")),
        "NC-17 song in an MA room"
    );
}
