//! Loading a content folder, validating it, and picking questions for a game.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

use rand::seq::SliceRandom;
use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::dupes::{DuplicateIndex, DuplicatePair, NotDuplicates, Verdict};
use crate::judge::normalize;
use crate::model::*;
use crate::pack::PackFile;
use crate::script::{validate_line, ScriptFile};

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("could not read {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not parse {path}: {message}")]
    Parse { path: PathBuf, message: String },
    #[error("{} problem(s) in the question bank:\n{}", .0.len(), .0.iter().map(|p| format!("  - {p}")).collect::<Vec<_>>().join("\n"))]
    Invalid(Vec<Problem>),
}

/// One validation problem, with where it was found.
#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    pub location: String,
    pub message: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.location, self.message)
    }
}

/// The file, next to `categories.toml`, listing look-alike questions to keep anyway.
pub const NOT_DUPLICATES: &str = "not-duplicates.toml";

#[derive(Deserialize)]
struct CategoriesFile {
    #[serde(rename = "category")]
    categories: Vec<Category>,
}

/// Which questions a round may use.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Filter {
    /// Category slugs or tags, such as `80s` or `music`. A question matches if its category
    /// or any of its tags is listed. Empty means every topic.
    #[serde(default)]
    pub topics: Vec<String>,
    /// Empty means every difficulty.
    #[serde(default)]
    pub difficulties: Vec<Difficulty>,
    /// Empty means every question type.
    #[serde(default)]
    pub types: Vec<QuestionType>,
    /// The highest rating allowed in the room. Defaults to PG (family mode).
    #[serde(default)]
    pub max_rating: Rating,
    /// Questions already played in this room or recently.
    #[serde(default)]
    pub exclude_ids: HashSet<String>,
}

impl Filter {
    pub fn matches(&self, q: &Question) -> bool {
        q.rating <= self.max_rating
            && !self.exclude_ids.contains(&q.id)
            && (self.difficulties.is_empty() || self.difficulties.contains(&q.difficulty))
            && (self.types.is_empty() || self.types.contains(&q.question_type()))
            && (self.topics.is_empty()
                || self
                    .topics
                    .iter()
                    .any(|t| *t == q.category || q.tags.contains(t)))
    }
}

#[derive(Debug, Clone, Default)]
pub struct QuestionBank {
    categories: Vec<Category>,
    questions: Vec<Question>,
    by_id: HashMap<String, usize>,
    /// Which pack file each question came from, relative to `packs/`.
    packs: HashMap<String, PathBuf>,
    /// Things worth fixing that don't stop the bank from loading, like a script for a
    /// question that no longer exists.
    warnings: Vec<Problem>,
    /// Look-alike pairs a person checked and kept, from `not-duplicates.toml`.
    kept: NotDuplicates,
}

impl QuestionBank {
    /// Load `<dir>/categories.toml` and every `.toml` and `.json` pack under `<dir>/packs`.
    pub fn load_dir(dir: impl AsRef<Path>) -> Result<Self, LoadError> {
        let dir = dir.as_ref();
        let cat_path = dir.join("categories.toml");
        let text = read(&cat_path)?;
        let cats: CategoriesFile = toml::from_str(&text).map_err(|e| LoadError::Parse {
            path: cat_path.clone(),
            message: e.to_string(),
        })?;

        let packs_dir = dir.join("packs");
        let mut files = Vec::new();
        collect_pack_files(&packs_dir, &mut files)?;
        files.sort();

        let mut questions = Vec::new();
        let mut packs = HashMap::new();
        for path in files {
            let rel = path.strip_prefix(&packs_dir).unwrap_or(&path).to_path_buf();
            for q in load_pack(&path)? {
                packs.insert(q.id.clone(), rel.clone());
                questions.push(q);
            }
        }
        let mut bank = Self::new(cats.categories, questions).map_err(LoadError::Invalid)?;
        bank.packs = packs;
        bank.load_scripts(&dir.join("scripts"))?;
        let kept_path = dir.join(NOT_DUPLICATES);
        bank.kept = NotDuplicates::read(&kept_path).map_err(|message| LoadError::Parse {
            path: kept_path,
            message,
        })?;
        for pair in bank.duplicates() {
            let what = match pair.verdict {
                Verdict::Duplicate => "looks like the same question as",
                Verdict::Possible => "might be the same question as",
            };
            let other = bank.get(&pair.b).map_or("", |q| q.prompt.as_str());
            bank.warnings.push(problem(
                &format!("question {}", pair.a),
                &format!(
                    "{what} {} ({other:?}); remove one, or list the pair in {NOT_DUPLICATES} to keep both",
                    pair.b
                ),
            ));
        }
        Ok(bank)
    }

