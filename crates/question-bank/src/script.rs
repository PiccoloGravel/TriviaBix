//! Per-question host scripts: a lead-in before a question and a joke, anecdote, or fun fact
//! after the reveal. Hand-written lines can sit right in a pack (`lead_in`, `reveal`,
//! `host_quip`, `fun_fact`); bulk-written ones live in script files under `content/scripts`,
//! keyed by question id, so packs stay untouched.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::judge::normalize;
use crate::model::{Question, Rating, ScriptLine, ScriptSlot};

/// The longest line the host will read. Anything longer drags on the TV.
pub const MAX_LINE_CHARS: usize = 280;

/// Script lines as written in a file: one line, a list, or lines grouped by rating.
///
/// ```toml
/// lead_in = "Bird nerds, this one's yours."
/// reveal = ["A murder of crows.", "They hold grudges, too."]
/// reveal = { PG = "They hold grudges.", MA = "Crows remember faces. Yours, specifically." }
/// ```
///
/// Lines without a rating get the question's rating.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
pub enum Lines {
    Plain(OneOrMany),
    ByRating(BTreeMap<Rating, OneOrMany>),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
pub enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    fn texts(&self) -> Vec<&str> {
        match self {
            OneOrMany::One(t) => vec![t],
            OneOrMany::Many(ts) => ts.iter().map(String::as_str).collect(),
        }
    }

    pub(crate) fn from_texts(mut texts: Vec<String>) -> Self {
        if texts.len() == 1 {
            OneOrMany::One(texts.remove(0))
        } else {
            OneOrMany::Many(texts)
        }
    }
}

impl Lines {
    pub fn to_script(&self, slot: ScriptSlot, default_rating: Rating) -> Vec<ScriptLine> {
        let line = |rating, text: &str| ScriptLine {
            slot,
            rating,
            text: text.trim().to_string(),
        };
        match self {
            Lines::Plain(texts) => texts
                .texts()
                .into_iter()
                .map(|t| line(default_rating, t))
                .collect(),
            Lines::ByRating(tiers) => tiers
                .iter()
                .flat_map(|(r, texts)| texts.texts().into_iter().map(move |t| line(*r, t)))
                .collect(),
        }
    }

    /// The lines for one slot, written as compactly as possible. `None` if there are none.
    pub fn from_script(
        script: &[ScriptLine],
        slot: ScriptSlot,
        default_rating: Rating,
    ) -> Option<Lines> {
        let mut tiers: BTreeMap<Rating, Vec<String>> = BTreeMap::new();
        for l in script.iter().filter(|l| l.slot == slot) {
            tiers.entry(l.rating).or_default().push(l.text.clone());
        }
        if tiers.is_empty() {
            return None;
        }
        if tiers.len() == 1 && tiers.contains_key(&default_rating) {
            let texts = tiers.remove(&default_rating).unwrap();
            return Some(Lines::Plain(OneOrMany::from_texts(texts)));
        }
        Some(Lines::ByRating(
            tiers
                .into_iter()
                .map(|(r, t)| (r, OneOrMany::from_texts(t)))
                .collect(),
        ))
    }
}

/// A script file: lines for questions that live in packs, matched by question id.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ScriptFile {
    #[serde(default, rename = "script")]
    pub scripts: Vec<ScriptEntry>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ScriptEntry {
    /// The question's id, as shown by `triviabix-questions scripts todo`.
    pub question: String,
    /// The question's prompt, for whoever reads the file. A mismatch is reported by
    /// `validate`, since it usually means the question was rewritten.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead_in: Option<Lines>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reveal: Option<Lines>,
}

impl ScriptEntry {
    pub fn lines(&self, default_rating: Rating) -> Vec<ScriptLine> {
        let mut out = Vec::new();
        if let Some(l) = &self.lead_in {
            out.extend(l.to_script(ScriptSlot::LeadIn, default_rating));
        }
        if let Some(l) = &self.reveal {
            out.extend(l.to_script(ScriptSlot::Reveal, default_rating));
        }
        out
    }

    /// Whether this entry was written for `q` as it reads now.
    pub fn matches_prompt(&self, q: &Question) -> bool {
        self.prompt
            .as_deref()
            .is_none_or(|p| normalize(p) == normalize(&q.prompt))
    }
}

impl ScriptFile {
    pub fn from_toml(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        // A bare array of entries is accepted too, since that's what most generators produce.
        serde_json::from_str::<Vec<ScriptEntry>>(text)
            .map(|scripts| ScriptFile { scripts })
            .or_else(|_| serde_json::from_str(text))
            .map_err(|e| e.to_string())
    }

    pub fn read(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        match path.extension().and_then(|e| e.to_str()) {
            Some("json") => Self::from_json(&text),
            _ => Self::from_toml(&text),
        }
    }

    pub fn to_toml(&self) -> Result<String, String> {
        toml::to_string_pretty(self).map_err(|e| e.to_string())
    }

    /// Adds or replaces entries by question id, keeping the file's order. A slot an entry leaves
    /// out keeps the lines it had. Returns how many entries were new.
    pub fn merge(&mut self, entries: impl IntoIterator<Item = ScriptEntry>) -> usize {
        let mut added = 0;
        for e in entries {
            match self.scripts.iter_mut().find(|s| s.question == e.question) {
                // A slot the new entry leaves out keeps its old lines.
                Some(old) => {
                    let lead_in = e.lead_in.or(old.lead_in.take());
                    let reveal = e.reveal.or(old.reveal.take());
                    *old = ScriptEntry {
                        lead_in,
                        reveal,
                        ..e
                    };
                }
                None => {
                    self.scripts.push(e);
                    added += 1;
                }
            }
        }
        added
    }
}

