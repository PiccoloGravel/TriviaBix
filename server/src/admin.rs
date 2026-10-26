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
        .route("/api/admin/duplicates", get(list_duplicates))
        .route("/api/admin/duplicates/keep", post(keep_pair))
        .route("/api/admin/questions/{id}/hidden", post(set_hidden))
        .route("/api/admin/voice", get(voice_status).put(set_voice_server))
        .route("/api/admin/music", get(music_status))
        .route("/api/admin/music/sync", post(music_sync))
        .route("/api/admin/render", get(render_status))
        .route("/api/admin/render/preview", post(render_preview))
        .route("/api/admin/render/start", post(render_start))
        .route("/api/admin/render/cleanup", post(render_cleanup))
        .route("/api/admin/render/import", post(render_import))
        .route("/api/admin/pipeline", get(pipeline))
        .route("/api/admin/pipeline/pack", post(upload_pack))
        .route("/api/admin/settings", get(settings_view).put(settings_set))
        .route("/api/admin/settings/test", post(settings_test))
        .route("/api/admin/voices", get(list_voices))
        .route("/api/admin/voices/{key}/clip", get(voice_clip))
        .route("/api/admin/voices/{key}/use", post(use_voice))
        .route("/api/admin/packs", get(list_packs).post(create_pack))
        .route("/api/admin/packs/{id}", axum::routing::put(update_pack))
        .route("/api/admin/packs/{id}/use", post(use_pack))
        .route("/api/admin/packs/{id}/sample", get(pack_sample))
        .route(
            "/api/admin/personas",
            get(list_personas).post(create_persona),
        )
        .route(
            "/api/admin/personas/{id}",
            axum::routing::put(update_persona),
        )
        .route("/api/admin/personas/{id}/use", post(use_persona))
        .route("/api/admin/render/jobs/{id}", get(render_job))
        .route("/api/admin/render/jobs/{id}/cancel", post(render_cancel))
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

/// Questions that look like the same question asked twice, and the ones hidden as duplicates.
async fn list_duplicates(State(app): State<AppState>) -> Response {
    match app.db.duplicate_report(&app.content_dir).await {
        Ok(report) => Json(report).into_response(),
        Err(e) => {
            tracing::error!("finding duplicate questions: {e:#}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Couldn't check for duplicates." })),
            )
                .into_response()
        }
    }
}

#[derive(Deserialize)]
struct KeepPair {
    a: i64,
    b: i64,
}

/// Marks two look-alike questions as different, so both stay and the pair stops showing up.
async fn keep_pair(State(app): State<AppState>, Json(body): Json<KeepPair>) -> Response {
    match app.db.keep_question_pair(body.a, body.b).await {
        Ok(()) => {
            tracing::info!(a = body.a, b = body.b, "admin kept a look-alike pair");
            Json(json!({ "ok": true })).into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("Couldn't keep that pair: {e}") })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct SetHidden {
    hidden: bool,
}

/// Hides a question as a duplicate (it stops being played, even after a reload) or brings it back.
async fn set_hidden(
    State(app): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<SetHidden>,
) -> Response {
    match app.db.set_question_hidden(id, body.hidden).await {
        Ok(true) => {
            tracing::info!(
                id,
                hidden = body.hidden,
                "admin changed a question's visibility"
            );
            Json(json!({ "ok": true })).into_response()
        }
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "No question with that id." })),
        )
            .into_response(),
        Err(e) => {
            tracing::error!("hiding question {id}: {e:#}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Couldn't save that." })),
            )
                .into_response()
        }
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

/// The music library: whether Lidarr is set up, how many songs it gave us, and what went wrong.
async fn music_status(State(app): State<AppState>) -> Json<crate::library::LibraryStatus> {
    Json(app.music.library.status(&app.db).await)
}

