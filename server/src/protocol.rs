//! JSON messages exchanged over the WebSocket. Every message has a `type` tag.

use serde::{Deserialize, Serialize};

use crate::db::{Difficulty, Rating};

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
    /// Host or VIP: abandon the game in progress and go back to the lobby to pick another.
    Lobby,
    /// Host: change settings while in the lobby (late joins can change any time).
    /// The VIP may only change `mode`; everything else is for the admin panel.
    Settings {
        rounds: Option<usize>,
        max_rating: Option<Rating>,
        mode: Option<GameMode>,
        clip_ms: Option<u64>,
        teams: Option<u8>,
        late_join: Option<bool>,
        trivia_rounds: Option<u8>,
        per_round: Option<usize>,
        song_source: Option<SongSource>,
        minigames: Option<bool>,
    },
    /// Player: switch to another team while in the lobby.
    Team { team: u8 },
    /// Player, in the lobby: change your name and/or character.
    Profile {
        name: Option<String>,
        character: Option<String>,
    },
    /// Host, or the VIP while in the lobby: remove a player from the room.
    Kick { player_id: u32 },
    /// TV: the host has finished saying line `id` (and its cutscene is over).
    Spoken { id: u64 },
    /// Player, in Button Mash, Quick Draw or Stop the Clock: one tap.
    MiniTap,
    /// Player, in Closest Guess, Lowball or Majority Rules (the option's index): lock in a number.
    MiniGuess { value: i64 },
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

/// Where Name That Tune's songs come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SongSource {
    /// Half from the music library, half from the curated hits.
    #[default]
    Mix,
    /// The music library only, topped up with hits if it runs short.
    Library,
    /// The curated hits only (played from the library when it has them, else Apple previews).
    Hits,
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
    /// 1-based number of the current question, puzzle or song across the whole game; 0 in the lobby.
    pub round: usize,
    pub total_rounds: usize,
    /// Trivia only: the game's rounds, easiest first. Empty in the lobby and in other modes.
    pub stages: Vec<StageView>,
    /// 1-based number of the round being played; 0 when there is none.
    pub stage: usize,
    /// 1-based question number within the round; 0 on the round's title card.
    pub stage_question: usize,
    /// Between rounds: what the round asked and who scored.
    pub recap: Option<RecapView<'a>>,
    pub question: Option<QuestionView<'a>>,
    /// Milliseconds until the current phase advances on its own.
    pub ms_left: Option<u64>,
    pub phase_ms: Option<u64>,
    pub reveal: Option<RevealView<'a>>,
    /// TV only: the next round's song preview, so it can load before it's needed.
    pub next_clip: Option<&'a str>,
    /// TV only: whether to show Bix and the words he says.
    pub tv: Option<TvView>,
    /// TV only: background music from the library for the lobby and scoreboards, within the
    /// room's rating. Empty without a music library.
    pub playlist: Vec<PlaylistView<'a>>,
    /// Only in speed hangman rounds.
    pub hangman: Option<HangmanView<'a>>,
    /// Only during a mini game between trivia rounds.
    pub minigame: Option<MinigameView>,
    /// What the host is saying right now, with graphics for the TV to show alongside it.
    pub host: Option<HostView<'a>>,
    /// Only on player connections.
    pub you: Option<YouView>,
}

#[derive(Debug, Serialize, Clone, Copy)]
pub struct TvView {
    pub bix: bool,
    pub captions: bool,
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
    /// Trivia only: how many rounds a game has (3 or 4), each harder than the last.
    pub trivia_rounds: u8,
    /// Trivia only: questions in each round.
    pub per_round: usize,
    /// Name That Tune: where the songs come from.
    pub song_source: SongSource,
    /// Trivia: a mini game between rounds.
    pub minigames: bool,
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

/// One round of a trivia game: its theme, difficulty and how much its questions are worth.
#[derive(Debug, Serialize, Clone)]
pub struct StageView {
    pub number: usize,
    /// "Round 2", or "Final Round".
    pub title: String,
    /// Kept secret until the round starts.
    pub category: Option<String>,
    pub difficulty: Difficulty,
    /// Points are scaled by this percentage: 100, 150, 200 or 300.
    pub points_pct: u32,
    pub final_round: bool,
    pub questions: usize,
    /// done, current or next.
    pub status: &'static str,
}

#[derive(Debug, Serialize, Clone)]
pub struct RecapView<'a> {
    /// 1-based number of the round that just ended.
    pub stage: usize,
    pub questions: Vec<RecapQuestionView<'a>>,
    /// Points each playing player won in the round.
    pub points: Vec<ResultView>,
}

#[derive(Debug, Serialize, Clone)]
pub struct RecapQuestionView<'a> {
    pub prompt: &'a str,
    pub answer: &'a str,
    /// Players who got it right.
    pub right: Vec<u32>,
}

