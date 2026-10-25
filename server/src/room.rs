//! One game room. Each room runs as its own task (an actor) that owns all of
//! the room's state; WebSocket connections talk to it through a channel.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, sleep_until};

use crate::characters::{self, CHARACTERS};
use crate::db::{Db, Difficulty, Question, Rating, Standing};
use crate::hangman::{self, Guess, Puzzle};
use crate::host::{self, Host, Script};
use crate::music::{self, Itunes};
use crate::protocol::*;
use crate::rounds;

pub type ConnTx = mpsc::UnboundedSender<String>;

const COLORS: [&str; 16] = [
    "#ff4f81", "#ffb400", "#2ec4b6", "#7b5cff", "#ff7a00", "#3fa9f5", "#9be15d", "#f15bb5",
    "#00bbf9", "#fee440", "#e76f51", "#43aa8b", "#c77dff", "#ff006e", "#8ac926", "#6a4c93",
];

/// Team names and colors, in order. A room can use up to this many teams.
const TEAMS: [(&str, &str); 4] = [
    ("Red Team", "#ff4f6d"),
    ("Blue Team", "#3fa9f5"),
    ("Gold Team", "#ffd23f"),
    ("Green Team", "#2ee59d"),
];

pub const MIN_ROUNDS: usize = 3;
pub const MIN_CLIP_MS: u64 = 500;
pub const MAX_CLIP_MS: u64 = 5_000;
pub const MAX_ROUNDS: usize = 30;
pub const MAX_TEAMS: u8 = TEAMS.len() as u8;
const MAX_NAME_CHARS: usize = 16;

// Speed hangman scoring. Letters pay per tile, misses cost everyone a strike, and solving
// early pays the most.
const LETTER_POINTS: i64 = 75;
const MISS_PENALTY: i64 = 50;
const SOLVE_POINTS: i64 = 250;
const SOLVE_POINTS_PER_BLANK: i64 = 50;
const SOLVE_SPEED_POINTS: i64 = 250;
/// Stops one fast thumb from running the whole alphabet.
const GUESS_COOLDOWN: Duration = Duration::from_millis(1000);
const MAX_SOLVE_CHARS: usize = 80;
const FEED_LEN: usize = 6;

#[derive(Debug, Clone)]
pub struct Timing {
    pub question: Duration,
    /// Time to crack one speed hangman phrase.
    pub hangman: Duration,
    /// Answer time for a Name That Tune round.
    pub song_question: Duration,
    pub reveal: Duration,
    pub scoreboard: Duration,
    /// The title card before each trivia round.
    pub round_intro: Duration,
    /// The scores and question recap after each trivia round.
    pub recap: Duration,
    /// The host's opening before the first question.
    pub intro: Duration,
    /// How fast the TV's voice talks, in characters per second. Phases stretch so the host can finish
    /// a line before the game moves on; 0 turns that off.
    pub talk_cps: f64,
    /// A room with no connections is closed after this long.
    pub idle_timeout: Duration,
}

#[derive(Debug, Clone, Copy)]
pub struct Settings {
    pub rounds: usize,
    pub max_rating: Rating,
    pub mode: GameMode,
    /// How much of the song the TV plays in Name That Tune.
    pub clip_ms: u64,
    /// 0 for no teams, otherwise 2 to MAX_TEAMS.
    pub teams: u8,
    /// Let new players join a running game; they start at the next question.
    pub late_join: bool,
    /// Trivia: rounds per game, from rounds::MIN_ROUNDS to rounds::MAX_ROUNDS.
    pub trivia_rounds: u8,
    /// Trivia: questions per round.
    pub per_round: usize,
}

impl Settings {
    fn view(&self) -> SettingsView {
        SettingsView {
            rounds: self.rounds,
            max_rating: self.max_rating,
            mode: self.mode,
            clip_ms: self.clip_ms,
            teams: self.teams,
            late_join: self.late_join,
            trivia_rounds: self.trivia_rounds,
            per_round: self.per_round,
        }
    }
}

/// 1 team is no teams; anything above the limit is capped.
pub fn clamp_teams(teams: u8) -> u8 {
    if teams < 2 { 0 } else { teams.min(MAX_TEAMS) }
}

/// A settings change from the TV or the admin panel; `None` leaves a setting as it is.
pub struct SettingsChange {
    pub rounds: Option<usize>,
    pub max_rating: Option<Rating>,
    pub mode: Option<GameMode>,
    pub clip_ms: Option<u64>,
    pub teams: Option<u8>,
    pub late_join: Option<bool>,
    pub trivia_rounds: Option<u8>,
    pub per_round: Option<usize>,
}

pub enum Join {
    Host {
        token: String,
    },
    Player {
        name: Option<String>,
        token: Option<String>,
        character: Option<String>,
    },
}

pub enum RoomCmd {
    Connect {
        join: Join,
        tx: ConnTx,
        reply: oneshot::Sender<Result<u64, String>>,
    },
    Disconnect {
        conn_id: u64,
    },
    Msg {
        conn_id: u64,
        msg: ClientMsg,
    },
    Info {
        reply: oneshot::Sender<RoomInfo>,
    },
    Admin {
        cmd: AdminCmd,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Snapshot {
        reply: oneshot::Sender<AdminRoomView>,
    },
}

#[derive(Debug, serde::Serialize)]
pub struct RoomInfo {
    pub code: String,
    pub phase: &'static str,
    pub players: usize,
    pub max_players: usize,
    pub joinable: bool,
    /// Characters already picked, so the join screen can grey them out.
    pub taken_characters: Vec<String>,
}

#[derive(Clone)]
pub struct RoomHandle {
    tx: mpsc::UnboundedSender<RoomCmd>,
}

impl RoomHandle {
    pub async fn connect(&self, join: Join, tx: ConnTx) -> Result<u64, String> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(RoomCmd::Connect { join, tx, reply })
            .map_err(|_| "That room has closed.".to_string())?;
        rx.await
            .unwrap_or_else(|_| Err("That room has closed.".into()))
    }

    pub fn send(&self, cmd: RoomCmd) {
        let _ = self.tx.send(cmd);
    }

    pub async fn info(&self) -> Option<RoomInfo> {
        let (reply, rx) = oneshot::channel();
        self.tx.send(RoomCmd::Info { reply }).ok()?;
        rx.await.ok()
    }

    pub async fn admin(&self, cmd: AdminCmd) -> Result<(), String> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(RoomCmd::Admin { cmd, reply })
            .map_err(|_| "That room has closed.".to_string())?;
        rx.await
            .unwrap_or_else(|_| Err("That room has closed.".into()))
    }

    pub async fn snapshot(&self) -> Option<AdminRoomView> {
        let (reply, rx) = oneshot::channel();
        self.tx.send(RoomCmd::Snapshot { reply }).ok()?;
        rx.await.ok()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Lobby,
    Intro,
    /// A trivia round's title card: its number, theme, difficulty and points.
    RoundIntro,
    Question,
    Reveal,
    Scoreboard,
    /// After a trivia round: scores and a look back at its questions.
    Recap,
    Final,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Phase::Lobby => "lobby",
            Phase::Intro => "intro",
            Phase::RoundIntro => "round_intro",
            Phase::Question => "question",
            Phase::Reveal => "reveal",
            Phase::Scoreboard => "scoreboard",
            Phase::Recap => "recap",
            Phase::Final => "final",
        }
    }
}

struct Player {
    id: u32,
    name: String,
    token: String,
    score: i64,
    color: &'static str,
    /// Correct answers in a row.
    streak: u32,
    character: String,
    /// Number of open sockets for this player (a phone may briefly have two).
    conns: usize,
    team: Option<u8>,
    /// Joined after the game started; plays from the next question.
    waiting: bool,
}

#[derive(Clone, Copy)]
enum Role {
    Host,
    Player(u32),
}

struct Conn {
    tx: ConnTx,
    role: Role,
}

struct HostLine {
    id: u64,
    text: String,
    callouts: Vec<Callout>,
}

struct Answer {
    choice: usize,
    after: Duration,
}

/// Speed hangman bookkeeping for the current round; the puzzle itself is in `Room::puzzles`.
#[derive(Default)]
struct HangmanRound {
    feed: Vec<GuessView>,
    last_guess: HashMap<u32, Instant>,
    solve_locked: HashSet<u32>,
    outcome: Option<Outcome>,
    solved_by: Option<u32>,
}