/// Syncs the music library from Lidarr now, e.g. after adding albums. Runs in the background;
/// poll `GET /api/admin/music` until `syncing` is false.
async fn music_sync(State(app): State<AppState>) -> Response {
    if !app.music.library.configured() {
        let error = "Set Lidarr's address and API key in Settings to use your music library.";
        return (StatusCode::CONFLICT, Json(json!({ "error": error }))).into_response();
    }
    tracing::info!("admin started a music library sync");
    Json(app.music.library.start_sync(&app.db).await).into_response()
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

#[derive(Deserialize)]
struct RenderQuery {
    #[serde(default)]
    refresh: bool,
    #[serde(default)]
    since: u64,
}

/// The Rendering page: what's set up, what's waiting to be made, the running job and past ones.
async fn render_status(
    State(app): State<AppState>,
    Query(q): Query<RenderQuery>,
) -> Json<serde_json::Value> {
    let r = &app.render;
    Json(json!({
        "ready": r.ready(),
        "missing": r.missing(q.refresh).await,
        "job": r.current(q.since),
        "history": r.history(20).await,
        "spent": r.spent().await,
    }))
}

#[derive(Deserialize)]
struct PreviewBody {
    kind: crate::render::Kind,
    #[serde(default)]
    options: crate::render::Options,
}

/// A dry run: what would be made, on which GPU, at what price, and the most it can cost.
async fn render_preview(State(app): State<AppState>, Json(body): Json<PreviewBody>) -> Response {
    match app.render.preview(body.kind, body.options).await {
        Ok(view) => Json(view).into_response(),
        Err(e) => (StatusCode::CONFLICT, Json(json!({ "error": e }))).into_response(),
    }
}

#[derive(Deserialize)]
struct StartBody {
    token: String,
    /// The admin saw the price and said yes.
    #[serde(default)]
    confirm: bool,
}

async fn render_start(State(app): State<AppState>, Json(body): Json<StartBody>) -> Response {
    if !body.confirm {
        let error = "Confirm the price first.";
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
    }
    match app.render.start(&body.token).await {
        Ok(job) => Json(job).into_response(),
        Err(e) => (StatusCode::CONFLICT, Json(json!({ "error": e }))).into_response(),
    }
}

/// Destroys every TriviaBix instance on the vast.ai account, e.g. one left from the command line.
async fn render_cleanup(State(app): State<AppState>) -> Response {
    match app.render.cleanup().await {
        Ok(job) => Json(job).into_response(),
        Err(e) => (StatusCode::CONFLICT, Json(json!({ "error": e }))).into_response(),
    }
}

async fn render_job(
    State(app): State<AppState>,
    Path(id): Path<i64>,
    Query(q): Query<RenderQuery>,
) -> Response {
    match app.render.job(id, q.since).await {
        Some(job) => Json(job).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "No such job." })),
        )
            .into_response(),
    }
}

async fn render_cancel(State(app): State<AppState>, Path(id): Path<i64>) -> Response {
    match app.render.cancel(id) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => (StatusCode::CONFLICT, Json(json!({ "error": e }))).into_response(),
    }
}

/// Starts an import from a trivia site. It's free, so there's no price check.
async fn render_import(
    State(app): State<AppState>,
    Json(options): Json<crate::render::Options>,
) -> Response {
    match app.render.import(options).await {
        Ok(job) => Json(job).into_response(),
        Err(e) => (StatusCode::CONFLICT, Json(json!({ "error": e }))).into_response(),
    }
}

/// Each recent import and how far its questions have got through lines and voice.
async fn pipeline(State(app): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({
        "batches": app.render.batches(&app.host_name).await,
        "questions": app.db.question_count().await.unwrap_or(0),
    }))
}

#[derive(Deserialize)]
struct PackUpload {
    name: String,
    toml: String,
}

/// Saves a pack file someone uploaded into content/packs/mine.
async fn upload_pack(State(app): State<AppState>, Json(body): Json<PackUpload>) -> Response {
    match app.render.upload_pack(&body.name, &body.toml).await {
        Ok(res) => Json(res).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
    }
}

/// The Settings page: every key, address and model, where its value comes from, and the values
/// of the ones that aren't secret.
async fn settings_view(State(app): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({ "fields": app.settings.view() }))
}

#[derive(Deserialize)]
struct SettingBody {
    key: String,
    /// Empty or missing forgets the saved value, so the container's is used again.
    value: Option<String>,
}

