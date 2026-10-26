//! Rendering jobs started from the admin panel: Bix's voice, candidate voices for him and the
//! cutscene art on a rented vast.ai GPU, and the host's per-question lines from Claude.
//!
//! Each job is one of the command-line tools run as a child process (`gpu/vast.py voice`,
//! `art/render_on_vast.py`, `triviabix-questions scripts generate`), so the panel and the command
//! line share the same spending limits and the same reuse of the asset store. With `TBX_EVENTS=1`
//! the tools print `@tbx {json}` lines (the GPU, its price, the instance id, progress), which is
//! how the panel shows a price before anything is rented and what a job cost afterwards.
//!
//! Starting a job takes two steps: a dry run that rents nothing and returns a one-time token with
//! the GPU, price and most it can cost, then a start with that token. Jobs of different kinds run
//! side by side, each on its own GPU with its own price check, log and cancel; two of the same kind
//! never do (see `Kind::clashes`).
//! Cancelling sends the tools SIGTERM, which they turn into destroying the instance; if a job
//! ends (or the server restarts) with an instance still rented, it's destroyed by id.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{broadcast, mpsc};

use crate::db::Db;
use crate::settings::{self, Settings};
use crate::voice::Voice;

/// Log lines kept per job, in memory and in the database.
const LOG_LINES: usize = 2000;
/// Longest log line kept; a dry run of the line writer prints whole API requests.
const LINE_CHARS: usize = 400;
/// How long a dry run's price stays good for starting the job.
const PREVIEW_TTL: Duration = Duration::from_secs(15 * 60);
/// How long a dry run may take (the art one first makes what it can from the asset store).
const PREVIEW_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// How long a cancelled job gets to destroy its instance before it's killed.
const CANCEL_GRACE: Duration = Duration::from_secs(90);
/// How long the "what's missing" counts are reused.
const MISSING_TTL: Duration = Duration::from_secs(60);
/// The most hours a voice run may keep a GPU (vast.py's default).
const VOICE_MAX_HOURS: f64 = 3.0;
/// The most candidate voices one design run may make.
const DESIGN_MAX: usize = 8;
/// Where gpu/vast.py keeps candidate voices in the asset store.
const SAMPLE_KIND: &str = "voice-sample";
/// What the Pipeline page suggests for a new voice, the same as tts/make_voice.py's defaults.
pub const VOICE_DESCRIPTION: &str = "A washed-up American game show host in his fifties, clinging to his last gig. \
His voice is warm and a little gravelly, cheeky and quick but world-weary, with dry, sarcastic timing and a tired \
sigh in it, and the old showman's bounce still comes back for the big moments. Always clear and easy to follow.";
/// Lines a candidate voice might say in its clip. Each candidate gets a different one, picked at
/// random, so every design run sounds fresh. The admin can edit the list.
pub const VOICE_SAMPLES: &[&str] = &[
    "Welcome back to TriviaBix. I'm Bix, your host. Still. Twenty years in show business, and this is the room they give me. Phones out, brains on. Let's play!",
    "Good evening, good evening. Please, hold your applause. No, really, you can hold it. I can tell you're all holding it very tightly.",
    "Another round, another chance for you lot to disappoint me. And honestly? I respect the consistency. Phones ready!",
    "I once hosted a show watched by eleven million people. Tonight I'm watched by a dog and a guy eating chips. Let's do this.",
    "Right answer! Look at you. I'd give you a prize, but the network took the prize budget. And the car. And my parking space.",
    "That was wrong. Very wrong. Impressively wrong. I've been doing this a long time and I've never seen wrong quite like that.",
    "My agent says this gig is a stepping stone. He's said that for nine years. Anyway, here's your next question.",
    "Ladies and gentlemen, and whoever is hiding in the kitchen, it's time for the final round. Try to look interested.",
    "Ten seconds left. Nine. Eight. You know what, I'm not counting the rest. I'm tired. Just answer the question.",
    "Our leader is pulling away, and the rest of you are pulling a muscle trying to keep up. It's beautiful, really.",
    "Fun fact: I used to have a catchphrase. It was on mugs. Now the mugs are in my garage. Next question!",
    "Ooh, a tie at the top. The tension, the drama. This is the most excitement I've had since my last contract renewal.",
    "You know, they told me hosting trivia in a living room would be humbling. They were right. They were so, so right.",
    "Congratulations to our winner. You beat everyone here, which, let's be honest, was not the toughest crowd. Goodnight, everybody!",
    "Don't worry if you get this one wrong. I've gotten plenty wrong. Career choices, mostly. Phones up, here we go.",
    "Is it hot in here, or is it just the pressure? It's the pressure. Also the lights. Mostly my career. Next question!",
];
/// The most lines the list can hold.
const VOICE_SAMPLES_MAX: usize = 60;
/// The most questions one line-writing run may cover.
const LINES_MAX: usize = 500;
/// Where the line writer saves, under content/scripts. Not a folder the image ships, so a
/// container restart doesn't replace it.
const LINES_FOLDER: &str = "generated";
const ART_KINDS: &[&str] = &["backdrop", "sticker", "category", "clip"];
/// The most questions one import may fetch (Open Trivia DB's own limit per request).
const IMPORT_MAX: usize = 50;
const TRIVIA_API_CATEGORIES: &[&str] = &[
    "music",
    "film_and_tv",
    "sport_and_leisure",
    "history",
    "geography",
    "science",
    "food_and_drink",
    "society_and_culture",
    "arts_and_literature",
    "general_knowledge",
];

#[derive(Debug, Clone)]
pub struct RenderConfig {
    /// The folder holding `gpu/` and `art/` (the repo, or /app in the image).
    pub tools_dir: PathBuf,
    pub python: String,
    /// The `triviabix-questions` command-line tool.
    pub questions_bin: String,
    /// Where the TV's copies of generated art go (served at /cutscenes/assets/gen/).
    pub art_dir: PathBuf,
    /// The shared asset store (docs/assets.md).
    pub assets_dir: PathBuf,
    /// This server, as the voice job reaches its admin API.
    pub server_url: String,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            tools_dir: ".".into(),
            python: "python3".into(),
            questions_bin: "triviabix-questions".into(),
            art_dir: "data/art".into(),
            assets_dir: "data/assets".into(),
            server_url: "http://127.0.0.1:8080".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Voice,
    /// Candidate voices for Bix from a description, kept in the asset store to listen to and pick.
    Design,
    Art,
    Lines,
    /// Destroys every TriviaBix instance on the vast.ai account. Costs nothing, so no preview.
    Cleanup,
    /// Pulls questions from a trivia site into a pack, skipping duplicates. Free, so no preview.
    Import,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Voice => "voice",
            Kind::Design => "design",
            Kind::Art => "art",
            Kind::Lines => "lines",
            Kind::Cleanup => "cleanup",
            Kind::Import => "import",
        }
    }

    /// Whether the job rents a GPU.
    fn rents(self) -> bool {
        matches!(self, Kind::Voice | Kind::Design | Kind::Art)
    }

    fn parse(s: &str) -> Option<Self> {
        serde_json::from_value(Value::String(s.into())).ok()
    }

    /// Whether the two can't run at once: two of a kind, or a cleanup (which destroys every
    /// TriviaBix instance) next to a job with a GPU.
    fn clashes(self, other: Kind) -> bool {
        self == other
            || (self == Kind::Cleanup && other.rents())
            || (other == Kind::Cleanup && self.rents())
    }

    fn title(self) -> &'static str {
        match self {
            Kind::Voice => "A voice run",
            Kind::Design => "A voice design run",
            Kind::Art => "An art run",
            Kind::Lines => "A line-writing run",
            Kind::Cleanup => "A cleanup",
            Kind::Import => "An import",
        }
    }
}

