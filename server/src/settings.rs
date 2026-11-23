//! Keys and addresses for the services TriviaBix uses, set from the admin panel's Settings page
//! and kept in the database. A saved value wins over the container's environment variable of the
//! same name, and clearing it goes back to the variable. Secrets are never sent back to the page.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use serde::Serialize;
use sqlx::Row;

use crate::db::Db;

/// What a setting holds, which decides how it's checked and shown.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Kind {
    /// Never sent back to the page.
    Secret,
    Url,
    Text,
    /// Several lines of text, up to `max` characters.
    LongText {
        max: usize,
    },
    /// A Hugging Face model, as "owner/name".
    Model,
    Choice {
        options: &'static [&'static str],
    },
    /// Dollars per hour, up to `max`.
    Price {
        max: f64,
    },
}

/// One thing the Settings page can set. `key` is also the environment variable it overrides.
pub struct Field {
    pub key: &'static str,
    pub group: &'static str,
    pub label: &'static str,
    pub kind: Kind,
    /// What's used when it's set nowhere.
    pub default: Option<&'static str>,
    pub help: &'static str,
}

/// The most any GPU job may pay per hour, whatever the settings say.
pub const PRICE_CEILING: f64 = 2.0;
pub const VOICE_PRICE: f64 = 0.50;
pub const ART_PRICE: f64 = 0.60;

const SHOW: &str = "Game show";
const VAST: &str = "GPU rental (vast.ai)";
const CLAUDE: &str = "Claude (Bix's lines)";
const VOICE: &str = "Bix's voice";
const ART: &str = "Art";
const LIDARR: &str = "Music library (Lidarr)";
const GITHUB: &str = "Private images (GitHub)";

/// The images on ghcr.io that TriviaBix runs, all under one owner.
const GHCR_OWNER: &str = "piccologravel";
const GHCR_IMAGES: &[&str] = &["triviabix", "triviabix-tts", "triviabix-art"];

