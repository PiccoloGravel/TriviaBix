//! Finding questions that ask the same thing in different words, so the bank, the importers and
//! the admin panel can skip or flag them.
//!
//! Two questions are compared by the content words of their prompts (lowercased, without filler
//! like "which" or "the", and lightly stemmed so "walked" meets "walk") and by their answers. The
//! same answer plus mostly the same words is a duplicate; a looser match is only flagged for a
//! person to look at. Pairs a person has checked and kept go in `content/not-duplicates.toml`.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::bank::validate_question;
use crate::judge::normalize;
use crate::model::*;

/// How sure we are that two questions are the same.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Worth a look: similar wording, but maybe a different question.
    Possible,
    /// The same question: same answer and mostly the same words, or the same prompt.
    Duplicate,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Possible => "possible",
            Verdict::Duplicate => "duplicate",
        }
    }
}

/// An earlier question that a new one matches.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Match {
    pub verdict: Verdict,
    pub id: String,
    pub prompt: String,
    /// From 0 to 1: how much of the wording the two prompts share.
    pub score: f32,
}

/// Two questions in the same bank that look alike. `a` comes first in the bank.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct DuplicatePair {
    pub verdict: Verdict,
    pub a: String,
    pub b: String,
    pub score: f32,
}

/// Pairs of question ids a person has looked at and decided are different questions.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct NotDuplicates {
    #[serde(default, rename = "pair")]
    pub pairs: Vec<KeptPair>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct KeptPair {
    pub ids: [String; 2],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl NotDuplicates {
    /// Reads `not-duplicates.toml`. A missing file is an empty list.
    pub fn read(path: &Path) -> Result<Self, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| e.to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn to_toml(&self) -> Result<String, String> {
        toml::to_string_pretty(self).map_err(|e| e.to_string())
    }

    pub fn ids(&self) -> impl Iterator<Item = (&str, &str)> {
        self.pairs
            .iter()
            .map(|p| (p.ids[0].as_str(), p.ids[1].as_str()))
    }
}

/// What a question is compared by.
#[derive(Clone, Debug)]
struct Print {
    prompt: String,
    words: BTreeSet<String>,
    /// Normalized accepted answers. Empty for true/false, whose answer says nothing on its own.
    answers: Vec<String>,
    true_false: Option<bool>,
}

fn print(q: &Question) -> Print {
    let (answers, true_false) = match &q.kind {
        QuestionKind::MultipleChoice { answer, .. } => (vec![answer.clone()], None),
        QuestionKind::TrueFalse { answer } => (vec![], Some(*answer)),
        QuestionKind::TypeAnswer { answers } => (answers.clone(), None),
        QuestionKind::ClosestNumber { answer, .. } => (vec![answer.to_string()], None),
        // A list of items isn't an answer another question would share.
        QuestionKind::PutInOrder { .. } => (vec![], None),
    };
    Print {
        prompt: normalize(&q.prompt),
        words: words(&q.prompt),
        answers: answers
            .iter()
            .map(|a| answer_key(a))
            .filter(|a| !a.is_empty())
            .collect(),
        true_false,
    }
}

const NUMBERS: [&str; 21] = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
    "twenty",
];