/// What the admin can choose for a job. Anything else is fixed by the scripts' limits.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Options {
    /// Voice: the most hours the GPU may run (up to 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_hours: Option<f64>,
    /// Design: what the voice should sound like.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Design: lines a candidate might say, one per line. Each candidate's clip, which its voice
    /// is cloned from, says a different one picked at random.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Design: the lines picked from `text`, one per candidate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<Vec<String>>,
    /// Design: how many candidates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<usize>,
    /// Art: only this kind (backdrop, sticker, category or clip).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<String>,
    /// Lines: the most questions to write lines for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// Lines: which lines count as missing (either, lead-in or reveal).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing: Option<String>,
    /// Lines: only the questions one import brought in (its job id). Without it, every question
    /// that's missing lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch: Option<i64>,
    /// Lines: the question ids `batch` stands for, filled in by the server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids: Option<Vec<String>>,
    /// Import: opentdb or trivia-api.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Import: how many questions to fetch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<usize>,
    /// Import: the site's category (an Open Trivia DB number, or a Trivia API name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// Import: easy, medium or hard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub difficulty: Option<String>,
}

impl Options {
    /// Fills in defaults and refuses anything outside the limits.
    fn check(self, kind: Kind) -> Result<Options, String> {
        Ok(match kind {
            Kind::Voice => {
                let hours = self.max_hours.unwrap_or(VOICE_MAX_HOURS);
                if !(0.25..=VOICE_MAX_HOURS).contains(&hours) {
                    return Err(format!(
                        "A voice run can keep the GPU for 15 minutes to {VOICE_MAX_HOURS} hours."
                    ));
                }
                Options {
                    max_hours: Some(hours),
                    ..Default::default()
                }
            }
            Kind::Design => {
                let tidy = |v: Option<String>, default: &str| {
                    let v = v.unwrap_or_default();
                    let v = v.split_whitespace().collect::<Vec<_>>().join(" ");
                    if v.is_empty() { default.to_string() } else { v }
                };
                let description = tidy(self.description, VOICE_DESCRIPTION);
                if !(20..=800).contains(&description.chars().count()) {
                    return Err("Describe the voice in 20 to 800 characters.".into());
                }
                let mut pool: Vec<String> = self
                    .text
                    .unwrap_or_default()
                    .lines()
                    .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
                    .filter(|l| !l.is_empty())
                    .collect();
                if pool.is_empty() {
                    pool = VOICE_SAMPLES.iter().map(|l| l.to_string()).collect();
                }
                pool.sort();
                pool.dedup();
                if pool.len() > VOICE_SAMPLES_MAX {
                    return Err(format!("Keep the list to {VOICE_SAMPLES_MAX} lines."));
                }
                // Ming clones best from a clip of 5 to 15 seconds.
                if let Some(l) = pool
                    .iter()
                    .find(|l| !(30..=300).contains(&l.chars().count()))
                {
                    return Err(format!(
                        "Each line should be 30 to 300 characters (5 to 15 seconds spoken): \"{l}\""
                    ));
                }
                let count = self.count.unwrap_or(4);
                if !(1..=DESIGN_MAX).contains(&count) {
                    return Err(format!("Make 1 to {DESIGN_MAX} candidates at a time."));
                }
                // A different line for each candidate while the list lasts.
                let mut rng = rand::rng();
                pool.shuffle(&mut rng);
                let lines = (0..count).map(|i| pool[i % pool.len()].clone()).collect();
                Options {
                    description: Some(description),
                    lines: Some(lines),
                    count: Some(count),
                    ..Default::default()
                }
            }
            Kind::Art => {
                let kinds = self.kinds.filter(|k| !k.is_empty() && k != "all");
                if let Some(k) = &kinds
                    && !ART_KINDS.contains(&k.as_str())
                {
                    return Err(format!(
                        "Art kind should be one of {}.",
                        ART_KINDS.join(", ")
                    ));
                }
                Options {
                    kinds,
                    ..Default::default()
                }
            }
            Kind::Lines => {
                let limit = self.limit.unwrap_or(50);
                if !(1..=LINES_MAX).contains(&limit) {
                    return Err(format!(
                        "Write lines for 1 to {LINES_MAX} questions at a time."
                    ));
                }
                let missing = self.missing.unwrap_or_else(|| "either".into());
                if !["either", "lead-in", "reveal"].contains(&missing.as_str()) {
                    return Err("Missing lines should be either, lead-in or reveal.".into());
                }
                Options {
                    limit: Some(limit),
                    missing: Some(missing),
                    batch: self.batch,
                    ..Default::default()
                }
            }
            Kind::Cleanup => Options::default(),
            Kind::Import => {
                let source = self.source.unwrap_or_else(|| "opentdb".into());
                let amount = self.amount.unwrap_or(20);
                if !(1..=IMPORT_MAX).contains(&amount) {
                    return Err(format!("Import 1 to {IMPORT_MAX} questions at a time."));
                }
                let category = self.category.filter(|c| !c.is_empty() && c != "any");
                match (source.as_str(), &category) {
                    ("opentdb", Some(c))
                        if !c.parse::<u32>().is_ok_and(|n| (9..=32).contains(&n)) =>
                    {
                        return Err("Pick an Open Trivia DB category from the list.".into());
                    }
                    ("trivia-api", Some(c)) if !TRIVIA_API_CATEGORIES.contains(&c.as_str()) => {
                        return Err("Pick a Trivia API category from the list.".into());
                    }
                    ("opentdb" | "trivia-api", _) => {}
                    _ => return Err("Import from opentdb or trivia-api.".into()),
                }
                let difficulty = self.difficulty.filter(|d| !d.is_empty() && d != "any");
                if let Some(d) = &difficulty
                    && !["easy", "medium", "hard"].contains(&d.as_str())
                {
                    return Err("Difficulty should be easy, medium or hard.".into());
                }
                Options {
                    source: Some(source),
                    amount: Some(amount),
                    category,
                    difficulty,
                    ..Default::default()
                }
            }
        })
    }
}

/// How a tool is run: counting what's missing, a dry run, or for real.
#[derive(Clone, Copy)]
enum Mode {
    DryRun,
    /// With the price limit to rent under, from the dry run.
    Run(Option<f64>),
}

/// A job, as the panel shows it.
#[derive(Debug, Clone, Serialize)]
pub struct JobView {
    pub id: i64,
    pub kind: Kind,
    pub options: Options,
    /// running, done, failed, cancelled or interrupted (the server restarted mid-job).
    pub state: String,
    pub started_at: u64,
    pub finished_at: Option<u64>,
    pub exit_code: Option<i32>,
    pub gpu: Option<String>,
    pub price_hr: Option<f64>,
    /// The most it could cost, from the dry run.
    pub max_cost: Option<f64>,
    /// What it cost: GPU time from renting to destroying, at the rented price. For a running job,
    /// the cost so far. Claude jobs report tokens in `details` instead.
    pub cost: Option<f64>,
    pub instance: Option<i64>,
    /// The instance was destroyed (or never rented).
    pub released: bool,
    pub cancelling: bool,
    /// What the tool reported: what it set out to make and how far it got.
    pub details: Value,
    /// Log lines after `since`, numbered.
    pub log: Vec<(u64, String)>,
    pub log_seq: u64,
}

struct Job {
    view: JobView,
    log: VecDeque<(u64, String)>,
    /// The child's process group, for cancelling.
    pgid: Option<i32>,
    rented_at: Option<Instant>,
}

impl Job {
    fn push(&mut self, line: String) {
        self.view.log_seq += 1;
        let mut line = line;
        if line.len() > LINE_CHARS {
            let mut cut = LINE_CHARS;
            while !line.is_char_boundary(cut) {
                cut -= 1;
            }
            line.truncate(cut);
            line.push('…');
        }
        self.log.push_back((self.view.log_seq, line));
        if self.log.len() > LOG_LINES {
            self.log.pop_front();
        }
    }