impl HangmanRound {
    fn push_feed(&mut self, item: GuessView) {
        self.feed.push(item);
        if self.feed.len() > FEED_LEN {
            self.feed.remove(0);
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Outcome {
    Solved,
    Hanged,
    Time,
}

impl Outcome {
    fn name(self) -> &'static str {
        match self {
            Outcome::Solved => "solved",
            Outcome::Hanged => "hanged",
            Outcome::Time => "time",
        }
    }
}

/// One round of a trivia game, covering `len` questions from `start` in `Room::questions`.
struct Stage {
    difficulty: Difficulty,
    category: String,
    points_pct: u32,
    final_round: bool,
    start: usize,
    len: usize,
}

pub struct Room {
    code: String,
    host_token: String,
    settings: Settings,
    timing: Timing,
    max_players: usize,
    db: Db,
    itunes: Itunes,
    host: Host,
    host_line: Option<HostLine>,
    host_seq: u64,

    players: Vec<Player>,
    next_player_id: u32,
    conns: HashMap<u64, Conn>,
    next_conn_id: u64,

    phase: Phase,
    deadline: Option<Instant>,
    phase_len: Option<Duration>,
    questions: Vec<Question>,
    used_question_ids: Vec<i64>,
    puzzles: Vec<Puzzle>,
    used_phrase_ids: Vec<i64>,
    used_song_ids: Vec<i64>,
    hm: HangmanRound,
    /// Trivia only: the game's rounds. Empty in other modes.
    stages: Vec<Stage>,
    /// Trivia only: who got each question right, by question index.
    right_answers: Vec<Vec<u32>>,
    /// Trivia only: points each player has won in the current round.
    stage_points: HashMap<u32, i64>,
    /// Index of the current question, puzzle or song (one "round" in hangman and songs).
    round: usize,
    question_started: Instant,
    question_len: Duration,
    /// Who was alone in first at the last scoreboard, to call out a new leader.
    leader: Option<u32>,
    answers: HashMap<u32, Answer>,
    last_points: HashMap<u32, i64>,
    game_id: Option<i64>,
    empty_since: Option<Instant>,
    created_at: u64,
    /// Set by the admin panel to shut the room down.
    closing: bool,
}

impl Room {
    /// Spawns the room task. `on_close` runs once the room shuts down.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        code: String,
        host_token: String,
        settings: Settings,
        timing: Timing,
        max_players: usize,
        db: Db,
        script: Arc<Script>,
        itunes: Itunes,
        on_close: impl FnOnce() + Send + 'static,
    ) -> RoomHandle {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut room = Room {
            code,
            host_token,
            settings,
            timing,
            max_players,
            db,
            itunes,
            host: Host::new(script),
            host_line: None,
            host_seq: 0,
            players: Vec::new(),
            next_player_id: 1,
            conns: HashMap::new(),
            next_conn_id: 1,
            phase: Phase::Lobby,
            deadline: None,
            phase_len: None,
            questions: Vec::new(),
            used_question_ids: Vec::new(),
            puzzles: Vec::new(),
            used_phrase_ids: Vec::new(),
            used_song_ids: Vec::new(),
            hm: HangmanRound::default(),
            stages: Vec::new(),
            right_answers: Vec::new(),
            stage_points: HashMap::new(),
            round: 0,
            question_started: Instant::now(),
            question_len: Duration::ZERO,
            leader: None,
            answers: HashMap::new(),
            last_points: HashMap::new(),
            game_id: None,
            empty_since: Some(Instant::now()),
            created_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
            closing: false,
        };
        let spelled = room
            .code
            .chars()
            .map(String::from)
            .collect::<Vec<_>>()
            .join(", ");
        let line = room.line("room_open", &[("code", &spelled)]);
        room.speak(vec![line], Vec::new());
        tokio::spawn(async move {
            room.run(rx).await;
            on_close();
        });
        RoomHandle { tx }
    }

    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<RoomCmd>) {
        let mut idle_check = tokio::time::interval(Duration::from_secs(5));
        loop {
            let deadline = self.deadline;
            tokio::select! {
                cmd = rx.recv() => match cmd {
                    Some(cmd) => {
                        self.handle(cmd).await;
                        if self.closing {
                            tracing::info!(code = %self.code, "room closed by admin");
                            break;
                        }
                    }
                    None => break,
                },
                _ = async { sleep_until(deadline.unwrap()).await }, if deadline.is_some() => {
                    self.advance().await;
                }
                _ = idle_check.tick() => {
                    if self.empty_since.is_some_and(|t| t.elapsed() >= self.timing.idle_timeout) {
                        tracing::info!(code = %self.code, "closing idle room");
                        break;
                    }
                }
            }
        }
    }

    async fn handle(&mut self, cmd: RoomCmd) {
        match cmd {
            RoomCmd::Connect { join, tx, reply } => {
                let result = self.connect(join, tx);
                let ok = result.is_ok();
                let _ = reply.send(result);
                if ok {
                    self.broadcast();
                }
            }
            RoomCmd::Disconnect { conn_id } => {
                if let Some(conn) = self.conns.remove(&conn_id) {
                    if let Role::Player(pid) = conn.role {
                        if let Some(p) = self.player_mut(pid) {
                            p.conns = p.conns.saturating_sub(1);
                        }
                    }
                    if self.conns.is_empty() {
                        self.empty_since = Some(Instant::now());
                    }
                    // Someone leaving can mean everyone still here has answered.
                    if self.phase == Phase::Question && self.all_answered() {
                        self.advance().await;
                    } else {
                        self.broadcast();
                    }
                }
            }
            RoomCmd::Msg { conn_id, msg } => {
                let Some(role) = self.conns.get(&conn_id).map(|c| c.role) else {
                    return;
                };
                if let Err(e) = self.on_msg(conn_id, role, msg).await {
                    self.send_to(
                        conn_id,
                        &ServerMsg::Error {
                            message: &e,
                            fatal: false,
                        },
                    );
                }
            }
            RoomCmd::Info { reply } => {
                let _ = reply.send(RoomInfo {
                    code: self.code.clone(),
                    phase: self.phase.name(),
                    players: self.players.len(),
                    max_players: self.max_players,
                    joinable: (self.phase == Phase::Lobby || self.settings.late_join)
                        && self.players.len() < self.max_players,
                    taken_characters: self.players.iter().map(|p| p.character.clone()).collect(),
                });
            }
            RoomCmd::Admin { cmd, reply } => {
                let _ = reply.send(self.on_admin(cmd).await);
            }
            RoomCmd::Snapshot { reply } => {
                let _ = reply.send(self.snapshot());
            }
        }
    }

    fn connect(&mut self, join: Join, tx: ConnTx) -> Result<u64, String> {
        let (role, token) = match join {
            Join::Host { token } => {
                if token != self.host_token {
                    return Err("That host link is not valid for this room.".into());
                }
                (Role::Host, token)
            }
            Join::Player {
                name,
                token,
                character,
            } => {
                // Rejoining with a token (page refresh, phone went to sleep).
                if let Some(p) = token
                    .as_deref()
                    .and_then(|t| self.players.iter_mut().find(|p| p.token == t))
                {
                    p.conns += 1;
                    (Role::Player(p.id), p.token.clone())
                } else {
                    let name = clean_name(name.as_deref().unwrap_or(""))?;
                    let late = self.phase != Phase::Lobby;
                    if late && !self.settings.late_join {
                        return Err("This game has already started. Catch the next one!".into());
                    }
                    if self.players.len() >= self.max_players {
                        return Err(format!("This room is full ({} players).", self.max_players));
                    }
                    if self
                        .players
                        .iter()
                        .any(|p| p.name.eq_ignore_ascii_case(&name))
                    {
                        return Err(format!(
                            "Someone is already called {name}. Pick another name."
                        ));
                    }
                    let character = match character.filter(|c| !c.is_empty()) {
                        Some(c) => self.check_character(&c, None)?,
                        None => self.free_character(),
                    };
                    let id = self.next_player_id;
                    self.next_player_id += 1;
                    let color = self.free_color();
                    let team = self.smallest_team();
                    let token = uuid::Uuid::new_v4().to_string();
                    if late {
                        tracing::info!(code = %self.code, %name, "player joining mid-game");
                    }
                    let line = self.line("join", &[("name", &name)]);
                    self.speak(vec![line], Vec::new());
                    self.players.push(Player {
                        id,
                        name,
                        token: token.clone(),
                        score: 0,
                        color,
                        streak: 0,
                        character,
                        conns: 1,
                        team,
                        waiting: late,
                    });
                    (Role::Player(id), token)
                }
            }
        };

        let conn_id = self.next_conn_id;
        self.next_conn_id += 1;
        let welcome = ServerMsg::Welcome {
            role: if matches!(role, Role::Host) {
                "host"
            } else {
                "player"
            },
            code: &self.code,
            player_id: match role {
                Role::Player(id) => Some(id),
                Role::Host => None,
            },
            token: &token,
        };
        let _ = tx.send(serde_json::to_string(&welcome).unwrap());
        self.conns.insert(conn_id, Conn { tx, role });
        self.empty_since = None;
        Ok(conn_id)
    }

