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
use tower_http::compression::{
    CompressionLayer,
    predicate::{DefaultPredicate, NotForContentType, Predicate},
};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;
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
    pub script: std::sync::Arc<crate::host::Script>,
    /// VOICE_NAMES
    pub voice_names: std::sync::Arc<[String]>,
    pub render: crate::render::Render,
    pub settings: crate::settings::Settings,
    pub packs: crate::packs::Packs,
    pub personas: crate::personas::Personas,
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
    let script = std::sync::Arc::new(script);
    // Keys and addresses saved on the Settings page win over the environment's.
    let settings = crate::settings::Settings::load(db.clone())
        .await?
        .starting_with("TTS_URL", config.tts_url.clone())
        .starting_with("LIDARR_URL", config.library.lidarr_url.clone())
        .starting_with("LIDARR_API_KEY", config.library.api_key.clone());
    let tts_url = settings.get("TTS_URL");
    let voice = Voice::new(tts_url.as_deref(), &config.tts_cache_dir);
    let host_name: std::sync::Arc<str> = script.host_name.as_str().into();
    if let Some(url) = &tts_url {
        tracing::info!(
            "host voice from {url}, cached in {}",
            config.tts_cache_dir.display()
        );
    }
    // Lines play from the voice pack in use (BIX_VOICE), and only what it lacks is queued.
    let packs = crate::packs::Packs::new(
        db.clone(),
        settings.clone(),
        voice.clone(),
        &config.render.assets_dir,
    );
    if let Err(e) = packs.sync().await {
        tracing::warn!("voice packs: {e}");
    }
    // Queued even with no TTS service, for one set later (such as a rented GPU, see gpu/).
    voice.warm(script.fixed_lines(), false);
    warm_questions(&voice, &db, &host_name).await;
    warm_expanded(&voice, &db, &script, &config.voice_names).await;
    let itunes = Itunes::new(&config.itunes_url, &config.music_country);
    if config.music_warmup {
        tokio::spawn(music::warm_up(db.clone(), itunes.clone()));
    }
    let library = Library::new(crate::library::LibraryConfig {
        lidarr_url: settings.get("LIDARR_URL"),
        api_key: settings.get("LIDARR_API_KEY"),
        ..config.library.clone()
    });
    let music = Music {
        itunes,
        library,
        media: Default::default(),
    };
    let rooms = Rooms::new(
        db.clone(),
        script.clone(),
        music.clone(),
        voice.clone(),
        config.timing.clone(),
        settings.clone(),
        config.max_players,
        config.max_rooms,
    );
    match &config.admin_password {
        None => tracing::info!("ADMIN_PASSWORD is not set, so the admin panel is off"),
        Some(p) if p.chars().count() < 12 => tracing::warn!(
            "ADMIN_PASSWORD is short; if the game can be reached from the internet, use 12 \
             characters or more"
        ),
        Some(_) => {}
    }
    let render = crate::render::Render::new(
        config.render.clone(),
        config.content_dir.clone(),
        config.static_dir.clone(),
        db.clone(),
        voice.clone(),
        settings.clone(),
    );
    render.recover().await;
    let personas = crate::personas::Personas::new(
        db.clone(),
        settings.clone(),
        render.clone(),
        &config.content_dir,
    );
    if let Err(e) = personas.sync().await {
        tracing::warn!("personas: {e}");
    }
    // After the startup writes above, so its first sync doesn't find the database busy.
    tokio::spawn(music.library.clone().run(db.clone()));
    let state = AppState {
        rooms,
        db,
        content_dir: config.content_dir.clone(),
        public_url: config.public_url.clone(),
        admin: Admin::new(config.admin_password.clone()),
        music,
        voice,
        host_name,
        script,
        voice_names: config.voice_names.clone().into(),
        render: render.clone(),
        settings,
        packs,
        personas,
    };
    // New questions and Claude's host lines are loaded and queued for the voice as soon as
    // they're written.
    tokio::spawn({
        let app = state.clone();
        async move {
            let mut done = render.finished();
            while let Ok(kind) = done.recv().await {
                if matches!(
                    kind,
                    crate::render::Kind::Lines | crate::render::Kind::Import
                ) {
                    load_content(&app.db, &app.content_dir).await;
                    warm_questions(&app.voice, &app.db, &app.host_name).await;
                    warm_expanded(&app.voice, &app.db, &app.script, &app.voice_names).await;
                }
                // A voice run may have filled a voice no pack has yet.
                if kind == crate::render::Kind::Voice
                    && let Err(e) = app.packs.sync().await
                {
                    tracing::warn!("voice packs: {e}");
                }
            }
        }
    });

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
        // A short address to type with a TV remote.
        .route_service("/tv", ServeFile::new(dir.join("host.html")))
        .route_service("/admin", ServeFile::new(dir.join("admin.html")))
        .route_service(
            "/admin/questions",
            ServeFile::new(dir.join("admin-questions.html")),
        )
        .route_service(
            "/admin/characters",
            ServeFile::new(dir.join("admin-characters.html")),
        )
        .route_service(
            "/admin/duplicates",
            ServeFile::new(dir.join("admin-duplicates.html")),
        )
        .route_service(
            "/admin/pipeline",
            ServeFile::new(dir.join("admin-pipeline.html")),
        )
        .route_service(
            "/admin/settings",
            ServeFile::new(dir.join("admin-settings.html")),
        )
        // The old address of the Pipeline page, from before it imported trivia.
        .route_service(
            "/admin/render",
            ServeFile::new(dir.join("admin-pipeline.html")),
        )
        // Art made from the admin panel lives in appdata, in the theme in use (crate::art); the
        // art the server ships is the fallback.
        .nest("/cutscenes/assets/gen", Router::new().fallback(themed_art))
        // Every theme's files by their own path, for the admin Art page.
        .nest_service(
            "/art-files",
            ServeDir::new(&config.render.art_dir)
                .fallback(ServeDir::new(dir.join("cutscenes/assets/gen"))),
        )
        .route_service("/admin/art", ServeFile::new(dir.join("admin-art.html")))
        .fallback_service(ServeDir::new(dir))
        // Browsers check the pages and scripts are current on every load (a cheap 304 when they
        // are), so an update reaches the admin panel and TV without a hard refresh.
        .layer(SetResponseHeaderLayer::if_not_present(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("no-cache"),
        ))
        // Pages, scripts and JSON shrink a lot, which matters when the TV is a long way from the
        // server (playing over the internet). Audio, video, pictures and the app are already packed,
        // and the music library's clips are sent in ranges, so those go as they are.
        .layer(
            CompressionLayer::new().compress_when(
                DefaultPredicate::new()
                    .and(NotForContentType::const_new("audio/"))
                    .and(NotForContentType::const_new("video/"))
                    .and(NotForContentType::const_new("application/vnd.android")),
            ),
        )
        .layer(TraceLayer::new_for_http())
        .with_state(state))
}