pub const FIELDS: &[Field] = &[
    Field {
        key: "GAME_PACE",
        group: SHOW,
        label: "Pace between questions",
        kind: Kind::Choice {
            options: &["quick", "normal", "relaxed"],
        },
        default: Some("normal"),
        help: "How long the TV stays on the answer, the scores and the round cards before moving on. Bix always finishes his line first, and the game moves on as soon as he has. Quick keeps the gaps to a couple of seconds; relaxed is the old, longer pace. Answer time doesn't change. Running games pick it up at their next question.",
    },
    Field {
        key: "QUESTION_SECS",
        group: SHOW,
        label: "Answer time",
        kind: Kind::Choice {
            options: &["10", "15", "20", "25", "30"],
        },
        default: Some("20"),
        help: "Seconds to answer a trivia question, on top of the time Bix spends reading it out. The question ends early once everyone has answered. Running games pick it up at their next question.",
    },
    Field {
        key: "HURRY_SECS",
        group: SHOW,
        label: "Last ones to answer",
        kind: Kind::Choice {
            options: &["off", "3", "5", "8"],
        },
        default: Some("5"),
        help: "With three or more playing, once everyone but the last few has answered, the clock drops to this many seconds: everyone but one in a small room, and up to one in five in a full one (two of ten, three of fifteen). Their phones say everyone is waiting on them and the TV calls them out, so a couple of slow thumbs never hold up the room. Off lets the clock run out as normal.",
    },
    Field {
        key: "CHATTER",
        group: SHOW,
        label: "Bix's chatter after answers",
        kind: Kind::Choice {
            options: &["snappy", "chatty", "full"],
        },
        default: Some("chatty"),
        help: "How much Bix says after each answer and at the end of a round. He always says the answer; on top of that he picks from who got it, the question's joke or fun fact, streaks and roasts until he runs out of time. Snappy keeps it to about 8 seconds, chatty about 13, and full says everything (the old way, up to half a minute). The callout stamps on the TV still show whatever he skips.",
    },
    Field {
        key: "BONUSES",
        group: SHOW,
        label: "Streak and comeback bonuses",
        kind: Kind::Choice {
            options: &["streaks+comebacks", "streaks", "off"],
        },
        default: Some("streaks+comebacks"),
        help: "Streaks pay +100 for a third right answer in a row, +200 for a fourth and +300 from then on. Comebacks give anyone in the bottom half of the scoreboard 20% more for a right answer, so nobody is out of it by round two. Both grow with the round's points (x1.5, x2, x3) and show on the phones. Running games pick it up at their next question.",
    },
    Field {
        key: "LOOK",
        group: SHOW,
        label: "Look",
        kind: Kind::Choice {
            options: &["after-dark", "classic"],
        },
        default: Some("after-dark"),
        help: "How the TV and the phones look. After dark is the show after the network pulled the plug: a dim velvet studio, one hot spotlight, tarnished gold and a little VHS fuzz. Still a party, just with the lights down. Classic is the original bright purple. Changes on the TV and phones at once. The generated art has its own After dark theme on the Art page.",
    },
    Field {
        key: "BIX_ON_TV",
        group: SHOW,
        label: "Bix on the TV",
        kind: Kind::Choice {
            options: &["show", "hide"],
        },
        default: Some("show"),
        help: "Show Bix along the bottom of the TV, or hide him and his speech bubble. His voice, cutscenes and the callout graphics still play. Changes on the TV at once.",
    },
    Field {
        key: "BIX_CAPTIONS",
        group: SHOW,
        label: "Bix's words on the TV",
        kind: Kind::Choice {
            options: &["show", "hide"],
        },
        default: Some("show"),
        help: "Show what Bix is saying in his speech bubble, like subtitles, or hide the bubble and just hear him. With his voice muted on the TV the words always show, so nobody misses a line.",
    },
    Field {
        key: "CAMEOS",
        group: SHOW,
        label: "Character cameos",
        kind: Kind::Choice {
            options: &["often", "sometimes", "off"],
        },
        default: Some("sometimes"),
        help: "How often Bix breaks into one of his characters for a line, like Detective Dirk Danger for a crime question or Colonel Bid McGavel the auctioneer for Closest Guess. The TV puts him in costume and plays the line in the character's style. Off keeps him as plain Bix (character lines are still said, just without the costume). Running games pick it up at their next line.",
    },
    Field {
        key: "VAST_API_KEY",
        group: VAST,
        label: "API key",
        kind: Kind::Secret,
        default: None,
        help: "Rents the GPUs for Bix's voice and the art. Make one at cloud.vast.ai/manage-keys.",
    },
    Field {
        key: "VAST_GPU_NAMES",
        group: VAST,
        label: "Only rent these GPUs",
        kind: Kind::Text,
        default: None,
        help: "Names as vast.ai shows them, separated by commas, e.g. RTX 4090, RTX 3090. Empty rents the cheapest that's big enough.",
    },
    Field {
        key: "VOICE_MAX_PRICE",
        group: VAST,
        label: "Voice: most per hour ($)",
        kind: Kind::Price { max: PRICE_CEILING },
        default: Some("0.50"),
        help: "The job never rents a GPU above this. Up to $2.00.",
    },
    Field {
        key: "ART_MAX_PRICE",
        group: VAST,
        label: "Art: most per hour ($)",
        kind: Kind::Price { max: PRICE_CEILING },
        default: Some("0.60"),
        help: "The art needs a 24 GB GPU, so it costs a little more. Up to $2.00.",
    },
    Field {
        key: "GHCR_TOKEN",
        group: GITHUB,
        label: "Package token",
        kind: Kind::Secret,
        default: None,
        help: "Lets rented GPUs pull the private triviabix-tts and triviabix-art images. A classic token with only the read:packages scope, from github.com/settings/tokens. Leave empty if the images are public.",
    },
    Field {
        key: "GHCR_USER",
        group: GITHUB,
        label: "GitHub username",
        kind: Kind::Text,
        default: Some("PiccoloGravel"),
        help: "The account that made the token.",
    },
    Field {
        key: "HF_TOKEN",
        group: "Hugging Face",
        label: "Access token",
        kind: Kind::Secret,
        default: None,
        help: "Lets the GPU download the voice and art models. A read token from huggingface.co/settings/tokens.",
    },
    Field {
        key: "ANTHROPIC_API_KEY",
        group: CLAUDE,
        label: "API key",
        kind: Kind::Secret,
        default: None,
        help: "Writes Bix's lines for new questions. Calls are billed to this Anthropic account.",
    },
    Field {
        key: "CLAUDE_MODEL",
        group: CLAUDE,
        label: "Model",
        kind: Kind::Choice {
            options: &["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"],
        },
        default: Some(triviabix_questions::DEFAULT_MODEL),
        help: "Opus writes the funniest lines; Sonnet and Haiku cost less.",
    },
    Field {
        key: "CLAUDE_EFFORT",
        group: CLAUDE,
        label: "Effort",
        kind: Kind::Choice {
            options: &["low", "medium", "high", "xhigh", "max"],
        },
        default: Some("medium"),
        help: "How hard Claude thinks about each batch. Higher costs more.",
    },
    Field {
        key: "HOST_PERSONA",
        group: CLAUDE,
        label: "Host persona",
        kind: Kind::LongText {
            max: triviabix_questions::MAX_PERSONA_CHARS,
        },
        default: Some(triviabix_questions::DEFAULT_PERSONA),
        help: "Who Bix is: his character, history and sense of humour, in a paragraph. Claude writes every new line in this voice. Lines already written stay as they are. This is the persona in use; Pipeline → Personas keeps others, each with its own lines, to switch between.",
    },
    Field {
        key: "TTS_URL",
        group: VOICE,
        label: "Voice server address",
        kind: Kind::Url,
        default: None,
        help: "An always-on Ming TTS server, e.g. http://192.168.1.20:8000. Leave empty to use rented GPUs only.",
    },
    Field {
        key: "TTS_MODEL",
        group: VOICE,
        label: "Voice model (on rented GPUs)",
        kind: Kind::Model,
        default: Some("inclusionAI/Ming-3.0-tts-0.5b"),
        help: "Lines already made keep playing; new lines use the new model.",
    },
    Field {
        key: "BIX_VOICE",
        group: VOICE,
        label: "Voice (on rented GPUs)",
        kind: Kind::Text,
        default: Some("ip:US_James"),
        help: "One of the model's built-in voices as ip:NAME (US_James, US_Emma, UK_George, UK_Charlotte), or a designed voice as clone:ID, which Pipeline → Design Bix's voice sets for you. Each voice is a voice pack (Pipeline → Voice packs) that keeps every line made in it, so switching back costs nothing.",
    },
    Field {
        key: "BIX_SPEED",
        group: VOICE,
        label: "Speaking speed",
        kind: Kind::Choice {
            options: &["1.0", "1.1", "1.15", "1.2", "1.25", "1.3", "1.4"],
        },
        default: Some("1.0"),
        help: "How fast Bix talks: 1.2 is a fifth faster. The TV speeds his lines up as they play, keeping his pitch, so nothing is made again and it changes at once (refresh the TV page). Each voice pack remembers its own speed.",
    },
    Field {
        key: "ART_IMAGE_MODEL",
        group: ART,
        label: "Picture model",
        kind: Kind::Model,
        default: Some("black-forest-labs/FLUX.1-schnell"),
        help: "Backdrops, stickers and category cards. Changing a model makes those pictures again the next time art runs; the old ones stay in the asset store.",
    },
    Field {
        key: "ART_CUTOUT_MODEL",
        group: ART,
        label: "Cut-out model",
        kind: Kind::Model,
        default: Some("ZhengPeng7/BiRefNet"),
        help: "Cuts stickers out of their background.",
    },
    Field {
        key: "ART_VIDEO_MODEL",
        group: ART,
        label: "Video model",
        kind: Kind::Model,
        default: Some("Wan-AI/Wan2.2-TI2V-5B-Diffusers"),
        help: "The short cutscene clips.",
    },
    Field {
        key: "LIDARR_URL",
        group: LIDARR,
        label: "Lidarr address",
        kind: Kind::Url,
        default: None,
        help: "e.g. http://192.168.1.20:8686",
    },
    Field {
        key: "LIDARR_API_KEY",
        group: LIDARR,
        label: "Lidarr API key",
        kind: Kind::Secret,
        default: None,
        help: "In Lidarr: Settings → General → API Key.",
    },
];