    /// Validates a requested character; `me` is the player changing theirs.
    fn check_character(&self, id: &str, me: Option<u32>) -> Result<String, String> {
        if !characters::exists(id) {
            return Err("That character doesn't exist.".into());
        }
        if self
            .players
            .iter()
            .any(|p| p.character == id && Some(p.id) != me)
        {
            return Err("Someone already picked that character.".into());
        }
        Ok(id.to_string())
    }

    /// A random character nobody has yet (or any, if a huge room used them all).
    fn free_character(&self) -> String {
        use rand::seq::IndexedRandom;
        let free: Vec<_> = CHARACTERS
            .iter()
            .filter(|c| !self.players.iter().any(|p| p.character == c.id))
            .collect();
        let pool: Vec<_> = if free.is_empty() {
            CHARACTERS.iter().collect()
        } else {
            free
        };
        pool.choose(&mut rand::rng()).unwrap().id.to_string()
    }

    /// The team with the fewest players (lowest number on a tie), if teams are on.
    fn smallest_team(&self) -> Option<u8> {
        (0..self.settings.teams)
            .min_by_key(|t| self.players.iter().filter(|p| p.team == Some(*t)).count())
    }

    /// Deals everyone onto teams in join order, e.g. after the team count changes.
    fn deal_teams(&mut self) {
        let teams = self.settings.teams;
        for (i, p) in self.players.iter_mut().enumerate() {
            p.team = (teams > 0).then(|| (i % teams as usize) as u8);
        }
    }