    /// Attach every script file under `dir` to the questions it names.
    fn load_scripts(&mut self, dir: &Path) -> Result<(), LoadError> {
        let mut files = Vec::new();
        collect_pack_files(dir, &mut files)?;
        files.sort();
        let mut problems = Vec::new();
        for path in files {
            let file = ScriptFile::read(&path).map_err(|message| LoadError::Parse {
                path: path.clone(),
                message,
            })?;
            let rel = path
                .strip_prefix(dir)
                .unwrap_or(&path)
                .display()
                .to_string();
            for entry in file.scripts {
                let at = format!("script for {} in {rel}", entry.question);
                let Some(&i) = self.by_id.get(&entry.question) else {
                    self.warnings
                        .push(problem(&at, "no question has this id; was it reworded?"));
                    continue;
                };
                let q = &mut self.questions[i];
                if !entry.matches_prompt(q) {
                    self.warnings.push(problem(
                        &at,
                        &format!(
                            "was written for a different prompt; it now reads {:?}",
                            q.prompt
                        ),
                    ));
                }
                for line in entry.lines(q.rating) {
                    for p in validate_line(&line) {
                        problems.push(problem(&at, &p));
                    }
                    q.script.push(line);
                }
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(LoadError::Invalid(problems))
        }
    }

    /// The pack file a question came from, relative to `packs/`. Only known for banks
    /// loaded with [`QuestionBank::load_dir`].
    pub fn pack_of(&self, id: &str) -> Option<&Path> {
        self.packs.get(id).map(PathBuf::as_path)
    }

    pub fn warnings(&self) -> &[Problem] {
        &self.warnings
    }

    /// The look-alike pairs a person checked and kept.
    pub fn not_duplicates(&self) -> &NotDuplicates {
        &self.kept
    }

    /// An index of every question, for checking new ones against the bank. Kept pairs stay kept.
    pub fn duplicate_index(&self) -> DuplicateIndex {
        let mut index = DuplicateIndex::of(&self.questions);
        for (a, b) in self.kept.ids() {
            index.keep_pair(a, b);
        }
        index
    }

    /// Questions that look like each other, surest first.
    pub fn duplicates(&self) -> Vec<DuplicatePair> {
        self.duplicate_index().pairs()
    }

    /// Build a bank from parts, validating everything.
    pub fn new(
        categories: Vec<Category>,
        mut questions: Vec<Question>,
    ) -> Result<Self, Vec<Problem>> {
        let mut problems = Vec::new();
        let mut cat_slugs = HashSet::new();
        for c in &categories {
            if !cat_slugs.insert(c.slug.clone()) {
                problems.push(problem(
                    &format!("category {}", c.slug),
                    "duplicate category slug",
                ));
            }
        }
        let min_rating: HashMap<&str, Rating> = categories
            .iter()
            .map(|c| (c.slug.as_str(), c.rating))
            .collect();

        let mut by_id = HashMap::new();
        let mut prompts: HashMap<String, String> = HashMap::new();
        for (i, q) in questions.iter_mut().enumerate() {
            if let Some(&floor) = min_rating.get(q.category.as_str()) {
                q.rating = q.rating.max(floor);
            }
            let at = format!("question {}", q.id);
            if !cat_slugs.contains(&q.category) {
                problems.push(problem(&at, &format!("unknown category {:?}", q.category)));
            }
            if by_id.insert(q.id.clone(), i).is_some() {
                problems.push(problem(&at, "duplicate id"));
            }
            if let Some(other) = prompts.insert(normalize(&q.prompt), q.id.clone()) {
                problems.push(problem(&at, &format!("same prompt as {other}")));
            }
            for p in validate_question(q) {
                problems.push(problem(&at, &p));
            }
        }
        if problems.is_empty() {
            Ok(QuestionBank {
                categories,
                questions,
                by_id,
                packs: HashMap::new(),
                warnings: Vec::new(),
                kept: NotDuplicates::default(),
            })
        } else {
            Err(problems)
        }
    }

    pub fn categories(&self) -> &[Category] {
        &self.categories
    }

    pub fn questions(&self) -> &[Question] {
        &self.questions
    }

    pub fn get(&self, id: &str) -> Option<&Question> {
        self.by_id.get(id).map(|&i| &self.questions[i])
    }

    /// How many questions a filter can draw from. Handy for hiding empty categories on the TV.
    pub fn count(&self, filter: &Filter) -> usize {
        self.questions.iter().filter(|q| filter.matches(q)).count()
    }

    /// Pick up to `count` random questions. Questions are dealt round robin across
    /// categories so a mixed round doesn't end up all geography.
    pub fn pick<R: Rng + ?Sized>(
        &self,
        filter: &Filter,
        count: usize,
        rng: &mut R,
    ) -> Vec<&Question> {
        let mut groups: BTreeMap<&str, Vec<&Question>> = BTreeMap::new();
        for q in self.questions.iter().filter(|q| filter.matches(q)) {
            groups.entry(q.category.as_str()).or_default().push(q);
        }
        let mut groups: Vec<Vec<&Question>> = groups.into_values().collect();
        for g in &mut groups {
            g.shuffle(rng);
        }
        groups.shuffle(rng);

        let mut picked = Vec::with_capacity(count);
        while picked.len() < count {
            let mut any = false;
            for g in &mut groups {
                if picked.len() == count {
                    break;
                }
                if let Some(q) = g.pop() {
                    picked.push(q);
                    any = true;
                }
            }
            if !any {
                break;
            }
        }
        picked
    }

    /// Question counts by category and type, for the `stats` command and an admin page.
    pub fn stats(&self) -> Stats {
        let mut s = Stats::default();
        for q in &self.questions {
            s.total += 1;
            *s.by_category.entry(q.category.clone()).or_default() += 1;
            *s.by_type
                .entry(q.question_type().as_str().to_string())
                .or_default() += 1;
            *s.by_difficulty
                .entry(q.difficulty.as_str().to_string())
                .or_default() += 1;
            *s.by_rating
                .entry(q.rating.as_str().to_string())
                .or_default() += 1;
            for t in &q.tags {
                *s.by_tag.entry(t.clone()).or_default() += 1;
            }
            if q.has_lines(ScriptSlot::LeadIn) {
                s.with_lead_in += 1;
            }
            if q.has_lines(ScriptSlot::Reveal) {
                s.with_reveal += 1;
            }
        }
        s
    }
}

#[derive(Serialize, Debug, Default)]
pub struct Stats {
    pub total: usize,