async fn settings_set(State(app): State<AppState>, Json(body): Json<SettingBody>) -> Response {
    if body.key == "BIX_VOICE" && voice_running(&app) {
        return (StatusCode::CONFLICT, Json(json!({ "error": SWITCH_LATER }))).into_response();
    }
    if let Err(e) = app.settings.set(&body.key, body.value.as_deref()).await {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response();
    }
    // The key's name only: never its value.
    tracing::info!("admin changed the {} setting", body.key);
    let s = &app.settings;
    match body.key.as_str() {
        "TTS_URL" => app.voice.set_default(s.get("TTS_URL").as_deref()),
        "BIX_VOICE" => voice_changed(&app).await,
        "HOST_PERSONA" => {
            if let Err(e) = app.personas.sync().await {
                tracing::warn!("personas: {e}");
            }
        }
        "BIX_SPEED" => {
            if let Err(e) = app.packs.sync().await {
                tracing::warn!("voice packs: {e}");
            }
        }
        "LIDARR_URL" | "LIDARR_API_KEY" => app
            .music
            .library
            .set_lidarr(s.get("LIDARR_URL"), s.get("LIDARR_API_KEY")),
        // Jobs read the rest when they start; art counts depend on the models.
        _ => app.render.forget_missing().await,
    }
    Json(json!({ "fields": s.view() })).into_response()
}

#[derive(Deserialize)]
struct TestBody {
    service: String,
}

/// Checks that a service answers with the settings in use, without spending anything.
async fn settings_test(State(app): State<AppState>, Json(body): Json<TestBody>) -> Response {
    match app.settings.test(&body.service).await {
        Ok(message) => Json(json!({ "ok": true, "message": message })).into_response(),
        Err(error) => Json(json!({ "ok": false, "error": error })).into_response(),
    }
}

/// The candidate voices designed for Bix, and the voice in use.
async fn list_voices(State(app): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({
        "voices": app.render.voices(),
        "bix_voice": app.settings.get("BIX_VOICE"),
        "speed": crate::web::voice_speed(&app.settings),
    }))
}

/// A candidate voice's clip, to listen to before picking it.
async fn voice_clip(State(app): State<AppState>, Path(key): Path<String>) -> Response {
    let Some(path) = app.render.voice_clip(&key) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match tokio::fs::read(&path).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, "audio/wav")], bytes).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Makes a candidate Bix's voice. Lines are made again in it the next time the voice runs, and
/// the old voice's lines keep playing until then.
async fn use_voice(State(app): State<AppState>, Path(key): Path<String>) -> Response {
    if app.render.voice_clip(&key).is_none() {
        let error = "There's no such voice in the asset store.";
        return (StatusCode::NOT_FOUND, Json(json!({ "error": error }))).into_response();
    }
    if voice_running(&app) {
        return (StatusCode::CONFLICT, Json(json!({ "error": SWITCH_LATER }))).into_response();
    }
    let spec = format!("clone:{key}");
    if let Err(e) = app.settings.set("BIX_VOICE", Some(&spec)).await {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response();
    }
    tracing::info!("admin picked Bix's voice {spec}");
    voice_changed(&app).await;
    list_voices(State(app)).await.into_response()
}

/// After BIX_VOICE changes: that voice's pack takes over (made if it's new), and what it lacks is
/// queued for the next voice run.
async fn voice_changed(app: &AppState) {
    if let Err(e) = app.packs.voice_changed().await {
        tracing::warn!("voice packs: {e}");
    }
    crate::web::warm_all(app).await;
}

/// Bix's voice packs, the one in use first, and the voices a new pack can have.
async fn list_packs(State(app): State<AppState>) -> Response {
    match app.packs.list().await {
        Ok(packs) => Json(json!({
            "packs": packs,
            "built_in": crate::packs::Packs::built_in(),
            "speeds": crate::packs::speeds(),
            "voice_running": voice_running(&app),
        }))
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e })),
        )
            .into_response(),
    }
}

const SWITCH_LATER: &str =
    "A voice run is making lines for the voice pack in use. Switch voices when it's done.";

/// A voice run in progress speaks in the pack in use, so switching waits until it's done.
fn voice_running(app: &AppState) -> bool {
    app.render
        .current(0)
        .is_some_and(|j| j.state == "running" && j.kind == crate::render::Kind::Voice)
}

#[derive(Deserialize)]
struct NewPack {
    name: String,
    voice: String,
    speed: Option<String>,
}