impl Question {
    /// Every line the host can say about this question: the script, plus the pack's
    /// `host_quip` and `fun_fact` as reveal lines at the question's rating.
    pub fn host_lines(&self) -> Vec<ScriptLine> {
        let mut out = self.script.clone();
        for text in [&self.host_quip, &self.fun_fact].into_iter().flatten() {
            out.push(ScriptLine {
                slot: ScriptSlot::Reveal,
                rating: self.rating,
                text: text.trim().to_string(),
            });
        }
        out
    }

    /// Whether the host has something to say about this question in `slot`.
    pub fn has_lines(&self, slot: ScriptSlot) -> bool {
        self.host_lines().iter().any(|l| l.slot == slot)
    }
}

/// Problems with one script line.
pub fn validate_line(line: &ScriptLine) -> Vec<String> {
    let mut out = Vec::new();
    let text = line.text.trim();
    if text.is_empty() {
        out.push(format!("empty {} line", line.slot.as_str()));
    }
    let len = text.chars().count();
    if len > MAX_LINE_CHARS {
        out.push(format!(
            "{} line is {len} characters; keep it under {MAX_LINE_CHARS}",
            line.slot.as_str()
        ));
    }
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        let end = rest[start..].find('}').map(|e| start + e);
        let name = end.map(|e| &rest[start + 1..e]);
        if name != Some("host") {
            out.push(format!(
                "{} line has a placeholder other than {{host}}: {text:?}",
                line.slot.as_str()
            ));
            break;
        }
        rest = &rest[end.unwrap() + 1..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_parse_every_shape() {
        let file = ScriptFile::from_toml(
            r#"
[[script]]
question = "a"
lead_in = "One."
reveal = ["Two.", "Three."]

[[script]]
question = "b"
reveal = { PG = "Tame.", MA = ["Spicy.", "Spicier."] }
"#,
        )
        .unwrap();
        let a = file.scripts[0].lines(Rating::Pg13);
        assert_eq!(a.len(), 3);
        assert!(a.iter().all(|l| l.rating == Rating::Pg13));
        assert_eq!(a[0].slot, ScriptSlot::LeadIn);
        let b = file.scripts[1].lines(Rating::Pg);
        let ratings: Vec<Rating> = b.iter().map(|l| l.rating).collect();
        assert_eq!(ratings, vec![Rating::Pg, Rating::Ma, Rating::Ma]);
    }

    #[test]
    fn lines_round_trip_compactly() {
        let script = vec![
            ScriptLine {
                slot: ScriptSlot::Reveal,
                rating: Rating::Pg,
                text: "Tame.".into(),
            },
            ScriptLine {
                slot: ScriptSlot::Reveal,
                rating: Rating::Ma,
                text: "Spicy.".into(),
            },
        ];
        let lines = Lines::from_script(&script, ScriptSlot::Reveal, Rating::Pg).unwrap();
        assert_eq!(lines.to_script(ScriptSlot::Reveal, Rating::Pg), script);
        assert!(Lines::from_script(&script, ScriptSlot::LeadIn, Rating::Pg).is_none());
        let plain = Lines::from_script(&script[..1], ScriptSlot::Reveal, Rating::Pg).unwrap();
        assert_eq!(plain, Lines::Plain(OneOrMany::One("Tame.".into())));
    }

    #[test]
    fn json_arrays_are_accepted() {
        let file = ScriptFile::from_json(
            r#"[{"question": "a", "lead_in": "Hi.", "reveal": {"PG-13": "Ha."}}]"#,
        )
        .unwrap();
        assert_eq!(file.scripts[0].lines(Rating::Pg).len(), 2);
    }

    #[test]
    fn merge_replaces_by_question() {
        let entry = |q: &str, text: &str| ScriptEntry {
            question: q.into(),
            prompt: None,
            lead_in: Some(Lines::Plain(OneOrMany::One(text.into()))),
            reveal: None,
        };
        let mut file = ScriptFile {
            scripts: vec![entry("a", "old"), entry("b", "b")],
        };
        assert_eq!(file.merge([entry("a", "new"), entry("c", "c")]), 1);
        assert_eq!(file.scripts.len(), 3);
        assert_eq!(file.scripts[0], entry("a", "new"));

        let reveal_only = ScriptEntry {
            question: "b".into(),
            prompt: None,
            lead_in: None,
            reveal: Some(Lines::Plain(OneOrMany::One("ta-da".into()))),
        };
        file.merge([reveal_only.clone()]);
        assert_eq!(
            file.scripts[1],
            ScriptEntry {
                lead_in: entry("b", "b").lead_in,
                ..reveal_only
            }
        );
    }

    #[test]
    fn bad_lines_are_caught() {
        let line = |text: &str| ScriptLine {
            slot: ScriptSlot::LeadIn,
            rating: Rating::Pg,
            text: text.into(),
        };
        assert!(validate_line(&line("I'm {host}. Hi.")).is_empty());
        assert!(!validate_line(&line("  ")).is_empty());
        assert!(!validate_line(&line("Hi {name}")).is_empty());
        assert!(!validate_line(&line(&"x".repeat(MAX_LINE_CHARS + 1))).is_empty());
    }
}