/// How a field looks on the Settings page. `value` is only filled in for fields that aren't secret.
#[derive(Debug, Serialize)]
pub struct FieldView {
    pub key: &'static str,
    pub group: &'static str,
    pub label: &'static str,
    pub kind: Kind,
    pub default: Option<&'static str>,
    pub help: &'static str,
    /// "saved" (set on this page), "env" (the container's variable) or "unset".
    pub source: &'static str,
    pub value: Option<String>,
}

#[derive(Clone)]
pub struct Settings {
    db: Db,
    saved: Arc<RwLock<HashMap<String, String>>>,
    /// Values the server was started with (from its environment), where the config has them.
    started: Arc<HashMap<&'static str, String>>,
}

fn field(key: &str) -> Option<&'static Field> {
    FIELDS.iter().find(|f| f.key == key)
}

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

impl Settings {
    pub async fn load(db: Db) -> anyhow::Result<Self> {
        let rows = sqlx::query("SELECT key, value FROM settings")
            .fetch_all(&db.pool)
            .await?;
        let saved = rows
            .iter()
            .map(|r| (r.get::<String, _>("key"), r.get::<String, _>("value")))
            .filter(|(k, _)| field(k).is_some())
            .collect();
        Ok(Self {
            db,
            saved: Arc::new(RwLock::new(saved)),
            started: Default::default(),
        })
    }

