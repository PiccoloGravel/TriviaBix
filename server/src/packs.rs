//! Bix's voice packs: a name for a voice (BIX_VOICE) and the speed he talks at (BIX_SPEED). Every
//! voice's lines are kept in their own folder of the asset store, so a pack keeps everything ever
//! made in it: a voice run only adds the lines it's missing, and switching to another pack and
//! back later loses nothing and pays for nothing twice.
//!
//! The pack in use is simply the one whose voice is BIX_VOICE. Changing the voice anywhere (the
//! Settings page, "Use this voice" on the Pipeline page) switches to that voice's pack, making it
//! if it's new, and changing the speed changes the pack in use.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;

use crate::db::Db;
use crate::settings::Settings;
use crate::voice::Voice;

/// The voice used when BIX_VOICE is set nowhere, as tts/server.py has it.
pub const DEFAULT_VOICE: &str = "ip:US_James";
/// The model's built-in voices, as `ip:<name>`.
pub const BUILT_IN: &[&str] = &["US_James", "US_Emma", "UK_George", "UK_Charlotte"];
const SAMPLE_KIND: &str = "voice-sample";
const MAX_NAME: usize = 40;

#[derive(Clone)]
pub struct Packs {
    db: Db,
    settings: Settings,
    voice: Voice,
    /// The asset store, where designed voices are (kind `voice-sample`).
    assets: PathBuf,
}

impl Packs {
    pub fn new(db: Db, settings: Settings, voice: Voice, assets: &Path) -> Self {
        Self {
            db,
            settings,
            voice,
            assets: assets.to_path_buf(),
        }
    }

