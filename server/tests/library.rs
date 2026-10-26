//! The music library: syncing from a fake Lidarr, Name That Tune from library files, and
//! streaming those files to the TV.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use triviabix::db::{Db, Rating};
use triviabix::library::{Library, LibraryConfig};
use triviabix::music::{Itunes, Music, build_round};
use triviabix::protocol::SongSource;
use triviabix::{Config, room::Timing};

const API_KEY: &str = "lidarr-secret";
/// Where the fake Lidarr keeps its music, as Lidarr sees it.
const LIDARR_ROOT: &str = "/data/music";

/// One track in the fake Lidarr.
#[derive(Clone)]
struct FakeTrack {
    artist: usize,
    album: usize,
    title: &'static str,
    /// Relative to the music folder.
    file: &'static str,
    duration: i64,
    explicit: bool,
}

const ARTISTS: &[(&str, &str)] = &[
    ("Queen", "rock"),
    ("The Fake Band", "indie rock"),
    ("DJ Nobody", "electronic"),
];

/// (artist, title, release date, secondary types)
const ALBUMS: &[(usize, &str, &str, &[&str])] = &[
    (0, "A Night at the Opera", "1975-11-21T00:00:00Z", &[]),
    (1, "Debut", "2009-03-01T00:00:00Z", &[]),
    (1, "Live at Home", "2011-05-01T00:00:00Z", &["Live"]),
    (2, "Bleeps", "2015-01-01T00:00:00Z", &[]),
];

fn tracks() -> Vec<FakeTrack> {
    let t = |artist, album, title, file, duration, explicit| FakeTrack {
        artist,
        album,
        title,
        file,
        duration,
        explicit,
    };
    vec![
        // Also in the curated hits, so that song plays from the file.
        t(
            0,
            0,
            "Bohemian Rhapsody",
            "Queen/A Night at the Opera/11 Bohemian Rhapsody.flac",
            354_000,
            false,
        ),
        t(
            0,
            0,
            "You're My Best Friend",
            "Queen/A Night at the Opera/04 You're My Best Friend.mp3",
            170_000,
            false,
        ),
        t(
            1,
            1,
            "Song One",
            "The Fake Band/Debut/01 Song One.mp3",
            200_000,
            false,
        ),
        t(
            1,
            1,
            "Song Two",
            "The Fake Band/Debut/02 Song Two.mp3",
            210_000,
            false,
        ),
        t(
            1,
            1,
            "Song Three",
            "The Fake Band/Debut/03 Song Three.m4a",
            220_000,
            false,
        ),
        t(
            1,
            1,
            "Dirty Song",
            "The Fake Band/Debut/04 Dirty Song.mp3",
            230_000,
            true,
        ),
        // Left out: an interlude, a live album, and a format browsers can't play.
        t(
            1,
            1,
            "Interlude",
            "The Fake Band/Debut/05 Interlude.mp3",
            30_000,
            false,
        ),
        t(
            1,
            2,
            "Song One (Live)",
            "The Fake Band/Live at Home/01 Song One.mp3",
            200_000,
            false,
        ),
        t(
            1,
            1,
            "Song Four",
            "The Fake Band/Debut/06 Song Four.wma",
            200_000,
            false,
        ),
        t(
            2,
            3,
            "Bleep",
            "DJ Nobody/Bleeps/01 Bleep.ogg",
            300_000,
            false,
        ),
        t(
            2,
            3,
            "Bloop",
            "DJ Nobody/Bleeps/02 Bloop.opus",
            300_000,
            false,
        ),
        t(
            2,
            3,
            "Blap",
            "DJ Nobody/Bleeps/03 Blap.flac",
            300_000,
            false,
        ),
        // Lidarr has it, but the file isn't in the folder we mounted.
        t(
            2,
            3,
            "Ghost",
            "DJ Nobody/Elsewhere/04 Ghost.mp3",
            300_000,
            false,
        ),
    ]
}

