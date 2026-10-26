//! Bix's voice from the TTS service in `tts/` (Ming-3.0-tts). Every line is synthesized once and
//! kept in the asset store (docs/assets.md) by its text, under a folder per voice, so editing a
//! line or switching voices makes new audio and everything else is reused.
//!
//! Lines the game can know ahead of time (the host script's lines without names in them, every
//! question and its own lead-in and reveal lines) are pre-generated in the background. Lines with
//! live details such as player names are made when the TV asks for them; the TV falls back on the
//! browser's voice for anything that isn't ready in time, or when `TTS_URL` isn't set.
//!
//! The service can also be swapped at runtime (see `set_server`), so `gpu/vast.py` can point the
//! game at a rented GPU, let it work through the backlog, then hand back. Lines it made stay in
//! the cache and keep playing after the GPU is gone.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::sync::Notify;

/// The file in the cache folder naming the voice it was last filled in.
const CURRENT: &str = "current-voice.txt";
const NO_SERVER: &str = "no TTS service is set";
/// Longest line the service will take.
const MAX_TEXT: usize = 1000;
/// How long to trust the service's answer about which voice it speaks with.
const VOICE_TTL: Duration = Duration::from_secs(30);
/// Formats the service can send, as file extensions and content types.
const FORMATS: &[(&str, &str)] = &[
    ("ogg", "audio/ogg"),
    ("flac", "audio/flac"),
    ("wav", "audio/wav"),
];

#[derive(Debug, PartialEq, Eq)]
pub enum VoiceError {
    /// No TTS service is configured, or the line isn't one the game has said.
    NotFound,
    /// The service is down, still loading the model, or failed on this line.
    Unavailable(String),
}

#[derive(Clone)]
pub struct Voice(Option<Arc<Inner>>);

/// A TTS service to ask for new lines.
#[derive(Clone)]
struct Server {
    base: String,
    /// Sent as a bearer token, for a service out on the internet (`TTS_TOKEN` there).
    token: Option<String>,
}

struct Inner {
    /// `TTS_URL` (or the Settings page's), what `set_server(None)` goes back to.
    default: Mutex<Option<Server>>,
    server: Mutex<Option<Server>>,
    client: reqwest::Client,
    dir: PathBuf,
    /// Hash -> text for every line the game might ask for, so the endpoint can't be used to
    /// synthesize arbitrary text.
    known: Mutex<HashMap<String, String>>,
    /// One synthesis per line at a time; a second request for the same line waits for the first.
    busy: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    voice: Mutex<Option<(String, Instant)>>,
    /// The model the service last said it runs, for the asset store's metadata.
    model: Mutex<Option<String>>,
    queue: Mutex<VecDeque<String>>,
    /// The voice folder's slug and the lines already made in it, so the host can tell which of his
    /// lines have audio without touching the disk.
    ready: Mutex<Option<(String, HashSet<String>)>>,
    queued: Mutex<HashSet<String>>,
    /// Every line pre-generation was asked for, in any voice, to count what a voice pack still
    /// lacks.
    wanted: Mutex<HashSet<String>>,
    wake: Notify,
}

#[derive(Deserialize)]
struct Health {
    ready: bool,
    voice: String,
    #[serde(default)]
    model: Option<String>,
}