    fn spec(&self) -> String {
        self.settings
            .get("BIX_VOICE")
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| DEFAULT_VOICE.into())
    }

    fn speed(&self) -> String {
        self.settings
            .get("BIX_SPEED")
            .unwrap_or_else(|| "1.0".into())
    }

    /// Makes sure there's a pack for the voice in use, with the speed in use, and one for every
    /// voice with lines in the asset store, then plays lines from the voice in use. Run at startup
    /// and whenever the voice or speed changes. The caller queues lines again afterwards.
    pub async fn sync(&self) -> Result<(), String> {
        let spec = self.spec();
        self.ensure(&spec).await?;
        sqlx::query("UPDATE voice_packs SET speed = ? WHERE voice = ?")
            .bind(self.speed())
            .bind(&spec)
            .execute(&self.db.pool)
            .await
            .map_err(|e| e.to_string())?;
        for key in self.voice.voices_on_disk() {
            if let Some(found) = spec_of(&key) {
                self.ensure(&found).await?;
            }
        }
        let current = self.voice.current_voice();
        // With nothing ever voiced, the TV keeps its own voice until there's something to play.
        if let Some(key) = self.key(&spec)
            && current.as_deref() != Some(key.as_str())
            && (current.is_some() || self.voice.counts(&key).0 > 0)
        {
            tracing::info!("voice pack: playing lines in {key}");
            self.voice.set_voice(&key);
        }
        Ok(())
    }

    /// After BIX_VOICE changes: its pack takes over, at the speed it was last played at.
    pub async fn voice_changed(&self) -> Result<(), String> {
        let spec = self.spec();
        let speed: Option<String> =
            sqlx::query_scalar("SELECT speed FROM voice_packs WHERE voice = ?")
                .bind(&spec)
                .fetch_optional(&self.db.pool)
                .await
                .map_err(|e| e.to_string())?;
        if let Some(speed) = speed
            && speed != self.speed()
        {
            self.settings.set("BIX_SPEED", Some(&speed)).await?;
        }
        sqlx::query("UPDATE voice_packs SET used_at = CURRENT_TIMESTAMP WHERE voice = ?")
            .bind(&spec)
            .execute(&self.db.pool)
            .await
            .map_err(|e| e.to_string())?;
        self.sync().await
    }

    /// Switches Bix to a pack. The caller queues lines again, so a voice run adds what it lacks.
    pub async fn switch(&self, id: i64) -> Result<(), String> {
        let voice = self.voice_of(id).await?;
        self.settings.set("BIX_VOICE", Some(&voice)).await?;
        self.voice_changed().await
    }

    /// Makes a pack for `voice`, without switching to it.
    pub async fn create(
        &self,
        name: &str,
        voice: &str,
        speed: Option<&str>,
    ) -> Result<i64, String> {
        let name = check_name(name)?;
        let voice = self.check_voice(voice)?;
        let speed = check_speed(speed.unwrap_or("1.0"))?;
        let res = sqlx::query("INSERT INTO voice_packs (name, voice, speed) VALUES (?, ?, ?)")
            .bind(&name)
            .bind(&voice)
            .bind(&speed)
            .execute(&self.db.pool)
            .await;
        match res {
            Ok(r) => Ok(r.last_insert_rowid()),
            Err(e) if e.to_string().contains("voice_packs.voice") => {
                Err("There's already a pack with that voice.".into())
            }
            Err(e) if e.to_string().contains("UNIQUE") => {
                Err("There's already a pack with that name.".into())
            }
            Err(e) => Err(e.to_string()),
        }
    }

    /// Renames a pack or changes its speed. The pack in use changes BIX_SPEED too.
    pub async fn update(
        &self,
        id: i64,
        name: Option<&str>,
        speed: Option<&str>,
    ) -> Result<(), String> {
        let voice = self.voice_of(id).await?;
        if let Some(name) = name {
            let name = check_name(name)?;
            sqlx::query("UPDATE voice_packs SET name = ? WHERE id = ?")
                .bind(&name)
                .bind(id)
                .execute(&self.db.pool)
                .await
                .map_err(|e| match e.to_string().contains("UNIQUE") {
                    true => "There's already a pack with that name.".to_string(),
                    false => e.to_string(),
                })?;
        }
        if let Some(speed) = speed {
            let speed = check_speed(speed)?;
            sqlx::query("UPDATE voice_packs SET speed = ? WHERE id = ?")
                .bind(&speed)
                .bind(id)
                .execute(&self.db.pool)
                .await
                .map_err(|e| e.to_string())?;
            if voice == self.spec() {
                self.settings.set("BIX_SPEED", Some(&speed)).await?;
            }
        }
        Ok(())
    }

    /// Every pack, the one in use first, with how many lines it has and still lacks.
    pub async fn list(&self) -> Result<Vec<Value>, String> {
        let rows = sqlx::query(
            "SELECT id, name, voice, speed, created_at, used_at FROM voice_packs
             ORDER BY used_at IS NULL, used_at DESC, id",
        )
        .fetch_all(&self.db.pool)
        .await
        .map_err(|e| e.to_string())?;
        let spec = self.spec();
        let mut out: Vec<Value> = rows
            .iter()
            .map(|r| {
                let voice: String = r.get("voice");
                let (made, missing) = self.key(&voice).map_or((0, 0), |k| self.voice.counts(&k));
                let sample = voice
                    .strip_prefix("clone:")
                    .and_then(|k| self.sample_meta(k));
                json!({
                    "id": r.get::<i64, _>("id"),
                    "name": r.get::<String, _>("name"),
                    "voice": voice,
                    "speed": r.get::<String, _>("speed").parse::<f64>().unwrap_or(1.0),
                    "active": voice == spec,
                    "made": made,
                    "missing": missing,
                    "description": sample.as_ref().and_then(|m| m["description"].as_str().map(str::to_string)),
                    "take": sample.as_ref().and_then(|m| m["seed"].as_u64()),
                    "created": r.get::<String, _>("created_at"),
                    "used": r.get::<Option<String>, _>("used_at"),
                })
            })
            .collect();
        out.sort_by_key(|p| !p["active"].as_bool().unwrap_or(false));
        Ok(out)
    }

    /// One line the pack has, picked at random, to hear it before switching.
    pub async fn sample(&self, id: i64) -> Option<(Vec<u8>, &'static str)> {
        let voice = self.voice_of(id).await.ok()?;
        self.voice.sample(&self.key(&voice)?).await
    }

    pub fn built_in() -> Vec<String> {
        BUILT_IN.iter().map(|n| format!("ip:{n}")).collect()
    }

    async fn voice_of(&self, id: i64) -> Result<String, String> {
        sqlx::query_scalar("SELECT voice FROM voice_packs WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.db.pool)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "There's no such voice pack.".to_string())
    }

    /// Adds a pack for `voice` if there's none, named after it.
    async fn ensure(&self, voice: &str) -> Result<(), String> {
        let have: Option<i64> = sqlx::query_scalar("SELECT id FROM voice_packs WHERE voice = ?")
            .bind(voice)
            .fetch_optional(&self.db.pool)
            .await
            .map_err(|e| e.to_string())?;
        if have.is_some() {
            return Ok(());
        }
        let base = self.name_for(voice);
        for n in 1.. {
            let name = if n == 1 {
                base.clone()
            } else {
                format!("{base} ({n})")
            };
            let res = sqlx::query(
                "INSERT OR IGNORE INTO voice_packs (name, voice, speed) VALUES (?, ?, ?)",
            )
            .bind(&name)
            .bind(voice)
            .bind(if voice == self.spec() {
                self.speed()
            } else {
                "1.0".into()
            })
            .execute(&self.db.pool)
            .await
            .map_err(|e| e.to_string())?;
            if res.rows_affected() == 1 {
                tracing::info!("voice pack {name:?} for {voice}");
                break;
            }
        }
        Ok(())
    }

    fn name_for(&self, voice: &str) -> String {
        let (kind, value) = voice.split_once(':').unwrap_or(("", voice));
        let name = match kind {
            "ip" => format!("{} (built-in)", value.rsplit('_').next().unwrap_or(value)),
            "clone" => match self.sample_meta(value).and_then(|m| m["seed"].as_u64()) {
                Some(seed) => format!("Take {seed}"),
                None => "Cloned voice".into(),
            },
            "fish" => "Fish Audio".into(),
            _ => "Designed voice".into(),
        };
        name.chars().take(MAX_NAME - 5).collect()
    }

    /// The name the TTS service gives `spec`, which names its folder of lines. For a cloned voice
    /// that's worked out from its clip the way tts/server.py does; a folder the service filled
    /// under another name for the same clip wins if it has more lines.
    fn key(&self, spec: &str) -> Option<String> {
        let Some(value) = spec.strip_prefix("clone:") else {
            return Some(spec.to_string());
        };
        let worked_out = self.clip(value).map(|(wav, text)| {
            let mut h = Sha256::new();
            h.update(&wav);
            h.update(text.as_bytes());
            let digest: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
            format!("clone:{value}.wav:{}", &digest[..12])
        });
        // A .wav given by path is named by its file name.
        let name = Path::new(value).file_name()?.to_str()?;
        let name = if is_key(value) {
            format!("{name}.wav")
        } else {
            name.to_string()
        };
        let prefix = format!("clone:{name}:");
        let on_disk = self
            .voice
            .voices_on_disk()
            .into_iter()
            .filter(|k| k.starts_with(&prefix) && Some(k) != worked_out.as_ref());
        // The first of equals is kept, so the worked-out name wins a tie.
        let mut best: Option<(usize, String)> = None;
        for key in worked_out.clone().into_iter().chain(on_disk) {
            let made = self.voice.counts(&key).0;
            if best.as_ref().is_none_or(|(n, _)| made > *n) {
                best = Some((made, key));
            }
        }
        best.map(|(_, k)| k)
    }

    /// A designed voice's clip and transcript.
    fn clip(&self, key: &str) -> Option<(Vec<u8>, String)> {
        let wav = self.sample_file(key, "wav")?;
        let meta = self.sample_meta(key)?;
        let text = meta["transcript"]
            .as_str()
            .or(meta["inputs"]["text"].as_str())?
            .trim()
            .to_string();
        Some((std::fs::read(wav).ok()?, text))
    }

    fn sample_meta(&self, key: &str) -> Option<Value> {
        let path = self.sample_file(key, "json")?;
        serde_json::from_slice(&std::fs::read(path).ok()?).ok()
    }

    fn sample_file(&self, key: &str, ext: &str) -> Option<PathBuf> {
        if !is_key(key) {
            return None;
        }
        std::fs::read_dir(self.assets.join(SAMPLE_KIND))
            .ok()?
            .flatten()
            .map(|style| style.path().join(format!("{key}.{ext}")))
            .find(|p| p.is_file())
    }

    fn check_voice(&self, voice: &str) -> Result<String, String> {
        let voice = voice.trim();
        let ok = match voice.split_once(':') {
            Some(("ip", name)) => BUILT_IN.contains(&name),
            Some(("clone", key)) => self.sample_file(key, "wav").is_some(),
            // Lines voiced elsewhere and filed here (gpu/fish_import.py), so only once there are some.
            Some(("fish", _)) => self.voice.voices_on_disk().iter().any(|k| k == voice),
            _ => false,
        };
        if ok {
            Ok(voice.to_string())
        } else {
            Err("Pick one of the built-in voices or a designed one.".into())
        }
    }
}