    /// Uses `value` from the server's config for `key` when nothing is saved, in place of reading
    /// the environment variable again.
    pub fn starting_with(mut self, key: &'static str, value: Option<String>) -> Self {
        if let Some(v) = value.filter(|v| !v.trim().is_empty()) {
            Arc::make_mut(&mut self.started).insert(key, v);
        }
        self
    }

    /// What's used when nothing is saved.
    fn fallback(&self, key: &str) -> Option<String> {
        self.started.get(key).cloned().or_else(|| env(key))
    }

    /// The value in use: the saved one, else the environment variable.
    pub fn get(&self, key: &str) -> Option<String> {
        self.saved
            .read()
            .unwrap()
            .get(key)
            .cloned()
            .or_else(|| self.fallback(key))
    }

    pub fn is_set(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// A job's price limit: the setting if it's a sensible number, never above the ceiling.
    pub fn price(&self, key: &str, default: f64) -> f64 {
        self.get(key)
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|p| p.is_finite() && *p > 0.0)
            .unwrap_or(default)
            .min(PRICE_CEILING)
    }

    /// The saved values, to pass to the tools a job runs. Variables come along on their own.
    pub fn saved_env(&self) -> Vec<(String, String)> {
        let saved = self.saved.read().unwrap();
        saved.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    pub fn view(&self) -> Vec<FieldView> {
        let saved = self.saved.read().unwrap();
        FIELDS
            .iter()
            .map(|f| {
                let (source, value) = match (saved.get(f.key), self.fallback(f.key)) {
                    (Some(v), _) => ("saved", Some(v.clone())),
                    (None, Some(v)) => ("env", Some(v)),
                    (None, None) => ("unset", None),
                };
                FieldView {
                    key: f.key,
                    group: f.group,
                    label: f.label,
                    kind: f.kind,
                    default: f.default,
                    help: f.help,
                    source,
                    value: value.filter(|_| f.kind != Kind::Secret),
                }
            })
            .collect()
    }

    /// Saves a value, or with `None` (or only spaces) forgets it so the variable is used again.
    pub async fn set(&self, key: &str, value: Option<&str>) -> Result<(), String> {
        let f = field(key).ok_or("There's no such setting.")?;
        let value = value.map(str::trim).filter(|v| !v.is_empty());
        match value {
            Some(v) => {
                let long = matches!(f.kind, Kind::LongText { .. });
                if v.contains('\0') || (!long && (v.len() > 4096 || v.contains(['\n', '\r']))) {
                    return Err(format!(
                        "That doesn't look like a {}.",
                        f.label.to_lowercase()
                    ));
                }
                let v = check(f, v)?;
                sqlx::query(
                    "INSERT INTO settings (key, value) VALUES (?, ?)
                     ON CONFLICT (key) DO UPDATE SET value = excluded.value, updated_at = CURRENT_TIMESTAMP",
                )
                .bind(key)
                .bind(&v)
                .execute(&self.db.pool)
                .await
                .map_err(|e| format!("Couldn't save it: {e}"))?;
                self.saved.write().unwrap().insert(key.into(), v);
            }
            None => {
                sqlx::query("DELETE FROM settings WHERE key = ?")
                    .bind(key)
                    .execute(&self.db.pool)
                    .await
                    .map_err(|e| format!("Couldn't clear it: {e}"))?;
                self.saved.write().unwrap().remove(key);
            }
        }
        Ok(())
    }
}

impl Settings {
    /// Asks a service whether the settings in use work: Lidarr, the voice server, vast.ai,
    /// Anthropic, Hugging Face or GitHub's package registry. Only free calls that read the account, so nothing is spent.
    pub async fn test(&self, service: &str) -> Result<String, String> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .user_agent("TriviaBix/0.1")
            .build()
            .map_err(|e| e.to_string())?;
        let need = |key: &str, what: &str| self.get(key).ok_or(format!("Set {what} first."));
        let answer = |res: Result<reqwest::Response, reqwest::Error>, who: &str| {
            let res = res.map_err(|e| format!("Couldn't reach {who}: {}", short(e)))?;
            match res.status().as_u16() {
                200..=299 => Ok(res),
                401 | 403 => Err(format!("{who} didn't accept the key.")),
                code => Err(format!("{who} answered {code}.")),
            }
        };
        match service {
            "lidarr" => {
                let url = need("LIDARR_URL", "Lidarr's address")?;
                let key = need("LIDARR_API_KEY", "Lidarr's API key")?;
                let res = http
                    .get(format!("{url}/api/v1/system/status"))
                    .header("X-Api-Key", key)
                    .send()
                    .await;
                let body: serde_json::Value =
                    answer(res, "Lidarr")?.json().await.unwrap_or_default();
                Ok(match body["version"].as_str() {
                    Some(v) => format!("Lidarr {v} is answering. The library syncs from it."),
                    None => "Lidarr is answering.".into(),
                })
            }
            "voice" => {
                let url = need("TTS_URL", "the voice server's address")?;
                let res = http.get(format!("{url}/healthz")).send().await;
                let body: serde_json::Value = answer(res, "The voice server")?
                    .json()
                    .await
                    .unwrap_or_default();
                Ok(if body["ready"] == true {
                    format!(
                        "The voice server is ready, speaking as {}.",
                        body["voice"].as_str().unwrap_or("Bix")
                    )
                } else {
                    "The voice server is up but still loading its model.".into()
                })
            }
            "vast" => {
                let key = need("VAST_API_KEY", "the vast.ai API key")?;
                let api = std::env::var("VAST_URL").unwrap_or("https://console.vast.ai".into());
                let res = http
                    .get(format!(
                        "{}/api/v0/users/current/",
                        api.trim_end_matches('/')
                    ))
                    .bearer_auth(key)
                    .send()
                    .await;
                let body: serde_json::Value =
                    answer(res, "vast.ai")?.json().await.unwrap_or_default();
                Ok(match body["credit"].as_f64() {
                    Some(c) => format!("vast.ai accepted the key. Credit left: ${c:.2}."),
                    None => "vast.ai accepted the key.".into(),
                })
            }
            "anthropic" => {
                let key = need("ANTHROPIC_API_KEY", "the Anthropic API key")?;
                let res = http
                    .get("https://api.anthropic.com/v1/models")
                    .header("x-api-key", key)
                    .header("anthropic-version", "2023-06-01")
                    .send()
                    .await;
                answer(res, "Anthropic")?;
                Ok("Anthropic accepted the key.".into())
            }
            "huggingface" => {
                let key = need("HF_TOKEN", "the Hugging Face token")?;
                let res = http
                    .get("https://huggingface.co/api/whoami-v2")
                    .bearer_auth(key)
                    .send()
                    .await;
                let body: serde_json::Value = answer(res, "Hugging Face")?
                    .json()
                    .await
                    .unwrap_or_default();
                Ok(match body["name"].as_str() {
                    Some(n) => format!("Hugging Face knows this token as {n}."),
                    None => "Hugging Face accepted the token.".into(),
                })
            }
            "github" => {
                let token = need("GHCR_TOKEN", "the GitHub package token")?;
                let user = self.get("GHCR_USER").unwrap_or("PiccoloGravel".into());
                let mut can = vec![];
                let mut cannot = vec![];
                for image in GHCR_IMAGES {
                    // The registry's own pull check: trade the token for a pull grant, then ask
                    // for the image's manifest with it. Nothing is downloaded.
                    let res = http
                        .get("https://ghcr.io/token")
                        .query(&[
                            ("service", "ghcr.io".to_string()),
                            ("scope", format!("repository:{GHCR_OWNER}/{image}:pull")),
                        ])
                        .basic_auth(&user, Some(&token))
                        .send()
                        .await;
                    let grant: serde_json::Value = answer(res, "GitHub")
                        .map_err(|e| {
                            if e.ends_with("didn't accept the key.") {
                                "GitHub didn't accept the token. Check the username, and that the token has the read:packages scope and hasn't expired.".into()
                            } else {
                                e
                            }
                        })?
                        .json()
                        .await
                        .unwrap_or_default();
                    let bearer = grant["token"].as_str().unwrap_or_default();
                    let res = http
                        .head(format!(
                            "https://ghcr.io/v2/{GHCR_OWNER}/{image}/manifests/latest"
                        ))
                        .bearer_auth(bearer)
                        .header(
                            "accept",
                            "application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json",
                        )
                        .send()
                        .await
                        .map_err(|e| format!("Couldn't reach GitHub: {}", short(e)))?;
                    if res.status().is_success() {
                        can.push(*image);
                    } else {
                        cannot.push(*image);
                    }
                }
                match (can.is_empty(), cannot.is_empty()) {
                    (_, true) => Ok(format!("The token can pull {}.", can.join(", "))),
                    (true, _) => Err(format!(
                        "GitHub accepted the token, but it can't pull {}. Check it has read:packages and that {user} can see the packages.",
                        cannot.join(", ")
                    )),
                    _ => Ok(format!(
                        "The token can pull {}. It can't pull {} (not published yet, or not visible to {user}).",
                        can.join(", "),
                        cannot.join(", ")
                    )),
                }
            }
            _ => Err("There's nothing to test by that name.".into()),
        }
    }
}