impl Voice {
    /// The voice for `TTS_URL`, caching audio under `dir`. With no URL and nothing cached yet,
    /// every line is left to the TV's own text-to-speech until a service is set with `set_server`.
    pub fn new(url: Option<&str>, dir: &Path) -> Self {
        let default = url.map(|u| Server {
            base: u.trim_end_matches('/').to_string(),
            token: None,
        });
        // The voice the cache was last filled in, so its lines play while no service is up.
        let last = std::fs::read_to_string(dir.join(CURRENT))
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .map(|v| (v, Instant::now() - VOICE_TTL));
        let inner = Arc::new(Inner {
            server: Mutex::new(default.clone()),
            default: Mutex::new(default),
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(3))
                .build()
                .expect("HTTP client"),
            dir: dir.to_path_buf(),
            known: Default::default(),
            busy: Default::default(),
            voice: Mutex::new(last),
            model: Default::default(),
            queue: Default::default(),
            ready: Default::default(),
            queued: Default::default(),
            wanted: Default::default(),
            wake: Notify::new(),
        });
        tokio::spawn(warm_worker(inner.clone()));
        Voice(Some(inner))
    }

    pub fn off() -> Self {
        Voice(None)
    }

    /// Whether the TV should ask for Bix's voice: there's a service to make lines, or lines made
    /// earlier in the cache.
    pub fn enabled(&self) -> bool {
        self.0.as_ref().is_some_and(|i| {
            i.server.lock().unwrap().is_some() || i.voice.lock().unwrap().is_some()
        })
    }

    /// Where the TV fetches `text` from, or `None` with no TTS service.
    pub fn url(&self, text: &str) -> Option<String> {
        if !self.enabled() {
            return None;
        }
        self.register(text).map(|hash| format!("/api/voice/{hash}"))
    }

    /// Whether `text` has audio in the current voice already, so playing it needs no TTS service.
    pub fn is_ready(&self, text: &str) -> bool {
        self.with_ready(|made| made.contains(&hash(text.trim())))
            .unwrap_or(false)
    }

    /// Runs `f` on the lines made in the current voice, or gives `None` with no voice yet.
    fn with_ready<T>(&self, f: impl FnOnce(&HashSet<String>) -> T) -> Option<T> {
        let inner = self.0.as_ref()?;
        let (key, _) = inner.voice.lock().unwrap().clone()?;
        let slug = slug(&key);
        let mut ready = inner.ready.lock().unwrap();
        if ready.as_ref().is_none_or(|(s, _)| *s != slug) {
            *ready = Some((slug.clone(), made(&inner.dir.join(&slug))));
        }
        Some(f(&ready.as_ref().unwrap().1))
    }

    /// The voice lines play in, as the TTS service names it (its cache folder's style).
    pub fn current_voice(&self) -> Option<String> {
        let inner = self.0.as_ref()?;
        inner.voice.lock().unwrap().as_ref().map(|(k, _)| k.clone())
    }

    /// Plays lines from `key`'s folder from now on, such as after switching voice packs, and
    /// queues again what that voice doesn't have yet. A TTS service that's up still has the last
    /// word: lines are always filed under the voice it actually speaks in.
    pub fn set_voice(&self, key: &str) {
        let Some(inner) = &self.0 else { return };
        *inner.voice.lock().unwrap() = Some((key.to_string(), Instant::now() - VOICE_TTL));
        if let Err(e) = std::fs::create_dir_all(&inner.dir)
            .and_then(|()| std::fs::write(inner.dir.join(CURRENT), key))
        {
            tracing::warn!("couldn't note the voice in {}: {e}", inner.dir.display());
        }
        // What's waiting was picked for the old voice; the caller queues again for this one.
        inner.queue.lock().unwrap().clear();
        inner.queued.lock().unwrap().clear();
    }

    /// Forgets what's waiting and what the game wants, before queueing it all again, such as
    /// after switching personas, whose lines are different.
    pub fn clear_queue(&self) {
        let Some(inner) = &self.0 else { return };
        inner.queue.lock().unwrap().clear();
        inner.queued.lock().unwrap().clear();
        inner.wanted.lock().unwrap().clear();
    }

    /// How many lines are made in `key`'s folder, and how many of the lines the game wants ahead
    /// of time it still lacks.
    pub fn counts(&self, key: &str) -> (usize, usize) {
        let Some(inner) = &self.0 else { return (0, 0) };
        let made = made(&inner.dir.join(slug(key)));
        let wanted = inner.wanted.lock().unwrap();
        (
            made.len(),
            wanted.iter().filter(|h| !made.contains(*h)).count(),
        )
    }

    /// One line made in `key`'s folder, picked at random, to hear a voice pack.
    pub async fn sample(&self, key: &str) -> Option<(Vec<u8>, &'static str)> {
        let inner = self.0.as_ref()?;
        let dir = inner.dir.join(slug(key));
        let made: Vec<String> = made(&dir).into_iter().collect();
        if made.is_empty() {
            return None;
        }
        let pick = &made[rand::random_range(0..made.len())];
        cached(&dir, pick).await
    }

    /// The voices with a folder of lines in the cache, as the TTS service named them.
    pub fn voices_on_disk(&self) -> Vec<String> {
        let Some(inner) = &self.0 else {
            return Vec::new();
        };
        std::fs::read_dir(&inner.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| std::fs::read_to_string(e.path().join("style.txt")).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// Remembers `text` as a line the game may ask for, and returns its hash.
    fn register(&self, text: &str) -> Option<String> {
        let inner = self.0.as_ref()?;
        let text = text.trim();
        if text.is_empty() || text.len() > MAX_TEXT {
            return None;
        }
        let hash = hash(text);
        inner
            .known
            .lock()
            .unwrap()
            .entry(hash.clone())
            .or_insert_with(|| text.to_string());
        Some(hash)
    }

    /// Switches to another TTS service, such as one on a rented GPU, or back to `TTS_URL` with
    /// `None`. Lines already made stay cached either way.
    pub fn set_server(&self, url: Option<&str>, token: Option<&str>) {
        let Some(inner) = &self.0 else { return };
        let server = match url {
            Some(u) => Some(Server {
                base: u.trim_end_matches('/').to_string(),
                token: token.filter(|t| !t.is_empty()).map(str::to_string),
            }),
            None => inner.default.lock().unwrap().clone(),
        };
        *inner.server.lock().unwrap() = server;
        // Ask the new service which voice it speaks before filing anything.
        if let Some((_, at)) = inner.voice.lock().unwrap().as_mut() {
            *at = Instant::now() - VOICE_TTL;
        }
        inner.wake.notify_one();
    }

    /// Changes the service the game goes back to after a rented GPU, from the Settings page.
    /// Takes over now unless a rented GPU is in use.
    pub fn set_default(&self, url: Option<&str>) {
        let Some(inner) = &self.0 else { return };
        let new = url.map(|u| Server {
            base: u.trim_end_matches('/').to_string(),
            token: None,
        });
        let old = std::mem::replace(&mut *inner.default.lock().unwrap(), new);
        let current = self.server();
        if current == old.map(|s| s.base) {
            self.set_server(None, None);
        }
    }

    /// The service lines come from now, without its token, if there is one.
    pub fn server(&self) -> Option<String> {
        let inner = self.0.as_ref()?;
        inner
            .server
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.base.clone())
    }

    /// Lines being made right now.
    pub fn in_flight(&self) -> usize {
        self.0.as_ref().map_or(0, |i| i.busy.lock().unwrap().len())
    }

    /// Queues lines to be made in the background. `soon` puts them ahead of the rest, for a game
    /// that's about to start.
    pub fn warm(&self, texts: impl IntoIterator<Item = String>, soon: bool) {
        let Some(inner) = &self.0 else { return };
        let mut hashes: Vec<String> = texts
            .into_iter()
            .filter_map(|text| self.register(&text))
            .collect();
        inner.wanted.lock().unwrap().extend(hashes.iter().cloned());
        // Lines the current voice has already are never made again, so they aren't waiting.
        if let Some(left) = self.with_ready(|made| {
            hashes
                .iter()
                .filter(|h| !made.contains(*h))
                .cloned()
                .collect::<Vec<_>>()
        }) {
            hashes = left;
        }
        let mut fresh = Vec::new();
        for hash in hashes {
            if soon || inner.queued.lock().unwrap().insert(hash.clone()) {
                fresh.push(hash);
            }
        }
        if fresh.is_empty() {
            return;
        }
        let mut queue = inner.queue.lock().unwrap();
        if soon {
            for hash in fresh.into_iter().rev() {
                queue.push_front(hash);
            }
        } else {
            queue.extend(fresh);
        }
        drop(queue);
        inner.wake.notify_one();
    }

    /// Lines still waiting to be pre-generated.
    pub fn backlog(&self) -> usize {
        self.0.as_ref().map_or(0, |i| i.queue.lock().unwrap().len())
    }

    /// The audio for the line with this hash, from the cache or freshly made, with its content type.
    pub async fn audio(&self, hash: &str) -> Result<(Vec<u8>, &'static str), VoiceError> {
        let inner = self.0.as_ref().ok_or(VoiceError::NotFound)?;
        if hash.len() != 32 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(VoiceError::NotFound);
        }
        inner.audio(hash, true).await
    }
}

