//! JSON messages exchanged over the WebSocket. Every message has a `type` tag.

use serde::{Deserialize, Serialize};

use crate::db::Rating;

/// Messages sent by the TV host screen or by a phone.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    /// Host or VIP: start the game from the lobby.
    Start,
    /// Player: lock in an answer for the current question.
    Answer { choice: usize },
    /// Player, speed hangman: guess one letter.
    Guess { letter: char },
    /// Player, speed hangman: try to solve the whole phrase.
    Solve { text: String },
    /// Host or VIP: skip the rest of the current timer.
    Next,
    /// Host or VIP: back to the lobby after the final scores.
    PlayAgain,
    /// Host only: change settings while in the lobby.
    Settings { rounds: Option<usize>, max_rating: Option<Rating>, mode: Option<GameMode> },
    /// Host only: remove a player from the room.
    Kick { player_id: u32 },
    /// Keep-alive from the browser; ignored.
    Ping,
}

/// Which game a room plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GameMode {
    /// Multiple choice questions.
    #[default]
    Trivia,
    /// One shared phrase per round; everyone guesses letters against the clock.
    Hangman,
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
    State(Box<StateView<'a>>),
    /// `fatal` errors are followed by the server closing the socket.
    Error { message: &'a str, fatal: bool },
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
    pub reveal: Option<RevealView>,
    /// Only in speed hangman rounds.
    pub hangman: Option<HangmanView<'a>>,
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
    pub mode: GameMode,
}

#[derive(Debug, Serialize)]
pub struct QuestionView<'a> {
    pub category: &'a str,
    pub prompt: &'a str,
    pub choices: &'a [String],
}

#[derive(Debug, Serialize)]
pub struct RevealView {
    pub correct: usize,
    pub results: Vec<ResultView>,
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
    pub hangman: Option<HangmanYouView>,
}

#[derive(Debug, Serialize)]
pub struct HangmanView<'a> {
    pub category: &'a str,
    /// The phrase with unfound letters as `_`. The whole phrase once the round is over.
    pub board: &'a str,
    pub hits: &'a str,
    pub misses: &'a str,
    pub strikes: usize,
    pub max_strikes: usize,
    /// The latest guesses, oldest first.
    pub feed: &'a [GuessView],
    /// Points each player has won or lost this round so far.
    pub results: Vec<ResultView>,
    /// `solved`, `hanged` or `time`, once the round is over.
    pub outcome: Option<&'static str>,
    pub solved_by: Option<u32>,
}

#[derive(Debug, Serialize, Clone)]
pub struct GuessView {
    pub player_id: u32,
    /// The letter guessed, or the attempted answer for a solve.
    pub guess: String,
    pub solve: bool,
    pub hit: bool,
    /// How many times a guessed letter appears.
    pub count: usize,
}

#[derive(Debug, Serialize)]
pub struct HangmanYouView {
    /// A wrong solve locks a player out of solving for the rest of the round.
    pub solve_locked: bool,
    /// Milliseconds before this player may guess another letter.
    pub cooldown_ms: u64,
}
