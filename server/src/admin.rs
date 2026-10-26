//! The operator's admin panel API: a password login, every live room at a
//! glance, and controls for each room. Turned on by setting ADMIN_PASSWORD.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::{
    Json, Router,
    extract::{Path, Query, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::future::join_all;
use serde::Deserialize;
use serde_json::json;

use crate::content::{AdminLine, QuestionSearch};
use crate::protocol::AdminCmd;
use crate::web::{AppState, CreateRoom};

const COOKIE: &str = "tbx_admin";
const SESSION_TTL: Duration = Duration::from_secs(12 * 60 * 60);

#[derive(Clone)]
pub struct Admin {
    password: Option<Arc<str>>,
    /// Login session token -> when it expires.
    sessions: Arc<Mutex<HashMap<String, Instant>>>,
}

impl Admin {
    pub fn new(password: Option<String>) -> Self {
        Self {
            password: password.map(Into::into),
            sessions: Default::default(),
        }
    }

    fn check_password(&self, attempt: &str) -> bool {
        self.password
            .as_deref()
            .is_some_and(|p| constant_time_eq(p.as_bytes(), attempt.as_bytes()))
    }

    fn new_session(&self) -> String {
        let token = uuid::Uuid::new_v4().simple().to_string();
        let mut sessions = self.sessions.lock().unwrap();
        let now = Instant::now();
        sessions.retain(|_, expires| *expires > now);
        sessions.insert(token.clone(), now + SESSION_TTL);
        token
    }

    fn is_logged_in(&self, headers: &HeaderMap) -> bool {
        let Some(token) = session_cookie(headers) else {
            return false;
        };
        self.sessions
            .lock()
            .unwrap()
            .get(token)
            .is_some_and(|expires| *expires > Instant::now())
    }

    fn end_session(&self, headers: &HeaderMap) {
        if let Some(token) = session_cookie(headers) {
            self.sessions.lock().unwrap().remove(token);
        }
    }
}

pub(crate) fn routes(state: AppState) -> Router<AppState> {
    let protected = Router::new()
        .route("/api/admin/rooms", get(list_rooms).post(create_room))
        .route("/api/admin/rooms/{code}", post(room_action))
        .route("/api/admin/questions", get(list_questions))
        .route(
            "/api/admin/questions/{id}/lines/{slot}",
            axum::routing::put(set_lines),
        )
        .route("/api/admin/content/reload", post(reload_content))
        .route("/api/admin/voice", get(voice_status).put(set_voice_server))
        .route_layer(middleware::from_fn_with_state(state, require_login));
    Router::new()
        .route("/api/admin/session", get(session))
        .route("/api/admin/login", post(login))
        .route("/api/admin/logout", post(logout))
        .merge(protected)
}

async fn require_login(State(app): State<AppState>, req: Request, next: Next) -> Response {
    if app.admin.is_logged_in(req.headers()) {
        next.run(req).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Log in first." })),
        )
            .into_response()
    }
}

async fn session(State(app): State<AppState>, headers: HeaderMap) -> Json<serde_json::Value> {
    Json(json!({
        "enabled": app.admin.password.is_some(),
        "logged_in": app.admin.is_logged_in(&headers),
    }))
}

#[derive(Deserialize)]
struct Login {
    password: String,
}

async fn login(State(app): State<AppState>, Json(body): Json<Login>) -> Response {
    if app.admin.password.is_none() {
        let error = "The admin panel is off. Set ADMIN_PASSWORD on the server to turn it on.";
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": error })),
        )
            .into_response();
    }
    if !app.admin.check_password(&body.password) {
        // Slows down password guessing.
        tokio::time::sleep(Duration::from_millis(750)).await;
        tracing::warn!("failed admin login");
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Wrong password." })),
        )
            .into_response();
    }
    let token = app.admin.new_session();
    let cookie = format!(
        "{COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}",
        SESSION_TTL.as_secs()
    );
    let mut res = Json(json!({ "ok": true })).into_response();
    res.headers_mut()
        .insert(header::SET_COOKIE, HeaderValue::from_str(&cookie).unwrap());
    res
}

async fn logout(State(app): State<AppState>, headers: HeaderMap) -> Response {
    app.admin.end_session(&headers);
    let mut res = Json(json!({ "ok": true })).into_response();
    let cookie = format!("{COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0");
    res.headers_mut()
        .insert(header::SET_COOKIE, HeaderValue::from_str(&cookie).unwrap());
    res
}

async fn list_rooms(State(app): State<AppState>) -> Json<serde_json::Value> {
    let mut rooms: Vec<_> = join_all(app.rooms.all().iter().map(|r| r.snapshot()))
        .await
        .into_iter()
        .flatten()
        .collect();
    rooms.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.code.cmp(&b.code))
    });
    Json(json!({ "rooms": rooms }))
}

