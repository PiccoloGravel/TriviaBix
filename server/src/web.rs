//! HTTP routes and the WebSocket bridge between browsers and room tasks.

use std::time::Duration;

use axum::{
    Json, Router,
    extract::{
        Path, Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

use crate::Config;
use crate::admin::{self, Admin};
use crate::db::{Db, Rating};
use crate::library::Library;
use crate::music::{self, Itunes, Music};
use crate::protocol::{ClientMsg, GameMode, ServerMsg, SongSource};
use crate::room::{
    Join, MAX_CLIP_MS, MAX_ROUNDS, MIN_CLIP_MS, MIN_ROUNDS, RoomCmd, RoomHandle, Settings,
    clamp_teams,
};
use crate::rooms::Rooms;
use crate::rounds;
use crate::voice::{Voice, VoiceError};

#[derive(Clone)]
pub(crate) struct AppState {
    pub rooms: Rooms,
    pub db: Db,
    pub content_dir: std::path::PathBuf,
    pub public_url: Option<String>,
    pub admin: Admin,
    pub music: Music,
    pub voice: Voice,
    /// What `{host}` reads as in question lines.
    pub host_name: std::sync::Arc<str>,
}

pub async fn router(config: Config) -> anyhow::Result<Router> {
    let db = Db::connect(&config.database_url).await?;
    load_content(&db, &config.content_dir).await;
    tracing::info!(
        "question bank has {} playable questions",
        db.question_count().await?
    );
    let script = match &config.host_script {
        Some(path) => crate::host::Script::load(path)?,
        None => crate::host::Script::builtin(),
    };
    let voice = Voice::new(config.tts_url.as_deref(), &config.tts_cache_dir);
    let host_name: std::sync::Arc<str> = script.host_name.as_str().into();
    if voice.enabled() {
        tracing::info!(
            "host voice from {}, cached in {}",
            config.tts_url.as_deref().unwrap_or_default(),
            config.tts_cache_dir.display()
        );
        voice.warm(script.fixed_lines(), false);
        warm_questions(&voice, &db, &host_name).await;
    }
    let itunes = Itunes::new(&config.itunes_url, &config.music_country);
    if config.music_warmup {
        tokio::spawn(music::warm_up(db.clone(), itunes.clone()));
    }
    let library = Library::new(config.library.clone());
    tokio::spawn(library.clone().run(db.clone()));
    let music = Music {
        itunes,
        library,
        media: Default::default(),
    };
    let rooms = Rooms::new(
        db.clone(),
        script,
        music.clone(),
        voice.clone(),
        config.timing.clone(),
        config.max_players,
        config.max_rooms,
    );
    if config.admin_password.is_none() {
        tracing::info!("ADMIN_PASSWORD is not set, so the admin panel is off");
    }
    let state = AppState {
        rooms,
        db,
        content_dir: config.content_dir.clone(),
        public_url: config.public_url.clone(),
        admin: Admin::new(config.admin_password.clone()),
        music,
        voice,
        host_name,
    };

    let dir = &config.static_dir;
    Ok(Router::new()
        .route("/healthz", get(health))
        .route("/api/config", get(client_config))
        .route("/api/qr", get(qr_svg))
        .route(
            "/api/characters",
            get(|| async { Json(crate::characters::CHARACTERS) }),
        )
        .route("/api/voice/{hash}", get(voice_line))
        .route("/api/rooms", post(create_room))
        .route("/api/rooms/{code}", get(room_info))
        .route("/media/{token}", get(media))
        .route("/ws/host", get(ws_host))
        .route("/ws/play", get(ws_play))
        .merge(admin::routes(state.clone()))
        .route_service("/host", ServeFile::new(dir.join("host.html")))
        .route_service("/admin", ServeFile::new(dir.join("admin.html")))
        .route_service(
            "/admin/questions",
            ServeFile::new(dir.join("admin-questions.html")),
        )
        .fallback_service(ServeDir::new(dir))
        .layer(TraceLayer::new_for_http())
        .with_state(state))
}

async fn health(State(app): State<AppState>) -> Response {
    match app.db.question_count().await {
        Ok(questions) => {
            Json(json!({ "ok": true, "rooms": app.rooms.len(), "questions": questions }))
                .into_response()
        }
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "ok": false, "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// Tells the TV which address phones should open. Behind a proxy or when the
/// TV is on `localhost`, set PUBLIC_URL to the address phones can reach.
async fn client_config(State(app): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({ "public_url": app.public_url, "voice": app.voice.enabled() }))
}

/// One of the host's lines as audio, made by the TTS service the first time it's asked for.
async fn voice_line(State(app): State<AppState>, Path(hash): Path<String>) -> Response {
    match app.voice.audio(&hash).await {
        Ok((bytes, content_type)) => (
            [
                (axum::http::header::CONTENT_TYPE, content_type),
                (axum::http::header::CACHE_CONTROL, "max-age=600"),
            ],
            bytes,
        )
            .into_response(),
        Err(VoiceError::NotFound) => StatusCode::NOT_FOUND.into_response(),
        Err(VoiceError::Unavailable(e)) => {
            tracing::warn!("voice: {e}");
            (StatusCode::SERVICE_UNAVAILABLE, e).into_response()
        }
    }
}

/// Queues every question and question line for the voice, behind anything already waiting.
pub(crate) async fn warm_questions(voice: &Voice, db: &Db, host_name: &str) {
    if !voice.enabled() {
        return;
    }
    match db.voice_texts().await {
        Ok(texts) => {
            voice.warm(
                texts.into_iter().map(|t| t.replace("{host}", host_name)),
                false,
            );
            tracing::info!("voice: {} lines waiting to be made", voice.backlog());
        }
        Err(e) => tracing::error!("listing lines for the voice: {e:#}"),
    }
}

#[derive(Deserialize)]
struct QrQuery {
    text: String,
}

/// A QR code for the join link, so players can scan the TV instead of typing.
async fn qr_svg(Query(q): Query<QrQuery>) -> Response {
    if q.text.len() > 512 {
        return StatusCode::BAD_REQUEST.into_response();
    }
    match qrcode::QrCode::new(q.text.as_bytes()) {
        Ok(code) => {
            let svg = code
                .render::<qrcode::render::svg::Color>()
                .quiet_zone(true)
                .min_dimensions(256, 256)
                .build();
            ([(axum::http::header::CONTENT_TYPE, "image/svg+xml")], svg).into_response()
        }
        Err(_) => StatusCode::BAD_REQUEST.into_response(),
    }
}

#[derive(Deserialize, Default)]
pub(crate) struct CreateRoom {
    rounds: Option<usize>,
    max_rating: Option<Rating>,
    mode: Option<GameMode>,
    clip_ms: Option<u64>,
    teams: Option<u8>,
    late_join: Option<bool>,
    trivia_rounds: Option<u8>,
    per_round: Option<usize>,
    song_source: Option<SongSource>,
    /// Admin panel only; the public endpoint ignores it.
    pub(crate) max_players: Option<usize>,
}

impl CreateRoom {
    pub fn settings(&self) -> Settings {
        Settings {
            rounds: self.rounds.unwrap_or(10).clamp(MIN_ROUNDS, MAX_ROUNDS),
            max_rating: self.max_rating.unwrap_or_default(),
            mode: self.mode.unwrap_or_default(),
            clip_ms: self
                .clip_ms
                .unwrap_or(2_000)
                .clamp(MIN_CLIP_MS, MAX_CLIP_MS),
            teams: clamp_teams(self.teams.unwrap_or(0)),
            late_join: self.late_join.unwrap_or(true),
            trivia_rounds: self
                .trivia_rounds
                .unwrap_or(rounds::MAX_ROUNDS)
                .clamp(rounds::MIN_ROUNDS, rounds::MAX_ROUNDS),
            per_round: self
                .per_round
                .unwrap_or(5)
                .clamp(rounds::MIN_PER_ROUND, rounds::MAX_PER_ROUND),
            song_source: self.song_source.unwrap_or_default(),
        }
    }
}

async fn create_room(State(app): State<AppState>, body: Option<Json<CreateRoom>>) -> Response {
    let body = body.map(|b| b.0).unwrap_or_default();
    match app.rooms.create(body.settings(), None) {
        Ok((code, host_token)) => {
            Json(json!({ "code": code, "host_token": host_token })).into_response()
        }
        Err(e) => (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": e }))).into_response(),
    }
}

async fn room_info(State(app): State<AppState>, Path(code): Path<String>) -> Response {
    let info = match app.rooms.get(&code) {
        Some(room) => room.info().await,
        None => None,
    };
    match info {
        Some(info) => Json(info).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "No game with that code." })),
        )
            .into_response(),
    }
}