impl Inner {
    async fn audio(&self, hash: &str, live: bool) -> Result<(Vec<u8>, &'static str), VoiceError> {
        let dir = self.dir.join(slug(&self.voice().await?));
        if let Some(hit) = cached(&dir, hash).await {
            return Ok(hit);
        }
        let text = self
            .known
            .lock()
            .unwrap()
            .get(hash)
            .cloned()
            .ok_or(VoiceError::NotFound)?;
        let lock = self
            .busy
            .lock()
            .unwrap()
            .entry(hash.to_string())
            .or_default()
            .clone();
        let _held = lock.lock().await;
        let out = match cached(&dir, hash).await {
            Some(hit) => Ok(hit),
            None => self.synthesize(&dir, hash, &text, live).await,
        };
        self.busy.lock().unwrap().remove(hash);
        out
    }

    async fn synthesize(
        &self,
        dir: &Path,
        hash: &str,
        text: &str,
        live: bool,
    ) -> Result<(Vec<u8>, &'static str), VoiceError> {
        let unavailable = |e: String| VoiceError::Unavailable(e);
        let server = self
            .current()
            .ok_or_else(|| unavailable(NO_SERVER.into()))?;
        let started = Instant::now();
        let res = server
            .auth(self.client.post(format!("{}/v1/speech", server.base)))
            .timeout(Duration::from_secs(if live { 30 } else { 180 }))
            .json(&serde_json::json!({ "text": text, "priority": if live { "live" } else { "warm" } }))
            .send()
            .await
            .map_err(|e| unavailable(format!("couldn't reach the TTS service: {e}")))?;
        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(unavailable(format!("TTS service said {status}: {body}")));
        }
        // The voice may have changed while this line was being made; file it under the one it's in.
        let voice = res
            .headers()
            .get("x-voice")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let content_type = res
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let (ext, content_type) = FORMATS
            .iter()
            .find(|(_, t)| content_type.starts_with(t))
            .copied()
            .ok_or_else(|| unavailable(format!("unexpected audio type {content_type:?}")))?;
        let bytes = res
            .bytes()
            .await
            .map_err(|e| unavailable(e.to_string()))?
            .to_vec();
        let dir = match &voice {
            Some(v) => self.dir.join(slug(v)),
            None => dir.to_path_buf(),
        };
        let meta = serde_json::json!({
            "kind": "voice",
            "style": voice,
            "inputs": { "text": text },
            "key": hash,
            "file": format!("{hash}.{ext}"),
            "content_type": content_type,
            "bytes": bytes.len(),
            "model": self.model.lock().unwrap().clone(),
            "created": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
            "generator": server.base,
        });
        match save(&dir, voice.as_deref(), hash, ext, &bytes, &meta).await {
            Ok(()) => {
                if let Some((slug, made)) = self.ready.lock().unwrap().as_mut()
                    && dir.ends_with(slug.as_str())
                {
                    made.insert(hash.to_string());
                }
            }
            Err(e) => tracing::warn!("couldn't cache voice line in {}: {e}", dir.display()),
        }
        tracing::debug!(
            live,
            ms = started.elapsed().as_millis() as u64,
            "voiced {text:?}"
        );
        Ok((bytes, content_type))
    }

    /// The voice the service is speaking with, which names the cache folder. While the service is
    /// down, the last voice it reported is used, so lines already made keep playing.
    async fn voice(&self) -> Result<String, VoiceError> {
        let last = self.voice.lock().unwrap().clone();
        if let Some((key, at)) = &last
            && at.elapsed() < VOICE_TTL
        {
            return Ok(key.clone());
        }
        match self.health().await {
            Ok(key) => {
                *self.voice.lock().unwrap() = Some((key.clone(), Instant::now()));
                if last.as_ref().is_none_or(|(k, _)| *k != key) {
                    let _ = tokio::fs::create_dir_all(&self.dir).await;
                    if let Err(e) = tokio::fs::write(self.dir.join(CURRENT), &key).await {
                        tracing::warn!("couldn't note the voice in {}: {e}", self.dir.display());
                    }
                }
                Ok(key)
            }
            Err(e) => last.map(|(key, _)| key).ok_or(e),
        }
    }

    fn current(&self) -> Option<Server> {
        self.server.lock().unwrap().clone()
    }

    async fn health(&self) -> Result<String, VoiceError> {
        let server = self
            .current()
            .ok_or_else(|| VoiceError::Unavailable(NO_SERVER.into()))?;
        let health: Health = server
            .auth(self.client.get(format!("{}/healthz", server.base)))
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| VoiceError::Unavailable(format!("couldn't reach the TTS service: {e}")))?
            .json()
            .await
            .map_err(|e| VoiceError::Unavailable(e.to_string()))?;
        if health.model.is_some() {
            *self.model.lock().unwrap() = health.model.clone();
        }
        if !health.ready {
            return Err(VoiceError::Unavailable(
                "the TTS service is still loading".into(),
            ));
        }
        Ok(health.voice)
    }
}

