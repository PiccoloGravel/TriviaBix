//! JSON messages exchanged over the WebSocket. Every message has a `type` tag.

use serde::{Deserialize, Serialize};

use crate::db::Rating;
use crate::room::Mode;

/// Messages sent by the TV host screen or by a phone.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    /// Host or VIP: start the game from the lobby.
    Start,
    /// Player: lock in an answer for the current question.
    Answer { choice: usize },
    /// Host or VIP: skip the rest of the current timer.
    Next,
    /// Host or VIP: back to the lobby after the final scores.
    PlayAgain,
    /// Host only: change settings while in the lobby.
    Settings {
        rounds: Option<usize>,
        max_rating: Option<Rating>,
        mode: Option<Mode>,
        clip_ms: Option<u64>,
    },
    /// Host only: remove a player from the room.
    Kick { player_id: u32 },
    /// Keep-alive from the browser; ignored.
    Ping,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg<'a> {
    /// First message on every connection.
    Welcome {
        role: &'static str,
        code: &'a str,
        player_id: Option<u32>,
        token: &'a str,
    },
    State(StateView<'a>),
    /// `fatal` errors are followed by the server closing the socket.
    Error {
        message: &'a str,
        fatal: bool,
    },
}

#[derive(Debug, Serialize)]
pub struct StateView<'a> {
    pub code: &'a str,
    pub phase: &'static str,
    pub players: Vec<PlayerView<'a>>,
    pub vip: Option<u32>,
    pub max_players: usize,
    pub settings: SettingsView,
    /// 1-based round number; 0 in the lobby.
    pub round: usize,
    pub total_rounds: usize,
    pub question: Option<QuestionView<'a>>,
    /// Milliseconds until the current phase advances on its own.
    pub ms_left: Option<u64>,
    pub phase_ms: Option<u64>,
    pub reveal: Option<RevealView<'a>>,
    /// TV only: the next round's song preview, so it can load before it's needed.
    pub next_clip: Option<&'a str>,
    /// Only on player connections.
    pub you: Option<YouView>,
}

#[derive(Debug, Serialize)]
pub struct PlayerView<'a> {
    pub id: u32,
    pub name: &'a str,
    pub score: i64,
    pub color: &'static str,
    pub connected: bool,
    pub answered: bool,
}

#[derive(Debug, Serialize, Clone, Copy)]
pub struct SettingsView {
    pub rounds: usize,
    pub max_rating: Rating,
    pub mode: Mode,
    pub clip_ms: u64,
}

#[derive(Debug, Serialize)]
pub struct QuestionView<'a> {
    /// "trivia" or "song".
    pub kind: &'static str,
    pub category: &'a str,
    pub prompt: &'a str,
    pub choices: &'a [String],
    /// TV only: what to play. Phones never get the audio, so they have to listen.
    pub clip: Option<ClipView<'a>>,
}

#[derive(Debug, Serialize)]
pub struct ClipView<'a> {
    pub preview_url: &'a str,
    pub start_ms: u64,
    pub clip_ms: u64,
}

#[derive(Debug, Serialize)]
pub struct RevealView<'a> {
    pub correct: usize,
    pub results: Vec<ResultView>,
    pub song: Option<SongView<'a>>,
}

/// Shown once a song round closes, with credit and a link to the full track.
#[derive(Debug, Serialize)]
pub struct SongView<'a> {
    pub title: &'a str,
    pub artist: &'a str,
    pub year: i64,
    pub artwork_url: Option<&'a str>,
    pub store_url: Option<&'a str>,
}

#[derive(Debug, Serialize)]
pub struct ResultView {
    pub player_id: u32,
    pub choice: Option<usize>,
    pub points: i64,
}

#[derive(Debug, Serialize)]
pub struct YouView {
    pub player_id: u32,
    pub answer: Option<usize>,
    pub is_vip: bool,
    pub rank: usize,
}
