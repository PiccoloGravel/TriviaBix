//! One game room. Each room runs as its own task (an actor) that owns all of
//! the room's state; WebSocket connections talk to it through a channel.

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, sleep_until};

use crate::characters::{self, CHARACTERS};
use crate::db::{Db, Question, Rating, Standing};
use crate::protocol::*;

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
pub const MAX_ROUNDS: usize = 30;
pub const MAX_TEAMS: u8 = TEAMS.len() as u8;
const MAX_NAME_CHARS: usize = 16;

#[derive(Debug, Clone)]
pub struct Timing {
    pub question: Duration,
    pub reveal: Duration,
    pub scoreboard: Duration,
    /// A room with no connections is closed after this long.
    pub idle_timeout: Duration,
}

#[derive(Debug, Clone, Copy)]
pub struct Settings {
    pub rounds: usize,
    pub max_rating: Rating,
    /// 0 for no teams, otherwise 2 to MAX_TEAMS.
    pub teams: u8,
    /// Let new players join a running game; they start at the next question.
    pub late_join: bool,
}

impl Settings {
    fn view(&self) -> SettingsView {
        SettingsView { rounds: self.rounds, max_rating: self.max_rating, teams: self.teams, late_join: self.late_join }
    }
}

/// 1 team is no teams; anything above the limit is capped.
pub fn clamp_teams(teams: u8) -> u8 {
    if teams < 2 { 0 } else { teams.min(MAX_TEAMS) }
}

pub enum Join {
    Host { token: String },
    Player { name: Option<String>, token: Option<String>, character: Option<String> },
}

pub enum RoomCmd {
    Connect {
        join: Join,
        tx: ConnTx,
        reply: oneshot::Sender<Result<u64, String>>,
    },
    Disconnect { conn_id: u64 },
    Msg { conn_id: u64, msg: ClientMsg },
    Info { reply: oneshot::Sender<RoomInfo> },
    Admin { cmd: AdminCmd, reply: oneshot::Sender<Result<(), String>> },
    Snapshot { reply: oneshot::Sender<AdminRoomView> },
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
        rx.await.unwrap_or_else(|_| Err("That room has closed.".into()))
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
        rx.await.unwrap_or_else(|_| Err("That room has closed.".into()))
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
    Question,
    Reveal,
    Scoreboard,
    Final,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Phase::Lobby => "lobby",
            Phase::Question => "question",
            Phase::Reveal => "reveal",
            Phase::Scoreboard => "scoreboard",
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

struct Answer {
    choice: usize,
    after: Duration,
}

pub struct Room {
    code: String,
    host_token: String,
    settings: Settings,
    timing: Timing,
    max_players: usize,
    db: Db,

    players: Vec<Player>,
    next_player_id: u32,
    conns: HashMap<u64, Conn>,
    next_conn_id: u64,