/// Streams a music library file to the TV. Links come from [`crate::library::Media`]; range
/// requests work, so the TV can jump straight to the clip.
async fn media(
    State(app): State<AppState>,
    Path(token): Path<String>,
    req: axum::extract::Request,
) -> Response {
    use tower::ServiceExt;
    let Some(path) = app.music.media.path(&token) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match ServeFile::new(path).oneshot(req).await {
        Ok(res) => res.into_response(),
        Err(e) => match e {},
    }
}

#[derive(Deserialize)]
struct HostQuery {
    code: String,
    token: String,
}

#[derive(Deserialize)]
struct PlayQuery {
    code: String,
    name: Option<String>,
    token: Option<String>,
    character: Option<String>,
}

async fn ws_host(
    ws: WebSocketUpgrade,
    State(app): State<AppState>,
    Query(q): Query<HostQuery>,
) -> Response {
    let room = app.rooms.get(&q.code);
    ws.on_upgrade(move |socket| run_socket(socket, room, Join::Host { token: q.token }))
}

async fn ws_play(
    ws: WebSocketUpgrade,
    State(app): State<AppState>,
    Query(q): Query<PlayQuery>,
) -> Response {
    let room = app.rooms.get(&q.code);
    ws.on_upgrade(move |socket| {
        run_socket(
            socket,
            room,
            Join::Player {
                name: q.name,
                token: q.token,
                character: q.character,
            },
        )
    })
}