/// An answer normalized for comparing, with small numbers as digits so "Six" meets "6".
fn answer_key(answer: &str) -> String {
    normalize(answer)
        .split(' ')
        .map(|w| match NUMBERS.iter().position(|n| *n == w) {
            Some(i) => i.to_string(),
            None => w.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Words that say nothing about what a question is about.
const FILLER: &[&str] = &[
    "a", "about", "after", "all", "also", "an", "and", "are", "as", "at", "be", "been", "before",
    "being", "by", "called", "can", "could", "did", "do", "does", "for", "from", "had", "has",
    "have", "her", "his", "how", "in", "into", "is", "it", "its", "known", "name", "named", "of",
    "on", "one", "or", "s", "that", "the", "their", "them", "these", "this", "those", "to", "true",
    "false", "was", "were", "what", "whats", "when", "where", "which", "who", "whom", "whose",
    "why", "will", "with", "would", "you", "your",
];

fn words(prompt: &str) -> BTreeSet<String> {
    normalize(prompt)
        .split(' ')
        .filter(|w| !w.is_empty() && !FILLER.contains(w))
        .map(stem)
        .collect()
}

/// Just enough stemming to match "walked", "walks" and "walking", or "countries" and "country".
fn stem(w: &str) -> String {
    let n = w.chars().count();
    if !w.is_ascii() || n <= 3 {
        return w.to_string();
    }
    if let Some(s) = w.strip_suffix("ies").filter(|_| n > 4) {
        return format!("{s}y");
    }
    for suffix in ["ing", "ed"] {
        if let Some(s) = w.strip_suffix(suffix).filter(|s| s.len() >= 3) {
            return s.to_string();
        }
    }
    if w.ends_with('s') && !w.ends_with("ss") && !w.ends_with("us") && !w.ends_with("is") {
        return w[..w.len() - 1].to_string();
    }
    w.to_string()
}

/// How much two prompts share, from 0 to 1: half the share of all their words, half the share of
/// the shorter one's words, so a prompt with an extra clause still matches.
fn similarity(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let shared = a.intersection(b).count() as f32;
    let union = (a.len() + b.len()) as f32 - shared;
    let smaller = a.len().min(b.len()) as f32;
    0.5 * shared / union + 0.5 * shared / smaller
}

/// One answer is the other, or all of one's words are in the other ("Armstrong" and
/// "Neil Armstrong").
fn answers_match(a: &[String], b: &[String]) -> bool {
    a.iter().any(|x| {
        b.iter().any(|y| {
            if x == y {
                return true;
            }
            let xw: HashSet<&str> = x.split(' ').collect();
            let yw: HashSet<&str> = y.split(' ').collect();
            let (small, big) = if xw.len() <= yw.len() {
                (&xw, &yw)
            } else {
                (&yw, &xw)
            };
            small.iter().all(|w| w.len() > 2 && big.contains(w))
        })
    })
}

fn compare(a: &Print, b: &Print) -> Option<(Verdict, f32)> {
    if a.prompt == b.prompt {
        return Some((Verdict::Duplicate, 1.0));
    }
    let score = similarity(&a.words, &b.words);
    let verdict = match (a.true_false, b.true_false) {
        // Statements: the same claim, or near enough to be the same fact either way round.
        (Some(x), Some(y)) if x == y && score >= 0.8 => Some(Verdict::Duplicate),
        (Some(_), Some(_)) if score >= 0.65 => Some(Verdict::Possible),
        // A statement and a question about the same thing, such as "Canberra is the capital of
        // Australia" and "What is the capital of Australia?".
        (Some(_), None) | (None, Some(_)) if score >= 0.7 => Some(Verdict::Possible),
        (None, None) if answers_match(&a.answers, &b.answers) => {
            if score >= 0.6 {
                Some(Verdict::Duplicate)
            } else if score >= 0.4 {
                Some(Verdict::Possible)
            } else {
                None
            }
        }
        // Different answers but almost the same words: maybe the same answer spelled
        // differently, maybe a genuinely different question.
        (None, None) if score >= 0.75 && !a.answers.is_empty() && !b.answers.is_empty() => {
            Some(Verdict::Possible)
        }
        _ => None,
    }?;
    Some((verdict, score))
}

fn pair_key<'a>(a: &'a str, b: &'a str) -> (&'a str, &'a str) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

/// The questions so far, to check new ones against.
#[derive(Clone, Debug, Default)]
pub struct DuplicateIndex {
    entries: Vec<(String, String, Print)>,
    by_word: HashMap<String, Vec<usize>>,
    by_prompt: HashMap<String, usize>,
    kept: HashSet<(String, String)>,
}

impl DuplicateIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// An index of `questions`.
    pub fn of<'a>(questions: impl IntoIterator<Item = &'a Question>) -> Self {
        let mut index = Self::new();
        for q in questions {
            index.add(q);
        }
        index
    }

    /// Never report this pair: a person decided they are different questions.
    pub fn keep_pair(&mut self, a: &str, b: &str) {
        let (a, b) = pair_key(a, b);
        self.kept.insert((a.to_string(), b.to_string()));
    }

    pub fn is_kept(&self, a: &str, b: &str) -> bool {
        let (a, b) = pair_key(a, b);
        self.kept.contains(&(a.to_string(), b.to_string()))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn add(&mut self, q: &Question) {
        let p = print(q);
        let i = self.entries.len();
        for w in &p.words {
            self.by_word.entry(w.clone()).or_default().push(i);
        }
        self.by_prompt.entry(p.prompt.clone()).or_insert(i);
        self.entries.push((q.id.clone(), q.prompt.clone(), p));
    }

    /// Every earlier question `q` looks like, surest and closest first. A question never
    /// matches one with its own id.
    pub fn matches(&self, q: &Question) -> Vec<Match> {
        let p = print(q);
        let mut candidates: HashSet<usize> = p
            .words
            .iter()
            .filter_map(|w| self.by_word.get(w))
            .flatten()
            .copied()
            .collect();
        if let Some(&i) = self.by_prompt.get(&p.prompt) {
            candidates.insert(i);
        }
        let mut out: Vec<Match> = candidates
            .into_iter()
            .filter_map(|i| {
                let (id, prompt, other) = &self.entries[i];
                if *id == q.id || self.is_kept(id, &q.id) {
                    return None;
                }
                let (verdict, score) = compare(&p, other)?;
                Some(Match {
                    verdict,
                    id: id.clone(),
                    prompt: prompt.clone(),
                    score,
                })
            })
            .collect();
        out.sort_by(|a, b| {
            b.verdict
                .cmp(&a.verdict)
                .then(b.score.total_cmp(&a.score))
                .then(a.id.cmp(&b.id))
        });
        out
    }

    /// The closest earlier question `q` looks like, if any.
    pub fn check(&self, q: &Question) -> Option<Match> {
        self.matches(q).into_iter().next()
    }

    /// Checks `q` and adds it unless it duplicates something already here. Returns the match
    /// either way, so a caller can skip duplicates and flag possible ones.
    pub fn admit(&mut self, q: &Question) -> Option<Match> {
        let found = self.check(q);
        if found
            .as_ref()
            .is_none_or(|m| m.verdict != Verdict::Duplicate)
        {
            self.add(q);
        }
        found
    }

    /// Every pair of look-alikes among the questions added, each pair once.
    pub fn pairs(&self) -> Vec<DuplicatePair> {
        let mut out = Vec::new();
        for (i, (id, _, p)) in self.entries.iter().enumerate() {
            let mut seen = HashSet::new();
            for w in &p.words {
                for &j in self.by_word.get(w).into_iter().flatten() {
                    if j <= i || !seen.insert(j) {
                        continue;
                    }
                    let (other, _, q) = &self.entries[j];
                    if other == id || self.is_kept(id, other) {
                        continue;
                    }
                    if let Some((verdict, score)) = compare(p, q) {
                        out.push(DuplicatePair {
                            verdict,
                            a: id.clone(),
                            b: other.clone(),
                            score,
                        });
                    }
                }
            }
        }
        out.sort_by(|x, y| {
            y.verdict
                .cmp(&x.verdict)
                .then(y.score.total_cmp(&x.score))
                .then(x.a.cmp(&y.a))
        });
        out
    }
}

/// What [`screen`] made of a batch of imported questions.
#[derive(Debug, Default)]
pub struct Screened {
    /// New questions to add, in the order fetched.
    pub fresh: Vec<Question>,
    /// Questions dropped because the bank (or an earlier one in the batch) already asks them.
    pub skipped: Vec<(Question, Match)>,
    /// Added questions that look a bit like one already there, for a person to check. With
    /// `strict`, these are skipped instead and listed in `skipped`.
    pub flagged: Vec<(Question, Match)>,
    /// Questions dropped because they are broken, such as an answer missing from the choices.
    pub invalid: usize,
}

/// Sorts imported questions into new ones and duplicates of what `index` already holds (the
/// bank and the pack being added to), adding the new ones to `index` as it goes so a batch
/// can't repeat itself either. With `strict`, possible duplicates are skipped too.
pub fn screen(new: Vec<Question>, index: &mut DuplicateIndex, strict: bool) -> Screened {
    let mut out = Screened::default();
    for q in new {
        if !validate_question(&q).is_empty() {
            out.invalid += 1;
            continue;
        }
        match index.check(&q) {
            Some(m) if m.verdict == Verdict::Duplicate || strict => out.skipped.push((q, m)),
            found => {
                index.add(&q);
                if let Some(m) = found {
                    out.flagged.push((q.clone(), m));
                }
                out.fresh.push(q);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mc(id: &str, prompt: &str, answer: &str) -> Question {
        Question {
            id: id.into(),
            category: "general".into(),
            tags: vec![],
            difficulty: Difficulty::Easy,
            rating: Rating::Pg,
            prompt: prompt.into(),
            kind: QuestionKind::MultipleChoice {
                choices: vec![answer.into(), "Something else".into()],
                answer: answer.into(),
                keep_order: false,
            },
            fun_fact: None,
            host_quip: None,
            script: vec![],
            source: None,
        }
    }

    fn tf(id: &str, prompt: &str, answer: bool) -> Question {
        Question {
            kind: QuestionKind::TrueFalse { answer },
            ..mc(id, prompt, "x")
        }
    }

    fn typed(id: &str, prompt: &str, answers: &[&str]) -> Question {
        Question {
            kind: QuestionKind::TypeAnswer {
                answers: answers.iter().map(|a| a.to_string()).collect(),
            },
            ..mc(id, prompt, "x")
        }
    }

    fn verdict(a: &Question, b: &Question) -> Option<Verdict> {
        DuplicateIndex::of([a]).check(b).map(|m| m.verdict)
    }

    #[test]
    fn rewordings_with_the_same_answer_are_duplicates() {
        let cases = [
            (
                mc("a", "What is the capital of Australia?", "Canberra"),
                typed("b", "Name Australia's capital city.", &["Canberra"]),
            ),
            (
                mc(
                    "a",
                    "Who was the first person to walk on the Moon?",
                    "Neil Armstrong",
                ),
                mc(
                    "b",
                    "In 1969, which astronaut first walked on the moon?",
                    "Armstrong",
                ),
            ),
            (
                mc("a", "Which planet is known as the Red Planet?", "Mars"),
                mc("b", "What planet is called the red planet", "Mars"),
            ),
            (
                mc("a", "How many strings does a standard guitar have?", "6"),
                mc("b", "A standard guitar has how many strings?", "Six"),
            ),
        ];
        for (a, b) in &cases {
            assert_eq!(
                verdict(a, b),
                Some(Verdict::Duplicate),
                "{} / {}",
                a.prompt,
                b.prompt
            );
        }
    }

    #[test]
    fn same_topic_different_question_is_not_a_duplicate() {
        let cases = [
            (
                mc("a", "Which planet is closest to the Sun?", "Mercury"),
                mc("b", "Which planet is farthest from the Sun?", "Neptune"),
            ),
            (
                mc("a", "Which planet is known as the Red Planet?", "Mars"),
                mc("b", "Which planet has a moon called Phobos?", "Mars"),
            ),
            (
                mc("a", "Who directed Jaws?", "Steven Spielberg"),
                mc("b", "Who directed Jurassic Park?", "Steven Spielberg"),
            ),
        ];
        for (a, b) in &cases {
            assert_eq!(verdict(a, b), None, "{} / {}", a.prompt, b.prompt);
        }
    }

    #[test]
    fn statements_and_questions_about_the_same_fact_are_flagged() {
        let a = tf(
            "a",
            "The Great Wall of China is visible from space with the naked eye.",
            false,
        );
        let b = tf(
            "b",
            "You can see the Great Wall of China from space with the naked eye.",
            false,
        );
        assert_eq!(verdict(&a, &b), Some(Verdict::Duplicate));
        let c = mc("c", "What is the capital of Australia?", "Canberra");
        let d = tf("d", "Canberra is the capital of Australia.", true);
        assert_eq!(verdict(&c, &d), Some(Verdict::Possible));
    }

    #[test]
    fn exact_prompt_is_a_duplicate_whatever_the_answer() {
        let a = mc("a", "Who sang 'Thriller'?", "Michael Jackson");
        let b = mc("b", "who sang thriller", "MJ");
        assert_eq!(verdict(&a, &b), Some(Verdict::Duplicate));
    }

    #[test]
    fn kept_pairs_and_own_id_are_never_reported() {
        let a = mc("a", "What is the capital of Australia?", "Canberra");
        let b = mc("b", "Name the capital of Australia.", "Canberra");
        let mut index = DuplicateIndex::of([&a, &b]);
        assert_eq!(index.pairs().len(), 1);
        assert!(index.check(&a).is_some_and(|m| m.id == "b"));
        index.keep_pair("b", "a");
        assert!(index.pairs().is_empty());
        assert!(index.check(&a).is_none());
    }

    #[test]
    fn admit_skips_duplicates_but_keeps_possible_ones() {
        let mut index = DuplicateIndex::new();
        assert!(index
            .admit(&mc("a", "What is the capital of Australia?", "Canberra"))
            .is_none());
        let dup = index.admit(&mc("b", "Name the capital of Australia.", "Canberra"));
        assert_eq!(
            dup.map(|m| (m.verdict, m.id)),
            Some((Verdict::Duplicate, "a".into()))
        );
        let maybe = index.admit(&tf("c", "Canberra is the capital of Australia.", true));
        assert_eq!(maybe.map(|m| m.verdict), Some(Verdict::Possible));
        assert_eq!(index.len(), 2);
    }

    #[test]
    fn not_duplicates_file_round_trips() {
        let text = "[[pair]]\nids = [\"a\", \"b\"]\nnote = \"different years\"\n";
        let list: NotDuplicates = toml::from_str(text).unwrap();
        assert_eq!(list.ids().collect::<Vec<_>>(), vec![("a", "b")]);
        let again: NotDuplicates = toml::from_str(&list.to_toml().unwrap()).unwrap();
        assert_eq!(again.pairs.len(), 1);
    }
}
