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
const ADMIN_PASSWORD: &str = "letmein";

async fn start_server() -> String {
    start_server_with_db().await.0
}

/// Also returns the database URL, for tests that need to control the content.
async fn start_server_with_db() -> (String, String) {
    start_server_with(None).await
}

/// With `tts_url`, the host's lines are voiced by that TTS service.
async fn start_server_with(tts_url: Option<String>) -> (String, String) {
    let db = std::env::temp_dir().join(format!("triviabix-test-{}.db", uuid::Uuid::new_v4()));
    let database_url = format!("sqlite://{}", db.display());
    let config = Config {
        database_url: database_url.clone(),
        static_dir: concat!(env!("CARGO_MANIFEST_DIR"), "/../web").into(),
        public_url: None,
        max_players: MAX_PLAYERS,
        max_rooms: 10,
        itunes_url: FakeItunes::start().await.url,
        music_country: "US".into(),
        music_warmup: false,
        library: Default::default(),
        timing: Timing {
            question: Duration::from_secs(5),
            song_question: Duration::from_secs(5),
            hangman: Duration::from_secs(5),
            reveal: Duration::from_millis(200),
            scoreboard: Duration::from_millis(200),
            round_intro: Duration::from_millis(200),
            recap: Duration::from_millis(200),
            intro: Duration::from_millis(200),
            talk_cps: 0.0,
            idle_timeout: Duration::from_secs(60),
        },
        admin_password: Some(ADMIN_PASSWORD.into()),
        host_script: None,
        content_dir: concat!(env!("CARGO_MANIFEST_DIR"), "/../content").into(),
        tts_url,
        voice_names: vec!["Zelda".into()],
        tts_cache_dir: std::env::temp_dir().join(format!("triviabix-tts-{}", uuid::Uuid::new_v4())),
        render: Default::default(),
    };
    let app = triviabix::router(config).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("127.0.0.1:{}", addr.port()), database_url)
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

