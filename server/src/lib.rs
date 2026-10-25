pub mod characters;
pub mod db;
pub mod host;
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
    pub timing: room::Timing,
    /// A host script to use instead of the built-in one.
    pub host_script: Option<PathBuf>,
}

impl Config {
    pub fn from_env() -> Self {
        fn var<T: std::str::FromStr>(key: &str, default: T) -> T {
            std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
        }
        let secs = |key, default| Duration::from_secs(var(key, default));
        Config {
            database_url: var("DATABASE_URL", "sqlite://data/triviabix.db".to_string()),
            static_dir: var("STATIC_DIR", PathBuf::from("web")),
            public_url: std::env::var("PUBLIC_URL").ok().filter(|u| !u.is_empty()),
            max_players: var("MAX_PLAYERS", 12usize).max(1),
            max_rooms: var("MAX_ROOMS", 500usize),
            timing: room::Timing {
                question: secs("QUESTION_SECS", 20),
                reveal: secs("REVEAL_SECS", 7),
                scoreboard: secs("SCOREBOARD_SECS", 6),
                intro: secs("INTRO_SECS", 6),
                talk_cps: var("HOST_TALK_CPS", 13.0),
                idle_timeout: secs("ROOM_IDLE_SECS", 15 * 60),
            },
            host_script: std::env::var("HOST_SCRIPT").ok().filter(|p| !p.is_empty()).map(PathBuf::from),
        }
    }
}
