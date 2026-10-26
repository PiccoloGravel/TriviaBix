//! Bix's voice from the TTS service in `tts/` (Ming-3.0-tts). Every line is synthesized once and
//! cached on disk by its text, under a folder per voice, so editing a line or switching voices
//! makes new audio and everything else is reused.
//!
//! Lines the game can know ahead of time (the host script's lines without names in them, every
//! question and its own lead-in and reveal lines) are pre-generated in the background. Lines with
//! live details such as player names are made when the TV asks for them; the TV falls back on the
//! browser's voice for anything that isn't ready in time, or when `TTS_URL` isn't set.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::sync::Notify;

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

struct Inner {
    base: String,
    client: reqwest::Client,
    dir: PathBuf,
    /// Hash -> text for every line the game might ask for, so the endpoint can't be used to
    /// synthesize arbitrary text.
    known: Mutex<HashMap<String, String>>,
    /// One synthesis per line at a time; a second request for the same line waits for the first.
    busy: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    voice: Mutex<Option<(String, Instant)>>,
    queue: Mutex<VecDeque<String>>,
    queued: Mutex<HashSet<String>>,
    wake: Notify,
}

#[derive(Deserialize)]
struct Health {
    ready: bool,
    voice: String,
}

impl Voice {
    /// The voice for `TTS_URL`, caching audio under `dir`. With no URL, every line is left to the
    /// TV's own text-to-speech.
    pub fn new(url: Option<&str>, dir: &Path) -> Self {
        let Some(base) = url.map(|u| u.trim_end_matches('/').to_string()) else {
            return Voice(None);
        };
        let inner = Arc::new(Inner {
            base,
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(3))
                .build()
                .expect("HTTP client"),
            dir: dir.to_path_buf(),
            known: Default::default(),
            busy: Default::default(),
            voice: Default::default(),
            queue: Default::default(),
            queued: Default::default(),
            wake: Notify::new(),
        });
        tokio::spawn(warm_worker(inner.clone()));
        Voice(Some(inner))
    }

    pub fn off() -> Self {
        Voice(None)
    }

    pub fn enabled(&self) -> bool {
        self.0.is_some()
    }

    /// Where the TV fetches `text` from, or `None` with no TTS service.
    pub fn url(&self, text: &str) -> Option<String> {
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
        Some(format!("/api/voice/{hash}"))
    }

    /// Queues lines to be made in the background. `soon` puts them ahead of the rest, for a game
    /// that's about to start.
    pub fn warm(&self, texts: impl IntoIterator<Item = String>, soon: bool) {
        let Some(inner) = &self.0 else { return };
        let mut fresh = Vec::new();
        for text in texts {
            let Some(url) = self.url(&text) else { continue };
            let hash = url.rsplit('/').next().unwrap().to_string();
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
        let started = Instant::now();
        let res = self
            .client
            .post(format!("{}/v1/speech", self.base))
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
        if let Err(e) = save(&dir, voice.as_deref(), hash, ext, &bytes).await {
            tracing::warn!("couldn't cache voice line in {}: {e}", dir.display());
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
                Ok(key)
            }
            Err(e) => last.map(|(key, _)| key).ok_or(e),
        }
    }

    async fn health(&self) -> Result<String, VoiceError> {
        let health: Health = self
            .client
            .get(format!("{}/healthz", self.base))
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| VoiceError::Unavailable(format!("couldn't reach the TTS service: {e}")))?
            .json()
            .await
            .map_err(|e| VoiceError::Unavailable(e.to_string()))?;
        if !health.ready {
            return Err(VoiceError::Unavailable(
                "the TTS service is still loading".into(),
            ));
        }
        Ok(health.voice)
    }
}

/// Works through the pre-generation queue one line at a time, backing off while the service is down.
async fn warm_worker(inner: Arc<Inner>) {
    let mut made = 0usize;
    loop {
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
                tracing::warn!("voice: {e}; trying again in 30s");
                inner.queue.lock().unwrap().push_front(hash);
                tokio::time::sleep(Duration::from_secs(30)).await;
                continue;
            }
        }
        inner.queued.lock().unwrap().remove(&hash);
    }
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
) -> std::io::Result<()> {
    tokio::fs::create_dir_all(dir).await?;
    if let Some(voice) = voice {
        // So a person browsing the cache can tell the folders apart.
        tokio::fs::write(dir.join("voice.txt"), voice).await?;
    }
    // Write then rename, so a half-written file is never served.
    let tmp = dir.join(format!("{hash}.{ext}.part"));
    tokio::fs::write(&tmp, bytes).await?;
    tokio::fs::rename(&tmp, dir.join(format!("{hash}.{ext}"))).await
}

fn hash(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest[..16].iter().map(|b| format!("{b:02x}")).collect()
}

fn slug(voice: &str) -> String {
    hash(voice)[..12].to_string()
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

    #[test]
    fn hashes_are_stable() {
        assert_eq!(hash("Hello"), "185f8db32271fe25f561a6fc938b2e26");
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