impl Server {
    fn auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.token {
            Some(token) => req.bearer_auth(token),
            None => req,
        }
    }
}

/// Works through the pre-generation queue one line at a time, backing off while the service is
/// down, and waiting while there's none.
async fn warm_worker(inner: Arc<Inner>) {
    let mut made = 0usize;
    loop {
        if inner.current().is_none() {
            inner.wake.notified().await;
            continue;
        }
        let next = inner.queue.lock().unwrap().pop_front();
        let Some(hash) = next else {
            if made > 0 {
                tracing::info!("voice: pre-generated {made} lines");
                made = 0;
            }
            inner.wake.notified().await;
            continue;
        };
        match inner.audio(&hash, false).await {
            Ok(_) => made += 1,
            Err(VoiceError::NotFound) => {}
            Err(VoiceError::Unavailable(e)) => {
                inner.queue.lock().unwrap().push_front(hash);
                if inner.current().is_some() {
                    tracing::warn!("voice: {e}; trying again in 30s");
                    // A new service (set_server) cuts the wait short.
                    let _ =
                        tokio::time::timeout(Duration::from_secs(30), inner.wake.notified()).await;
                }
                continue;
            }
        }
        inner.queued.lock().unwrap().remove(&hash);
    }
}

/// The hashes of the lines made in a voice folder.
fn made(dir: &Path) -> HashSet<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return HashSet::new();
    };
    entries
        .filter_map(|e| {
            let name = e.ok()?.file_name().into_string().ok()?;
            let (stem, ext) = name.split_once('.')?;
            FORMATS
                .iter()
                .any(|(x, _)| *x == ext)
                .then(|| stem.to_string())
        })
        .collect()
}