    fn free_color(&self) -> &'static str {
        COLORS
            .iter()
            .copied()
            .find(|c| !self.players.iter().any(|p| p.color == *c))
            .unwrap_or(COLORS[self.players.len() % COLORS.len()])
    }

    async fn on_msg(&mut self, _conn_id: u64, role: Role, msg: ClientMsg) -> Result<(), String> {
        let is_host = matches!(role, Role::Host);
        let is_leader = is_host || matches!(role, Role::Player(id) if Some(id) == self.vip());
        match msg {
            ClientMsg::Ping => {}
            ClientMsg::Start => {
                if !is_leader {
                    return Err("Only the host or the VIP can start the game.".into());
                }
                self.start_game().await?;
            }
            ClientMsg::Answer { choice } => {
                let Role::Player(pid) = role else {
                    return Err("The TV can't answer, nice try.".into());
                };
                if self.player_mut(pid).is_some_and(|p| p.waiting) {
                    return Err("You're in from the next question. Hang tight!".into());
                }
                if self.phase != Phase::Question || self.settings.mode == GameMode::Hangman {
                    return Err("Not taking answers right now.".into());
                }
                let n = self.questions[self.round].choices.len();
                if choice >= n {
                    return Err("That's not one of the choices.".into());
                }
                if self.answers.contains_key(&pid) {
                    return Err("You already locked in an answer.".into());
                }
                self.answers.insert(
                    pid,
                    Answer {
                        choice,
                        after: self.question_started.elapsed(),
                    },
                );
                if self.all_answered() {
                    self.advance().await;
                } else {
                    self.broadcast();
                }
            }
            ClientMsg::Guess { letter } => {
                let pid = self.hangman_guesser(role)?;
                if self
                    .hm
                    .last_guess
                    .get(&pid)
                    .is_some_and(|t| t.elapsed() < GUESS_COOLDOWN)
                {
                    return Err("Easy, speedy! One letter a second.".into());
                }
                let letter = letter.to_ascii_uppercase();
                let result = self.puzzles[self.round].guess(letter);
                let (points, count) = match result {
                    Guess::NotALetter => return Err("Letters only.".into()),
                    Guess::AlreadyGuessed => return Err(format!("{letter} is already taken.")),
                    Guess::Hit(n) => (LETTER_POINTS * n as i64, n),
                    Guess::Miss => (-MISS_PENALTY, 0),
                };
                self.hm.last_guess.insert(pid, Instant::now());
                self.award(pid, points);
                self.hm.push_feed(GuessView {
                    player_id: pid,
                    guess: letter.to_string(),
                    solve: false,
                    hit: count > 0,
                    count,
                });
                let puzzle = &self.puzzles[self.round];
                if puzzle.is_complete() {
                    // Finding the last letter counts as solving it.
                    self.solved(pid, 0);
                    self.advance().await;
                } else if puzzle.is_hanged() {
                    self.hm.outcome = Some(Outcome::Hanged);
                    self.advance().await;
                } else {
                    self.broadcast();
                }
            }
            ClientMsg::Solve { text } => {
                let pid = self.hangman_guesser(role)?;
                if self.hm.solve_locked.contains(&pid) {
                    return Err("You already blew your solve this round.".into());
                }
                let text: String = text.trim().chars().take(MAX_SOLVE_CHARS).collect();
                if text.is_empty() {
                    return Err("Type the answer first.".into());
                }
                let puzzle = &self.puzzles[self.round];
                let blanks = puzzle.hidden();
                if puzzle.solves(&text) {
                    let answer = puzzle.answer.clone();
                    self.hm.push_feed(GuessView {
                        player_id: pid,
                        guess: answer,
                        solve: true,
                        hit: true,
                        count: blanks,
                    });
                    self.solved(pid, blanks);
                    self.advance().await;
                } else {
                    self.hm.solve_locked.insert(pid);
                    self.puzzles[self.round].strike();
                    self.award(pid, -MISS_PENALTY);
                    self.hm.push_feed(GuessView {
                        player_id: pid,
                        guess: text,
                        solve: true,
                        hit: false,
                        count: 0,
                    });
                    if self.puzzles[self.round].is_hanged() {
                        self.hm.outcome = Some(Outcome::Hanged);
                        self.advance().await;
                    } else {
                        self.broadcast();
                    }
                }
            }
            ClientMsg::Next => {
                if !is_leader {
                    return Err("Only the host or the VIP can skip ahead.".into());
                }
                self.skip().await;
            }
            ClientMsg::PlayAgain => {
                if !is_leader {
                    return Err("Only the host or the VIP can restart.".into());
                }
                if self.phase != Phase::Final {
                    return Err("The game isn't over yet.".into());
                }
                let line = self.line("rematch", &[]);
                self.speak(vec![line], Vec::new());
                self.back_to_lobby();
            }
            ClientMsg::Settings {
                rounds,
                max_rating,
                mode,
                clip_ms,
                teams,
                late_join,
                trivia_rounds,
                per_round,
            } => {
                if !is_host {
                    return Err("Only the host can change settings.".into());
                }
                self.change_settings(SettingsChange {
                    rounds,
                    max_rating,
                    mode,
                    clip_ms,
                    teams,
                    late_join,
                    trivia_rounds,
                    per_round,
                })?;
            }
            ClientMsg::Team { team } => {
                let Role::Player(pid) = role else {
                    return Err("The TV isn't on a team.".into());
                };
                if self.phase != Phase::Lobby {
                    return Err("Teams are locked once the game starts.".into());
                }
                if team >= self.settings.teams {
                    return Err("That team doesn't exist.".into());
                }
                if let Some(p) = self.player_mut(pid) {
                    p.team = Some(team);
                }
                self.broadcast();
            }
            ClientMsg::Profile { name, character } => {
                let Role::Player(pid) = role else {
                    return Err("The TV doesn't get a character.".into());
                };
                if self.phase != Phase::Lobby {
                    return Err("You can change your look between games.".into());
                }
                let name = match name {
                    Some(n) => {
                        let n = clean_name(&n)?;
                        if self
                            .players
                            .iter()
                            .any(|p| p.id != pid && p.name.eq_ignore_ascii_case(&n))
                        {
                            return Err(format!(
                                "Someone is already called {n}. Pick another name."
                            ));
                        }
                        Some(n)
                    }
                    None => None,
                };
                let character = character
                    .map(|c| self.check_character(&c, Some(pid)))
                    .transpose()?;
                if let Some(p) = self.player_mut(pid) {
                    if let Some(n) = name {
                        p.name = n;
                    }
                    if let Some(c) = character {
                        p.character = c;
                    }
                }
                self.broadcast();
            }
            ClientMsg::Kick { player_id } => {
                if !is_host {
                    return Err("Only the host can kick players.".into());
                }
                self.kick(player_id).await;
            }
        }
        Ok(())
    }

    async fn on_admin(&mut self, cmd: AdminCmd) -> Result<(), String> {
        match cmd {
            AdminCmd::Start => self.start_game().await?,
            AdminCmd::Skip => {
                if !self.skip().await {
                    return Err("Nothing to skip right now.".into());
                }
            }
            AdminCmd::EndGame => {
                if !self.running() {
                    return Err("There's no game running.".into());
                }
                if self.phase == Phase::Question && self.settings.mode != GameMode::Hangman {
                    self.score_question();
                }
                self.finale();
                self.set_phase(Phase::Final, None);
                self.record_results().await;
                self.broadcast();
            }
            AdminCmd::Lobby => {
                if self.phase == Phase::Lobby {
                    return Err("Already in the lobby.".into());
                }
                // Abandoned games are not recorded.
                self.game_id = None;
                self.back_to_lobby();
            }
            AdminCmd::Close => {
                for conn_id in self.conns.keys() {
                    self.send_to(
                        *conn_id,
                        &ServerMsg::Error {
                            message: "The organizer closed this room.",
                            fatal: true,
                        },
                    );
                }
                self.conns.clear();
                self.closing = true;
            }
            AdminCmd::Kick { player_id } => {
                if !self.players.iter().any(|p| p.id == player_id) {
                    return Err("No such player.".into());
                }
                self.kick(player_id).await;
            }
            AdminCmd::SetTeam { player_id, team } => {
                if team.is_some_and(|t| t >= self.settings.teams) {
                    return Err("That team doesn't exist.".into());
                }
                let p = self.player_mut(player_id).ok_or("No such player.")?;
                p.team = team;
                self.broadcast();
            }
            AdminCmd::Settings {
                rounds,
                max_rating,
                mode,
                clip_ms,
                teams,
                late_join,
                trivia_rounds,
                per_round,
            } => {
                self.change_settings(SettingsChange {
                    rounds,
                    max_rating,
                    mode,
                    clip_ms,
                    teams,
                    late_join,
                    trivia_rounds,
                    per_round,
                })?;
            }
        }
        Ok(())
    }

    /// Moves on from the intro, question, reveal or scoreboard. False if there was nothing to skip.
    async fn skip(&mut self) -> bool {
        let running = self.running();
        if running {
            self.advance().await;
        }
        running
    }

    fn back_to_lobby(&mut self) {
        for p in &mut self.players {
            p.score = 0;
            p.waiting = false;
        }
        self.answers.clear();
        self.last_points.clear();
        self.set_phase(Phase::Lobby, None);
        self.broadcast();
    }

    /// Rounds, rating and teams are locked once a game starts; late joins can be switched any time.
    fn change_settings(&mut self, change: SettingsChange) -> Result<(), String> {
        let SettingsChange {
            rounds,
            max_rating,
            mode,
            clip_ms,
            teams,
            late_join,
            trivia_rounds,
            per_round,
        } = change;
        let locked = rounds.is_some()
            || max_rating.is_some()
            || mode.is_some()
            || clip_ms.is_some()
            || teams.is_some()
            || trivia_rounds.is_some()
            || per_round.is_some();
        if locked && self.phase != Phase::Lobby {
            return Err("Settings are locked once the game starts.".into());
        }
        if let Some(r) = rounds {
            self.settings.rounds = r.clamp(MIN_ROUNDS, MAX_ROUNDS);
        }
        if let Some(r) = max_rating {
            self.settings.max_rating = r;
        }
        if let Some(m) = mode {
            self.settings.mode = m;
        }
        if let Some(ms) = clip_ms {
            self.settings.clip_ms = ms.clamp(MIN_CLIP_MS, MAX_CLIP_MS);
        }
        if let Some(t) = teams.map(clamp_teams)
            && t != self.settings.teams
        {
            self.settings.teams = t;
            self.deal_teams();
        }
        if let Some(l) = late_join {
            self.settings.late_join = l;
        }
        if let Some(r) = trivia_rounds {
            self.settings.trivia_rounds = r.clamp(rounds::MIN_ROUNDS, rounds::MAX_ROUNDS);
        }
        if let Some(n) = per_round {
            self.settings.per_round = n.clamp(rounds::MIN_PER_ROUND, rounds::MAX_PER_ROUND);
        }
        self.broadcast();
        Ok(())
    }

    async fn kick(&mut self, player_id: u32) {
        let Some(idx) = self.players.iter().position(|p| p.id == player_id) else {
            return;
        };
        self.players.remove(idx);
        self.answers.remove(&player_id);
        let kicked: Vec<u64> = self
            .conns
            .iter()
            .filter(|(_, c)| matches!(c.role, Role::Player(id) if id == player_id))
            .map(|(id, _)| *id)
            .collect();
        for conn_id in kicked {
            self.send_to(
                conn_id,
                &ServerMsg::Error {
                    message: "The host removed you from the game.",
                    fatal: true,
                },
            );
            // Dropping the sender ends that socket's write loop, which closes it.
            self.conns.remove(&conn_id);
        }
        if self.phase == Phase::Question && self.all_answered() {
            self.advance().await;
        } else {
            self.broadcast();
        }
    }

    async fn start_game(&mut self) -> Result<(), String> {
        if self.phase != Phase::Lobby {
            return Err("The game is already running.".into());
        }
        if self.players.is_empty() {
            return Err("Need at least one player to start.".into());
        }
        match self.settings.mode {
            GameMode::Trivia => {
                self.load_trivia().await?;
                self.puzzles.clear();
            }
            GameMode::Songs => {
                self.load_songs().await?;
                self.puzzles.clear();
                self.stages.clear();
            }
            GameMode::Hangman => {
                self.load_puzzles().await?;
                self.questions.clear();
                self.stages.clear();
            }
        }
        self.right_answers.clear();
        self.stage_points.clear();
        for p in &mut self.players {
            p.score = 0;
            p.streak = 0;
        }
        self.leader = None;
        self.game_id = match self
            .db
            .start_game(&self.code, self.game_len(), self.players.len())
            .await
        {
            Ok(id) => Some(id),
            Err(e) => {
                tracing::warn!("recording game start: {e:#}");
                None
            }
        };
        self.round = 0;
        self.answers.clear();
        self.last_points.clear();
        let names: Vec<&str> = self.players.iter().map(|p| p.name.as_str()).collect();
        let (count, rounds, names) = (
            names.len().to_string(),
            self.game_len().to_string(),
            host::name_list(&names),
        );
        let line = self.line(
            "intro",
            &[("count", &count), ("rounds", &rounds), ("names", &names)],
        );
        self.speak(vec![line], Vec::new());
        self.set_phase(Phase::Intro, Some(self.timing.intro.max(self.talk())));
        self.broadcast();
        Ok(())
    }

    /// Plans the game's rounds, easiest first, and loads their questions.
    async fn load_trivia(&mut self) -> Result<(), String> {
        let load_error = |e: anyhow::Error| {
            tracing::error!("loading questions: {e:#}");
            "Couldn't load questions. Try again?".to_string()
        };
        if self.used_question_ids.len() > 200 {
            self.used_question_ids.clear();
        }
        let pool = self
            .db
            .question_pool(self.settings.max_rating)
            .await
            .map_err(load_error)?;
        let played: HashSet<i64> = self.used_question_ids.iter().copied().collect();
        let (plan, reset) = rounds::plan(
            &pool,
            self.settings.trivia_rounds,
            self.settings.per_round,
            &played,
            &mut rand::rng(),
        );
        if reset {
            // Ran out of unseen questions; repeats beat a short game.
            self.used_question_ids.clear();
        }
        let ids: Vec<i64> = plan.iter().flat_map(|r| r.ids.iter().copied()).collect();
        let questions = self
            .db
            .questions_by_ids(&ids, self.settings.max_rating)
            .await
            .map_err(load_error)?;
        if questions.is_empty() {
            return Err("The question bank is empty.".into());
        }
        // Lay the rounds over the loaded questions; a question deleted meanwhile just drops out.
        self.stages.clear();
        let mut start = 0;
        for r in plan {
            let len = questions[start..]
                .iter()
                .take_while(|q| r.ids.contains(&q.id))
                .count();
            if len == 0 {
                continue;
            }
            self.stages.push(Stage {
                difficulty: r.difficulty,
                category: r.category,
                points_pct: r.points_pct,
                final_round: r.final_round,
                start,
                len,
            });
            start += len;
        }
        self.used_question_ids
            .extend(questions.iter().map(|q| q.id));
        self.questions = questions;
        Ok(())
    }

    async fn load_songs(&mut self) -> Result<(), String> {
        let questions = music::build_round(
            &self.db,
            &self.itunes,
            self.settings.rounds,
            self.settings.max_rating,
            &self.used_song_ids,
        )
        .await
        .map_err(|e| {
            tracing::error!("loading songs: {e:#}");
            "Couldn't reach Apple Music for song clips. Check the server's internet and try again."
                .to_string()
        })?;
        if questions.is_empty() {
            return Err(
                "Couldn't find any playable songs right now. Try again in a minute.".into(),
            );
        }
        // build_round falls back to repeats on its own once the list is used up.
        if questions.iter().any(|q| self.used_song_ids.contains(&q.id)) {
            self.used_song_ids.clear();
        }
        self.used_song_ids.extend(questions.iter().map(|q| q.id));
        self.questions = questions;
        Ok(())
    }

    async fn load_puzzles(&mut self) -> Result<(), String> {
        let (n, rating) = (self.settings.rounds, self.settings.max_rating);
        let load_error = |e: anyhow::Error| {
            tracing::error!("loading hangman phrases: {e:#}");
            "Couldn't load puzzles. Try again?".to_string()
        };
        let mut phrases = self
            .db
            .random_phrases(n, rating, &self.used_phrase_ids)
            .await
            .map_err(load_error)?;
        if phrases.len() < n {
            self.used_phrase_ids.clear();
            phrases = self
                .db
                .random_phrases(n, rating, &[])
                .await
                .map_err(load_error)?;
        }
        if phrases.is_empty() {
            return Err("There are no hangman puzzles at this rating.".into());
        }
        self.used_phrase_ids.extend(phrases.iter().map(|p| p.id));
        self.puzzles = phrases
            .into_iter()
            .map(|p| Puzzle::new(p.id, p.category, p.phrase))
            .collect();
        Ok(())
    }

    fn game_len(&self) -> usize {
        match self.settings.mode {
            GameMode::Trivia | GameMode::Songs => self.questions.len(),
            GameMode::Hangman => self.puzzles.len(),
        }
    }

    /// Checks that `role` is a player who may guess in a live hangman round.
    fn hangman_guesser(&self, role: Role) -> Result<u32, String> {
        let Role::Player(pid) = role else {
            return Err("The TV can't guess, nice try.".into());
        };
        if self.players.iter().any(|p| p.id == pid && p.waiting) {
            return Err("You're in from the next puzzle. Hang tight!".into());
        }
        if self.settings.mode != GameMode::Hangman
            || self.phase != Phase::Question
            || self.hm.outcome.is_some()
        {
            return Err("Not taking guesses right now.".into());
        }
        Ok(pid)
    }

    /// Adds points (or takes them away) mid-round, so the TV can show them live.
    fn award(&mut self, pid: u32, points: i64) {
        if let Some(p) = self.player_mut(pid) {
            p.score += points;
        }
        *self.last_points.entry(pid).or_default() += points;
    }

    /// Solving pays a base, a bonus for every letter still blank, and a bonus for time left.
    fn solved(&mut self, pid: u32, blanks: usize) {
        let left = match (self.deadline, self.phase_len) {
            (Some(d), Some(len)) => {
                d.saturating_duration_since(Instant::now()).as_secs_f64()
                    / len.as_secs_f64().max(0.001)
            }
            _ => 0.0,
        };
        let points = SOLVE_POINTS
            + SOLVE_POINTS_PER_BLANK * blanks as i64
            + (SOLVE_SPEED_POINTS as f64 * left.clamp(0.0, 1.0)).round() as i64;
        self.award(pid, points);
        self.hm.outcome = Some(Outcome::Solved);
        self.hm.solved_by = Some(pid);
    }

    fn begin_question(&mut self) {
        // Anyone who joined since the last question plays from here on.
        for p in &mut self.players {
            p.waiting = false;
        }
        self.answers.clear();
        self.last_points.clear();
        self.hm = HangmanRound::default();
        let total = self.game_len();
        let event = match self.round {
            0 => "question_first",
            r if r + 1 == total => "question_last",
            _ => "question",
        };
        let category = match self.settings.mode {
            GameMode::Trivia | GameMode::Songs => self.questions[self.round].category.clone(),
            GameMode::Hangman => self.puzzles[self.round].category.clone(),
        };
        // In trivia the host counts questions within the round.
        let (round, total) = match self.stage_idx().map(|i| &self.stages[i]) {
            Some(st) => (self.round - st.start + 1, st.len),
            None => (self.round + 1, total),
        };
        let (round, total) = (round.to_string(), total.to_string());
        let line = self.line(
            event,
            &[
                ("round", &round),
                ("total", &total),
                ("category", &category),
            ],
        );
        let mut parts = vec![line];
        if self.settings.mode == GameMode::Trivia {
            let own = &self.questions[self.round].lines.lead_in;
            parts.extend(self.host.say_own(own, self.settings.max_rating));
        }
        self.speak(parts, Vec::new());
        // The clock starts now, but players get extra time while the host does the intro banter.
        let base = match self.settings.mode {
            GameMode::Trivia => self.timing.question,
            GameMode::Songs => self.timing.song_question,
            GameMode::Hangman => self.timing.hangman,
        };
        self.question_len = base + self.talk();
        self.question_started = Instant::now();
        self.set_phase(Phase::Question, Some(self.question_len));
        self.broadcast();
    }

    /// Moves to the next phase, either because a timer ran out or someone skipped.
    async fn advance(&mut self) {
        let pause = Duration::from_millis(1500);
        match self.phase {
            Phase::Intro if !self.stages.is_empty() => {
                self.begin_stage();
                return;
            }
            Phase::Intro | Phase::RoundIntro => {
                self.begin_question();
                return;
            }
            Phase::Question => {
                match self.settings.mode {
                    GameMode::Trivia | GameMode::Songs => self.score_question(),
                    GameMode::Hangman => {
                        self.hm.outcome.get_or_insert(Outcome::Time);
                        self.react_to_puzzle();
                    }
                }
                self.set_phase(
                    Phase::Reveal,
                    Some(self.timing.reveal.max(self.talk() + pause)),
                );
            }
            Phase::Reveal if self.stage_ends_here() => {
                self.recap();
                self.set_phase(
                    Phase::Recap,
                    Some(self.timing.recap.max(self.talk() + pause)),
                );
            }
            Phase::Reveal | Phase::Recap if self.round + 1 >= self.game_len() => {
                self.finale();
                self.set_phase(Phase::Final, None);
                self.record_results().await;
            }
            // Trivia goes straight to the next question; the scores wait for the end of the round.
            Phase::Reveal if !self.stages.is_empty() => {
                self.round += 1;
                self.begin_question();
                return;
            }
            Phase::Reveal => {
                let (parts, callouts) = self.score_update();
                self.speak(parts, callouts);
                self.set_phase(
                    Phase::Scoreboard,
                    Some(self.timing.scoreboard.max(self.talk() + pause)),
                );
            }
            Phase::Scoreboard => {
                self.round += 1;
                self.begin_question();
                return;
            }
            Phase::Recap => {
                self.round += 1;
                self.begin_stage();
                return;
            }
            Phase::Lobby | Phase::Final => self.set_phase(self.phase, None),
        }
        self.broadcast();
    }

    /// Correct answers earn 500 points plus up to 500 more for speed, scaled up in harder trivia rounds. Then the host reacts: the answer,
    /// who got it, and one extra beat (a streak, a broken streak, or a roast of a wrong answer).
    fn score_question(&mut self) {
        let scale = self.stage_idx().map_or(100, |i| self.stages[i].points_pct) as f64 / 100.0;
        let q = &self.questions[self.round];
        let limit = self.question_len.as_secs_f64().max(0.001);
        let (mut right, mut wrong, mut silent, mut streaks, mut broken) =
            (vec![], vec![], vec![], vec![], vec![]);
        for p in self.players.iter_mut().filter(|p| !p.waiting) {
            let answer = self.answers.get(&p.id);
            let points = match answer {
                Some(a) if a.choice == q.answer => {
                    let speed = (1.0 - a.after.as_secs_f64() / limit).clamp(0.0, 1.0);
                    ((500.0 + 500.0 * speed) * scale).round() as i64
                }
                _ => 0,
            };
            p.score += points;
            self.last_points.insert(p.id, points);
            *self.stage_points.entry(p.id).or_default() += points;
            if points > 0 {
                p.streak += 1;
                right.push((p.id, points));
                if p.streak >= 3 {
                    streaks.push((p.id, p.streak));
                }
            } else {
                if p.streak >= 3 {
                    broken.push((p.id, p.streak));
                }
                p.streak = 0;
                match answer {
                    Some(a) => wrong.push((p.id, q.choices[a.choice].clone())),
                    None if p.conns > 0 => silent.push(p.id),
                    None => {}
                }
            }
        }

        if !self.stages.is_empty() {
            self.right_answers.resize(self.questions.len(), Vec::new());
            self.right_answers[self.round] = right.iter().map(|r| r.0).collect();
        }
        let answer = q.choices[q.answer].clone();
        let n = self.players.len();
        let mut parts = vec![self.line("reveal_answer", &[("answer", &answer)])];
        // The question's own joke or fun fact comes right after the answer. It takes the place of
        // the generic roast, so the host doesn't ramble.
        let own = self.host.say_own(
            &self.questions[self.round].lines.reveal,
            self.settings.max_rating,
        );
        let has_own = own.is_some();
        parts.extend(own);
        let mut callouts = Vec::new();
        let mut call = |kind, player_id, label: &str| {
            callouts.push(Callout {
                kind,
                player_id,
                label: label.into(),
            })
        };

        if right.is_empty() {
            parts.push(self.line("nobody", &[("answer", &answer)]));
            call("nobody", None, "NOBODY!");
        } else if right.len() == n && n > 1 {
            parts.push(self.line("everyone", &[]));
            call("everyone", None, "CLEAN SWEEP");
        } else if right.len() == 1 && n > 1 {
            let id = right[0].0;
            parts.push(self.line("only_one", &[("name", &self.name_of(id))]));
            call("only_one", Some(id), "LONE GENIUS");
        } else {
            let id = right.iter().max_by_key(|r| r.1).unwrap().0;
            parts.push(self.line("fastest", &[("name", &self.name_of(id))]));
            call("fastest", Some(id), "FASTEST FINGER");
        }

        if let Some(&(id, streak)) = streaks.iter().max_by_key(|s| s.1) {
            let streak = streak.to_string();
            parts.push(self.line(
                "streak",
                &[("name", &self.name_of(id)), ("streak", &streak)],
            ));
            call("streak", Some(id), &format!("{streak} IN A ROW"));
        } else if let Some(&(id, streak)) = broken.iter().max_by_key(|s| s.1) {
            let streak = streak.to_string();
            parts.push(self.line(
                "streak_broken",
                &[("name", &self.name_of(id)), ("streak", &streak)],
            ));
            call("streak_broken", Some(id), "STREAK OVER");
        } else if has_own {
        } else if !wrong.is_empty() {
            let (id, choice) = &wrong[rand::random_range(0..wrong.len())];
            parts.push(self.line("roast", &[("name", &self.name_of(*id)), ("wrong", choice)]));
            call("roast", Some(*id), "ROASTED");
        } else if !right.is_empty() && !silent.is_empty() {
            let id = silent[rand::random_range(0..silent.len())];
            parts.push(self.line("asleep", &[("name", &self.name_of(id))]));
            call("asleep", Some(id), "ASLEEP?");
        }
        self.speak(parts, callouts);
    }

    /// The host's reaction to a finished hangman puzzle: the phrase, then who cracked it or that nobody did.
    fn react_to_puzzle(&mut self) {
        let answer = self.puzzles[self.round].answer.clone();
        let mut parts = vec![self.line("reveal_answer", &[("answer", &answer)])];
        let mut callouts = Vec::new();
        match self.hm.solved_by {
            Some(id) => {
                parts.push(self.line("only_one", &[("name", &self.name_of(id))]));
                callouts.push(Callout {
                    kind: "only_one",
                    player_id: Some(id),
                    label: "CRACKED IT".into(),
                });
            }
            None => {
                parts.push(self.line("nobody", &[("answer", &answer)]));
                callouts.push(Callout {
                    kind: "nobody",
                    player_id: None,
                    label: "NOBODY!".into(),
                });
            }
        }
        self.speak(parts, callouts);
    }

    /// The host's take on the scoreboard: who leads (or just took over), a tie, and sometimes a dig
    /// at last place.
    fn score_update(&mut self) -> (Vec<String>, Vec<Callout>) {
        let ranked: Vec<(usize, u32, String, i64)> = self
            .ranked()
            .into_iter()
            .map(|(r, p)| (r, p.id, p.name.clone(), p.score))
            .collect();
        let mut parts = Vec::new();
        let mut callouts = Vec::new();
        if ranked.len() > 1 {
            let top: Vec<_> = ranked.iter().filter(|r| r.0 == 1).collect();
            if top.len() > 1 {
                let names: Vec<&str> = top.iter().map(|r| r.2.as_str()).collect();
                parts.push(self.line("tie", &[("names", &host::name_list(&names))]));
                callouts.push(Callout {
                    kind: "tie",
                    player_id: None,
                    label: "TIED AT THE TOP".into(),
                });
                self.leader = None;
            } else {
                let (_, id, name, score) = top[0];
                if self.leader.is_some_and(|l| l != *id) {
                    parts.push(self.line("new_leader", &[("name", name)]));
                    callouts.push(Callout {
                        kind: "new_leader",
                        player_id: Some(*id),
                        label: "NEW LEADER".into(),
                    });
                } else {
                    let gap = (score - ranked[1].3).to_string();
                    parts.push(self.line("leader", &[("name", name), ("gap", &gap)]));
                    callouts.push(Callout {
                        kind: "leader",
                        player_id: Some(*id),
                        label: "IN THE LEAD".into(),
                    });
                }
                self.leader = Some(*id);
            }
            let last = ranked.last().unwrap();
            let alone_in_last = ranked.iter().filter(|r| r.0 == last.0).count() == 1;
            if ranked.len() >= 3 && alone_in_last && rand::random_bool(0.5) {
                parts.push(self.line("last_place", &[("name", &last.2)]));
                callouts.push(Callout {
                    kind: "last_place",
                    player_id: Some(last.1),
                    label: "LAST PLACE".into(),
                });
            }
        }
        if self.round + 2 == self.game_len() && self.stages.is_empty() {
            parts.push(self.line("one_left", &[]));
        }
        (parts, callouts)
    }

    /// Opens a trivia round with its title card: number, theme, difficulty and what it's worth.
    fn begin_stage(&mut self) {
        let Some(i) = self.stage_idx() else {
            return self.begin_question();
        };
        self.answers.clear();
        self.last_points.clear();
        self.stage_points.clear();
        let st = &self.stages[i];
        let (final_round, category, difficulty, points) = (
            st.final_round,
            st.category.clone(),
            st.difficulty.name(),
            rounds::points_phrase(st.points_pct),
        );
        let (round, total) = ((i + 1).to_string(), self.stages.len().to_string());
        let line = if final_round {
            self.line(
                "round_final",
                &[("category", &category), ("points", points)],
            )
        } else {
            self.line(
                "round_intro",
                &[
                    ("round", &round),
                    ("total", &total),
                    ("category", &category),
                    ("difficulty", difficulty),
                    ("points", points),
                ],
            )
        };
        self.speak(vec![line], Vec::new());
        let pause = Duration::from_millis(1000);
        self.set_phase(
            Phase::RoundIntro,
            Some(self.timing.round_intro.max(self.talk() + pause)),
        );
        self.broadcast();
    }

    /// The host wraps up a trivia round: who won it, then the standings unless the finale is next.
    fn recap(&mut self) {
        let Some(i) = self.stage_idx() else {
            return;
        };
        let round = (i + 1).to_string();
        let best = self
            .players
            .iter()
            .filter(|p| !p.waiting)
            .map(|p| self.stage_points.get(&p.id).copied().unwrap_or(0))
            .max()
            .unwrap_or(0);
        let mut parts = Vec::new();
        let mut callouts = Vec::new();
        if best <= 0 {
            parts.push(self.line("recap_nobody", &[("round", &round)]));
        } else {
            let winners: Vec<(u32, String)> = self
                .players
                .iter()
                .filter(|p| !p.waiting && self.stage_points.get(&p.id) == Some(&best))
                .map(|p| (p.id, p.name.clone()))
                .collect();
            let names: Vec<&str> = winners.iter().map(|w| w.1.as_str()).collect();
            let (names, points) = (host::name_list(&names), best.to_string());
            parts.push(self.line(
                "recap",
                &[("round", &round), ("name", &names), ("points", &points)],
            ));
            callouts.push(Callout {
                kind: "round_winner",
                player_id: (winners.len() == 1).then(|| winners[0].0),
                label: format!("WON ROUND {round}"),
            });
        }
        if i + 1 < self.stages.len() {
            let (more, more_callouts) = self.score_update();
            parts.extend(more);
            callouts.extend(more_callouts);
        }
        self.speak(parts, callouts);
    }

    fn finale(&mut self) {
        let ranked: Vec<(usize, u32, String, i64)> = self
            .ranked()
            .into_iter()
            .map(|(r, p)| (r, p.id, p.name.clone(), p.score))
            .collect();
        let mut parts = Vec::new();
        let mut callouts = Vec::new();
        let top: Vec<_> = ranked.iter().filter(|r| r.0 == 1).collect();
        if top.len() == 1 {
            let (_, id, name, score) = top[0];
            parts.push(self.line(
                "finale_win",
                &[("name", name), ("score", &score.to_string())],
            ));
            callouts.push(Callout {
                kind: "winner",
                player_id: Some(*id),
                label: "CHAMPION".into(),
            });
        } else if top.len() > 1 {
            let names: Vec<&str> = top.iter().map(|r| r.2.as_str()).collect();
            parts.push(self.line("finale_tie", &[("names", &host::name_list(&names))]));
            callouts.push(Callout {
                kind: "tie",
                player_id: None,
                label: "SHARED CROWN".into(),
            });
        }
        if let Some(last) = ranked.last().filter(|_| ranked.len() >= 3) {
            if ranked.iter().filter(|r| r.0 == last.0).count() == 1 {
                parts.push(self.line("finale_last", &[("name", &last.2)]));
            }
        }
        parts.push(self.line("sign_off", &[]));
        self.speak(parts, callouts);
    }

    fn line(&mut self, event: &'static str, vars: &[(&str, &str)]) -> String {
        self.host.say(event, self.settings.max_rating, vars)
    }

    /// Sets what the host is saying. An empty line clears it.
    fn speak(&mut self, parts: Vec<String>, callouts: Vec<Callout>) {
        self.host_seq += 1;
        self.host_line = (!parts.is_empty()).then(|| HostLine {
            id: self.host_seq,
            text: parts.join(" "),
            callouts,
        });
    }

    /// How long the host's current line takes to say.
    fn talk(&self) -> Duration {
        self.host_line.as_ref().map_or(Duration::ZERO, |l| {
            host::talk_time(&l.text, self.timing.talk_cps)
        })
    }

    fn name_of(&self, id: u32) -> String {
        self.players
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.name.clone())
            .unwrap_or_default()
    }

    async fn record_results(&mut self) {
        let Some(game_id) = self.game_id.take() else {
            return;
        };
        let standings: Vec<Standing> = self
            .ranked()
            .into_iter()
            .map(|(rank, p)| Standing {
                name: p.name.clone(),
                score: p.score,
                rank,
            })
            .collect();
        if let Err(e) = self.db.finish_game(game_id, &standings).await {
            tracing::warn!("recording game results: {e:#}");
        }
    }

    fn set_phase(&mut self, phase: Phase, len: Option<Duration>) {
        self.phase = phase;
        self.phase_len = len;
        self.deadline = len.map(|d| Instant::now() + d);
    }

    fn all_answered(&self) -> bool {
        if self.settings.mode == GameMode::Hangman {
            return false;
        }
        let mut active = self
            .players
            .iter()
            .filter(|p| p.conns > 0 && !p.waiting)
            .peekable();
        active.peek().is_some() && active.all(|p| self.answers.contains_key(&p.id))
    }

    /// The VIP is the earliest-joined player who is still connected and playing.
    fn vip(&self) -> Option<u32> {
        self.players
            .iter()
            .find(|p| p.conns > 0 && !p.waiting)
            .map(|p| p.id)
    }

    fn player_mut(&mut self, id: u32) -> Option<&mut Player> {
        self.players.iter_mut().find(|p| p.id == id)
    }

    /// Playing players by score, with ties sharing a rank (1, 2, 2, 4).
    fn ranked(&self) -> Vec<(usize, &Player)> {
        let mut sorted: Vec<&Player> = self.players.iter().filter(|p| !p.waiting).collect();
        sorted.sort_by(|a, b| b.score.cmp(&a.score));
        let mut out = Vec::with_capacity(sorted.len());
        for (i, p) in sorted.iter().enumerate() {
            let rank = if i > 0 && sorted[i - 1].score == p.score {
                out.last().map(|(r, _)| *r).unwrap()
            } else {
                i + 1
            };
            out.push((rank, *p));
        }
        out
    }

    /// Team standings, best first. A team's score is the average of its playing members.
    fn team_views(&self) -> Vec<TeamView> {
        let mut teams: Vec<TeamView> = (0..self.settings.teams)
            .map(|t| {
                let members: Vec<&Player> =
                    self.players.iter().filter(|p| p.team == Some(t)).collect();
                let playing: Vec<i64> = members
                    .iter()
                    .filter(|p| !p.waiting)
                    .map(|p| p.score)
                    .collect();
                let score = if playing.is_empty() {
                    0
                } else {
                    playing.iter().sum::<i64>() / playing.len() as i64
                };
                let (name, color) = TEAMS[t as usize];
                TeamView {
                    id: t,
                    name,
                    color,
                    score,
                    members: members.iter().map(|p| p.id).collect(),
                }
            })
            .collect();
        teams.sort_by(|a, b| b.score.cmp(&a.score).then(a.id.cmp(&b.id)));
        teams
    }

    /// A game is under way (anything but the lobby and the final scores).
    fn running(&self) -> bool {
        !matches!(self.phase, Phase::Lobby | Phase::Final)
    }

    /// Trivia: the round the current question belongs to.
    fn stage_idx(&self) -> Option<usize> {
        self.stages
            .iter()
            .position(|st| (st.start..st.start + st.len).contains(&self.round))
    }

    /// Trivia: the last question of a round has just been played.
    fn stage_ends_here(&self) -> bool {
        self.stage_idx().is_some_and(|i| {
            let st = &self.stages[i];
            self.round + 1 == st.start + st.len
        })
    }

    /// Trivia: the round being played, from its title card to its recap.
    fn current_stage(&self) -> Option<usize> {
        match self.phase {
            Phase::RoundIntro | Phase::Question | Phase::Reveal | Phase::Recap => self.stage_idx(),
            _ => None,
        }
    }

    /// The round ladder for the TV. Themes of rounds not yet reached stay hidden.
    fn stage_views(&self) -> Vec<StageView> {
        if self.phase == Phase::Lobby {
            return Vec::new();
        }
        let current = self.current_stage();
        self.stages
            .iter()
            .enumerate()
            .map(|(i, st)| {
                let status = if self.phase == Phase::Final || current.is_some_and(|c| i < c) {
                    "done"
                } else if current == Some(i) {
                    "current"
                } else {
                    "next"
                };
                StageView {
                    number: i + 1,
                    title: if st.final_round {
                        "Final Round".into()
                    } else {
                        format!("Round {}", i + 1)
                    },
                    category: (status != "next").then(|| st.category.clone()),
                    difficulty: st.difficulty,
                    points_pct: st.points_pct,
                    final_round: st.final_round,
                    questions: st.len,
                    status,
                }
            })
            .collect()
    }

    fn recap_view(&self) -> Option<RecapView<'_>> {
        let i = self
            .current_stage()
            .filter(|_| self.phase == Phase::Recap)?;
        let st = &self.stages[i];
        Some(RecapView {
            stage: i + 1,
            questions: (st.start..st.start + st.len)
                .map(|n| {
                    let q = &self.questions[n];
                    RecapQuestionView {
                        prompt: &q.prompt,
                        answer: &q.choices[q.answer],
                        right: self.right_answers.get(n).cloned().unwrap_or_default(),
                    }
                })
                .collect(),
            points: self
                .players
                .iter()
                .filter(|p| !p.waiting)
                .map(|p| ResultView {
                    player_id: p.id,
                    choice: None,
                    points: self.stage_points.get(&p.id).copied().unwrap_or(0),
                })
                .collect(),
        })
    }

    fn in_round(&self) -> bool {
        matches!(
            self.phase,
            Phase::Question | Phase::Reveal | Phase::Scoreboard
        )
    }

    fn round_number(&self) -> usize {
        match self.phase {
            _ if self.in_round() => self.round + 1,
            Phase::Final | Phase::Recap => self.round + 1,
            // The title card comes before the round's first question.
            Phase::RoundIntro => self.round,
            _ => 0,
        }
    }

    fn total_rounds(&self) -> usize {
        if self.phase == Phase::Lobby {
            match self.settings.mode {
                GameMode::Trivia => self.settings.trivia_rounds as usize * self.settings.per_round,
                _ => self.settings.rounds,
            }
        } else {
            self.game_len()
        }
    }

    fn ms_left(&self) -> Option<u64> {
        self.deadline
            .map(|d| d.saturating_duration_since(Instant::now()).as_millis() as u64)
    }

    fn snapshot(&self) -> AdminRoomView {
        let question = self
            .questions
            .get(self.round)
            .filter(|_| self.in_round())
            .map(|q| AdminQuestionView {
                category: q.category.clone(),
                prompt: q.prompt.clone(),
                answer: q.choices[q.answer].clone(),
            });
        let puzzle = self
            .puzzles
            .get(self.round)
            .filter(|_| self.in_round())
            .map(|p| AdminQuestionView {
                category: p.category.clone(),
                prompt: p.mask(),
                answer: p.answer.clone(),
            });
        AdminRoomView {
            code: self.code.clone(),
            host_token: self.host_token.clone(),
            created_at: self.created_at,
            phase: self.phase.name(),
            round: self.round_number(),
            total_rounds: self.total_rounds(),
            stage: self
                .current_stage()
                .and_then(|i| self.stage_views().into_iter().nth(i)),
            ms_left: self.ms_left(),
            settings: self.settings.view(),
            max_players: self.max_players,
            host_connected: self.conns.values().any(|c| matches!(c.role, Role::Host)),
            vip: self.vip(),
            players: self
                .players
                .iter()
                .map(|p| AdminPlayerView {
                    id: p.id,
                    name: p.name.clone(),
                    score: p.score,
                    color: p.color,
                    character: p.character.clone(),
                    connected: p.conns > 0,
                    answered: self.answers.contains_key(&p.id),
                    team: p.team,
                    waiting: p.waiting,
                })
                .collect(),
            teams: self.team_views(),
            question: question.or(puzzle),
        }
    }

    fn send_to(&self, conn_id: u64, msg: &ServerMsg) {
        if let Some(c) = self.conns.get(&conn_id) {
            let _ = c.tx.send(serde_json::to_string(msg).unwrap());
        }
    }

    /// Sends every connection a fresh snapshot of the room.
    fn broadcast(&self) {
        let ranks: HashMap<u32, usize> =
            self.ranked().into_iter().map(|(r, p)| (p.id, r)).collect();
        let vip = self.vip();
        let in_round = matches!(
            self.phase,
            Phase::Question | Phase::Reveal | Phase::Scoreboard
        );
        let question = self.questions.get(self.round).filter(|_| in_round);
        let puzzle = self.puzzles.get(self.round).filter(|_| in_round);
        let live_puzzle = puzzle.filter(|_| self.phase == Phase::Question);
        // The whole phrase is only sent once the round is over.
        let board = puzzle.map(|p| {
            if live_puzzle.is_some() {
                p.mask()
            } else {
                p.answer.clone()
            }
        });
        let (hits, misses) = puzzle.map(|p| (p.hits(), p.misses())).unwrap_or_default();
        let ms_left = self
            .deadline
            .map(|d| d.saturating_duration_since(Instant::now()).as_millis() as u64);
        // The TV preloads the next song: the first one during the intro, then one round ahead.
        let next_round = match self.phase {
            Phase::Intro => Some(0),
            _ if in_round => Some(self.round + 1),
            _ => None,
        };
        let next_clip = next_round
            .and_then(|r| self.questions.get(r))
            .and_then(|q| q.song.as_ref())
            .map(|s| s.preview_url.as_str());

        let teams = self.team_views();
        let stages = self.stage_views();
        let stage = self.current_stage();
        let stage_question = match (stage, self.phase) {
            (Some(i), Phase::Question | Phase::Reveal | Phase::Recap) => {
                self.round - self.stages[i].start + 1
            }
            _ => 0,
        };
        let recap = self.recap_view();

        for conn in self.conns.values() {
            let is_host = matches!(conn.role, Role::Host);
            let you = match conn.role {
                Role::Host => None,
                Role::Player(id) => Some(YouView {
                    player_id: id,
                    answer: self.answers.get(&id).map(|a| a.choice),
                    is_vip: vip == Some(id),
                    rank: ranks.get(&id).copied().unwrap_or(0),
                    team: self
                        .players
                        .iter()
                        .find(|p| p.id == id)
                        .and_then(|p| p.team),
                    waiting: self.players.iter().any(|p| p.id == id && p.waiting),
                    hangman: live_puzzle.map(|_| HangmanYouView {
                        solve_locked: self.hm.solve_locked.contains(&id),
                        cooldown_ms: self.hm.last_guess.get(&id).map_or(0, |t| {
                            GUESS_COOLDOWN.saturating_sub(t.elapsed()).as_millis() as u64
                        }),
                    }),
                }),
            };
            let view = StateView {
                code: &self.code,
                phase: self.phase.name(),
                players: self
                    .players
                    .iter()
                    .map(|p| PlayerView {
                        id: p.id,
                        name: &p.name,
                        score: p.score,
                        color: p.color,
                        character: &p.character,
                        connected: p.conns > 0,
                        answered: self.answers.contains_key(&p.id),
                        team: p.team,
                        waiting: p.waiting,
                    })
                    .collect(),
                vip,
                max_players: self.max_players,
                settings: self.settings.view(),
                teams: teams.clone(),
                round: self.round_number(),
                total_rounds: self.total_rounds(),
                stages: stages.clone(),
                stage: stage.map_or(0, |i| i + 1),
                stage_question,
                recap: recap.clone(),
                question: question.map(|q| QuestionView {
                    kind: if q.song.is_some() { "song" } else { "trivia" },
                    category: &q.category,
                    prompt: &q.prompt,
                    choices: &q.choices,
                    clip: q.song.as_ref().filter(|_| is_host).map(|s| ClipView {
                        preview_url: &s.preview_url,
                        start_ms: s.start_ms,
                        clip_ms: self.settings.clip_ms,
                    }),
                }),
                ms_left,
                phase_ms: self.phase_len.map(|d| d.as_millis() as u64),
                // The correct answer is only sent once the question is closed.
                reveal: question
                    .filter(|_| self.phase != Phase::Question)
                    .map(|q| RevealView {
                        correct: q.answer,
                        results: self
                            .players
                            .iter()
                            .filter(|p| !p.waiting)
                            .map(|p| ResultView {
                                player_id: p.id,
                                choice: self.answers.get(&p.id).map(|a| a.choice),
                                points: self.last_points.get(&p.id).copied().unwrap_or(0),
                            })
                            .collect(),
                        song: q.song.as_ref().map(|s| SongView {
                            title: &s.title,
                            artist: &s.artist,
                            year: s.year,
                            artwork_url: s.artwork_url.as_deref(),
                            store_url: s.store_url.as_deref(),
                        }),
                    }),
                next_clip: next_clip.filter(|_| is_host),
                hangman: puzzle.map(|p| HangmanView {
                    category: &p.category,
                    board: board.as_deref().unwrap_or_default(),
                    hits: &hits,
                    misses: &misses,
                    strikes: p.strikes.min(hangman::MAX_STRIKES),
                    max_strikes: hangman::MAX_STRIKES,
                    feed: &self.hm.feed,
                    results: self
                        .players
                        .iter()
                        .map(|p| ResultView {
                            player_id: p.id,
                            choice: None,
                            points: self.last_points.get(&p.id).copied().unwrap_or(0),
                        })
                        .collect(),
                    outcome: self.hm.outcome.map(Outcome::name),
                    solved_by: self.hm.solved_by,
                }),
                host: self.host_line.as_ref().map(|l| HostView {
                    name: self.host.name(),
                    id: l.id,
                    text: &l.text,
                    callouts: &l.callouts,
                }),
                you,
            };
            let _ = conn
                .tx
                .send(serde_json::to_string(&ServerMsg::State(Box::new(view))).unwrap());
        }
    }
}

fn clean_name(raw: &str) -> Result<String, String> {
    let name: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if name.is_empty() {
        return Err("Pick a name first.".into());
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(format!("Names can be up to {MAX_NAME_CHARS} characters."));
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::clean_name;

    #[test]
    fn names_are_trimmed_and_limited() {
        assert_eq!(clean_name("  Big   Dog ").unwrap(), "Big Dog");
        assert!(clean_name("   ").is_err());
        assert!(clean_name("abcdefghijklmnopq").is_err());
    }
}