    pub by_category: BTreeMap<String, usize>,
    pub by_type: BTreeMap<String, usize>,
    pub by_difficulty: BTreeMap<String, usize>,
    pub by_rating: BTreeMap<String, usize>,
    pub by_tag: BTreeMap<String, usize>,
    /// Questions the host can set up with their own lead-in.
    pub with_lead_in: usize,
    /// Questions with their own joke or fun fact for the reveal.
    pub with_reveal: usize,
}

fn problem(location: &str, message: &str) -> Problem {
    Problem {
        location: location.to_string(),
        message: message.to_string(),
    }
}

/// Problems with a single question, independent of the rest of the bank.
pub fn validate_question(q: &Question) -> Vec<String> {
    let mut out = Vec::new();
    if q.id.trim().is_empty() {
        out.push("empty id".into());
    }
    if q.prompt.trim().is_empty() {
        out.push("empty prompt".into());
    }
    match &q.kind {
        QuestionKind::MultipleChoice {
            choices, answer, ..
        } => {
            if !(2..=6).contains(&choices.len()) {
                out.push(format!(
                    "multiple choice needs 2 to 6 choices, has {}",
                    choices.len()
                ));
            }
            if !choices.iter().any(|c| c.trim() == answer.trim()) {
                out.push(format!("answer {answer:?} is not one of the choices"));
            }
            let unique: HashSet<String> = choices.iter().map(|c| normalize(c)).collect();
            if unique.len() != choices.len() {
                out.push("choices repeat".into());
            }
        }
        QuestionKind::TrueFalse { .. } => {}
        QuestionKind::TypeAnswer { answers } => {
            if answers.iter().all(|a| normalize(a).is_empty()) {
                out.push("type answer needs at least one accepted answer".into());
            }
        }
        QuestionKind::ClosestNumber {
            answer, tolerance, ..
        } => {
            if !answer.is_finite() {
                out.push("answer must be a number".into());
            }
            if *tolerance < 0.0 {
                out.push("tolerance can't be negative".into());
            }
        }
        QuestionKind::PutInOrder { items, .. } => {
            if !(3..=6).contains(&items.len()) {
                out.push(format!(
                    "put in order needs 3 to 6 items, has {}",
                    items.len()
                ));
            }
            let unique: HashSet<&str> = items.iter().map(|i| i.trim()).collect();
            if unique.len() != items.len() {
                out.push("items repeat".into());
            }
        }
    }
    for line in q.host_lines() {
        out.extend(validate_line(&line));
    }
    out
}

fn read(path: &Path) -> Result<String, LoadError> {
    std::fs::read_to_string(path).map_err(|source| LoadError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Parse one pack file and apply its defaults.
pub fn load_pack(path: &Path) -> Result<Vec<Question>, LoadError> {
    let text = read(path)?;
    let parsed = if path.extension().is_some_and(|e| e == "json") {
        PackFile::from_json(&text)
    } else {
        PackFile::from_toml(&text)
    };
    let pack = parsed.map_err(|message| LoadError::Parse {
        path: path.to_path_buf(),
        message,
    })?;
    let key = path.file_stem().and_then(|s| s.to_str()).unwrap_or("pack");
    Ok(pack.resolve(key))
}

fn collect_pack_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), LoadError> {
    if !dir.exists() {
        return Ok(());
    }
    let entries = std::fs::read_dir(dir).map_err(|source| LoadError::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let path = entry
            .map_err(|source| LoadError::Io {
                path: dir.to_path_buf(),
                source,
            })?
            .path();
        if path.is_dir() {
            collect_pack_files(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "toml" || e == "json") {
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cat(slug: &str) -> Category {
        Category {
            slug: slug.into(),
            name: slug.into(),
            emoji: String::new(),
            color: String::new(),
            description: String::new(),
            rating: if slug == "after-dark" {
                Rating::Pg13
            } else {
                Rating::Pg
            },
        }
    }

    fn tf(id: &str, category: &str, tags: &[&str]) -> Question {
        Question {
            id: id.into(),
            category: category.into(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            difficulty: Difficulty::Easy,
            rating: Rating::Pg,
            prompt: format!("prompt {id}"),
            kind: QuestionKind::TrueFalse { answer: true },
            fun_fact: None,
            host_quip: None,
            script: vec![],
            source: None,
        }
    }

    fn bank() -> QuestionBank {
        QuestionBank::new(
            vec![cat("80s"), cat("music"), cat("after-dark")],
            vec![
                tf("a", "80s", &[]),
                tf("b", "music", &["80s"]),
                tf("c", "music", &[]),
                tf("d", "after-dark", &[]),
            ],
        )
        .unwrap()
    }

    #[test]
    fn topics_match_category_or_tag() {
        let b = bank();
        let f = Filter {
            topics: vec!["80s".into()],
            ..Default::default()
        };
        assert_eq!(b.count(&f), 2);
    }

    #[test]
    fn category_rating_floor_applies_and_rooms_filter_by_rating() {
        let b = bank();
        assert_eq!(b.get("d").unwrap().rating, Rating::Pg13);
        assert_eq!(b.count(&Filter::default()), 3);
        assert_eq!(
            b.count(&Filter {
                max_rating: Rating::Pg13,
                ..Default::default()
            }),
            4
        );
        assert_eq!(
            b.count(&Filter {
                max_rating: Rating::Nc17,
                ..Default::default()
            }),
            4
        );
    }

    #[test]
    fn pick_respects_count_and_exclusions() {
        let b = bank();
        let mut rng = rand::thread_rng();
        let f = Filter {
            exclude_ids: ["a".to_string()].into(),
            ..Default::default()
        };
        let picked = b.pick(&f, 10, &mut rng);
        assert_eq!(picked.len(), 2);
        assert!(picked.iter().all(|q| q.id != "a"));
        assert_eq!(b.pick(&Filter::default(), 1, &mut rng).len(), 1);
    }

    #[test]
    fn validation_catches_mistakes() {
        let mut bad = tf("x", "nope", &[]);
        bad.kind = QuestionKind::MultipleChoice {
            choices: vec!["A".into(), "B".into()],
            answer: "C".into(),
            keep_order: false,
        };
        let dup = tf("x", "80s", &[]);
        let problems = QuestionBank::new(vec![cat("80s")], vec![bad, dup]).unwrap_err();
        let text: Vec<String> = problems.iter().map(|p| p.to_string()).collect();
        assert!(
            text.iter().any(|p| p.contains("unknown category")),
            "{text:?}"
        );
        assert!(
            text.iter().any(|p| p.contains("not one of the choices")),
            "{text:?}"
        );
        assert!(text.iter().any(|p| p.contains("duplicate id")), "{text:?}");
    }

    #[test]
    fn starter_content_loads() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
        let bank = QuestionBank::load_dir(&dir).unwrap_or_else(|e| panic!("{e}"));
        let stats = bank.stats();
        assert!(stats.total >= 150, "only {} questions", stats.total);
        for t in QuestionType::ALL {
            assert!(
                stats.by_type.get(t.as_str()).copied().unwrap_or(0) >= 10,
                "few {}",
                t.as_str()
            );
        }
        for c in bank.categories() {
            assert!(
                stats.by_category.get(&c.slug).copied().unwrap_or(0) >= 5,
                "few in {}",
                c.slug
            );
        }
        assert!(bank.warnings().is_empty(), "{:?}", bank.warnings());
        let pairs = bank.duplicates();
        assert!(pairs.is_empty(), "asked twice: {pairs:?}");
    }

    #[test]
    fn look_alikes_are_warnings_unless_kept() {
        let dir = std::env::temp_dir().join(format!("tbx-dupes-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("packs")).unwrap();
        std::fs::write(
            dir.join("categories.toml"),
            "[[category]]\nslug = \"science\"\nname = \"Science\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("packs/a.toml"),
            r#"[pack]
category = "science"
[[question]]
id = "one"
type = "multiple_choice"
prompt = "What is the hardest natural substance?"
choices = ["Diamond", "Quartz"]
answer = "Diamond"
[[question]]
id = "two"
type = "multiple_choice"
prompt = "Which natural substance is the hardest on Earth?"
choices = ["Diamond", "Granite"]
answer = "Diamond"
"#,
        )
        .unwrap();
        let bank = QuestionBank::load_dir(&dir).unwrap();
        assert_eq!(bank.duplicates().len(), 1);
        assert!(bank.warnings()[0].message.contains("same question as two"));

        std::fs::write(
            dir.join(NOT_DUPLICATES),
            "[[pair]]\nids = [\"two\", \"one\"]\n",
        )
        .unwrap();
        let bank = QuestionBank::load_dir(&dir).unwrap();
        assert!(bank.duplicates().is_empty());
        assert!(bank.warnings().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