async fn cached(dir: &Path, hash: &str) -> Option<(Vec<u8>, &'static str)> {
    for (ext, content_type) in FORMATS {
        if let Ok(bytes) = tokio::fs::read(dir.join(format!("{hash}.{ext}"))).await {
            return Some((bytes, content_type));
        }
    }
    None
}

async fn save(
    dir: &Path,
    voice: Option<&str>,
    hash: &str,
    ext: &str,
    bytes: &[u8],
    meta: &serde_json::Value,
) -> std::io::Result<()> {
    tokio::fs::create_dir_all(dir).await?;
    if let Some(voice) = voice {
        // Names the style folder, as for every kind of asset.
        tokio::fs::write(dir.join("style.txt"), voice).await?;
    }
    write_new(&dir.join(format!("{hash}.{ext}")), bytes).await?;
    let meta = serde_json::to_vec_pretty(meta).expect("JSON");
    write_new(&dir.join(format!("{hash}.json")), &meta).await
}

/// Write then rename, so a half-written file is never served.
async fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".part");
    tokio::fs::write(&tmp, bytes).await?;
    tokio::fs::rename(&tmp, path).await
}

/// A voice line's key in the asset store: the hash of its inputs as canonical JSON.
fn hash(text: &str) -> String {
    let inputs = serde_json::json!({ "text": text }).to_string();
    hex(&Sha256::digest(inputs.as_bytes())[..16])
}

/// A style's folder name in the asset store.
fn slug(voice: &str) -> String {
    hex(&Sha256::digest(voice.as_bytes())[..6])
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Ming's inline sound effects, such as `[Laughter]`. The TTS service performs them; anywhere
/// else they're dropped.
pub const SOUND_TAGS: &[&str] = &[
    "[Breathing]",
    "[Cough]",
    "[Sigh]",
    "[Laughter]",
    "[Uhm]",
    "[Surprise-ah]",
    "[Surprise-oh]",
    "[Dissatisfaction-hnn]",
    "[Confirmation-en]",
    "[Question-ei]",
    "[Question-ah]",
    "[Question-en]",
    "[Question-oh]",
    "[Question-huh]",
];

/// `text` without sound effect tags, for timing how long the host talks.
pub fn without_tags(text: &str) -> String {
    let mut out = text.to_string();
    for tag in SOUND_TAGS {
        out = out.replace(tag, "");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELLO_KEY: &str = "ef73ae6a8b47cd3601ab3fad1fb8099e";
    const JAMES_SLUG: &str = "8f7aee184357";

    #[test]
    fn hashes_are_stable() {
        // The same keys gpu/assets.py makes: key_of({"text": "Hello"}) and slug_of("ip:US_James").
        assert_eq!(hash("Hello"), HELLO_KEY);
        assert_eq!(slug("ip:US_James"), JAMES_SLUG);
        assert_eq!(hash("Hello").len(), 32);
        assert_ne!(slug("ip:US_James"), slug("ip:UK_George"));
    }

    #[tokio::test]
    async fn off_without_a_url() {
        let voice = Voice::off();
        assert!(!voice.enabled());
        assert_eq!(voice.url("Hi"), None);
        assert_eq!(voice.audio(&hash("Hi")).await, Err(VoiceError::NotFound));
    }

    #[test]
    fn tags_are_dropped() {
        assert_eq!(without_tags("Ha[Laughter] ha"), "Ha ha");
    }
}
