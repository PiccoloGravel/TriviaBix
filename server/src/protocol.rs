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
    /// Host only: change settings while in the lobby (late joins can change any time).
    Settings {
        rounds: Option<usize>,
        max_rating: Option<Rating>,
        mode: Option<GameMode>,
        clip_ms: Option<u64>,
        teams: Option<u8>,
        late_join: Option<bool>,
    },
    /// Player: switch to another team while in the lobby.
    Team { team: u8 },
    /// Player, in the lobby: change your name and/or character.
    Profile {
        name: Option<String>,
        character: Option<String>,
    },
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
    /// Name That Tune: the TV plays a second or two of a song and phones pick the title.
    Songs,
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
    /// Team standings, best first; empty when teams are off.
    pub teams: Vec<TeamView>,
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
    /// Only in speed hangman rounds.
    pub hangman: Option<HangmanView<'a>>,
    /// What the host is saying right now, with graphics for the TV to show alongside it.
    pub host: Option<HostView<'a>>,
    /// Only on player connections.
    pub you: Option<YouView>,
}

#[derive(Debug, Serialize)]
pub struct PlayerView<'a> {
    pub id: u32,
    pub name: &'a str,
    pub score: i64,
    pub color: &'static str,
    pub character: &'a str,
    pub connected: bool,
    pub answered: bool,
    pub team: Option<u8>,
    /// Joined mid-game; plays from the next question.
    pub waiting: bool,
}

#[derive(Debug, Serialize, Clone, Copy)]
pub struct SettingsView {
    pub rounds: usize,
    pub max_rating: Rating,
    pub mode: GameMode,
    pub clip_ms: u64,
    /// Number of teams; 0 means everyone plays for themselves.
    pub teams: u8,
    /// Whether new players can join after the game has started.
    pub late_join: bool,
}

#[derive(Debug, Serialize, Clone)]
pub struct TeamView {
    pub id: u8,
    pub name: &'static str,
    pub color: &'static str,
    /// The average of its playing members' scores, so uneven teams stay fair.
    pub score: i64,
    pub members: Vec<u32>,
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
pub struct HostView<'a> {
    pub name: &'a str,
    /// Changes with every new line, so the TV speaks each one once.
    pub id: u64,
    pub text: &'a str,
    pub callouts: &'a [Callout],
}

/// A graphic that pops up on the TV with the host's line: a streak flame, a roast stamp, a crown.
#[derive(Debug, Serialize, Clone)]
pub struct Callout {
    /// streak, streak_broken, roast, asleep, only_one, fastest, nobody, everyone, new_leader,
    /// leader, tie, last_place, winner.
    pub kind: &'static str,
    pub player_id: Option<u32>,
    /// Short overlay text, like "3 IN A ROW".
    pub label: String,
}

#[derive(Debug, Serialize)]
pub struct YouView {
    pub player_id: u32,
    pub answer: Option<usize>,
    pub is_vip: bool,
    pub rank: usize,
    pub hangman: Option<HangmanYouView>,
    pub team: Option<u8>,
    pub waiting: bool,
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

/// Operator commands from the admin panel. Admins can do anything the host can.
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum AdminCmd {
    Start,
    Skip,
    /// Jump to the final scores now.
    EndGame,
    /// Back to the lobby, from the final scores or mid-game.
    Lobby,
    /// Shut the room down and disconnect everyone.
    Close,
    Kick {
        player_id: u32,
    },
    SetTeam {
        player_id: u32,
        team: Option<u8>,
    },
    Settings {
        rounds: Option<usize>,
        max_rating: Option<Rating>,
        mode: Option<GameMode>,
        clip_ms: Option<u64>,
        teams: Option<u8>,
        late_join: Option<bool>,
    },
}

/// Everything about a room, for the admin panel.
#[derive(Debug, Serialize)]
pub struct AdminRoomView {
    pub code: String,
    /// Lets the operator open (or cast) this room's TV page.
    pub host_token: String,
    /// Unix time in milliseconds.
    pub created_at: u64,
    pub phase: &'static str,
    pub round: usize,
    pub total_rounds: usize,
    pub ms_left: Option<u64>,
    pub settings: SettingsView,
    pub max_players: usize,
    pub host_connected: bool,
    pub vip: Option<u32>,
    pub players: Vec<AdminPlayerView>,
    pub teams: Vec<TeamView>,
    pub question: Option<AdminQuestionView>,
}

#[derive(Debug, Serialize)]
pub struct AdminPlayerView {
    pub id: u32,
    pub name: String,
    pub score: i64,
    pub color: &'static str,
    pub character: String,
    pub connected: bool,
    pub answered: bool,
    pub team: Option<u8>,
    pub waiting: bool,
}

#[derive(Debug, Serialize)]
pub struct AdminQuestionView {
    pub category: String,
    pub prompt: String,
    pub answer: String,
}