/// Stands in for Lidarr's v1 API. Tracks can be removed to test re-syncs.
#[derive(Clone)]
struct FakeLidarr {
    url: String,
    tracks: Arc<Mutex<Vec<FakeTrack>>>,
}

impl FakeLidarr {
    async fn start() -> Self {
        use axum::{
            Json,
            extract::{Query, State},
            http::{HeaderMap, StatusCode},
            response::{IntoResponse, Response},
            routing::get,
        };
        use std::collections::HashMap;

        fn authed(h: &HeaderMap) -> bool {
            h.get("x-api-key").and_then(|v| v.to_str().ok()) == Some(API_KEY)
        }
        fn deny() -> Response {
            (StatusCode::UNAUTHORIZED, "no").into_response()
        }
        async fn artists(h: HeaderMap) -> Response {
            if !authed(&h) {
                return deny();
            }
            let list: Vec<Value> = ARTISTS
                .iter()
                .enumerate()
                .map(|(i, (name, genre))| {
                    json!({ "id": i + 1, "artistName": name, "genres": [genre],
                            "statistics": { "trackFileCount": 3 } })
                })
                .collect();
            Json(list).into_response()
        }
        async fn albums(h: HeaderMap) -> Response {
            if !authed(&h) {
                return deny();
            }
            let list: Vec<Value> = ALBUMS
                .iter()
                .enumerate()
                .map(|(i, (artist, title, date, secondary))| {
                    let secondary: Vec<Value> = secondary
                        .iter()
                        .map(|s| json!({ "id": 1, "name": s }))
                        .collect();
                    json!({ "id": i + 1, "artistId": artist + 1, "title": title,
                            "releaseDate": date, "albumType": "Album", "genres": [],
                            "secondaryTypes": secondary,
                            "images": [{ "coverType": "cover", "url": "/MediaCover/1/cover.jpg",
                                         "remoteUrl": format!("https://coverart.test/{}.jpg", i + 1) }] })
                })
                .collect();
            Json(list).into_response()
        }
        async fn roots(h: HeaderMap) -> Response {
            if !authed(&h) {
                return deny();
            }
            Json(json!([{ "id": 1, "path": format!("{LIDARR_ROOT}/") }])).into_response()
        }
        async fn track(
            State(f): State<FakeLidarr>,
            h: HeaderMap,
            Query(q): Query<HashMap<String, String>>,
        ) -> Response {
            if !authed(&h) {
                return deny();
            }
            let artist: usize = q["artistId"].parse().unwrap();
            let list: Vec<Value> = f
                .tracks
                .lock()
                .unwrap()
                .iter()
                .enumerate()
                .filter(|(_, t)| t.artist + 1 == artist)
                .map(|(i, t)| {
                    json!({ "id": i + 1, "artistId": artist, "albumId": t.album + 1,
                            "trackFileId": i + 100, "title": t.title, "duration": t.duration,
                            "explicit": t.explicit, "hasFile": true, "trackNumber": "1" })
                })
                .collect();
            Json(list).into_response()
        }
        async fn trackfile(
            State(f): State<FakeLidarr>,
            h: HeaderMap,
            Query(q): Query<HashMap<String, String>>,
        ) -> Response {
            if !authed(&h) {
                return deny();
            }
            let artist: usize = q["artistId"].parse().unwrap();
            let list: Vec<Value> = f
                .tracks
                .lock()
                .unwrap()
                .iter()
                .enumerate()
                .filter(|(_, t)| t.artist + 1 == artist)
                .map(|(i, t)| {
                    json!({ "id": i + 100, "albumId": t.album + 1,
                            "path": format!("{LIDARR_ROOT}/{}", t.file) })
                })
                .collect();
            Json(list).into_response()
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fake = FakeLidarr {
            url: format!("http://{}", listener.local_addr().unwrap()),
            tracks: Arc::new(Mutex::new(tracks())),
        };
        let app = axum::Router::new()
            .route("/api/v1/artist", get(artists))
            .route("/api/v1/album", get(albums))
            .route("/api/v1/rootfolder", get(roots))
            .route("/api/v1/track", get(track))
            .route("/api/v1/trackfile", get(trackfile))
            .with_state(fake.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        fake
    }

    fn remove(&self, title: &str) {
        self.tracks.lock().unwrap().retain(|t| t.title != title);
    }
}

/// A music folder with every fake track's file in it, except Ghost's.
fn music_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("triviabix-music-{}", uuid::Uuid::new_v4()));
    for t in tracks().iter().filter(|t| t.title != "Ghost") {
        let path = dir.join(t.file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Not real audio, but enough to stream and range over.
        let bytes: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(path, bytes).unwrap();
    }
    dir
}

fn library_config(lidarr: &FakeLidarr, music: &Path, key: &str) -> LibraryConfig {
    LibraryConfig {
        lidarr_url: Some(lidarr.url.clone()),
        api_key: Some(key.into()),
        music_dir: music.to_path_buf(),
        lidarr_music_path: None,
        rating: Rating::Pg13,
        sync_every: Duration::ZERO,
    }
}

async fn temp_db() -> Db {
    let path = std::env::temp_dir().join(format!("triviabix-test-{}.db", uuid::Uuid::new_v4()));
    Db::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap()
}

#[tokio::test]
async fn syncs_the_library_and_plays_songs_from_it() {
    let lidarr = FakeLidarr::start().await;
    let music_dir = music_dir();
    let db = temp_db().await;
    let library = Library::new(library_config(&lidarr, &music_dir, API_KEY));
    let curated = db.songs(Rating::Nc17).await.unwrap().len();

    let status = library.sync(&db).await;
    assert_eq!(status.error, None);
    assert!(status.last_sync.is_some());
    // Bohemian Rhapsody joins its curated twin; 8 more are new; Ghost is missing; the interlude,
    // the live track and the .wma are skipped.
    assert_eq!(status.songs, 9, "{status:?}");
    assert_eq!(status.missing_files, 1);
    assert_eq!(status.skipped, 3);
    let report = status.last_report.unwrap();
    assert_eq!((report.added, report.matched), (8, 1));

    let all = db.songs(Rating::Nc17).await.unwrap();
    assert_eq!(all.len(), curated + 8);
    let bo_rhap = all.iter().find(|s| s.title == "Bohemian Rhapsody").unwrap();
    assert_eq!(bo_rhap.source, "hits");
    assert_eq!(
        bo_rhap.file_path.as_deref(),
        Some(
            music_dir
                .join("Queen/A Night at the Opera/11 Bohemian Rhapsody.flac")
                .to_str()
                .unwrap()
        )
    );
    let bleep = all.iter().find(|s| s.title == "Bleep").unwrap();
    assert_eq!((bleep.source.as_str(), bleep.year), ("library", 2015));
    assert_eq!(bleep.genre.as_deref(), Some("Electronic"));
    assert_eq!(
        bleep.artwork_url.as_deref(),
        Some("https://coverart.test/4.jpg")
    );
    // Explicit tracks are MA; the rest take LIBRARY_RATING (PG-13 here).
    let pg13 = db.songs(Rating::Pg13).await.unwrap();
    assert!(!pg13.iter().any(|s| s.title == "Dirty Song"));
    assert!(pg13.iter().any(|s| s.title == "Song One"));
    assert!(
        !db.songs(Rating::Pg)
            .await
            .unwrap()
            .iter()
            .any(|s| s.title == "Song One")
    );

    // A library-only round never needs Apple, even when Apple can't be reached.
    let music = Music {
        itunes: Itunes::new("http://127.0.0.1:9", "US"),
        library: library.clone(),
        media: Default::default(),
    };
    let round = build_round(&db, &music, 6, Rating::Pg13, &[], SongSource::Library)
        .await
        .unwrap();
    assert_eq!(round.len(), 6);
    for q in &round {
        let clip = q.song.as_ref().unwrap();
        assert!(clip.from_library, "{clip:?}");
        assert!(clip.preview_url.starts_with("/media/"));
        assert!(clip.store_url.is_none());
        let song = all.iter().find(|s| s.id == q.id).unwrap();
        // Clips start between a fifth and half way in.
        let d = song.duration_ms.unwrap() as u64;
        assert!(
            clip.start_ms >= d / 5 && clip.start_ms <= d / 2,
            "{} of {d}",
            clip.start_ms
        );
        assert_ne!(q.choices[q.answer], "Dirty Song — The Fake Band");
        let path = music
            .media
            .path(clip.preview_url.trim_start_matches("/media/"));
        assert_eq!(path.as_deref().map(Path::exists), Some(true));
    }

    // Removed from Lidarr: gone from the game (the curated song falls back to Apple).
    lidarr.remove("Bleep");
    lidarr.remove("Bohemian Rhapsody");
    let status = library.sync(&db).await;
    assert_eq!(status.songs, 7);
    assert_eq!(status.last_report.unwrap().removed, 1);
    let all = db.songs(Rating::Nc17).await.unwrap();
    assert!(!all.iter().any(|s| s.title == "Bleep"));
    let bo_rhap = all.iter().find(|s| s.title == "Bohemian Rhapsody").unwrap();
    assert_eq!(bo_rhap.file_path, None);
}

#[tokio::test]
async fn sync_problems_are_reported_without_losing_the_library() {
    let lidarr = FakeLidarr::start().await;
    let music_dir = music_dir();
    let db = temp_db().await;

    let wrong_key = Library::new(library_config(&lidarr, &music_dir, "nope"));
    let status = wrong_key.sync(&db).await;
    assert!(status.error.unwrap().contains("LIDARR_API_KEY"));
    assert_eq!(status.songs, 0);

    let good = Library::new(library_config(&lidarr, &music_dir, API_KEY));
    assert_eq!(good.sync(&db).await.songs, 9);

    // The music folder isn't mounted where we expect: say so, and keep the songs we had.
    let empty = std::env::temp_dir().join(format!("triviabix-empty-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&empty).unwrap();
    let unmounted = Library::new(library_config(&lidarr, &empty, API_KEY));
    let status = unmounted.sync(&db).await;
    assert!(status.error.unwrap().contains("LIDARR_MUSIC_PATH"));
    assert_eq!(status.songs, 9);

    // Not set up at all: nothing happens.
    let off = Library::new(LibraryConfig::default());
    assert!(!off.configured());
    assert_eq!(off.sync(&db).await.error, None);
}

async fn start_server(library: LibraryConfig) -> String {
    let db = std::env::temp_dir().join(format!("triviabix-test-{}.db", uuid::Uuid::new_v4()));
    let config = Config {
        database_url: format!("sqlite://{}", db.display()),
        static_dir: concat!(env!("CARGO_MANIFEST_DIR"), "/../web").into(),
        public_url: None,
        max_players: 8,
        max_rooms: 10,
        // Apple is unreachable: every song here comes from the library.
        itunes_url: "http://127.0.0.1:9".into(),
        music_country: "US".into(),
        music_warmup: false,
        library,
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
        admin_password: Some("pw".into()),
        host_script: None,
        content_dir: concat!(env!("CARGO_MANIFEST_DIR"), "/../content").into(),
        tts_url: None,
        voice_names: Vec::new(),
        tts_cache_dir: std::env::temp_dir().join(format!("triviabix-tts-{}", uuid::Uuid::new_v4())),
        render: Default::default(),
    };
    let app = triviabix::router(config).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("127.0.0.1:{}", addr.port())
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn wait_state(ws: &mut Ws, pred: impl Fn(&Value) -> bool) -> Value {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(10), ws.next())
            .await
            .expect("timed out")
            .expect("socket closed")
            .unwrap();
        if let Message::Text(text) = msg {
            let v: Value = serde_json::from_str(&text).unwrap();
            if v["type"] == "error" {
                panic!("server error: {v}");
            }
            if v["type"] == "state" && pred(&v) {
                return v;
            }
        }
    }
}

#[tokio::test]
async fn the_tv_streams_library_songs_and_background_music() {
    let lidarr = FakeLidarr::start().await;
    let music_dir = music_dir();
    let addr = start_server(library_config(&lidarr, &music_dir, API_KEY)).await;
    let http = reqwest::Client::new();

    // Log in to the admin panel and wait for the startup sync.
    let res = http
        .post(format!("http://{addr}/api/admin/login"))
        .json(&json!({ "password": "pw" }))
        .send()
        .await
        .unwrap();
    let cookie = res.headers()["set-cookie"].to_str().unwrap();
    let cookie = cookie.split(';').next().unwrap().to_string();
    let status = async {
        loop {
            let s: Value = http
                .get(format!("http://{addr}/api/admin/music"))
                .header("cookie", &cookie)
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            if s["songs"] == 9 && s["syncing"] == false {
                return s;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    let status = tokio::time::timeout(Duration::from_secs(10), status)
        .await
        .expect("library never synced");
    assert_eq!(status["configured"], true);

    // "Sync now" from the admin panel runs in the background.
    let resync: Value = http
        .post(format!("http://{addr}/api/admin/music/sync"))
        .header("cookie", &cookie)
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resync["configured"], true);

    let room: Value = http
        .post(format!("http://{addr}/api/admin/rooms"))
        .header("cookie", &cookie)
        .json(&json!({ "mode": "songs", "rounds": 3, "max_rating": "PG-13", "song_source": "library" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let (code, token) = (
        room["code"].as_str().unwrap(),
        room["host_token"].as_str().unwrap(),
    );
    let (mut host, _) = connect_async(format!("ws://{addr}/ws/host?code={code}&token={token}"))
        .await
        .unwrap();
    let (mut phone, _) = connect_async(format!("ws://{addr}/ws/play?code={code}&name=Ann"))
        .await
        .unwrap();

    // The TV gets background music from the library, within the room's rating; phones don't.
    let lobby = wait_state(&mut host, |s| s["players"].as_array().unwrap().len() == 1).await;
    assert_eq!(lobby["settings"]["song_source"], "library");
    let playlist = lobby["playlist"].as_array().unwrap();
    assert!(!playlist.is_empty());
    assert!(
        playlist
            .iter()
            .all(|t| t["url"].as_str().unwrap().starts_with("/media/"))
    );
    assert!(!playlist.iter().any(|t| t["title"] == "Dirty Song"));
    let p = wait_state(&mut phone, |_| true).await;
    assert_eq!(p["playlist"].as_array().unwrap().len(), 0);

    use futures_util::SinkExt;
    host.send(Message::Text(json!({ "type": "start" }).to_string().into()))
        .await
        .unwrap();
    let q = wait_state(&mut host, |s| s["phase"] == "question").await;
    let url = q["question"]["clip"]["preview_url"].as_str().unwrap();
    assert!(url.starts_with("/media/"), "{q}");

    // The clip streams with range support, so the TV can seek straight to it.
    let res = http
        .get(format!("http://{addr}{url}"))
        .header("range", "bytes=100-199")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 206);
    let body = res.bytes().await.unwrap();
    assert_eq!(body.len(), 100);
    assert_eq!(body[0], 100);
    let res = http
        .get(format!("http://{addr}/media/notatoken"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);

    // The reveal credits the library instead of Apple.
    let r = wait_state(&mut host, |s| s["phase"] == "reveal").await;
    assert_eq!(r["reveal"]["song"]["from_library"], true);
    assert!(r["reveal"]["song"]["album"].is_string());
}