/// A trivia room of three rounds with `questions / 3` questions each (at least one).
async fn create_room(addr: &str, questions: usize) -> (String, String) {
    let per_round = (questions / 3).max(1);
    create_room_with(addr, json!({ "trivia_rounds": 3, "per_round": per_round })).await
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
    // Each line carries a cue so the TV knows which cutscene to play.
    assert_eq!(intro["host"]["cue"], "intro");

    for round in 1..=3 {
        let q = wait_state(&mut host, |s| {
            s["phase"] == "question" && s["round"] == round
        })
        .await;
        // Multiple choice has four choices, true/false two.
        let choices = q["question"]["choices"].as_array().unwrap().len();
        assert!(choices == 4 || choices == 2, "{q}");
        // Trivia plays the question card scene before every question; the final round's title
        // card plays the final-round scene instead.
        assert_eq!(q["host"]["cue"], "round");
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
        assert!(reveal["host"]["cue"].is_string(), "{}", reveal["host"]);
    }

    let fin = wait_state(&mut host, |s| s["phase"] == "final").await;
    assert_eq!(fin["round"], 3);
    assert!(!fin["host"]["text"].as_str().unwrap().is_empty());
    assert!(["winner", "finale_tie"].contains(&fin["host"]["cue"].as_str().unwrap()));

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
async fn host_reads_each_questions_own_lines_within_the_room_rating() {
    use triviabix::content::{AdminLine, QuestionSearch};
    use triviabix::db::Db;
    let (addr, url) = start_server_with_db().await;
    let db = Db::connect(&url).await.unwrap();
    let line = |rating: &str, text: &str| AdminLine {
        slot: String::new(),
        rating: rating.into(),
        text: text.into(),
        origin: String::new(),
    };
    // Give every question a PG lead-in, and a reveal line only an MA room may hear.
    let mut offset = 0;
    loop {
        let page = db
            .admin_questions(&QuestionSearch {
                offset,
                limit: Some(200),
                ..Default::default()
            })
            .await
            .unwrap();
        for q in &page.questions {
            db.set_edited_lines(q.id, "lead_in", &[line("PG", "Here's one about {host}.")])
                .await
                .unwrap();
            db.set_edited_lines(q.id, "reveal", &[line("MA", "SPICY REVEAL")])
                .await
                .unwrap();
        }
        offset += 200;
        if offset >= page.total {
            break;
        }
    }

    let (code, host_token) = create_room(&addr, 1).await;
    let mut host = open(&addr, &format!("/ws/host?code={code}&token={host_token}")).await;
    let mut phone = open(&addr, &format!("/ws/play?code={code}&name=Ann")).await;
    recv(&mut phone).await;
    send(&mut host, json!({ "type": "start" })).await;
    let q = wait_state(&mut host, |s| s["phase"] == "question").await;
    let said = q["host"]["text"].as_str().unwrap();
    assert!(said.contains("Here's one about Bix."), "{said}");
    send(&mut host, json!({ "type": "skip" })).await;
    let reveal = wait_state(&mut host, |s| s["phase"] == "reveal").await;
    let said = reveal["host"]["text"].as_str().unwrap();
    assert!(
        !said.contains("SPICY"),
        "a PG room heard an MA line: {said}"
    );

    // An MA room hears it, and Bix still roasts a wrong answer.
    let (code, host_token) =
        create_room_with(&addr, json!({ "rounds": 1, "max_rating": "MA" })).await;
    let mut host = open(&addr, &format!("/ws/host?code={code}&token={host_token}")).await;
    let mut phones = Vec::new();
    for name in ["Bo", "Cy"] {
        let mut phone = open(&addr, &format!("/ws/play?code={code}&name={name}")).await;
        recv(&mut phone).await;
        phones.push(phone);
    }
    send(&mut host, json!({ "type": "start" })).await;
    wait_state(&mut host, |s| s["phase"] == "question").await;
    // Different picks, so one of them is wrong.
    for (i, phone) in phones.iter_mut().enumerate() {
        send(phone, json!({ "type": "answer", "choice": i })).await;
    }
    let reveal = wait_state(&mut host, |s| s["phase"] == "reveal").await;
    let said = reveal["host"]["text"].as_str().unwrap();
    assert!(said.contains("SPICY REVEAL"), "{said}");
    let kinds: Vec<&str> = reveal["host"]["callouts"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["kind"].as_str())
        .collect();
    assert!(kinds.contains(&"roast"), "{kinds:?}: {said}");
}

#[tokio::test]
async fn static_pages_are_served() {
    let addr = start_server().await;
    for path in [
        "/",
        "/host",
        "/admin",
        "/admin/questions",
        "/admin-questions.js",
        "/admin/render",
        "/admin/pipeline",
        "/admin-pipeline.js",
        "/admin-nav.js",
        "/admin/settings",
        "/admin-settings.js",
        "/style.css",
        "/player.js",
        "/host.js",
        "/admin.js",
    ] {
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
    use triviabix::db::{Db, Difficulty, Rating};
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

    // Every question has a difficulty, and the seed set has all three.
    let pool = db.question_pool(Rating::Pg13).await.unwrap();
    assert_eq!(pool.len(), pg13.len());
    for d in [Difficulty::Easy, Difficulty::Medium, Difficulty::Hard] {
        assert!(pool.iter().any(|q| q.difficulty == d), "no {d:?} questions");
    }
    let ids: Vec<i64> = pool.iter().rev().take(5).map(|q| q.id).collect();
    let loaded = db.questions_by_ids(&ids, Rating::Pg13).await.unwrap();
    assert_eq!(loaded.iter().map(|q| q.id).collect::<Vec<_>>(), ids);

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
    use triviabix::music::{Itunes, Music, build_round};
    use triviabix::protocol::SongSource::Hits;
    let fake = FakeItunes::start().await;
    let itunes = Music {
        itunes: Itunes::new(&fake.url, "US"),
        library: triviabix::library::Library::new(Default::default()),
        media: Default::default(),
    };
    let path = std::env::temp_dir().join(format!("triviabix-test-{}.db", uuid::Uuid::new_v4()));
    let db = Db::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();

    let pg_songs = db.songs(Rating::Pg).await.unwrap();
    let all_songs = db.songs(Rating::Nc17).await.unwrap();
    assert!(pg_songs.len() >= 50 && all_songs.len() > pg_songs.len());

    let round = build_round(&db, &itunes, 5, Rating::Pg, &[], Hits)
        .await
        .unwrap();
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
    let again = build_round(&db, &itunes, 5, Rating::Pg, &ids, Hits)
        .await
        .unwrap();
    assert_eq!(again.len(), 5);
    assert_eq!(fake.searches.load(Ordering::SeqCst), searched);
    assert_eq!(fake.lookups.load(Ordering::SeqCst), lookups + 1);

    // An MA room can play Rihanna's explicit track; Gotye has no preview and gets retired.
    // Searches are capped per game, so it takes a few games to find everything.
    let mut everything = Vec::new();
    for _ in 0..6 {
        everything = build_round(&db, &itunes, 100, Rating::Ma, &[], Hits)
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
    assert!(
        q2["players"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["waiting"] == false)
    );
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
    let (code, host_token) = (
        res["code"].as_str().unwrap(),
        res["host_token"].as_str().unwrap(),
    );
    let mut host = open(&addr, &format!("/ws/host?code={code}&token={host_token}")).await;

    let mut players = Vec::new();
    for name in ["A", "B", "C"] {
        let mut ws = open(&addr, &format!("/ws/play?code={code}&name={name}")).await;
        recv(&mut ws).await;
        players.push(ws);
    }
    let s = wait_state(&mut host, |s| s["players"].as_array().unwrap().len() == 3).await;
    let teams: Vec<_> = s["players"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["team"].as_u64().unwrap())
        .collect();
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
    let score = |id: u64| {
        s["players"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == id)
            .unwrap()["score"]
            .as_i64()
            .unwrap()
    };
    for team in s["teams"].as_array().unwrap() {
        let members: Vec<i64> = team["members"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| score(m.as_u64().unwrap()))
            .collect();
        assert_eq!(
            team["score"].as_i64().unwrap(),
            members.iter().sum::<i64>() / members.len() as i64,
            "{team}"
        );
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
    let res = reqwest::get(format!("http://{addr}/api/admin/rooms"))
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    let (status, _) = admin(&addr, "tbx_admin=forged", "rooms", None).await;
    assert_eq!(status, 401);
    let cookie = admin_login(&addr).await;

    // Two games side by side: one made by a TV, one from the admin panel.
    let (code_a, token_a) = create_room(&addr, 5).await;
    let (status, made) = admin(
        &addr,
        &cookie,
        "rooms",
        Some(json!({ "rounds": 3, "max_rating": "PG-13" })),
    )
    .await;
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
    let (status, _) = admin(
        &addr,
        &cookie,
        &act(&code_a),
        Some(json!({ "action": "start" })),
    )
    .await;
    assert_eq!(status, 200);
    wait_state(&mut host_a, |s| s["phase"] == "question").await;
    let (_, list) = admin(&addr, &cookie, "rooms", None).await;
    let a = list["rooms"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["code"] == code_a.as_str())
        .unwrap()
        .clone();
    assert!(
        a["question"]["answer"].is_string(),
        "the operator can see the answer"
    );

    admin(
        &addr,
        &cookie,
        &act(&code_a),
        Some(json!({ "action": "skip" })),
    )
    .await;
    wait_state(&mut host_a, |s| s["phase"] == "reveal").await;

    admin(
        &addr,
        &cookie,
        &act(&code_a),
        Some(json!({ "action": "kick", "player_id": sam_id })),
    )
    .await;
    assert_eq!(next_error(&mut sam).await["fatal"], true);

    let (status, _) = admin(
        &addr,
        &cookie,
        &act(&code_a),
        Some(json!({ "action": "end_game" })),
    )
    .await;
    assert_eq!(status, 200);
    let fin = wait_state(&mut host_a, |s| s["phase"] == "final").await;
    assert_eq!(fin["players"].as_array().unwrap().len(), 1);
    let (status, err) = admin(
        &addr,
        &cookie,
        &act(&code_a),
        Some(json!({ "action": "skip" })),
    )
    .await;
    assert_eq!(status, 409, "{err}");

    // Settings and teams from the panel.
    admin(
        &addr,
        &cookie,
        &act(&code_a),
        Some(json!({ "action": "lobby" })),
    )
    .await;
    admin(
        &addr,
        &cookie,
        &act(&code_a),
        Some(json!({ "action": "settings", "teams": 3, "late_join": false })),
    )
    .await;
    let s = wait_state(&mut host_a, |s| s["settings"]["teams"] == 3).await;
    assert_eq!(s["settings"]["late_join"], false);
    let kim_id = s["players"][0]["id"].clone();
    admin(
        &addr,
        &cookie,
        &act(&code_a),
        Some(json!({ "action": "set_team", "player_id": kim_id, "team": 2 })),
    )
    .await;
    wait_state(&mut host_a, |s| s["players"][0]["team"] == 2).await;

    // Closing room B sends its TV away and drops it from the list.
    admin(
        &addr,
        &cookie,
        &act(&code_b),
        Some(json!({ "action": "close" })),
    )
    .await;
    assert_eq!(next_error(&mut host_b).await["fatal"], true);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let (_, list) = admin(&addr, &cookie, "rooms", None).await;
    assert_eq!(list["rooms"].as_array().unwrap().len(), 1);

    // Logging out ends the session.
    let http = reqwest::Client::new();
    http.post(format!("http://{addr}/api/admin/logout"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    let (status, _) = admin(&addr, &cookie, "rooms", None).await;
    assert_eq!(status, 401);
}

#[tokio::test]
async fn admin_edits_a_questions_lines() {
    let addr = start_server().await;
    let cookie = admin_login(&addr).await;
    let (status, page) = admin(&addr, &cookie, "questions?q=Canberra", None).await;
    assert_eq!(status, 200, "{page}");
    assert_eq!(page["total"], 1);
    let q = &page["questions"][0];
    assert_eq!(q["answer"], "Canberra");
    let has = |q: &Value, origin: &str| {
        q["lines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["slot"] == "lead_in" && l["origin"] == origin)
    };
    assert!(has(q, "content"), "shipped questions have lines: {q}");

    let put = |lines: Value| {
        let url = format!(
            "http://{addr}/api/admin/questions/{}/lines/lead_in",
            q["id"]
        );
        let cookie = cookie.clone();
        async move {
            reqwest::Client::new()
                .put(url)
                .header("cookie", cookie)
                .json(&json!({ "lines": lines }))
                .send()
                .await
                .unwrap()
                .status()
                .as_u16()
        }
    };
    assert_eq!(
        put(json!([{ "rating": "PG", "text": "Mine now." }])).await,
        200
    );
    assert_eq!(
        put(json!([{ "rating": "R", "text": "Bad rating." }])).await,
        400
    );
    assert_eq!(
        put(json!([{ "rating": "PG", "text": "Hi {name}" }])).await,
        400
    );
    let (_, page) = admin(&addr, &cookie, "questions?q=Mine%20now", None).await;
    assert_eq!(page["total"], 1);
    assert!(has(&page["questions"][0], "edited"));

    // Reloading the packs keeps the edit.
    let (status, report) = admin(&addr, &cookie, "content/reload", Some(json!({}))).await;
    assert_eq!(status, 200, "{report}");
    assert_eq!(report["added"], 0);
    let (_, page) = admin(&addr, &cookie, "questions?q=Mine%20now", None).await;
    assert_eq!(page["total"], 1);

    // Going back to the pack's lines.
    assert_eq!(put(json!([])).await, 200);
    let (_, page) = admin(&addr, &cookie, "questions?q=Mine%20now", None).await;
    assert_eq!(page["total"], 0);
}

#[tokio::test]
async fn trivia_rounds_get_harder_with_recaps_between() {
    let addr = start_server().await;
    let (code, host_token) =
        create_room_with(&addr, json!({ "trivia_rounds": 4, "per_round": 2 })).await;
    let mut host = open(&addr, &format!("/ws/host?code={code}&token={host_token}")).await;
    let mut ann = open(&addr, &format!("/ws/play?code={code}&name=Ann")).await;
    recv(&mut ann).await;
    let lobby = wait_state(&mut host, |s| s["players"].as_array().unwrap().len() == 1).await;
    assert_eq!(lobby["settings"]["trivia_rounds"], 4);
    assert_eq!(lobby["total_rounds"], 8);
    send(&mut host, json!({ "type": "start" })).await;

    let mut total_points = 0;
    for stage in 1..=4u64 {
        // Each round opens on a title card with its theme, difficulty and points.
        let card = wait_state(&mut host, |s| {
            s["phase"] == "round_intro" && s["stage"] == stage
        })
        .await;
        assert!(card["question"].is_null(), "no question on the title card");
        let ladder = card["stages"].as_array().unwrap();
        assert_eq!(ladder.len(), 4);
        let me = &ladder[stage as usize - 1];
        assert!(me["category"].is_string());
        let (difficulty, pct) =
            [("easy", 100), ("medium", 150), ("hard", 200), ("hard", 300)][stage as usize - 1];
        assert_eq!(me["difficulty"], difficulty);
        assert_eq!(me["points_pct"], pct);
        assert_eq!(me["final_round"], stage == 4);
        for (i, r) in ladder.iter().enumerate() {
            let want = match (i + 1) as u64 {
                n if n < stage => "done",
                n if n == stage => "current",
                _ => "next",
            };
            assert_eq!(r["status"], want, "{r}");
            if want == "next" {
                assert!(r["category"].is_null(), "future themes stay secret: {r}");
            }
        }
        let cue = if stage == 4 {
            json!("final_round")
        } else {
            Value::Null
        };
        assert_eq!(card["host"]["cue"], cue);
        let said = card["host"]["text"].as_str().unwrap();
        assert!(!said.contains('{'), "unfilled placeholder: {said}");

        for n in 1..=2u64 {
            let q = wait_state(&mut host, |s| {
                s["phase"] == "question" && s["stage"] == stage && s["stage_question"] == n
            })
            .await;
            assert_eq!(q["round"], (stage - 1) * 2 + n);
            send(&mut ann, json!({ "type": "answer", "choice": 0 })).await;
            let reveal = wait_state(&mut ann, |s| s["phase"] == "reveal").await;
            let correct = reveal["reveal"]["correct"].as_u64().unwrap();
            let points = reveal["reveal"]["results"][0]["points"].as_i64().unwrap();
            if correct == 0 {
                // A right answer pays 500 to 1000, scaled up by the round.
                let pct = pct as i64;
                assert!(
                    points >= 500 * pct / 100 && points <= 1000 * pct / 100,
                    "{points}"
                );
            } else {
                assert_eq!(points, 0);
            }
            total_points += points;
        }

        // After every round: the scores and what was asked.
        let recap = wait_state(&mut host, |s| s["phase"] == "recap").await;
        assert_eq!(recap["recap"]["stage"], stage);
        let questions = recap["recap"]["questions"].as_array().unwrap();
        assert_eq!(questions.len(), 2);
        assert!(
            questions
                .iter()
                .all(|q| q["answer"].is_string() && q["prompt"].is_string())
        );
        assert!(recap["question"].is_null());
        assert!(!recap["host"]["text"].as_str().unwrap().contains('{'));
    }

    let fin = wait_state(&mut host, |s| s["phase"] == "final").await;
    assert_eq!(fin["players"][0]["score"].as_i64().unwrap(), total_points);
    assert!(
        fin["stages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["status"] == "done")
    );
}

#[tokio::test]
async fn the_vip_runs_the_whole_game_from_their_phone() {
    let addr = start_server().await;
    // The TV opens a room with no settings at all.
    let (code, host_token) = create_room_with(&addr, json!({})).await;
    let mut host = open(&addr, &format!("/ws/host?code={code}&token={host_token}")).await;
    let mut vip = open(&addr, &format!("/ws/play?code={code}&name=Vee")).await;
    recv(&mut vip).await;
    let mut bob = open(&addr, &format!("/ws/play?code={code}&name=Bob")).await;
    recv(&mut bob).await;
    let mut ghost = open(&addr, &format!("/ws/play?code={code}&name=Ghost")).await;
    let ghost_id = recv(&mut ghost).await["player_id"].as_u64().unwrap();
    wait_state(&mut host, |s| s["players"].as_array().unwrap().len() == 3).await;

    // Only the VIP picks the game.
    send(&mut bob, json!({ "type": "settings", "mode": "hangman" })).await;
    assert!(
        next_error(&mut bob).await["message"]
            .as_str()
            .unwrap()
            .contains("VIP")
    );
    send(&mut bob, json!({ "type": "kick", "player_id": ghost_id })).await;
    next_error(&mut bob).await;

    // The VIP picks the game, but the rating, length and teams are for the admin only.
    send(
        &mut vip,
        json!({ "type": "settings", "mode": "hangman", "max_rating": "NC-17" }),
    )
    .await;
    assert!(
        next_error(&mut vip).await["message"]
            .as_str()
            .unwrap()
            .contains("admin")
    );
    for setting in [
        json!({ "rounds": 5 }),
        json!({ "teams": 2 }),
        json!({ "late_join": false }),
        json!({ "per_round": 3 }),
        json!({ "clip_ms": 1000 }),
    ] {
        let mut msg = setting;
        msg["type"] = json!("settings");
        send(&mut vip, msg).await;
        next_error(&mut vip).await;
    }
    send(&mut vip, json!({ "type": "settings", "mode": "hangman" })).await;
    let s = wait_state(&mut host, |s| s["settings"]["mode"] == "hangman").await;
    assert_eq!(s["settings"]["max_rating"], "PG");
    assert_eq!(s["settings"]["teams"], 0);
    send(&mut vip, json!({ "type": "settings", "mode": "trivia" })).await;
    send(&mut host, json!({ "type": "settings", "per_round": 3 })).await;
    wait_state(&mut host, |s| {
        s["settings"]["mode"] == "trivia" && s["settings"]["per_round"] == 3
    })
    .await;

    // The VIP clears out a player, but can't remove themselves.
    let vip_id = wait_state(&mut vip, |_| true).await["you"]["player_id"].clone();
    send(&mut vip, json!({ "type": "kick", "player_id": vip_id })).await;
    next_error(&mut vip).await;
    send(&mut vip, json!({ "type": "kick", "player_id": ghost_id })).await;
    assert_eq!(next_error(&mut ghost).await["fatal"], true);
    wait_state(&mut host, |s| s["players"].as_array().unwrap().len() == 2).await;

    // Start, skip ahead, then drop the game to pick another one.
    send(&mut vip, json!({ "type": "start" })).await;
    wait_state(&mut host, |s| s["phase"] != "lobby").await;
    send(&mut vip, json!({ "type": "next" })).await;
    send(&mut bob, json!({ "type": "lobby" })).await;
    next_error(&mut bob).await;
    send(&mut vip, json!({ "type": "lobby" })).await;
    let s = wait_state(&mut host, |s| s["phase"] == "lobby").await;
    assert!(
        s["players"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["score"] == 0)
    );
    send(&mut vip, json!({ "type": "lobby" })).await;
    next_error(&mut vip).await;

    // Kicking mid-game is left to the admin panel.
    send(&mut vip, json!({ "type": "start" })).await;
    wait_state(&mut host, |s| s["phase"] != "lobby").await;
    send(
        &mut vip,
        json!({ "type": "kick", "player_id": s["players"][1]["id"] }),
    )
    .await;
    next_error(&mut vip).await;
}

#[tokio::test]
async fn admin_sets_each_rooms_size() {
    let addr = start_server().await;
    let cookie = admin_login(&addr).await;
    let (status, room) = admin(&addr, &cookie, "rooms", Some(json!({ "max_players": 3 }))).await;
    assert_eq!(status, 200);
    let code = room["code"].as_str().unwrap().to_string();
    let host_token = room["host_token"].as_str().unwrap().to_string();
    let mut host = open(&addr, &format!("/ws/host?code={code}&token={host_token}")).await;
    assert_eq!(wait_state(&mut host, |_| true).await["max_players"], 3);

    // Phones can't pick the size: the public endpoint ignores it.
    let (other, _) = create_room_with(&addr, json!({ "max_players": 2 })).await;
    let info: Value = reqwest::get(format!("http://{addr}/api/rooms/{other}"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_ne!(info["max_players"], 2);

    let mut players = Vec::new();
    for name in ["A", "B"] {
        let mut ws = open(&addr, &format!("/ws/play?code={code}&name={name}")).await;
        recv(&mut ws).await;
        players.push(ws);
    }
    // Shrinking stops at the players already in.
    let path = format!("rooms/{code}");
    admin(
        &addr,
        &cookie,
        &path,
        Some(json!({ "action": "settings", "max_players": 1 })),
    )
    .await;
    wait_state(&mut host, |s| s["max_players"] == 2).await;
    let mut c = open(&addr, &format!("/ws/play?code={code}&name=C")).await;
    assert_eq!(next_error(&mut c).await["fatal"], true);

    admin(
        &addr,
        &cookie,
        &path,
        Some(json!({ "action": "settings", "max_players": 99 })),
    )
    .await;
    wait_state(&mut host, |s| s["max_players"] == 24).await;
    let mut c = open(&addr, &format!("/ws/play?code={code}&name=C")).await;
    assert_eq!(recv(&mut c).await["type"], "welcome");
}

/// Stands in for the TTS service in tts/: "audio" is the text, and it counts what it's asked for.
#[derive(Clone, Default)]
struct FakeTts {
    url: String,
    live: Arc<Mutex<Vec<String>>>,
    warm: Arc<AtomicUsize>,
    /// The bearer token each speech request came with.
    tokens: Arc<Mutex<Vec<Option<String>>>>,
}

impl FakeTts {
    async fn start() -> Self {
        use axum::{extract::State, routing::get, routing::post};

        async fn speech(
            State(f): State<FakeTts>,
            headers: axum::http::HeaderMap,
            axum::Json(body): axum::Json<Value>,
        ) -> impl axum::response::IntoResponse {
            f.tokens.lock().unwrap().push(
                headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string),
            );
            let text = body["text"].as_str().unwrap().to_string();
            if body["priority"] == "warm" {
                f.warm.fetch_add(1, Ordering::SeqCst);
            } else {
                f.live.lock().unwrap().push(text.clone());
            }
            (
                [("content-type", "audio/ogg"), ("x-voice", "ip:US_James")],
                format!("OggS {text}"),
            )
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fake = FakeTts {
            url: format!("http://{}", listener.local_addr().unwrap()),
            ..Default::default()
        };
        let app = axum::Router::new()
            .route(
                "/healthz",
                get(|| async { axum::Json(json!({ "ready": true, "voice": "ip:US_James" })) }),
            )
            .route("/v1/speech", post(speech))
            .with_state(fake.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        fake
    }
}

#[tokio::test]
async fn the_tv_gets_bix_lines_as_audio_from_the_tts_service() {
    let tts = FakeTts::start().await;
    let (addr, _) = start_server_with(Some(tts.url.clone())).await;
    let http = reqwest::Client::new();
    let config: Value = http
        .get(format!("http://{addr}/api/config"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(config["voice"], true);

    let (code, host_token) = create_room(&addr, 3).await;
    let mut host = open(&addr, &format!("/ws/host?code={code}&token={host_token}")).await;
    let mut phone = open(&addr, &format!("/ws/play?code={code}&name=Zelda")).await;
    // The host greets Zelda by name, so that line is made on demand.
    let s = wait_state(&mut host, |s| {
        s["host"]["text"]
            .as_str()
            .is_some_and(|t| t.contains("Zelda"))
    })
    .await;
    let part = &s["host"]["voice"][0];
    assert!(part["text"].as_str().unwrap().contains("Zelda"));
    let url = part["audio"].as_str().unwrap().to_string();
    assert!(url.starts_with("/api/voice/"), "{url}");
    let phone_state = wait_state(&mut phone, |s| s["host"].is_object()).await;
    assert!(
        phone_state["host"].get("voice").is_none(),
        "only the TV gets audio"
    );

    for _ in 0..2 {
        let res = http
            .get(format!("http://{addr}{url}"))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        assert_eq!(res.headers()["content-type"], "audio/ogg");
        let body = res.text().await.unwrap();
        assert!(
            body.starts_with("OggS ") && body.contains("Zelda"),
            "{body}"
        );
    }
    let zelda = tts
        .live
        .lock()
        .unwrap()
        .iter()
        .filter(|t| t.contains("Zelda"))
        .count();
    assert_eq!(zelda, 1, "the second fetch comes from the cache");

    // Questions are read out too, and the fixed lines are made ahead of time.
    send(&mut host, json!({ "type": "start" })).await;
    let q = wait_state(&mut host, |s| s["phase"] == "question").await;
    let prompt = q["question"]["prompt_audio"].as_str().unwrap();
    let res = http
        .get(format!("http://{addr}{prompt}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert!(
        res.text()
            .await
            .unwrap()
            .contains(q["question"]["prompt"].as_str().unwrap())
    );
    let res = http
        .get(format!("http://{addr}/api/voice/{}", "0".repeat(32)))
        .send()
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        404,
        "only lines the game has said can be made"
    );
    for _ in 0..50 {
        if tts.warm.load(Ordering::SeqCst) > 10 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        tts.warm.load(Ordering::SeqCst) > 10,
        "lines should be pre-generated in the background"
    );
}

#[tokio::test]
async fn a_batch_run_voices_regulars_lines_so_games_need_no_live_tts() {
    let tts = FakeTts::start().await;
    let (addr, _) = start_server_with(None).await;
    let http = reqwest::Client::new();
    let cookie = admin_login(&addr).await;
    let set = |url: Option<String>| {
        http.put(format!("http://{addr}/api/admin/voice"))
            .header("cookie", &cookie)
            .json(&json!({ "url": url }))
            .send()
    };
    set(Some(tts.url.clone())).await.unwrap();
    // Work through the whole batch, as gpu/vast.py voice does.
    for _ in 0..600 {
        let s: Value = http
            .get(format!("http://{addr}/api/admin/voice"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if s["backlog"] == 0 && s["in_flight"] == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    set(None).await.unwrap();

    // Zelda is in VOICE_NAMES, so Bix greets her with a line that's already made.
    let (code, host_token) = create_room(&addr, 3).await;
    let mut host = open(&addr, &format!("/ws/host?code={code}&token={host_token}")).await;
    let _phone = open(&addr, &format!("/ws/play?code={code}&name=Zelda")).await;
    let s = wait_state(&mut host, |s| {
        s["host"]["text"]
            .as_str()
            .is_some_and(|t| t.contains("Zelda"))
    })
    .await;
    let url = s["host"]["voice"][0]["audio"].as_str().unwrap().to_string();
    let res = http
        .get(format!("http://{addr}{url}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "the greeting was made in the batch");
    assert!(tts.live.lock().unwrap().is_empty(), "nothing was made live");
}

#[tokio::test]
async fn a_voice_server_can_be_set_at_runtime_and_its_lines_outlive_it() {
    let tts = FakeTts::start().await;
    let (addr, _) = start_server_with(None).await;
    let http = reqwest::Client::new();
    let voice_on = || async {
        let config: Value = http
            .get(format!("http://{addr}/api/config"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        config["voice"].as_bool().unwrap()
    };
    assert!(!voice_on().await, "no TTS service and nothing cached yet");

    let cookie = admin_login(&addr).await;
    let set = |url: Option<String>| {
        http.put(format!("http://{addr}/api/admin/voice"))
            .header("cookie", &cookie)
            .json(&json!({ "url": url, "token": "gpu-token" }))
            .send()
    };
    let status: Value = set(Some(tts.url.clone()))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["server"], tts.url.as_str());
    assert!(status["backlog"].as_u64().unwrap() > 10, "{status}");
    assert!(voice_on().await);

    // The backlog queued at startup goes to the new service, with its token.
    for _ in 0..50 {
        if tts.warm.load(Ordering::SeqCst) > 10 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(tts.warm.load(Ordering::SeqCst) > 10);
    assert!(
        tts.tokens
            .lock()
            .unwrap()
            .iter()
            .all(|t| t.as_deref() == Some("Bearer gpu-token"))
    );

    let (code, host_token) = create_room(&addr, 3).await;
    let mut host = open(&addr, &format!("/ws/host?code={code}&token={host_token}")).await;
    let _phone = open(&addr, &format!("/ws/play?code={code}&name=Zelda")).await;
    let s = wait_state(&mut host, |s| {
        s["host"]["text"]
            .as_str()
            .is_some_and(|t| t.contains("Zelda"))
    })
    .await;
    let url = s["host"]["voice"][0]["audio"].as_str().unwrap().to_string();
    let get = || http.get(format!("http://{addr}{url}")).send();
    assert_eq!(get().await.unwrap().status(), 200);

    // The GPU goes away: what it made keeps playing, and new lines are left to the browser.
    let status: Value = set(None).await.unwrap().json().await.unwrap();
    assert!(status["server"].is_null());
    let res = get().await.unwrap();
    assert_eq!(res.status(), 200);
    assert!(res.text().await.unwrap().contains("Zelda"));
    assert!(voice_on().await, "cached lines still play");
}