/// The BIX_VOICE that made a folder the TTS service named `key`, where that can be told.
fn spec_of(key: &str) -> Option<String> {
    let (kind, rest) = key.split_once(':')?;
    match kind {
        "ip" | "freeform" | "design" => Some(key.to_string()),
        // fish:<name>, voiced by Fish Audio on CouchCritters' render box (gpu/fish_import.py).
        "fish" if !rest.is_empty() => Some(key.to_string()),
        // clone:<asset key>.wav:<hash of the clip>, from a designed voice.
        "clone" => {
            let (name, _) = rest.split_once(':')?;
            let k = name.strip_suffix(".wav")?;
            is_key(k).then(|| format!("clone:{k}"))
        }
        _ => None,
    }
}

fn check_name(name: &str) -> Result<String, String> {
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    match name.chars().count() {
        0 => Err("Give the pack a name.".into()),
        n if n > MAX_NAME => Err(format!("Keep the name under {MAX_NAME} characters.")),
        _ => Ok(name),
    }
}

/// The speeds the Settings page offers for BIX_SPEED.
pub fn speeds() -> &'static [&'static str] {
    crate::settings::FIELDS
        .iter()
        .find(|f| f.key == "BIX_SPEED")
        .and_then(|f| match f.kind {
            crate::settings::Kind::Choice { options } => Some(options),
            _ => None,
        })
        .unwrap_or(&["1.0"])
}