    /// Takes in one `@tbx` event from the tool.
    fn event(&mut self, ev: &Value) {
        let d = &mut self.view.details;
        match ev["event"].as_str() {
            Some("offer") => {
                self.view.gpu = ev["gpu"]
                    .as_str()
                    .map(|g| format!("{g} {}GB", ev["vram_gb"]));
                self.view.price_hr = ev["price_hr"].as_f64();
                if let Some(e) = estimate(ev) {
                    d["estimate"] = e;
                }
            }
            Some("rented") => {
                self.view.instance = ev["instance"].as_i64();
                self.view.released = false;
                self.view.price_hr = ev["price_hr"].as_f64().or(self.view.price_hr);
                self.rented_at = Some(Instant::now());
            }
            Some("destroyed") => {
                self.view.released = true;
                let cost = ev["cost"].as_f64().unwrap_or(0.0);
                self.view.cost = Some(self.view.cost.unwrap_or(0.0) + cost);
                self.rented_at = None;
            }
            Some("backlog") => {
                let n = ev["lines"].clone();
                if d.get("backlog_start").is_none() {
                    d["backlog_start"] = n.clone();
                }
                d["backlog"] = n;
            }
            Some("todo") => d["todo"] = ev.clone(),
            Some("progress") => d["progress"] = ev.clone(),
            Some("imported") => d["imported"] = ev.clone(),
            Some("made") => {
                if !d["made"].is_array() {
                    d["made"] = json!([]);
                }
                d["made"].as_array_mut().unwrap().push(ev["key"].clone());
            }
            _ => {}
        }
    }

    /// The view with only the log lines after `since`, and the cost so far while a GPU is rented.
    fn snapshot(&self, since: u64) -> JobView {
        let mut v = self.view.clone();
        v.log = self
            .log
            .iter()
            .filter(|(n, _)| *n > since)
            .cloned()
            .collect();
        if let (Some(at), Some(price)) = (self.rented_at, v.price_hr) {
            v.cost = Some(v.cost.unwrap_or(0.0) + price * at.elapsed().as_secs_f64() / 3600.0);
        }
        v
    }
}

/// The expected time and cost in a tool's `offer` event, if it gave them.
fn estimate(offer: &Value) -> Option<Value> {
    let minutes = offer["est_minutes"].as_f64()?;
    Some(json!({ "minutes": minutes, "cost": offer["est_cost"].as_f64() }))
}

/// A dry run that can be started with its token.
struct Preview {
    kind: Kind,
    options: Options,
    price_hr: Option<f64>,
    max_cost: Option<f64>,
    estimate: Option<Value>,
    gpu: Option<String>,
    made: Instant,
}

/// What a dry run found.
#[derive(Debug, Serialize)]
pub struct PreviewView {
    /// Pass this to start the job. Absent when there's nothing to do or the dry run failed.
    pub token: Option<String>,
    pub kind: Kind,
    pub options: Options,
    pub ok: bool,
    /// Nothing needs making, so nothing would be rented or billed.
    pub nothing: bool,
    pub gpu: Option<String>,
    pub price_hr: Option<f64>,
    pub max_cost: Option<f64>,
    /// The longest the GPU may run, in minutes.
    pub minutes: Option<f64>,
    /// How long the job is expected to take and cost on the quoted GPU: {minutes, cost}.
    pub estimate: Option<Value>,
    pub details: Value,
    /// The tool's last words, when it failed (such as a missing API key or no GPU under the limit).
    pub error: Option<String>,
    pub log: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Missing {
    /// Voice lines waiting to be made.
    pub voice: usize,
    /// Art the TV doesn't have yet (some may come free from the asset store).
    pub art: Option<usize>,
    /// Questions without host lines.
    pub lines: Option<usize>,
    pub errors: Vec<String>,
}

#[derive(Clone)]
pub struct Render(Arc<Inner>);

struct Inner {
    cfg: RenderConfig,
    content_dir: PathBuf,
    static_dir: PathBuf,
    db: Db,
    voice: Voice,
    settings: Settings,
    /// The running jobs, at most one of each kind, and the last finished one of each kind.
    jobs: Mutex<Vec<Job>>,
    previews: Mutex<HashMap<String, Preview>>,
    /// The kinds with a price check under way.
    previewing: Mutex<HashSet<Kind>>,
    missing: tokio::sync::Mutex<Option<(Instant, Missing)>>,
    /// The persona Bix's lines are written for, and where they're saved.
    persona: Mutex<triviabix_questions::Persona>,
    finished: broadcast::Sender<Kind>,
}

/// Why a `kind` job can't start next to `jobs`, if one it clashes with is running.
fn blocked(jobs: &[Job], kind: Kind) -> Option<String> {
    let other = jobs
        .iter()
        .find(|j| j.view.state == "running" && kind.clashes(j.view.kind))?;
    Some(if other.view.kind == kind {
        format!(
            "{} is already running. Wait for it, or cancel it.",
            kind.title()
        )
    } else if kind == Kind::Cleanup {
        format!(
            "{} is using a GPU. Cancel it, or wait for it to give the GPU back, before destroying leftovers.",
            other.view.kind.title()
        )
    } else {
        format!(
            "{} is destroying every GPU. Wait for it to finish.",
            other.view.kind.title()
        )
    })
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Whether `cmd` can be run: a path that exists, or a name on PATH.
fn runnable(cmd: &str) -> bool {
    if cmd.contains('/') {
        return Path::new(cmd).is_file();
    }
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(cmd).is_file()))
}

fn signal_group(pgid: i32, signal: i32) {
    // SAFETY: kill() has no memory effects; a negative pid addresses the job's process group,
    // which only holds the tool and its children.
    unsafe {
        libc::kill(-pgid, signal);
    }
}

impl Render {
    pub fn new(
        cfg: RenderConfig,
        content_dir: PathBuf,
        static_dir: PathBuf,
        db: Db,
        voice: Voice,
        settings: Settings,
    ) -> Self {
        Self(Arc::new(Inner {
            settings,
            cfg,
            content_dir,
            static_dir,
            db,
            voice,
            jobs: Default::default(),
            previews: Default::default(),
            previewing: Default::default(),
            missing: Default::default(),
            persona: Default::default(),
            finished: broadcast::channel(8).0,
        }))
    }

    /// Writes lines for this persona from now on (see crate::personas), and counts again.
    pub async fn set_persona(&self, persona: triviabix_questions::Persona) {
        *self.0.persona.lock().unwrap() = persona;
        self.forget_missing().await;
    }

    /// Counts what's missing again next time, e.g. after the art models change.
    pub async fn forget_missing(&self) {
        *self.0.missing.lock().await = None;
    }

    /// The most a GPU job may pay per hour: the Settings page's limit, never above the ceiling.
    fn cap(&self, kind: Kind) -> f64 {
        match kind {
            Kind::Voice | Kind::Design => self
                .0
                .settings
                .price("VOICE_MAX_PRICE", settings::VOICE_PRICE),
            _ => self.0.settings.price("ART_MAX_PRICE", settings::ART_PRICE),
        }
    }

    /// Hands a tool the keys and model choices saved on the Settings page. Ones set only in the
    /// container's environment reach it anyway.
    fn with_settings(&self, cmd: &mut Command) {
        for (key, value) in self.0.settings.saved_env() {
            cmd.env(key, value);
        }
    }

    pub fn art_dir(&self) -> &Path {
        &self.0.cfg.art_dir
    }