/// An HTTP error without the URL, which could carry a key in a query string.
fn short(e: reqwest::Error) -> String {
    let e = e.without_url();
    if e.is_timeout() {
        "it didn't answer in time".into()
    } else if e.is_connect() {
        "nothing answered at that address".into()
    } else {
        e.to_string()
    }
}

/// Checks a new value and gives the form to keep.
fn check(f: &Field, v: &str) -> Result<String, String> {
    match f.kind {
        Kind::Secret | Kind::Text => Ok(v.to_string()),
        Kind::LongText { max } => {
            let v = v.replace("\r\n", "\n").replace('\r', "\n");
            let n = v.chars().count();
            if n > max {
                Err(format!("Keep it under {max} characters; that's {n}."))
            } else {
                Ok(v)
            }
        }
        Kind::Url => {
            if v.starts_with("http://") || v.starts_with("https://") {
                Ok(v.trim_end_matches('/').to_string())
            } else {
                Err("The address should start with http:// or https://.".into())
            }
        }
        Kind::Model => {
            let ok = v.split_once('/').is_some_and(|(owner, name)| {
                [owner, name].iter().all(|p| {
                    !p.is_empty()
                        && p.chars()
                            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
                })
            });
            if ok {
                Ok(v.to_string())
            } else {
                Err("Give the model as it's named on Hugging Face, like owner/name.".into())
            }
        }
        Kind::Choice { options } => {
            if options.contains(&v) {
                Ok(v.to_string())
            } else {
                Err(format!("Pick one of: {}.", options.join(", ")))
            }
        }
        Kind::Price { max } => match v.trim_start_matches('$').parse::<f64>() {
            Ok(p) if p.is_finite() && p >= 0.05 && p <= max => Ok(format!("{p:.2}")),
            _ => Err(format!("Give a price per hour from $0.05 to ${max:.2}.")),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn db() -> Db {
        let path =
            std::env::temp_dir().join(format!("triviabix-settings-{}.db", uuid::Uuid::new_v4()));
        Db::connect(&format!("sqlite://{}", path.display()))
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn saved_values_win_and_survive_a_restart() {
        let db = db().await;
        let s = Settings::load(db.clone())
            .await
            .unwrap()
            .starting_with("TTS_URL", Some("http://from-config:8000".into()));
        assert_eq!(s.get("TTS_URL").as_deref(), Some("http://from-config:8000"));
        assert_eq!(
            s.view().iter().find(|f| f.key == "TTS_URL").unwrap().source,
            "env"
        );

        s.set("TTS_URL", Some("http://saved:8000/")).await.unwrap();
        s.set("ANTHROPIC_API_KEY", Some("sk-test")).await.unwrap();
        assert_eq!(s.get("TTS_URL").as_deref(), Some("http://saved:8000"));

        let again = Settings::load(db)
            .await
            .unwrap()
            .starting_with("TTS_URL", Some("http://from-config:8000".into()));
        assert_eq!(again.get("TTS_URL").as_deref(), Some("http://saved:8000"));
        assert_eq!(again.get("ANTHROPIC_API_KEY").as_deref(), Some("sk-test"));
        let view = again.view();
        let key = view.iter().find(|f| f.key == "ANTHROPIC_API_KEY").unwrap();
        assert_eq!((key.source, key.value.as_deref()), ("saved", None));

        again.set("TTS_URL", Some("  ")).await.unwrap();
        assert_eq!(
            again.get("TTS_URL").as_deref(),
            Some("http://from-config:8000")
        );
    }

    #[tokio::test]
    async fn the_persona_takes_several_lines() {
        let s = Settings::load(db().await).await.unwrap();
        assert_eq!(s.get("HOST_PERSONA"), None);
        s.set("HOST_PERSONA", Some("Bix is tired.\r\n\r\nVery tired."))
            .await
            .unwrap();
        assert_eq!(
            s.get("HOST_PERSONA").as_deref(),
            Some("Bix is tired.\n\nVery tired.")
        );
        let long = "a".repeat(triviabix_questions::MAX_PERSONA_CHARS + 1);
        assert!(s.set("HOST_PERSONA", Some(&long)).await.is_err());
        // Other text settings still take one line.
        assert!(s.set("BIX_VOICE", Some("ip:US_James\nx")).await.is_err());
    }

    #[tokio::test]
    async fn prices_never_pass_the_ceiling() {
        let s = Settings::load(db().await).await.unwrap();
        assert_eq!(s.price("VOICE_MAX_PRICE", VOICE_PRICE), VOICE_PRICE);
        assert!(s.set("VOICE_MAX_PRICE", Some("2.01")).await.is_err());
        assert!(s.set("VOICE_MAX_PRICE", Some("0.01")).await.is_err());
        s.set("VOICE_MAX_PRICE", Some("2")).await.unwrap();
        assert_eq!(s.price("VOICE_MAX_PRICE", VOICE_PRICE), 2.0);
        // Even a value written to the database by hand is held to the ceiling.
        s.saved
            .write()
            .unwrap()
            .insert("VOICE_MAX_PRICE".into(), "50".into());
        assert_eq!(s.price("VOICE_MAX_PRICE", VOICE_PRICE), PRICE_CEILING);
    }
}