async fn create_room(State(app): State<AppState>, Json(body): Json<CreateRoom>) -> Response {
    match app.rooms.create(body.settings(), body.max_players) {
        Ok((code, host_token)) => {
            Json(json!({ "code": code, "host_token": host_token })).into_response()
        }
        Err(e) => (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": e }))).into_response(),
    }
}

async fn room_action(
    State(app): State<AppState>,
    Path(code): Path<String>,
    Json(cmd): Json<AdminCmd>,
) -> Response {
    let Some(room) = app.rooms.get(&code) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "No game with that code." })),
        )
            .into_response();
    };
    tracing::info!(%code, ?cmd, "admin action");
    match room.admin(cmd).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => (StatusCode::CONFLICT, Json(json!({ "error": e }))).into_response(),
    }
}

async fn list_questions(
    State(app): State<AppState>,
    Query(search): Query<QuestionSearch>,
) -> Response {
    match app.db.admin_questions(&search).await {
        Ok(page) => Json(page).into_response(),
        Err(e) => {
            tracing::error!("listing questions: {e:#}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Couldn't load questions." })),
            )
                .into_response()
        }
    }
}

#[derive(Deserialize)]
struct SetLines {
    lines: Vec<AdminLine>,
}

async fn set_lines(
    State(app): State<AppState>,
    Path((id, slot)): Path<(i64, String)>,
    Json(body): Json<SetLines>,
) -> Response {
    match app.db.set_edited_lines(id, &slot, &body.lines).await {
        Ok(()) => {
            tracing::info!(id, %slot, lines = body.lines.len(), "admin edited host lines");
            let host = app.host_name.clone();
            let texts = body.lines.iter().map(|l| l.text.replace("{host}", &host));
            app.voice.warm(texts.collect::<Vec<_>>(), true);
            Json(json!({ "ok": true })).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
    }
}

/// Reloads the question packs and script files from disk, e.g. after an import.
async fn reload_content(State(app): State<AppState>) -> Response {
    match crate::web::load_content(&app.db, &app.content_dir).await {
        Some(report) => {
            crate::web::warm_questions(&app.voice, &app.db, &app.host_name).await;
            crate::web::warm_expanded(&app.voice, &app.db, &app.script, &app.voice_names).await;
            Json(report).into_response()
        }
        None => (
            StatusCode::CONFLICT,
            Json(json!({ "error": "Couldn't load the question packs; the server log says why." })),
        )
            .into_response(),
    }
}

fn session_cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|pair| pair.trim().strip_prefix(COOKIE)?.strip_prefix('='))
}

/// Compares without stopping at the first difference, so timing doesn't leak the password.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = a.len() ^ b.len();
    for (i, x) in a.iter().enumerate() {
        diff |= (*x ^ b.get(i).copied().unwrap_or(0)) as usize;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords_compare_exactly() {
        assert!(constant_time_eq(b"hunter2", b"hunter2"));
        assert!(!constant_time_eq(b"hunter2", b"hunter"));
        assert!(!constant_time_eq(b"hunter2", b"hunter22"));
        assert!(!constant_time_eq(b"hunter2", b"Hunter2"));
    }

    #[test]
    fn finds_the_session_cookie() {
        let mut h = HeaderMap::new();
        h.insert(
            header::COOKIE,
            HeaderValue::from_static("a=1; tbx_admin=abc; b=2"),
        );
        assert_eq!(session_cookie(&h), Some("abc"));
        h.insert(header::COOKIE, HeaderValue::from_static("tbx_admin_x=zzz"));
        assert_eq!(session_cookie(&h), None);
    }
}

/// Where Bix's new lines come from and how many are still waiting to be made.
async fn voice_status(State(app): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({
        "server": app.voice.server(),
        "backlog": app.voice.backlog(),
        "in_flight": app.voice.in_flight(),
        "enabled": app.voice.enabled(),
    }))
}

#[derive(Deserialize)]
struct VoiceServer {
    /// `None` goes back to `TTS_URL`.
    url: Option<String>,
    token: Option<String>,
}

/// Points the voice at another TTS service, such as one on a rented GPU (`gpu/vast.py voice`).
async fn set_voice_server(State(app): State<AppState>, Json(body): Json<VoiceServer>) -> Response {
    let url = body.url.as_deref().map(str::trim).filter(|u| !u.is_empty());
    if let Some(url) = url
        && !(url.starts_with("http://") || url.starts_with("https://"))
    {
        let error = "The voice server address should start with http:// or https://.";
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
    }
    app.voice.set_server(url, body.token.as_deref());
    if url.is_some() {
        // New regulars since startup get their lines too.
        crate::web::warm_expanded(&app.voice, &app.db, &app.script, &app.voice_names).await;
    }
    tracing::info!(
        "voice server is now {}",
        app.voice.server().as_deref().unwrap_or("none")
    );
    voice_status(State(app)).await.into_response()
}