    /// The candidate voices for Bix in the asset store, newest first, each with whether it's the
    /// voice in use (BIX_VOICE=clone:<key>).
    pub fn voices(&self) -> Vec<Value> {
        let current = self.0.settings.get("BIX_VOICE").unwrap_or_default();
        let mut out: Vec<Value> = std::fs::read_dir(self.0.cfg.assets_dir.join(SAMPLE_KIND))
            .into_iter()
            .flatten()
            .flatten()
            .flat_map(|style| std::fs::read_dir(style.path()).into_iter().flatten().flatten())
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| {
                let meta: Value = serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok()?;
                let key = meta["key"].as_str().filter(|k| is_key(k))?;
                e.path().with_file_name(format!("{key}.wav")).is_file().then(|| {
                    json!({
                        "key": key,
                        "description": meta["description"].as_str().or(meta["inputs"]["description"].as_str()),
                        "text": meta["transcript"].as_str().or(meta["inputs"]["text"].as_str()),
                        "seed": meta["seed"].as_u64().or(meta["inputs"]["seed"].as_u64()),
                        "model": meta["model"],
                        "created": meta["created"],
                        "current": current == format!("clone:{key}"),
                    })
                })
            })
            .collect();
        out.sort_by_key(|v| {
            (
                std::cmp::Reverse(v["created"].as_u64().unwrap_or(0)),
                v["seed"].as_u64().unwrap_or(0),
            )
        });
        out
    }

    /// One candidate voice's clip.
    pub fn voice_clip(&self, key: &str) -> Option<PathBuf> {
        if !is_key(key) {
            return None;
        }
        std::fs::read_dir(self.0.cfg.assets_dir.join(SAMPLE_KIND))
            .ok()?
            .flatten()
            .map(|style| style.path().join(format!("{key}.wav")))
            .find(|p| p.is_file())
    }

    /// Tells the subscriber which kind of job just finished successfully.
    pub fn finished(&self) -> broadcast::Receiver<Kind> {
        self.0.finished.subscribe()
    }

    /// After a restart: jobs that were running are marked interrupted, and any GPU one of them
    /// still had rented is destroyed. (The instance also stops itself at its time limit.)
    pub async fn recover(&self) {
        let rows = match sqlx::query(
            "SELECT id, instance, released FROM render_jobs WHERE state = 'running'",
        )
        .fetch_all(&self.0.db.pool)
        .await
        {
            Ok(rows) => rows,
            Err(e) => {
                tracing::error!("render jobs: {e:#}");
                return;
            }
        };
        for row in rows {
            let id: i64 = row.get("id");
            let instance: Option<i64> = row.get("instance");
            let released: bool = row.get("released");
            tracing::warn!("render job {id} was interrupted by a restart");
            let _ = sqlx::query(
                "UPDATE render_jobs SET state = 'interrupted', finished_at = ? WHERE id = ?",
            )
            .bind(now_ms() as i64)
            .bind(id)
            .execute(&self.0.db.pool)
            .await;
            if let (Some(instance), false) = (instance, released) {
                let this = self.clone();
                tokio::spawn(async move {
                    if this.destroy(instance, None).await {
                        let _ = sqlx::query("UPDATE render_jobs SET released = 1 WHERE id = ?")
                            .bind(id)
                            .execute(&this.0.db.pool)
                            .await;
                    }
                });
            }
        }
    }

    /// What's set up: the tools, and which keys the server has (never the keys themselves).
    pub fn ready(&self) -> Value {
        let c = &self.0.cfg;
        json!({
            "python": runnable(&c.python),
            "gpu_tools": c.tools_dir.join("gpu/vast.py").is_file(),
            "art_tools": c.tools_dir.join("art/render_on_vast.py").is_file(),
            "line_writer": runnable(&c.questions_bin),
            "vast_key": self.0.settings.is_set("VAST_API_KEY"),
            "anthropic_key": self.0.settings.is_set("ANTHROPIC_API_KEY"),
            "hf_token": self.0.settings.is_set("HF_TOKEN"),
            "voice_max_price": self.cap(Kind::Voice),
            "art_max_price": self.cap(Kind::Art),
            "voice_max_hours": VOICE_MAX_HOURS,
            "design_max": DESIGN_MAX,
            "voice_description": VOICE_DESCRIPTION,
            "voice_samples": VOICE_SAMPLES,
            "lines_max": LINES_MAX,
        })
    }

    /// The job the panel opens on: the newest running one, or else the newest that finished.
    pub fn current(&self, since: u64) -> Option<JobView> {
        let jobs = self.0.jobs.lock().unwrap();
        jobs.iter()
            .filter(|j| j.view.state == "running")
            .max_by_key(|j| j.view.id)
            .or_else(|| jobs.iter().max_by_key(|j| j.view.id))
            .map(|j| j.snapshot(since))
    }

    /// Every running job, oldest first, without their logs.
    pub fn running_jobs(&self) -> Vec<JobView> {
        let jobs = self.0.jobs.lock().unwrap();
        let mut out: Vec<JobView> = jobs
            .iter()
            .filter(|j| j.view.state == "running")
            .map(|j| j.snapshot(u64::MAX))
            .collect();
        out.sort_by_key(|v| v.id);
        out
    }

    /// Whether any job is running, e.g. so an update waits for it.
    pub fn running(&self) -> bool {
        !self.running_jobs().is_empty()
    }

    /// Whether a job of one of these kinds is running.
    pub fn running_any(&self, kinds: &[Kind]) -> bool {
        self.0
            .jobs
            .lock()
            .unwrap()
            .iter()
            .any(|j| j.view.state == "running" && kinds.contains(&j.view.kind))
    }

    /// Why a `kind` job can't start now, if one it clashes with is running.
    fn blocked(&self, kind: Kind) -> Option<String> {
        blocked(&self.0.jobs.lock().unwrap(), kind)
    }

    /// Runs `f` on the live job with this id, if it's still kept.
    fn with_job<T>(&self, id: i64, f: impl FnOnce(&mut Job) -> T) -> Option<T> {
        self.0
            .jobs
            .lock()
            .unwrap()
            .iter_mut()
            .find(|j| j.view.id == id)
            .map(f)
    }

    /// One job: a live one, or from the history.
    pub async fn job(&self, id: i64, since: u64) -> Option<JobView> {
        if let Some(v) = self.with_job(id, |j| j.snapshot(since)) {
            return Some(v);
        }
        let row = sqlx::query("SELECT * FROM render_jobs WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.0.db.pool)
            .await
            .ok()??;
        let mut v = view_of(&row);
        let log: String = row.get("log");
        v.log = log
            .lines()
            .enumerate()
            .map(|(i, l)| (i as u64 + 1, l.to_string()))
            .filter(|(n, _)| *n > since)
            .collect();
        v.log_seq = log.lines().count() as u64;
        Some(v)
    }

    /// Past jobs, newest first, without their logs.
    pub async fn history(&self, limit: i64) -> Vec<JobView> {
        sqlx::query("SELECT * FROM render_jobs ORDER BY id DESC LIMIT ?")
            .bind(limit)
            .fetch_all(&self.0.db.pool)
            .await
            .map(|rows| rows.iter().map(view_of).collect())
            .unwrap_or_default()
    }

    /// Money spent on GPUs, all time and in the last 30 days.
    pub async fn spent(&self) -> Value {
        let month = now_ms().saturating_sub(30 * 24 * 3600 * 1000) as i64;
        let row = sqlx::query(
            "SELECT COALESCE(SUM(cost), 0.0) AS total,
                    COALESCE(SUM(CASE WHEN started_at >= ? THEN cost END), 0.0) AS month
             FROM render_jobs",
        )
        .bind(month)
        .fetch_one(&self.0.db.pool)
        .await;
        match row {
            Ok(r) => {
                json!({ "total": r.get::<f64, _>("total"), "month": r.get::<f64, _>("month") })
            }
            Err(_) => json!({ "total": 0.0, "month": 0.0 }),
        }
    }

    /// How much is waiting to be made. Counting art and lines runs the tools, so it's reused
    /// for a minute unless `refresh`.
    pub async fn missing(&self, refresh: bool) -> Missing {
        let mut cache = self.0.missing.lock().await;
        if !refresh
            && let Some((at, m)) = cache.as_ref()
            && at.elapsed() < MISSING_TTL
        {
            let mut m = m.clone();
            m.voice = self.0.voice.backlog();
            return m;
        }
        self.seed_art_dir();
        let c = &self.0.cfg;
        let mut art = Command::new(&c.python);
        art.arg(c.tools_dir.join("art/generate.py"))
            .arg("--plan")
            .arg("--out")
            .arg(&c.art_dir)
            .arg("--library")
            .arg(&c.assets_dir);
        self.with_settings(&mut art);
        let mut lines = self.command(
            Kind::Lines,
            &Options {
                limit: Some(1),
                missing: Some("either".into()),
                ..Default::default()
            },
            Mode::DryRun,
        );
        let (art, lines) = tokio::join!(
            collect(&mut art, Duration::from_secs(60), &c.tools_dir),
            collect(&mut lines, Duration::from_secs(60), &c.tools_dir)
        );
        let mut m = Missing {
            voice: self.0.voice.backlog(),
            ..Default::default()
        };
        match art {
            Ok(out) if out.code == Some(0) => {
                // generate.py --plan ends with "N to make, M up to date".
                m.art = out.lines.iter().rev().find_map(|l| {
                    l.strip_suffix(" up to date")?
                        .split(" to make")
                        .next()?
                        .trim()
                        .parse()
                        .ok()
                });
            }
            Ok(out) => m.errors.push(format!("art: {}", out.last_words())),
            Err(e) => m.errors.push(format!("art: {e}")),
        }
        match lines {
            Ok(out) => match out.events.iter().find(|e| e["event"] == "todo") {
                Some(ev) => m.lines = ev["total"].as_u64().map(|n| n as usize),
                None => m.errors.push(format!("lines: {}", out.last_words())),
            },
            Err(e) => m.errors.push(format!("lines: {e}")),
        }
        *cache = Some((Instant::now(), m.clone()));
        m
    }

    /// Runs the job's dry run, which rents nothing, and returns the price with a token to start it.
    /// Lines for one import: looks up which questions it brought in.
    async fn with_batch(&self, kind: Kind, mut options: Options) -> Result<Options, String> {
        let Some(batch) = options.batch.filter(|_| kind == Kind::Lines) else {
            return Ok(options);
        };
        let details: Option<String> = sqlx::query_scalar(
            "SELECT details FROM render_jobs WHERE id = ? AND kind = 'import' AND state = 'done'",
        )
        .bind(batch)
        .fetch_optional(&self.0.db.pool)
        .await
        .map_err(|e| e.to_string())?
        .flatten();
        let ids: Vec<String> = details
            .and_then(|d| serde_json::from_str::<Value>(&d).ok())
            .and_then(|d| {
                d["imported"]["ids"].as_array().map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .filter(|id| !id.contains(','))
                        .collect()
                })
            })
            .unwrap_or_default();
        if ids.is_empty() {
            return Err("That import has no questions to write lines for.".into());
        }
        options.ids = Some(ids);
        Ok(options)
    }

    pub async fn preview(&self, kind: Kind, options: Options) -> Result<PreviewView, String> {
        if matches!(kind, Kind::Cleanup | Kind::Import) {
            return Err("That job is free, so it needs no price check.".into());
        }
        let options = options.check(kind)?;
        if let Some(why) = self.blocked(kind) {
            return Err(why);
        }
        let options = self.with_batch(kind, options).await?;
        // Cleared however the check ends, even if the page that asked goes away mid-check.
        struct Busy<'a>(&'a Mutex<HashSet<Kind>>, Kind);
        impl Drop for Busy<'_> {
            fn drop(&mut self) {
                self.0.lock().unwrap().remove(&self.1);
            }
        }
        let _busy = {
            let mut busy = self.0.previewing.lock().unwrap();
            if !busy.insert(kind) {
                return Err("That price check is already running.".into());
            }
            Busy(&self.0.previewing, kind)
        };
        let out = self.dry_run(kind, &options).await?;

        let offer = out.events.iter().rev().find(|e| e["event"] == "offer");
        let mut details = json!({});
        for ev in &out.events {
            match ev["event"].as_str() {
                Some("backlog") => details["backlog"] = ev["lines"].clone(),
                Some("todo") => details["todo"] = ev.clone(),
                _ => {}
            }
        }
        let nothing = match kind {
            Kind::Voice => details["backlog"].as_u64() == Some(0),
            _ => details["todo"]["count"].as_u64() == Some(0),
        };
        let ok = out.code == Some(0) && (nothing || offer.is_some() || kind == Kind::Lines);
        let mut view = PreviewView {
            token: None,
            kind,
            options: options.clone(),
            ok,
            nothing,
            gpu: offer.and_then(|o| o["gpu"].as_str().map(|g| format!("{g} {}GB", o["vram_gb"]))),
            price_hr: offer.and_then(|o| o["price_hr"].as_f64()),
            max_cost: offer.and_then(|o| o["max_cost"].as_f64()),
            minutes: offer.and_then(|o| o["minutes"].as_f64()),
            estimate: offer.and_then(estimate),
            details,
            error: (!ok).then(|| out.last_words()),
            log: out.lines.iter().rev().take(40).rev().cloned().collect(),
        };
        if ok && !nothing {
            let token = uuid::Uuid::new_v4().simple().to_string();
            let mut previews = self.0.previews.lock().unwrap();
            previews.retain(|_, p| p.made.elapsed() < PREVIEW_TTL);
            previews.insert(
                token.clone(),
                Preview {
                    kind,
                    options,
                    price_hr: view.price_hr,
                    max_cost: view.max_cost,
                    estimate: view.estimate.clone(),
                    gpu: view.gpu.clone(),
                    made: Instant::now(),
                },
            );
            view.token = Some(token);
        }
        Ok(view)
    }

    async fn dry_run(&self, kind: Kind, options: &Options) -> Result<Output, String> {
        if kind == Kind::Art {
            self.seed_art_dir();
        }
        let mut cmd = self.command(kind, options, Mode::DryRun);
        collect(&mut cmd, PREVIEW_TIMEOUT, &self.0.cfg.tools_dir).await
    }

    /// Starts the job a dry run priced. The token can be used once.
    pub async fn start(&self, token: &str) -> Result<JobView, String> {
        let preview = self
            .0
            .previews
            .lock()
            .unwrap()
            .remove(token)
            .filter(|p| p.made.elapsed() < PREVIEW_TTL)
            .ok_or("That price check has expired. Check the price again.")?;
        // Rent at no more than a little over the price shown, and never over the script's limit,
        // so the most it can cost stays close to what was confirmed.
        let cap = preview.kind.rents().then(|| self.cap(preview.kind));
        let max_price = cap.map(|cap| {
            preview
                .price_hr
                .map_or(cap, |p| ((p * 1.15 + 0.01) * 100.0 - 1e-6).ceil() / 100.0)
                .min(cap)
        });
        let mut details = json!({});
        if let Some(gpu) = preview.gpu {
            details["quoted_gpu"] = gpu.into();
        }
        if let Some(e) = preview.estimate {
            details["estimate"] = e;
        }
        self.launch(
            preview.kind,
            preview.options,
            Mode::Run(max_price),
            preview.max_cost,
            details,
        )
        .await
    }

    /// Imports questions from a trivia site. Free, so it starts straight away.
    pub async fn import(&self, options: Options) -> Result<JobView, String> {
        let options = options.check(Kind::Import)?;
        self.launch(Kind::Import, options, Mode::Run(None), None, json!({}))
            .await
    }

    /// Saves an uploaded pack under content/packs/mine, if the question bank still loads with it,
    /// and records it as an import batch so the pipeline can follow it.
    pub async fn upload_pack(&self, name: &str, text: &str) -> Result<Value, String> {
        let slug = triviabix_questions::slugify(name.trim_end_matches(".toml"));
        if slug.is_empty() || slug.len() > 60 {
            return Err("Give the pack a short name.".into());
        }
        let pack = triviabix_questions::PackFile::from_toml(text)
            .map_err(|e| format!("That isn't a pack file: {e}"))?;
        if pack.questions.is_empty() {
            return Err("The pack has no questions.".into());
        }
        let rel = PathBuf::from(format!("mine/{slug}.toml"));
        let path = self.0.content_dir.join("packs").join(&rel);
        let old = std::fs::read(&path).ok();
        let write = |bytes: &[u8]| -> std::io::Result<()> {
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::fs::write(&path, bytes)
        };
        write(text.as_bytes()).map_err(|e| format!("Couldn't save the pack: {e}"))?;
        // The whole bank has to load with it, or the server would stop seeing new questions.
        let bank = match triviabix_questions::QuestionBank::load_dir(&self.0.content_dir) {
            Ok(bank) => bank,
            Err(e) => {
                match &old {
                    Some(bytes) => {
                        let _ = write(bytes);
                    }
                    None => {
                        let _ = std::fs::remove_file(&path);
                    }
                }
                return Err(format!("Not saved: {e}"));
            }
        };
        let ids: Vec<&str> = bank
            .questions()
            .iter()
            .filter(|q| bank.pack_of(&q.id) == Some(rel.as_path()))
            .map(|q| q.id.as_str())
            .collect();
        let imported = json!({
            "event": "imported", "source": "upload", "fetched": pack.questions.len(),
            "added": ids.len(), "pack": Path::new("packs").join(&rel), "ids": ids,
            "replaced": old.is_some(),
        });
        let now = now_ms() as i64;
        let log = format!(
            "Saved {} questions to packs/{}{}",
            ids.len(),
            rel.display(),
            if old.is_some() {
                ", replacing the old file"
            } else {
                ""
            }
        );
        sqlx::query(
            "INSERT INTO render_jobs (kind, options, state, started_at, finished_at, exit_code, details, log)
             VALUES ('import', '{\"source\":\"upload\"}', 'done', ?, ?, 0, ?, ?)",
        )
        .bind(now)
        .bind(now)
        .bind(json!({ "imported": imported }).to_string())
        .bind(&log)
        .execute(&self.0.db.pool)
        .await
        .map_err(|e| format!("Saved, but couldn't record it: {e}"))?;
        tracing::info!("admin uploaded a pack: {log}");
        *self.0.missing.lock().await = None;
        let _ = self.0.finished.send(Kind::Import);
        Ok(json!({ "ok": true, "message": log, "warnings": bank.warnings().len() }))
    }

    /// The recent imports, each with how far its questions have got: in the game, with host
    /// lines, and voiced (the question and all its lines).
    pub async fn batches(&self, host_name: &str) -> Vec<Value> {
        let rows = sqlx::query(
            "SELECT id, started_at, state, options, details FROM render_jobs
             WHERE kind = 'import' ORDER BY id DESC LIMIT 20",
        )
        .fetch_all(&self.0.db.pool)
        .await
        .unwrap_or_default();
        let mut out = Vec::new();
        for row in rows {
            let details: Option<String> = row.get("details");
            let details: Value = details
                .and_then(|d| serde_json::from_str(&d).ok())
                .unwrap_or_default();
            let options: String = row.get("options");
            let imported = &details["imported"];
            let ids: Vec<String> = imported["ids"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let progress = self.progress_of(&ids, host_name).await;
            out.push(json!({
                "id": row.get::<i64, _>("id"),
                "started_at": row.get::<i64, _>("started_at"),
                "state": row.get::<String, _>("state"),
                "options": serde_json::from_str::<Value>(&options).unwrap_or_default(),
                "imported": imported,
                "in_game": progress.0,
                "with_lines": progress.1,
                "voiced": progress.2,
            }));
        }
        out
    }

    /// Of these questions (bank ids): how many the game has, how many have both a lead-in and a
    /// reveal line, and how many have audio for the question and every line.
    async fn progress_of(&self, ids: &[String], host_name: &str) -> (usize, usize, usize) {
        if ids.is_empty() {
            return (0, 0, 0);
        }
        let marks = vec!["?"; ids.len()].join(",");
        let sql = format!(
            "SELECT q.id, q.prompt,
                    EXISTS (SELECT 1 FROM question_lines l WHERE l.question_id = q.id
                            AND l.slot = 'lead_in' AND l.text != '') AS lead_in,
                    EXISTS (SELECT 1 FROM question_lines l WHERE l.question_id = q.id
                            AND l.slot = 'reveal' AND l.text != '') AS reveal
             FROM questions q WHERE q.active = 1 AND q.bank_id IN ({marks})"
        );
        // Only "?" placeholders are formatted in; the ids are bound.
        let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
        for id in ids {
            query = query.bind(id);
        }
        let Ok(rows) = query.fetch_all(&self.0.db.pool).await else {
            return (0, 0, 0);
        };
        let voice = &self.0.voice;
        let (mut lines, mut voiced) = (0, 0);
        for row in &rows {
            let qid: i64 = row.get("id");
            if row.get::<bool, _>("lead_in") && row.get::<bool, _>("reveal") {
                lines += 1;
            }
            let texts: Vec<String> = sqlx::query_scalar(
                "SELECT text FROM question_lines WHERE question_id = ? AND text != ''",
            )
            .bind(qid)
            .fetch_all(&self.0.db.pool)
            .await
            .unwrap_or_default();
            let prompt: String = row.get("prompt");
            if voice.is_ready(&prompt)
                && texts
                    .iter()
                    .all(|t| voice.is_ready(&t.replace("{host}", host_name)))
            {
                voiced += 1;
            }
        }
        (rows.len(), lines, voiced)
    }

    /// Destroys every TriviaBix instance on the vast.ai account.
    pub async fn cleanup(&self) -> Result<JobView, String> {
        self.launch(
            Kind::Cleanup,
            Options::default(),
            Mode::Run(None),
            None,
            json!({}),
        )
        .await
    }

    async fn launch(
        &self,
        kind: Kind,
        options: Options,
        mode: Mode,
        max_cost: Option<f64>,
        details: Value,
    ) -> Result<JobView, String> {
        if kind == Kind::Art {
            self.seed_art_dir();
        }
        let started_at = now_ms();
        let id: i64 = sqlx::query(
            "INSERT INTO render_jobs (kind, options, state, started_at, max_cost)
             VALUES (?, ?, 'running', ?, ?) RETURNING id",
        )
        .bind(kind.as_str())
        .bind(serde_json::to_string(&options).unwrap())
        .bind(started_at as i64)
        .bind(max_cost)
        .fetch_one(&self.0.db.pool)
        .await
        .map_err(|e| format!("Couldn't record the job: {e}"))?
        .get("id");

        let mut cmd = self.command(kind, &options, mode);
        cmd.current_dir(&self.0.cfg.tools_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        {
            // Checked again under the lock it's added with, so two starts can't both get through.
            let mut jobs = self.0.jobs.lock().unwrap();
            if let Some(why) = blocked(&jobs, kind) {
                drop(jobs);
                self.forget(id);
                return Err(why);
            }
            let mut job = Job {
                view: JobView {
                    id,
                    kind,
                    options,
                    state: "running".into(),
                    started_at,
                    finished_at: None,
                    exit_code: None,
                    gpu: None,
                    price_hr: None,
                    max_cost,
                    cost: None,
                    instance: None,
                    released: true,
                    cancelling: false,
                    details,
                    log: vec![],
                    log_seq: 0,
                },
                log: VecDeque::new(),
                pgid: None,
                rented_at: None,
            };
            let mut child = match cmd.spawn() {
                Ok(child) => child,
                Err(e) => {
                    drop(jobs);
                    self.forget(id);
                    return Err(format!("Couldn't start the job: {e}"));
                }
            };
            job.pgid = child.id().map(|p| p as i32);
            tracing::info!(id, kind = kind.as_str(), "render job started");
            let (tx, rx) = mpsc::unbounded_channel();
            pump(child.stdout.take(), tx.clone());
            pump(child.stderr.take(), tx);
            let view = job.snapshot(0);
            // Finished jobs of the same kind make way; their logs are in the history.
            jobs.retain(|j| j.view.state == "running" || j.view.kind != kind);
            jobs.push(job);
            let this = self.clone();
            tokio::spawn(async move { this.watch(id, child, rx).await });
            Ok(view)
        }
    }

    fn forget(&self, id: i64) {
        let db = self.0.db.clone();
        tokio::spawn(async move {
            let _ = sqlx::query("DELETE FROM render_jobs WHERE id = ?")
                .bind(id)
                .execute(&db.pool)
                .await;
        });
    }

    /// Follows a running job's output until it exits, then records how it went.
    async fn watch(
        &self,
        id: i64,
        mut child: tokio::process::Child,
        mut rx: mpsc::UnboundedReceiver<String>,
    ) {
        // Both pipes close only once every process in the job is done, including a vast.py the
        // art job started, so this waits for the instance to be destroyed.
        while let Some(line) = rx.recv().await {
            let rented = self.with_job(id, |job| match line.strip_prefix("@tbx ") {
                Some(ev) => match serde_json::from_str::<Value>(ev) {
                    Ok(ev) => {
                        job.event(&ev);
                        (ev["event"] == "rented").then_some(job.view.instance)
                    }
                    Err(_) => {
                        job.push(line);
                        None
                    }
                },
                None => {
                    job.push(line);
                    None
                }
            });
            let Some(rented) = rented else { break };
            // Saved straight away, so a restart can still find and destroy the instance.
            if let Some(Some(instance)) = rented {
                let _ =
                    sqlx::query("UPDATE render_jobs SET instance = ?, released = 0 WHERE id = ?")
                        .bind(instance)
                        .bind(id)
                        .execute(&self.0.db.pool)
                        .await;
            }
        }
        let code = child.wait().await.ok().and_then(|s| s.code());

        // The tools always destroy what they rent; this is the backstop for a crash or a kill.
        let leftover = self
            .with_job(id, |j| j.view.instance.filter(|_| !j.view.released))
            .flatten();
        if let Some(instance) = leftover
            && self.destroy(instance, Some(id)).await
        {
            self.with_job(id, |job| {
                if let (Some(at), Some(price)) = (job.rented_at.take(), job.view.price_hr) {
                    let cost = price * at.elapsed().as_secs_f64() / 3600.0;
                    job.view.cost = Some(job.view.cost.unwrap_or(0.0) + cost);
                }
                job.view.released = true;
            });
        }

        let Some((view, log)) = self.with_job(id, |job| {
            job.view.exit_code = code;
            job.view.finished_at = Some(now_ms());
            job.view.state = if job.view.cancelling {
                "cancelled"
            } else if code == Some(0) {
                "done"
            } else {
                "failed"
            }
            .into();
            job.pgid = None;
            let log: Vec<&str> = job.log.iter().map(|(_, l)| l.as_str()).collect();
            (job.view.clone(), log.join("\n"))
        }) else {
            return;
        };
        tracing::info!(id, state = %view.state, cost = ?view.cost, "render job finished");
        let saved = sqlx::query(
            "UPDATE render_jobs SET state = ?, finished_at = ?, exit_code = ?, gpu = ?,
                    price_hr = ?, cost = ?, instance = ?, released = ?, details = ?, log = ?
             WHERE id = ?",
        )
        .bind(&view.state)
        .bind(view.finished_at.map(|t| t as i64))
        .bind(view.exit_code)
        .bind(&view.gpu)
        .bind(view.price_hr)
        .bind(view.cost)
        .bind(view.instance)
        .bind(view.released)
        .bind(view.details.to_string())
        .bind(log)
        .bind(id)
        .execute(&self.0.db.pool)
        .await;
        if let Err(e) = saved {
            tracing::error!("saving render job {id}: {e:#}");
        }
        *self.0.missing.lock().await = None;
        if view.state == "done" {
            let _ = self.0.finished.send(view.kind);
        }
    }

    /// Cancels the running job. The tools destroy their instance on SIGTERM; one that hasn't
    /// exited after a grace period is killed, and its instance destroyed by id.
    pub fn cancel(&self, id: i64) -> Result<(), String> {
        let pgid = self
            .with_job(id, |job| {
                if job.view.state != "running" {
                    return Err("That job isn't running.");
                }
                job.view.cancelling = true;
                job.push("[admin] cancelled; stopping and giving the GPU back".into());
                job.pgid.ok_or("That job is already stopping.")
            })
            .unwrap_or(Err("That job isn't running."))?;
        tracing::info!(id, "render job cancelled");
        signal_group(pgid, libc::SIGTERM);
        let this = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(CANCEL_GRACE).await;
            let still = this.with_job(id, |j| j.pgid).flatten();
            if let Some(pgid) = still {
                tracing::warn!(id, "render job ignored the stop; killing it");
                signal_group(pgid, libc::SIGKILL);
            }
        });
        Ok(())
    }

    /// Destroys one instance with `vast.py destroy`. Returns whether it's gone.
    async fn destroy(&self, instance: i64, job: Option<i64>) -> bool {
        let c = &self.0.cfg;
        let mut cmd = Command::new(&c.python);
        cmd.arg(c.tools_dir.join("gpu/vast.py"))
            .arg("destroy")
            .arg(instance.to_string());
        self.with_settings(&mut cmd);
        let out = collect(&mut cmd, Duration::from_secs(120), &c.tools_dir).await;
        let ok = matches!(&out, Ok(o) if o.code == Some(0));
        let said = match &out {
            Ok(o) => o.last_words(),
            Err(e) => e.clone(),
        };
        if ok {
            tracing::info!("destroyed leftover instance {instance}: {said}");
        } else {
            tracing::error!(
                "COULDN'T DESTROY vast.ai instance {instance}: {said}. Destroy it at https://cloud.vast.ai/instances/"
            );
        }
        if let Some(id) = job {
            self.with_job(id, |j| {
                j.push(if ok {
                    format!("[admin] destroyed instance {instance}")
                } else {
                    format!("[admin] COULDN'T DESTROY instance {instance}: {said}. Use Destroy leftovers, or https://cloud.vast.ai/instances/")
                });
            });
        }
        ok
    }

    fn command(&self, kind: Kind, o: &Options, mode: Mode) -> Command {
        let c = &self.0.cfg;
        let tool = |rel: &str| {
            let mut cmd = Command::new(&c.python);
            cmd.arg(c.tools_dir.join(rel));
            cmd
        };
        let mut cmd = match kind {
            Kind::Voice => {
                let mut cmd = tool("gpu/vast.py");
                cmd.args(["voice", "--server", &c.server_url, "--max-hours"])
                    .arg(o.max_hours.unwrap_or(VOICE_MAX_HOURS).to_string())
                    .arg("--library")
                    .arg(&c.assets_dir);
                cmd
            }
            Kind::Design => {
                let mut cmd = tool("gpu/vast.py");
                cmd.arg("design")
                    .arg("--library")
                    .arg(&c.assets_dir)
                    .args([
                        "--description",
                        o.description.as_deref().unwrap_or(VOICE_DESCRIPTION),
                    ])
                    .arg("--count")
                    .arg(o.count.unwrap_or(4).to_string());
                for line in o.lines.iter().flatten() {
                    cmd.args(["--text", line]);
                }
                cmd
            }
            Kind::Art => {
                let mut cmd = tool("art/render_on_vast.py");
                cmd.arg("--out")
                    .arg(&c.art_dir)
                    .arg("--library")
                    .arg(&c.assets_dir);
                if let Some(k) = &o.kinds {
                    cmd.args(["--kinds", k]);
                }
                cmd
            }
            Kind::Lines => {
                let mut cmd = Command::new(&c.questions_bin);
                let persona = self.0.persona.lock().unwrap().clone();
                let into = match &persona.folder {
                    // A persona's own folder, and only what it hasn't said itself counts as missing.
                    Some(folder) => {
                        cmd.env("SCRIPTS_PERSONA", folder);
                        folder.clone()
                    }
                    None => {
                        cmd.env_remove("SCRIPTS_PERSONA");
                        PathBuf::from(LINES_FOLDER)
                    }
                };
                cmd.arg("--content")
                    .arg(&self.0.content_dir)
                    .args(["scripts", "generate", "--into"])
                    .arg(into)
                    .arg("--limit")
                    .arg(o.limit.unwrap_or(50).to_string())
                    .args(["--missing", o.missing.as_deref().unwrap_or("either")]);
                if let Some(ids) = &o.ids {
                    cmd.arg("--ids").arg(ids.join(","));
                }
                let settings = &self.0.settings;
                if let Some(model) = settings.get("CLAUDE_MODEL") {
                    cmd.args(["--model", &model]);
                }
                if let Some(effort) = settings.get("CLAUDE_EFFORT") {
                    cmd.args(["--effort", &effort]);
                }
                cmd
            }
            Kind::Cleanup => {
                let mut cmd = tool("gpu/vast.py");
                cmd.arg("cleanup");
                cmd
            }
            Kind::Import => {
                let mut cmd = Command::new(&c.questions_bin);
                cmd.arg("--content").arg(&self.0.content_dir).arg("import");
                let opentdb = o.source.as_deref() != Some("trivia-api");
                cmd.arg(if opentdb { "opentdb" } else { "trivia-api" })
                    .arg("--amount")
                    .arg(o.amount.unwrap_or(20).to_string());
                if let Some(c) = &o.category {
                    cmd.args(["--category", c]);
                }
                if let Some(d) = &o.difficulty {
                    cmd.args(["--difficulty", d]);
                }
                cmd
            }
        };
        match mode {
            Mode::DryRun => {
                cmd.arg("--dry-run");
                // Price the GPU the job would be allowed to rent.
                if kind.rents() {
                    cmd.arg("--max-price").arg(format!("{:.2}", self.cap(kind)));
                }
            }
            Mode::Run(max_price) => {
                if kind.rents() {
                    cmd.arg("--yes");
                }
                if let Some(p) = max_price {
                    cmd.arg("--max-price").arg(format!("{p:.2}"));
                }
            }
        }
        cmd.env("TBX_EVENTS", "1")
            .env("PYTHONUNBUFFERED", "1")
            .env("ASSETS_DIR", &c.assets_dir);
        self.with_settings(&mut cmd);
        cmd
    }

    /// The TV's art is served from `art_dir` once it has a manifest. Before the first art job
    /// writes there, it starts as a copy of the art that ships with the server.
    fn seed_art_dir(&self) {
        let dir = &self.0.cfg.art_dir;
        let shipped = self.0.static_dir.join("cutscenes/assets/gen");
        if dir.join("manifest.json").exists() || !shipped.join("manifest.json").exists() {
            return;
        }
        if let Err(e) = copy_dir(&shipped, dir) {
            tracing::warn!("copying the shipped art to {}: {e}", dir.display());
        }
    }
}