#[derive(Debug, Serialize)]
pub struct QuestionView<'a> {
    /// "trivia" or "song".
    pub kind: &'static str,
    pub category: &'a str,
    pub prompt: &'a str,
    /// TV only, with a TTS service: where to fetch the prompt read aloud.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_audio: Option<String>,
    pub choices: &'a [String],
    /// TV only: what to play. Phones never get the audio, so they have to listen.
    pub clip: Option<ClipView<'a>>,
}

#[derive(Debug, Serialize, Clone)]
pub struct PlaylistView<'a> {
    pub url: &'a str,
    pub title: &'a str,
    pub artist: &'a str,
    pub start_ms: u64,
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
    pub album: Option<&'a str>,
    /// Played from the operator's music library rather than an Apple preview.
    pub from_library: bool,
}

#[derive(Debug, Serialize, Clone)]
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
    /// TV only: the line in pieces, each with where to fetch its audio (with a TTS service) and
    /// the character Bix plays for it, if any.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub voice: &'a [VoicePart],
    pub callouts: &'a [Callout],
    /// The moment this line is for, so the TV can play a cutscene and pick the host's face: intro,
    /// round, final_round, winner, finale_tie, or the biggest callout's kind. See web/cutscenes/.
    pub cue: Option<&'static str>,
}

/// One piece of a host line and its synthesized audio, if the TTS service can make it.
#[derive(Debug, Serialize, Clone)]
pub struct VoicePart {
    pub text: String,
    pub audio: Option<String>,
    /// The character Bix plays for this piece, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cameo: Option<CameoView>,
}

/// A character Bix breaks into for one piece of a line: the TV dresses him up and plays the
/// audio at `rate`, shifting the pitch with it if `shift`.
#[derive(Debug, Serialize, Clone)]
pub struct CameoView {
    pub id: &'static str,
    pub name: &'static str,
    pub emoji: &'static str,
    pub rate: f32,
    pub shift: bool,
}

impl From<&'static triviabix_questions::Cameo> for CameoView {
    fn from(c: &'static triviabix_questions::Cameo) -> Self {
        CameoView {
            id: c.slug,
            name: c.name,
            emoji: c.emoji,
            rate: c.rate,
            shift: c.shift,
        }
    }
}

/// A graphic that pops up on the TV with the host's line: a streak flame, a roast stamp, a crown.
#[derive(Debug, Serialize, Clone)]
pub struct Callout {
    /// streak, streak_broken, roast, asleep, only_one, fastest, nobody, everyone, new_leader,
    /// leader, tie, last_place, round_winner, winner, mini_winner.
    pub kind: &'static str,
    pub player_id: Option<u32>,
    /// Short overlay text, like "3 IN A ROW".
    pub label: String,
}

/// A mini game between trivia rounds (see crate::minigames).
#[derive(Debug, Serialize)]
pub struct MinigameView {
    /// mash, closest, quick_draw, stop_clock, majority or lowball.
    pub kind: &'static str,
    pub title: &'static str,
    pub rules: &'static str,
    /// rules, ready (Quick Draw, before GO), play or results.
    pub step: &'static str,
    /// Closest Guess or Majority Rules: the question; Closest Guess: the answer's unit.
    pub prompt: Option<&'static str>,
    pub unit: Option<&'static str>,
    /// Closest Guess, in the results only.
    pub answer: Option<i64>,
    /// Majority Rules: the answers to pick from.
    pub options: &'static [&'static str],
    /// Lowball: numbers go from 1 to this.
    pub max_pick: Option<i64>,
    /// Stop the Clock: the moment to tap, in ms after the start.
    pub target_ms: Option<i64>,
    /// Every playing player; best first in the results.
    pub entries: Vec<MiniEntryView>,
}

#[derive(Debug, Serialize)]
pub struct MiniEntryView {
    pub player_id: u32,
    /// Taps, the guess or pick (hidden from others until the results), or time in ms.
    pub value: Option<i64>,
    /// Has nothing left to do (guessed, or tapped after GO).
    pub done: bool,
    /// Quick Draw: tapped before GO.
    pub false_start: bool,
    /// Bonus points won, in the results.
    pub points: i64,
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
    /// Bix says a character's preview line on the TV, in the lobby or on the final scores.
    Cameo {
        id: String,
    },
    Settings {
        rounds: Option<usize>,
        max_rating: Option<Rating>,
        mode: Option<GameMode>,
        clip_ms: Option<u64>,
        teams: Option<u8>,
        late_join: Option<bool>,
        trivia_rounds: Option<u8>,
        per_round: Option<usize>,
        song_source: Option<SongSource>,
        minigames: Option<bool>,
        /// Room size, any time; never below the players already in the room.
        max_players: Option<usize>,
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
    /// Trivia only: the round being played, if any.
    pub stage: Option<StageView>,
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