async fn create_pack(State(app): State<AppState>, Json(body): Json<NewPack>) -> Response {
    match app
        .packs
        .create(&body.name, &body.voice, body.speed.as_deref())
        .await
    {
        Ok(_) => list_packs(State(app)).await,
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
    }
}

#[derive(Deserialize)]
struct PackChange {
    name: Option<String>,
    speed: Option<String>,
}

async fn update_pack(
    State(app): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<PackChange>,
) -> Response {
    match app
        .packs
        .update(id, body.name.as_deref(), body.speed.as_deref())
        .await
    {
        Ok(()) => list_packs(State(app)).await,
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
    }
}

/// Switches Bix to a pack. Its lines play at once; the next voice run adds the ones it lacks.
async fn use_pack(State(app): State<AppState>, Path(id): Path<i64>) -> Response {
    if voice_running(&app) {
        return (StatusCode::CONFLICT, Json(json!({ "error": SWITCH_LATER }))).into_response();
    }
    if let Err(e) = app.packs.switch(id).await {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response();
    }
    tracing::info!("admin switched Bix's voice pack to {id}");
    crate::web::warm_all(&app).await;
    list_packs(State(app)).await
}

/// One of a pack's lines at random, to hear it.
async fn pack_sample(State(app): State<AppState>, Path(id): Path<i64>) -> Response {
    match app.packs.sample(id).await {
        Some((bytes, content_type)) => (
            [
                (header::CONTENT_TYPE, content_type),
                (header::CACHE_CONTROL, "no-store"),
            ],
            bytes,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Bix's personas, the one in use first.
async fn list_personas(State(app): State<AppState>) -> Response {
    match app.personas.list().await {
        Ok(personas) => Json(json!({
            "personas": personas,
            "busy": lines_running(&app),
            "max_chars": triviabix_questions::MAX_PERSONA_CHARS,
        }))
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e })),
        )
            .into_response(),
    }
}

/// A run writing or voicing lines is working for the persona in use, so switching waits.
fn lines_running(app: &AppState) -> bool {
    app.render.current(0).is_some_and(|j| {
        j.state == "running"
            && matches!(
                j.kind,
                crate::render::Kind::Lines | crate::render::Kind::Voice
            )
    })
}

const PERSONA_LATER: &str =
    "A run is writing or voicing lines for the persona in use. Switch personas when it's done.";

#[derive(Deserialize)]
struct NewPersona {
    name: String,
    persona: String,
    #[serde(default = "yes")]
    fill_gaps: bool,
}

fn yes() -> bool {
    true
}

async fn create_persona(State(app): State<AppState>, Json(body): Json<NewPersona>) -> Response {
    match app
        .personas
        .create(&body.name, &body.persona, body.fill_gaps)
        .await
    {
        Ok(_) => list_personas(State(app)).await,
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
    }
}

#[derive(Deserialize)]
struct PersonaChange {
    name: Option<String>,
    persona: Option<String>,
    fill_gaps: Option<bool>,
}

async fn update_persona(
    State(app): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<PersonaChange>,
) -> Response {
    if body.fill_gaps.is_some() && lines_running(&app) {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "error": PERSONA_LATER })),
        )
            .into_response();
    }
    match app
        .personas
        .update(
            id,
            body.name.as_deref(),
            body.persona.as_deref(),
            body.fill_gaps,
        )
        .await
    {
        Ok(changed) => {
            if changed {
                lines_changed(&app).await;
            }
            list_personas(State(app)).await
        }
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
    }
}

/// Switches Bix to a persona: its lines play from the next question on, and the next line and
/// voice runs fill in what it lacks.
async fn use_persona(State(app): State<AppState>, Path(id): Path<i64>) -> Response {
    if lines_running(&app) {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "error": PERSONA_LATER })),
        )
            .into_response();
    }
    if let Err(e) = app.personas.switch(id).await {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response();
    }
    tracing::info!("admin switched Bix's persona to {id}");
    lines_changed(&app).await;
    list_personas(State(app)).await
}

/// The lines in play changed: load them, and queue them for the voice.
async fn lines_changed(app: &AppState) {
    crate::web::load_content(&app.db, &app.content_dir).await;
    crate::web::warm_all(app).await;
}
