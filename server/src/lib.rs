pub mod admin;
pub mod characters;
pub mod content;
pub mod db;
pub mod hangman;
pub mod host;
pub mod music;
pub mod protocol;
pub mod room;
pub mod rooms;
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
    pub timing: room::Timing,
    /// The question bank folder (categories.toml, packs/, scripts/), loaded into the database at
    /// startup. Skipped if it doesn't exist.
    pub content_dir: PathBuf,
    /// A host script to use instead of the built-in one.
    pub host_script: Option<PathBuf>,
    /// Turns on the admin panel at /admin. Without it the panel stays locked.
    pub admin_password: Option<String>,
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
        Config {
            database_url: var("DATABASE_URL", "sqlite://data/triviabix.db".to_string()),
            static_dir: var("STATIC_DIR", PathBuf::from("web")),
            public_url: std::env::var("PUBLIC_URL").ok().filter(|u| !u.is_empty()),
            max_players: var("MAX_PLAYERS", 12usize).max(1),
            max_rooms: var("MAX_ROOMS", 500usize),
            itunes_url: var("ITUNES_API_URL", "https://itunes.apple.com".to_string()),
            music_country: var("MUSIC_COUNTRY", "US".to_string()),
            music_warmup: var("MUSIC_WARMUP", true),
            timing: room::Timing {
                question: secs("QUESTION_SECS", 20),
                hangman: secs("HANGMAN_SECS", 45),
                song_question: secs("SONG_SECS", 15),
                reveal: secs("REVEAL_SECS", 7),
                scoreboard: secs("SCOREBOARD_SECS", 6),
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
        }
    }
}