/// An asset store key: 32 hex digits.
fn is_key(key: &str) -> bool {
    key.len() == 32 && key.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let dest = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &dest)?;
        } else {
            std::fs::copy(entry.path(), dest)?;
        }
    }
    Ok(())
}

fn view_of(row: &sqlx::sqlite::SqliteRow) -> JobView {
    let kind: String = row.get("kind");
    let options: String = row.get("options");
    let details: Option<String> = row.get("details");
    JobView {
        id: row.get("id"),
        kind: Kind::parse(&kind).unwrap_or(Kind::Cleanup),
        options: serde_json::from_str(&options).unwrap_or_default(),
        state: row.get("state"),
        started_at: row.get::<i64, _>("started_at") as u64,
        finished_at: row.get::<Option<i64>, _>("finished_at").map(|t| t as u64),
        exit_code: row.get("exit_code"),
        gpu: row.get("gpu"),
        price_hr: row.get("price_hr"),
        max_cost: row.get("max_cost"),
        cost: row.get("cost"),
        instance: row.get("instance"),
        released: row.get("released"),
        cancelling: false,
        details: details
            .and_then(|d| serde_json::from_str(&d).ok())
            .unwrap_or_else(|| json!({})),
        log: vec![],
        log_seq: 0,
    }
}

/// Sends each line a pipe gives to `tx`.
fn pump<R: tokio::io::AsyncRead + Unpin + Send + 'static>(
    pipe: Option<R>,
    tx: mpsc::UnboundedSender<String>,
) {
    let Some(pipe) = pipe else { return };
    tokio::spawn(async move {
        let mut lines = BufReader::new(pipe).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
}

/// A finished short run of a tool.
struct Output {
    code: Option<i32>,
    lines: Vec<String>,
    events: Vec<Value>,
}

impl Output {
    /// The last thing the tool said, which is why it failed when it did.
    fn last_words(&self) -> String {
        self.lines
            .iter()
            .rev()
            .find(|l| !l.trim().is_empty())
            .map(|l| l.trim_start_matches("[vast] ").to_string())
            .unwrap_or_else(|| format!("exited with {:?}", self.code))
    }
}

/// Runs a tool to the end (or `timeout`), keeping its output and events.
async fn collect(cmd: &mut Command, timeout: Duration, dir: &Path) -> Result<Output, String> {
    cmd.current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("TBX_EVENTS", "1")
        .env("PYTHONUNBUFFERED", "1")
        .kill_on_drop(true)
        .process_group(0);
    let mut child = cmd.spawn().map_err(|e| format!("couldn't run it: {e}"))?;
    let pgid = child.id().map(|p| p as i32);
    let (tx, mut rx) = mpsc::unbounded_channel();
    pump(child.stdout.take(), tx.clone());
    pump(child.stderr.take(), tx);
    let run = async {
        let mut lines = Vec::new();
        let mut events = Vec::new();
        while let Some(line) = rx.recv().await {
            match line
                .strip_prefix("@tbx ")
                .and_then(|e| serde_json::from_str(e).ok())
            {
                Some(ev) => events.push(ev),
                None => {
                    lines.push(line);
                    if lines.len() > LOG_LINES {
                        lines.remove(0);
                    }
                }
            }
        }
        let code = child.wait().await.ok().and_then(|s| s.code());
        Output {
            code,
            lines,
            events,
        }
    };
    match tokio::time::timeout(timeout, run).await {
        Ok(out) => Ok(out),
        Err(_) => {
            if let Some(pgid) = pgid {
                signal_group(pgid, libc::SIGTERM);
            }
            Err(format!("it took over {} seconds", timeout.as_secs()))
        }
    }
}