/// The TV's generated art, in the theme in use: its own files first, then the default theme's,
/// then the art the server ships. Its manifest lists both, the theme's winning.
async fn themed_art(State(app): State<AppState>, req: axum::extract::Request) -> Response {
    use tower::ServiceExt;
    let render = &app.render;
    let theme = render.art_theme();
    let (art, shipped) = (render.art_dir().to_path_buf(), render.shipped_art());
    let base = ServeDir::new(&art).fallback(ServeDir::new(&shipped));
    if theme == crate::art::DEFAULT_THEME {
        return base.oneshot(req).await.into_response();
    }
    if req.uri().path() == "/manifest.json"
        && let Some(m) = crate::art::merged_manifest(&art, &shipped, &theme)
    {
        return Json(m).into_response();
    }
    ServeDir::new(render.theme_dir(&theme))
        .fallback(base)
        .oneshot(req)
        .await
        .into_response()
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
    Json(json!({
        "public_url": app.public_url,
        "voice": app.voice.enabled(),
        "voice_speed": voice_speed(&app.settings),
    }))
}

/// How fast the TV plays Bix's lines (BIX_SPEED), 1 being as made.
pub(crate) fn voice_speed(settings: &crate::settings::Settings) -> f64 {
    settings
        .get("BIX_SPEED")
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite())
        .map_or(1.0, |v| v.clamp(0.5, 2.0))
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

/// Queues every line the game can know ahead, such as after Bix's voice changes, so the next
/// voice run makes them all in the new voice.
pub(crate) async fn warm_all(app: &AppState) {
    app.voice.clear_queue();
    app.voice.warm(app.script.fixed_lines(), false);
    warm_questions(&app.voice, &app.db, &app.host_name).await;
    warm_expanded(&app.voice, &app.db, &app.script, &app.voice_names).await;
}

/// Queues the host's lines with categories, answers and regulars' names filled in, last, so a
/// batch run leaves little for a game to need from a live TTS service.
pub(crate) async fn warm_expanded(
    voice: &Voice,
    db: &Db,
    script: &crate::host::Script,
    names: &[String],
) {
    let lines = crate::pregen::lines(script, db, names).await;
    tracing::info!(
        "voice: {} lines with names, categories and answers to make ahead",
        lines.len()
    );
    voice.warm(lines, false);
}

/// Queues every question and question line for the voice, behind anything already waiting.
pub(crate) async fn warm_questions(voice: &Voice, db: &Db, host_name: &str) {
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
    minigames: Option<bool>,
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
            minigames: self.minigames.unwrap_or(true),
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