    phase: Phase,
    deadline: Option<Instant>,
    phase_len: Option<Duration>,
    questions: Vec<Question>,
    used_question_ids: Vec<i64>,
    round: usize,
    question_started: Instant,
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
    pub fn spawn(
        code: String,
        host_token: String,
        settings: Settings,
        timing: Timing,
        max_players: usize,
        db: Db,
        on_close: impl FnOnce() + Send + 'static,
    ) -> RoomHandle {
        let (tx, rx) = mpsc::unbounded_channel();
        let room = Room {
            code,
            host_token,
            settings,
            timing,
            max_players,
            db,
            players: Vec::new(),
            next_player_id: 1,
            conns: HashMap::new(),
            next_conn_id: 1,
            phase: Phase::Lobby,
            deadline: None,
            phase_len: None,
            questions: Vec::new(),
            used_question_ids: Vec::new(),
            round: 0,
            question_started: Instant::now(),
            answers: HashMap::new(),
            last_points: HashMap::new(),
            game_id: None,
            empty_since: Some(Instant::now()),
            created_at: SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0),
            closing: false,
        };
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
                let Some(role) = self.conns.get(&conn_id).map(|c| c.role) else { return };
                if let Err(e) = self.on_msg(conn_id, role, msg).await {
                    self.send_to(conn_id, &ServerMsg::Error { message: &e, fatal: false });
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
            Join::Player { name, token, character } => {
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
                    if self.players.iter().any(|p| p.name.eq_ignore_ascii_case(&name)) {
                        return Err(format!("Someone is already called {name}. Pick another name."));
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
                    self.players.push(Player {
                        id,
                        name,
                        token: token.clone(),
                        score: 0,
                        color,
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
            role: if matches!(role, Role::Host) { "host" } else { "player" },
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
        if self.players.iter().any(|p| p.character == id && Some(p.id) != me) {
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
        let pool: Vec<_> = if free.is_empty() { CHARACTERS.iter().collect() } else { free };
        pool.choose(&mut rand::rng()).unwrap().id.to_string()
    }

    /// The team with the fewest players (lowest number on a tie), if teams are on.
    fn smallest_team(&self) -> Option<u8> {
        (0..self.settings.teams).min_by_key(|t| self.players.iter().filter(|p| p.team == Some(*t)).count())
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
                if self.phase != Phase::Question {
                    return Err("Not taking answers right now.".into());
                }
                let n = self.questions[self.round].choices.len();
                if choice >= n {
                    return Err("That's not one of the choices.".into());
                }
                if self.answers.contains_key(&pid) {
                    return Err("You already locked in an answer.".into());
                }
                self.answers.insert(pid, Answer { choice, after: self.question_started.elapsed() });
                if self.all_answered() {
                    self.advance().await;
                } else {
                    self.broadcast();
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
                self.back_to_lobby();
            }
            ClientMsg::Settings { rounds, max_rating, teams, late_join } => {
                if !is_host {
                    return Err("Only the host can change settings.".into());
                }
                self.change_settings(rounds, max_rating, teams, late_join)?;
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
                        if self.players.iter().any(|p| p.id != pid && p.name.eq_ignore_ascii_case(&n)) {
                            return Err(format!("Someone is already called {n}. Pick another name."));
                        }
                        Some(n)
                    }
                    None => None,
                };
                let character = character.map(|c| self.check_character(&c, Some(pid))).transpose()?;
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
                if !matches!(self.phase, Phase::Question | Phase::Reveal | Phase::Scoreboard) {
                    return Err("There's no game running.".into());
                }
                if self.phase == Phase::Question {
                    self.score_question();
                }
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
                    self.send_to(*conn_id, &ServerMsg::Error { message: "The organizer closed this room.", fatal: true });
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
            AdminCmd::Settings { rounds, max_rating, teams, late_join } => {
                self.change_settings(rounds, max_rating, teams, late_join)?;
            }
        }
        Ok(())
    }

    /// Moves on from the current question, reveal or scoreboard. False if there was nothing to skip.
    async fn skip(&mut self) -> bool {
        let running = matches!(self.phase, Phase::Question | Phase::Reveal | Phase::Scoreboard);
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
    fn change_settings(
        &mut self,
        rounds: Option<usize>,
        max_rating: Option<Rating>,
        teams: Option<u8>,
        late_join: Option<bool>,
    ) -> Result<(), String> {
        let locked = rounds.is_some() || max_rating.is_some() || teams.is_some();
        if locked && self.phase != Phase::Lobby {
            return Err("Settings are locked once the game starts.".into());
        }
        if let Some(r) = rounds {
            self.settings.rounds = r.clamp(MIN_ROUNDS, MAX_ROUNDS);
        }
        if let Some(r) = max_rating {
            self.settings.max_rating = r;
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
        self.broadcast();
        Ok(())
    }

    async fn kick(&mut self, player_id: u32) {
        let Some(idx) = self.players.iter().position(|p| p.id == player_id) else { return };
        self.players.remove(idx);
        self.answers.remove(&player_id);
        let kicked: Vec<u64> = self
            .conns
            .iter()
            .filter(|(_, c)| matches!(c.role, Role::Player(id) if id == player_id))
            .map(|(id, _)| *id)
            .collect();
        for conn_id in kicked {
            self.send_to(conn_id, &ServerMsg::Error { message: "The host removed you from the game.", fatal: true });
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
        if self.used_question_ids.len() > 200 {
            self.used_question_ids.clear();
        }
        let mut questions = self
            .db
            .random_questions(self.settings.rounds, self.settings.max_rating, &self.used_question_ids)
            .await
            .map_err(|e| {
                tracing::error!("loading questions: {e:#}");
                "Couldn't load questions. Try again?".to_string()
            })?;
        if questions.len() < self.settings.rounds {
            // Ran out of unseen questions; allow repeats rather than a short game.
            self.used_question_ids.clear();
            questions = self
                .db
                .random_questions(self.settings.rounds, self.settings.max_rating, &[])
                .await
                .map_err(|e| e.to_string())?;
        }
        if questions.is_empty() {
            return Err("The question bank is empty.".into());
        }
        self.used_question_ids.extend(questions.iter().map(|q| q.id));
        self.questions = questions;
        for p in &mut self.players {
            p.score = 0;
        }
        self.game_id = match self.db.start_game(&self.code, self.questions.len(), self.players.len()).await {
            Ok(id) => Some(id),
            Err(e) => {
                tracing::warn!("recording game start: {e:#}");
                None
            }
        };
        self.round = 0;
        self.begin_question();
        Ok(())
    }

    fn begin_question(&mut self) {
        // Anyone who joined since the last question plays from here on.
        for p in &mut self.players {
            p.waiting = false;
        }
        self.answers.clear();
        self.last_points.clear();
        self.question_started = Instant::now();
        self.set_phase(Phase::Question, Some(self.timing.question));
        self.broadcast();
    }

    /// Moves to the next phase, either because a timer ran out or someone skipped.
    async fn advance(&mut self) {
        match self.phase {
            Phase::Question => {
                self.score_question();
                self.set_phase(Phase::Reveal, Some(self.timing.reveal));
            }
            Phase::Reveal if self.round + 1 >= self.questions.len() => {
                self.set_phase(Phase::Final, None);
                self.record_results().await;
            }
            Phase::Reveal => self.set_phase(Phase::Scoreboard, Some(self.timing.scoreboard)),
            Phase::Scoreboard => {
                self.round += 1;
                self.begin_question();
                return;
            }
            Phase::Lobby | Phase::Final => self.set_phase(self.phase, None),
        }
        self.broadcast();
    }

    /// Correct answers earn 500 points plus up to 500 more for speed.
    fn score_question(&mut self) {
        let q = &self.questions[self.round];
        let limit = self.timing.question.as_secs_f64().max(0.001);
        for p in self.players.iter_mut().filter(|p| !p.waiting) {
            let points = match self.answers.get(&p.id) {
                Some(a) if a.choice == q.answer => {
                    let speed = (1.0 - a.after.as_secs_f64() / limit).clamp(0.0, 1.0);
                    500 + (500.0 * speed).round() as i64
                }
                _ => 0,
            };
            p.score += points;
            self.last_points.insert(p.id, points);
        }
    }

    async fn record_results(&mut self) {
        let Some(game_id) = self.game_id.take() else { return };
        let standings: Vec<Standing> = self
            .ranked()
            .into_iter()
            .map(|(rank, p)| Standing { name: p.name.clone(), score: p.score, rank })
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
        let mut active = self.players.iter().filter(|p| p.conns > 0 && !p.waiting).peekable();
        active.peek().is_some() && active.all(|p| self.answers.contains_key(&p.id))
    }

    /// The VIP is the earliest-joined player who is still connected and playing.
    fn vip(&self) -> Option<u32> {
        self.players.iter().find(|p| p.conns > 0 && !p.waiting).map(|p| p.id)
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
            let rank = if i > 0 && sorted[i - 1].score == p.score { out.last().map(|(r, _)| *r).unwrap() } else { i + 1 };
            out.push((rank, *p));
        }
        out
    }

    /// Team standings, best first. A team's score is the average of its playing members.
    fn team_views(&self) -> Vec<TeamView> {
        let mut teams: Vec<TeamView> = (0..self.settings.teams)
            .map(|t| {
                let members: Vec<&Player> = self.players.iter().filter(|p| p.team == Some(t)).collect();
                let playing: Vec<i64> = members.iter().filter(|p| !p.waiting).map(|p| p.score).collect();
                let score = if playing.is_empty() { 0 } else { playing.iter().sum::<i64>() / playing.len() as i64 };
                let (name, color) = TEAMS[t as usize];
                TeamView { id: t, name, color, score, members: members.iter().map(|p| p.id).collect() }
            })
            .collect();
        teams.sort_by(|a, b| b.score.cmp(&a.score).then(a.id.cmp(&b.id)));
        teams
    }

    fn in_round(&self) -> bool {
        matches!(self.phase, Phase::Question | Phase::Reveal | Phase::Scoreboard)
    }

    fn round_number(&self) -> usize {
        if self.in_round() || self.phase == Phase::Final { self.round + 1 } else { 0 }
    }

    fn total_rounds(&self) -> usize {
        if self.phase == Phase::Lobby { self.settings.rounds } else { self.questions.len() }
    }

    fn ms_left(&self) -> Option<u64> {
        self.deadline.map(|d| d.saturating_duration_since(Instant::now()).as_millis() as u64)
    }

    fn snapshot(&self) -> AdminRoomView {
        let question = self.questions.get(self.round).filter(|_| self.in_round());
        AdminRoomView {
            code: self.code.clone(),
            host_token: self.host_token.clone(),
            created_at: self.created_at,
            phase: self.phase.name(),
            round: self.round_number(),
            total_rounds: self.total_rounds(),
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
            question: question.map(|q| AdminQuestionView {
                category: q.category.clone(),
                prompt: q.prompt.clone(),
                answer: q.choices[q.answer].clone(),
            }),
        }
    }

    fn send_to(&self, conn_id: u64, msg: &ServerMsg) {
        if let Some(c) = self.conns.get(&conn_id) {
            let _ = c.tx.send(serde_json::to_string(msg).unwrap());
        }
    }

    /// Sends every connection a fresh snapshot of the room.
    fn broadcast(&self) {
        let ranks: HashMap<u32, usize> = self.ranked().into_iter().map(|(r, p)| (p.id, r)).collect();
        let vip = self.vip();
        let question = self.questions.get(self.round).filter(|_| self.in_round());
        let ms_left = self.ms_left();
        let teams = self.team_views();

        for conn in self.conns.values() {
            let you = match conn.role {
                Role::Host => None,
                Role::Player(id) => {
                    let me = self.players.iter().find(|p| p.id == id);
                    Some(YouView {
                        player_id: id,
                        answer: self.answers.get(&id).map(|a| a.choice),
                        is_vip: vip == Some(id),
                        rank: ranks.get(&id).copied().unwrap_or(0),
                        team: me.and_then(|p| p.team),
                        waiting: me.is_some_and(|p| p.waiting),
                    })
                }
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
                question: question.map(|q| QuestionView { category: &q.category, prompt: &q.prompt, choices: &q.choices }),
                ms_left,
                phase_ms: self.phase_len.map(|d| d.as_millis() as u64),
                // The correct answer is only sent once the question is closed.
                reveal: question.filter(|_| self.phase != Phase::Question).map(|q| RevealView {
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
                }),
                you,
            };
            let _ = conn.tx.send(serde_json::to_string(&ServerMsg::State(view)).unwrap());
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
