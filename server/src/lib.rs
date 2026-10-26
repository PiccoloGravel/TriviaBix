pub mod admin;
pub mod characters;
pub mod content;
pub mod db;
pub mod duplicates;
pub mod hangman;
pub mod host;
pub mod library;
pub mod music;
pub mod pregen;
pub mod protocol;
pub mod render;
pub mod room;
pub mod rooms;
pub mod rounds;
pub mod voice;
pub mod web;

use std::path::PathBuf;
use std::time::Duration;

pub use web::router;

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub static_dir: PathBuf,
    pub public_url: Option<String>,
    pub max_players: usize,
    pub max_rooms: usize,
    /// Base URL of the iTunes Search API; tests point this at a fake.
    pub itunes_url: String,
    /// Store country for song previews (two-letter code).
    pub music_country: String,
    /// Look up every song in the background at startup so the first music game starts fast.
    pub music_warmup: bool,
    /// The music library in Lidarr, for Name That Tune and background music.
    pub library: library::LibraryConfig,
    pub timing: room::Timing,
    /// The question bank folder (categories.toml, packs/, scripts/), loaded into the database at
    /// startup. Skipped if it doesn't exist.
    pub content_dir: PathBuf,
    /// A host script to use instead of the built-in one.
    pub host_script: Option<PathBuf>,
    /// Turns on the admin panel at /admin. Without it the panel stays locked.
    pub admin_password: Option<String>,
    /// The TTS service that gives Bix his voice (see tts/). Without it the TV uses the browser's.
    pub tts_url: Option<String>,
    /// Names to voice lines for ahead of time, besides the players who've played the most.
    pub voice_names: Vec<String>,
    /// Where synthesized lines are kept: the asset store's voice folder (docs/assets.md).
    pub tts_cache_dir: PathBuf,
    /// Rendering jobs run from the admin panel (voice, art, host lines).
    pub render: render::RenderConfig,
}

impl Config {
    pub fn from_env() -> Self {
        fn var<T: std::str::FromStr>(key: &str, default: T) -> T {
            std::env::var(key)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default)
        }
        let secs = |key, default| Duration::from_secs(var(key, default));
        let nonempty = |key: &str| std::env::var(key).ok().filter(|v| !v.trim().is_empty());
        Config {
            database_url: var("DATABASE_URL", "sqlite://data/triviabix.db".to_string()),
            static_dir: var("STATIC_DIR", PathBuf::from("web")),
            public_url: std::env::var("PUBLIC_URL").ok().filter(|u| !u.is_empty()),
            max_players: var("MAX_PLAYERS", 12usize).max(1),
            max_rooms: var("MAX_ROOMS", 500usize),
            itunes_url: var("ITUNES_API_URL", "https://itunes.apple.com".to_string()),
            music_country: var("MUSIC_COUNTRY", "US".to_string()),
            music_warmup: var("MUSIC_WARMUP", true),
            library: library::LibraryConfig {
                lidarr_url: nonempty("LIDARR_URL"),
                api_key: nonempty("LIDARR_API_KEY"),
                music_dir: var("MUSIC_DIR", PathBuf::from("/music")),
                lidarr_music_path: nonempty("LIDARR_MUSIC_PATH"),
                rating: nonempty("LIBRARY_RATING")
                    .and_then(|r| db::Rating::parse(&r.to_ascii_uppercase()))
                    .unwrap_or(db::Rating::Pg13),
                sync_every: Duration::from_secs(var("LIDARR_SYNC_HOURS", 6u64) * 60 * 60),
            },
            timing: room::Timing {
                question: secs("QUESTION_SECS", 20),
                hangman: secs("HANGMAN_SECS", 45),
                song_question: secs("SONG_SECS", 15),
                reveal: secs("REVEAL_SECS", 7),
                scoreboard: secs("SCOREBOARD_SECS", 6),
                round_intro: secs("ROUND_INTRO_SECS", 5),
                recap: secs("RECAP_SECS", 14),
                intro: secs("INTRO_SECS", 6),
                talk_cps: var("HOST_TALK_CPS", 13.0),
                idle_timeout: secs("ROOM_IDLE_SECS", 15 * 60),
            },
            content_dir: var("CONTENT_DIR", PathBuf::from("content")),
            host_script: std::env::var("HOST_SCRIPT")
                .ok()
                .filter(|p| !p.is_empty())
                .map(PathBuf::from),
            admin_password: std::env::var("ADMIN_PASSWORD")
                .ok()
                .filter(|p| !p.is_empty()),
            tts_url: std::env::var("TTS_URL").ok().filter(|u| !u.is_empty()),
            voice_names: std::env::var("VOICE_NAMES")
                .unwrap_or_default()
                .split(',')
                .map(|n| n.trim().to_string())
                .filter(|n| !n.is_empty())
                .collect(),
            tts_cache_dir: var(
                "TTS_CACHE_DIR",
                var("ASSETS_DIR", PathBuf::from("data/assets")).join("voice"),
            ),
            render: render::RenderConfig {
                tools_dir: var("RENDER_TOOLS_DIR", PathBuf::from(".")),
                python: var("PYTHON", "python3".to_string()),
                questions_bin: var("QUESTIONS_BIN", "triviabix-questions".to_string()),
                art_dir: var("ART_DIR", PathBuf::from("data/art")),
                assets_dir: var("ASSETS_DIR", PathBuf::from("data/assets")),
                server_url: nonempty("RENDER_SERVER_URL")
                    .unwrap_or_else(|| format!("http://127.0.0.1:{}", var("PORT", 8080u16))),
            },
        }
    }
}
