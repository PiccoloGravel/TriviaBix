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
use crate::db::{Db, Rating};
use crate::music::{self, Itunes};
use crate::protocol::{ClientMsg, ServerMsg};
use crate::room::{
    Join, MAX_CLIP_MS, MAX_ROUNDS, MIN_CLIP_MS, MIN_ROUNDS, Mode, RoomCmd, RoomHandle, Settings,
};
use crate::rooms::Rooms;

#[derive(Clone)]
struct AppState {
    rooms: Rooms,
    db: Db,
    public_url: Option<String>,
}

pub async fn router(config: Config) -> anyhow::Result<Router> {
    let db = Db::connect(&config.database_url).await?;
    tracing::info!("question bank has {} questions", db.question_count().await?);
    let itunes = Itunes::new(&config.itunes_url, &config.music_country);
    if config.music_warmup {
        tokio::spawn(music::warm_up(db.clone(), itunes.clone()));
    }
    let rooms = Rooms::new(
        db.clone(),
        itunes,
        config.timing.clone(),
        config.max_players,
        config.max_rooms,
    );
    let state = AppState {
        rooms,
        db,
        public_url: config.public_url.clone(),
    };

    let dir = &config.static_dir;
    Ok(Router::new()
        .route("/healthz", get(health))
        .route("/api/config", get(client_config))
        .route("/api/qr", get(qr_svg))
        .route("/api/rooms", post(create_room))
        .route("/api/rooms/{code}", get(room_info))
        .route("/ws/host", get(ws_host))
        .route("/ws/play", get(ws_play))
        .route_service("/host", ServeFile::new(dir.join("host.html")))
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
    Json(json!({ "public_url": app.public_url }))
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
struct CreateRoom {
    rounds: Option<usize>,
    max_rating: Option<Rating>,
    mode: Option<Mode>,
    clip_ms: Option<u64>,
}

async fn create_room(State(app): State<AppState>, body: Option<Json<CreateRoom>>) -> Response {
    let body = body.map(|b| b.0).unwrap_or_default();
    let settings = Settings {
        rounds: body.rounds.unwrap_or(10).clamp(MIN_ROUNDS, MAX_ROUNDS),
        max_rating: body.max_rating.unwrap_or_default(),
        mode: body.mode.unwrap_or_default(),
        clip_ms: body
            .clip_ms
            .unwrap_or(2_000)
            .clamp(MIN_CLIP_MS, MAX_CLIP_MS),
    };
    match app.rooms.create(settings) {
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