fn check_speed(speed: &str) -> Result<String, String> {
    let speed = speed.trim();
    // 1.20 from a number box is 1.2.
    speeds()
        .iter()
        .find(|s| {
            *s == &speed
                || s.parse::<f64>().ok() == speed.parse::<f64>().ok().filter(|v| v.is_finite())
        })
        .map(|s| s.to_string())
        .ok_or_else(|| format!("Pick one of these speeds: {}.", speeds().join(", ")))
}

fn is_key(key: &str) -> bool {
    key.len() == 32 && key.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_name_their_voices() {
        let k = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            spec_of(&format!("clone:{k}.wav:abcdef012345")),
            Some(format!("clone:{k}"))
        );
        assert_eq!(spec_of("ip:US_James").as_deref(), Some("ip:US_James"));
        assert_eq!(spec_of("clone:bix.wav:abcdef012345"), None);
        assert_eq!(spec_of("nonsense"), None);
        assert_eq!(spec_of("fish:bix").as_deref(), Some("fish:bix"));
        assert_eq!(spec_of("fish:"), None);
    }

    #[test]
    fn names_and_speeds_are_checked() {
        assert_eq!(check_name("  Bix   Classic ").unwrap(), "Bix Classic");
        assert!(check_name(" ").is_err());
        assert!(check_name(&"x".repeat(41)).is_err());
        assert_eq!(check_speed("1.2").unwrap(), "1.2");
        assert_eq!(check_speed("1.20").unwrap(), "1.2");
        assert!(check_speed("9").is_err());
        assert!(check_speed("fast").is_err());
    }
}