/// Pumps messages between one browser socket and its room until either side goes away.
async fn run_socket(mut socket: WebSocket, room: Option<RoomHandle>, join: Join) {
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let joined = match room {
        Some(room) => room.connect(join, tx).await.map(|id| (room, id)),
        None => Err("No game with that code. Check the TV!".to_string()),
    };
    let (room, conn_id) = match joined {
        Ok(ok) => ok,
        Err(message) => {
            let msg = ServerMsg::Error {
                message: &message,
                fatal: true,
            };
            let _ = socket
                .send(Message::Text(serde_json::to_string(&msg).unwrap().into()))
                .await;
            let _ = socket.close().await;
            return;
        }
    };

    let (mut sink, mut stream) = socket.split();
    // Pings keep idle sockets alive through proxies and TV browsers.
    let mut ping = tokio::time::interval(Duration::from_secs(20));
    loop {
        tokio::select! {
            out = rx.recv() => match out {
                Some(text) => {
                    if sink.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
                // The room dropped this connection (kicked, or the room closed).
                None => {
                    let _ = sink.close().await;
                    break;
                }
            },
            incoming = stream.next() => match incoming {
                Some(Ok(Message::Text(text))) => match serde_json::from_str::<ClientMsg>(&text) {
                    Ok(msg) => room.send(RoomCmd::Msg { conn_id, msg }),
                    Err(_) => tracing::debug!("ignoring bad message: {text}"),
                },
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            _ = ping.tick() => {
                if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            }
        }
    }
    room.send(RoomCmd::Disconnect { conn_id });
}

/// Loads the content packs into the database. A broken pack is logged, not fatal: the server
/// keeps playing whatever the database already has.
pub async fn load_content(db: &Db, dir: &std::path::Path) -> Option<crate::content::SyncReport> {
    if !dir.join("categories.toml").exists() {
        tracing::info!(
            "no question packs at {}; playing what the database has",
            dir.display()
        );
        return None;
    }
    match crate::content::load(db, dir).await {
        Ok(report) => {
            tracing::info!(
                "loaded question packs from {}: {} new, {} updated, {} retired, {} host lines",
                dir.display(),
                report.added,
                report.updated,
                report.retired,
                report.lines
            );
            for w in &report.warnings {
                tracing::warn!("content: {w}");
            }
            Some(report)
        }
        Err(e) => {
            tracing::error!("question packs not loaded: {e:#}");
            None
        }
    }
}
