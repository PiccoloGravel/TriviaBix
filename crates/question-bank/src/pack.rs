//! The pack file format: a TOML (or JSON) file holding a batch of questions with
//! shared defaults. See `content/README.md` for the authoring guide.

use serde::{Deserialize, Serialize};

use crate::model::{Difficulty, Question, QuestionKind, Rating, ScriptSlot, Source};
use crate::script::Lines;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct PackFile {
    #[serde(default)]
    pub pack: PackMeta,
    #[serde(default, rename = "question")]
    pub questions: Vec<PackQuestion>,
}

/// Pack level settings. `category`, `tags`, `difficulty`, `rating`, and `source` are
/// defaults for every question in the pack.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct PackMeta {
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub difficulty: Option<Difficulty>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating: Option<Rating>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
}

/// A question as written in a pack. Anything left out falls back to the pack defaults.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PackQuestion {
    /// Optional. Without one, the id is built from the pack name and the prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub difficulty: Option<Difficulty>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating: Option<Rating>,
    pub prompt: String,
    #[serde(flatten)]
    pub kind: QuestionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fun_fact: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_quip: Option<String>,
    /// What the host says to set the question up. See [`Lines`] for the accepted shapes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead_in: Option<Lines>,
    /// More things the host can say after the reveal, alongside `host_quip` and `fun_fact`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reveal: Option<Lines>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
}

impl PackFile {
    pub fn from_toml(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| e.to_string())
    }

    pub fn to_toml(&self) -> Result<String, String> {
        toml::to_string_pretty(self).map_err(|e| e.to_string())
    }

    /// Apply the pack defaults. `pack_key` names the pack in generated ids, usually the file stem.
    pub fn resolve(&self, pack_key: &str) -> Vec<Question> {
        let meta = &self.pack;
        self.questions
            .iter()
            .map(|q| {
                let mut tags = meta.tags.clone();
                for t in &q.tags {
                    if !tags.contains(t) {
                        tags.push(t.clone());
                    }
                }
                let rating = q.rating.or(meta.rating).unwrap_or_default();
                let mut script = Vec::new();
                if let Some(l) = &q.lead_in {
                    script.extend(l.to_script(ScriptSlot::LeadIn, rating));
                }
                if let Some(l) = &q.reveal {
                    script.extend(l.to_script(ScriptSlot::Reveal, rating));
                }
                Question {
                    id: q
                        .id
                        .clone()
                        .unwrap_or_else(|| generated_id(pack_key, &q.prompt)),
                    category: q
                        .category
                        .clone()
                        .or_else(|| meta.category.clone())
                        .unwrap_or_default(),
                    tags,
                    difficulty: q.difficulty.or(meta.difficulty).unwrap_or_default(),
                    rating,
                    prompt: q.prompt.trim().to_string(),
                    kind: q.kind.clone(),
                    fun_fact: q.fun_fact.clone(),
                    host_quip: q.host_quip.clone(),
                    script,
                    source: q.source.clone().or_else(|| meta.source.clone()),
                }
            })
            .collect()
    }
}

impl From<&Question> for PackQuestion {
    fn from(q: &Question) -> Self {
        PackQuestion {
            id: Some(q.id.clone()),
            category: Some(q.category.clone()),
            tags: q.tags.clone(),
            difficulty: Some(q.difficulty),
            rating: (q.rating != Rating::Pg).then_some(q.rating),
            prompt: q.prompt.clone(),
            kind: q.kind.clone(),
            fun_fact: q.fun_fact.clone(),
            host_quip: q.host_quip.clone(),
            lead_in: Lines::from_script(&q.script, ScriptSlot::LeadIn, q.rating),
            reveal: Lines::from_script(&q.script, ScriptSlot::Reveal, q.rating),
            source: q.source.clone(),
        }
    }
}

/// A stable id: `<pack>-<8 hex digits of a hash of the normalized prompt>`. It only
/// changes if the prompt's words change, so game history keeps pointing at the same question.
pub fn generated_id(pack_key: &str, prompt: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in crate::judge::normalize(prompt).bytes() {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{}-{:08x}", slugify(pack_key), hash as u32)
}

pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[pack]
name = "Sample"
category = "80s"
tags = ["retro"]
difficulty = "easy"

[[question]]
type = "multiple_choice"
prompt = "Which band sang 'Take On Me'?"
choices = ["a-ha", "Duran Duran", "Tears for Fears", "Eurythmics"]
answer = "a-ha"
tags = ["music"]

[[question]]
id = "wall"
type = "closest_number"
prompt = "In what year did the Berlin Wall fall?"
answer = 1989
difficulty = "medium"
"#;

    #[test]
    fn pack_defaults_apply() {
        let pack = PackFile::from_toml(SAMPLE).unwrap();
        let qs = pack.resolve("sample");
        assert_eq!(qs.len(), 2);
        assert_eq!(qs[0].category, "80s");
        assert_eq!(qs[0].tags, vec!["retro", "music"]);
        assert_eq!(qs[0].difficulty, Difficulty::Easy);
        assert!(qs[0].id.starts_with("sample-"));
        assert_eq!(qs[1].id, "wall");
        assert_eq!(qs[1].difficulty, Difficulty::Medium);
        assert!(
            matches!(qs[1].kind, QuestionKind::ClosestNumber { answer, .. } if answer == 1989.0)
        );
    }

    #[test]
    fn generated_ids_ignore_punctuation_and_case() {
        assert_eq!(
            generated_id("p", "Who is it?"),
            generated_id("p", "who is it")
        );
        assert_ne!(
            generated_id("p", "Who is it?"),
            generated_id("p", "Who was it?")
        );
    }

    #[test]
    fn pack_round_trips_through_toml() {
        let pack = PackFile::from_toml(SAMPLE).unwrap();
        let text = pack.to_toml().unwrap();
        let again = PackFile::from_toml(&text).unwrap();
        assert_eq!(pack.resolve("x"), again.resolve("x"));
    }
}
