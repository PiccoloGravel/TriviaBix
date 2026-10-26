//! Writes host lines for questions with Claude, for `triviabix-questions scripts generate`.
//!
//! Each call sends a batch of questions (prompt, answer, rating, and any lines they already
//! have) and gets back lead-ins and reveal lines grouped by rating, using structured outputs
//! so the reply always parses. Needs an Anthropic API key; nothing here runs without one.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::model::{Question, QuestionKind, Rating, ScriptSlot};
use crate::script::{Lines, OneOrMany, ScriptEntry};

pub const DEFAULT_MODEL: &str = crate::DEFAULT_MODEL;
const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2026-10-21";
/// Lets the API retry a declined request on another model instead of failing the batch.
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
const MAX_TOKENS: u32 = 16_000;

const INTRO: &str =
    "You write lines for Bix, the host of TriviaBix, a party trivia game played on a TV with \
phones as buzzers.";

/// Everything after the persona: what to write and the rules every line follows.
const RULES: &str = "His lines are read aloud by text-to-speech, so they must sound natural spoken. Stay in \
character in every line.

For each question you get, write the slots listed in its `write` field:
- lead_in: said right after Bix reads the question, before anyone answers. Set the mood, tease the topic, or \
make a joke. Never give away, hint at, or rule out the answer or any choice.
- reveal: said right after the answer is shown. A joke, a short anecdote, or a fun fact about the answer. Facts \
must be true and well known enough that you are sure of them; if you are not sure, write a joke instead.

Ratings: PG is family friendly. PG-13 allows mild innuendo and light swearing. MA allows adult humour, swearing, \
drinking jokes, and harsher roasts. NC-17 is anything goes for a consenting adult room, but nothing hateful \
toward real groups of people. Write several lines per slot so games don't repeat: at least two at the question's \
own rating, and when the question's rating is below MA, one or two spicier ones at MA too. Never rate a line below \
the question's rating. Roasting the players (\"half of you are guessing\", \"your phone can't help you now\") is \
welcome.

Rules for every line: one to three short sentences, at most 280 characters, plain text with no emoji, stage \
directions, or quotation marks around the whole line. The voice can perform a few sounds written as tags: [Sigh], \
[Laughter], [Uhm] and [Breathing]. Use one now and then where it lands a joke, never more than one per line and \
not in most lines; they're hidden on screen. Bix can say his own name as {host}; use no other braces. \
Don't repeat or paraphrase lines in the question's `existing` list. Return every question you were given, using \
its `question` id exactly, with an empty list for any slot you were not asked to write.";

/// Which slots to write: lead-in, reveal, or both.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Wanted {
    LeadIn,
    Reveal,
    Either,
}

impl Wanted {
    /// The slots `q` still needs.
    pub fn slots_for(self, q: &Question) -> Vec<ScriptSlot> {
        let want = |slot| match self {
            Wanted::LeadIn => slot == ScriptSlot::LeadIn,
            Wanted::Reveal => slot == ScriptSlot::Reveal,
            Wanted::Either => true,
        };
        ScriptSlot::ALL
            .into_iter()
            .filter(|&s| want(s) && !q.has_lines(s))
            .collect()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GenerateError {
    #[error("Claude API request failed: {0}")]
    Http(String),
    #[error("Claude API returned {status}: {message}")]
    Api { status: u16, message: String },
    #[error("Claude declined this batch ({0})")]
    Refused(String),
    #[error("Claude ran out of room before finishing; try a smaller --batch")]
    Truncated,
    #[error("couldn't read Claude's reply: {0}")]
    Format(String),
}

pub struct Generator {
    api_key: String,
    pub model: String,
    pub effort: String,
    /// Who Bix is, in a paragraph; the rest of the instructions are fixed.
    pub persona: String,
    agent: ureq::Agent,
}

/// What one batch cost, as reported by the API.
#[derive(Default, Debug, Clone, Copy)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl Generator {
    pub fn new(
        api_key: impl Into<String>,
        model: impl Into<String>,
        effort: impl Into<String>,
    ) -> Self {
        Self {
            api_key: api_key.into(),
            model: model.into(),
            effort: effort.into(),
            persona: crate::DEFAULT_PERSONA.into(),
            // A batch can take a few minutes while Claude thinks.
            agent: ureq::AgentBuilder::new()
                .timeout_read(Duration::from_secs(600))
                .user_agent("triviabix-questions")
                .build(),
        }
    }

    /// Uses this persona instead of the default one. Empty keeps the default.
    pub fn with_persona(mut self, persona: &str) -> Self {
        let persona = persona.trim();
        if !persona.is_empty() {
            self.persona = persona.chars().take(crate::MAX_PERSONA_CHARS).collect();
        }
        self
    }

    /// The system prompt: the fixed introduction, the persona, then the rules.
    pub fn system(&self) -> String {
        format!("{INTRO} {}\n\n{RULES}", self.persona)
    }

    /// Writes lines for one batch. Questions with no slots to write are skipped.
    pub fn write(
        &self,
        batch: &[(&Question, Vec<ScriptSlot>)],
    ) -> Result<(Vec<ScriptEntry>, Usage), GenerateError> {
        let body = self.request(batch);
        let res = self
            .agent
            .post(API_URL)
            .set("x-api-key", &self.api_key)
            .set("anthropic-version", API_VERSION)
            .set("anthropic-beta", FALLBACK_BETA)
            .send_json(body);
        let res = match res {
            Ok(r) => r,
            Err(ureq::Error::Status(status, r)) => {
                let text = r.into_string().unwrap_or_default();
                let message = serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
                    .unwrap_or(text);
                return Err(GenerateError::Api { status, message });
            }
            Err(e) => return Err(GenerateError::Http(e.to_string())),
        };
        let reply: Value = res
            .into_json()
            .map_err(|e| GenerateError::Format(e.to_string()))?;
        parse_reply(&reply, batch)
    }

    /// The Messages API request for one batch.
    pub fn request(&self, batch: &[(&Question, Vec<ScriptSlot>)]) -> Value {
        let questions: Vec<Value> = batch
            .iter()
            .filter(|(_, slots)| !slots.is_empty())
            .map(|(q, slots)| brief(q, slots))
            .collect();
        let questions = serde_json::to_string_pretty(&questions).unwrap_or_default();
        json!({
            "model": self.model,
            "max_tokens": MAX_TOKENS,
            "fallbacks": "default",
            "system": self.system(),
            "output_config": {
                "effort": self.effort,
                "format": { "type": "json_schema", "schema": reply_schema() },
            },
            "messages": [{
                "role": "user",
                "content": format!("Write Bix's lines for these questions.\n\n{questions}"),
            }],
        })
    }
}

/// What Claude sees about one question.
fn brief(q: &Question, slots: &[ScriptSlot]) -> Value {
    let mut v = json!({
        "question": q.id,
        "prompt": q.prompt,
        "category": q.category,
        "type": q.question_type().as_str(),
        "answer": q.answer_text(),
        "rating": q.rating.as_str(),
        "difficulty": q.difficulty.as_str(),
        "existing": q.host_lines().into_iter().map(|l| l.text).collect::<Vec<_>>(),
        "write": slots.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
    });
    if let QuestionKind::MultipleChoice { choices, .. } = &q.kind {
        v["choices"] = json!(choices);
    }
    v
}

fn reply_schema() -> Value {
    let lines = json!({
        "type": "array",
        "items": {
            "type": "object",
            "properties": {
                "rating": { "type": "string", "enum": ["PG", "PG-13", "MA", "NC-17"] },
                "text": { "type": "string" },
            },
            "required": ["rating", "text"],
            "additionalProperties": false,
        },
    });
    json!({
        "type": "object",
        "properties": {
            "scripts": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "question": { "type": "string" },
                        "lead_in": lines,
                        "reveal": lines,
                    },
                    "required": ["question", "lead_in", "reveal"],
                    "additionalProperties": false,
                },
            },
        },
        "required": ["scripts"],
        "additionalProperties": false,
    })
}

#[derive(Deserialize)]
struct Reply {
    scripts: Vec<ReplyEntry>,
}

#[derive(Deserialize)]
struct ReplyEntry {
    question: String,
    #[serde(default)]
    lead_in: Vec<ReplyLine>,
    #[serde(default)]
    reveal: Vec<ReplyLine>,
}

#[derive(Deserialize)]
struct ReplyLine {
    rating: String,
    text: String,
}

/// Turns an API response into script entries, keeping only the slots each question asked for.
/// Lines rated below their question are bumped up to its rating; blank lines are dropped.
pub fn parse_reply(
    reply: &Value,
    batch: &[(&Question, Vec<ScriptSlot>)],
) -> Result<(Vec<ScriptEntry>, Usage), GenerateError> {
    match reply["stop_reason"].as_str() {
        Some("refusal") => {
            let why = reply["stop_details"]["category"]
                .as_str()
                .unwrap_or("no reason given");
            return Err(GenerateError::Refused(why.to_string()));
        }
        Some("max_tokens") => return Err(GenerateError::Truncated),
        _ => {}
    }
    let text = reply["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect::<String>();
    let parsed: Reply =
        serde_json::from_str(&text).map_err(|e| GenerateError::Format(e.to_string()))?;
    let usage = Usage {
        input_tokens: reply["usage"]["input_tokens"].as_u64().unwrap_or(0),
        output_tokens: reply["usage"]["output_tokens"].as_u64().unwrap_or(0),
    };

    let mut entries = Vec::new();
    for e in parsed.scripts {
        let Some((q, slots)) = batch.iter().find(|(q, _)| q.id == e.question) else {
            continue;
        };
        let lines = |slot, lines: Vec<ReplyLine>| {
            if !slots.contains(&slot) {
                return None;
            }
            let mut tiers: BTreeMap<Rating, Vec<String>> = BTreeMap::new();
            for l in lines {
                let text = l.text.trim();
                if text.is_empty() {
                    continue;
                }
                let rating = Rating::parse(&l.rating).unwrap_or(q.rating).max(q.rating);
                tiers.entry(rating).or_default().push(text.to_string());
            }
            (!tiers.is_empty()).then(|| {
                Lines::ByRating(
                    tiers
                        .into_iter()
                        .map(|(r, t)| (r, OneOrMany::from_texts(t)))
                        .collect(),
                )
            })
        };
        let entry = ScriptEntry {
            question: q.id.clone(),
            prompt: Some(q.prompt.clone()),
            lead_in: lines(ScriptSlot::LeadIn, e.lead_in),
            reveal: lines(ScriptSlot::Reveal, e.reveal),
        };
        if entry.lead_in.is_some() || entry.reveal.is_some() {
            entries.push(entry);
        }
    }
    Ok((entries, usage))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::PackFile;

    fn questions() -> Vec<Question> {
        PackFile::from_toml(
            r#"
[pack]
name = "Test"

[[question]]
id = "q-crows"
type = "true_false"
category = "nature"
prompt = "A group of crows is called a murder."
answer = true
rating = "PG-13"
reveal = "They hold grudges."

[[question]]
id = "q-lakes"
type = "multiple_choice"
category = "geography"
prompt = "Which country has the most lakes?"
choices = ["Canada", "Russia", "United States", "Finland"]
answer = "Canada"
"#,
        )
        .unwrap()
        .resolve("test")
    }

    #[test]
    fn asks_only_for_missing_slots() {
        let qs = questions();
        assert_eq!(Wanted::Either.slots_for(&qs[0]), vec![ScriptSlot::LeadIn]);
        assert!(Wanted::Reveal.slots_for(&qs[0]).is_empty());
        assert_eq!(Wanted::Either.slots_for(&qs[1]).len(), 2);
    }

    #[test]
    fn request_carries_the_briefs_and_schema() {
        let qs = questions();
        let batch: Vec<_> = qs
            .iter()
            .map(|q| (q, Wanted::Either.slots_for(q)))
            .collect();
        let req = Generator::new("key", DEFAULT_MODEL, "medium").request(&batch);
        assert_eq!(req["model"], DEFAULT_MODEL);
        assert_eq!(req["fallbacks"], "default");
        assert_eq!(req["output_config"]["effort"], "medium");
        assert_eq!(req["output_config"]["format"]["type"], "json_schema");
        assert!(req.get("thinking").is_none() && req.get("temperature").is_none());
        let user = req["messages"][0]["content"].as_str().unwrap();
        let briefs: Vec<Value> = serde_json::from_str(user.split_once("\n\n").unwrap().1).unwrap();
        assert_eq!(briefs[0]["question"], "q-crows");
        assert_eq!(briefs[0]["write"], json!(["lead_in"]));
        assert_eq!(briefs[0]["existing"], json!(["They hold grudges."]));
        assert_eq!(briefs[1]["choices"][3], "Finland");
        let system = req["system"].as_str().unwrap();
        assert!(system.contains("seen better days") && system.contains("lead_in:"));
    }

    #[test]
    fn a_custom_persona_replaces_the_default() {
        let gen = Generator::new("key", DEFAULT_MODEL, "medium")
            .with_persona("  Bix is a pirate who only speaks in sea shanties.  ");
        let system = gen.system();
        assert!(system.contains("Bix is a pirate who only speaks in sea shanties.\n\n"));
        assert!(!system.contains("seen better days"));
        assert!(system.contains("Never give away"));
        let blank = Generator::new("key", DEFAULT_MODEL, "medium").with_persona("   ");
        assert_eq!(blank.persona, crate::DEFAULT_PERSONA);
    }

    fn reply(body: Value) -> Value {
        json!({
            "stop_reason": "end_turn",
            "content": [
                { "type": "thinking", "thinking": "" },
                { "type": "text", "text": body.to_string() },
            ],
            "usage": { "input_tokens": 1200, "output_tokens": 800 },
        })
    }

    #[test]
    fn parses_lines_and_keeps_only_what_was_asked() {
        let qs = questions();
        let batch: Vec<_> = qs
            .iter()
            .map(|q| (q, Wanted::Either.slots_for(q)))
            .collect();
        let body = json!({ "scripts": [
            { "question": "q-crows",
              "lead_in": [{ "rating": "PG", "text": "Bird question." }, { "rating": "MA", "text": " Spicy bird. " }],
              "reveal": [{ "rating": "PG-13", "text": "Not asked for." }] },
            { "question": "q-lakes",
              "lead_in": [],
              "reveal": [{ "rating": "PG", "text": "Over half the world's lakes." }, { "rating": "PG", "text": "  " }] },
            { "question": "q-unknown", "lead_in": [{ "rating": "PG", "text": "Who?" }], "reveal": [] },
        ]});
        let (entries, usage) = parse_reply(&reply(body), &batch).unwrap();
        assert_eq!(usage.output_tokens, 800);
        assert_eq!(entries.len(), 2);

        let crows = entries[0].lines(qs[0].rating);
        assert!(entries[0].reveal.is_none());
        // The PG line is bumped to the question's PG-13; text is trimmed.
        let got: Vec<_> = crows.iter().map(|l| (l.rating, l.text.as_str())).collect();
        assert_eq!(
            got,
            vec![
                (Rating::Pg13, "Bird question."),
                (Rating::Ma, "Spicy bird.")
            ]
        );

        assert!(entries[1].lead_in.is_none());
        assert_eq!(entries[1].lines(Rating::Pg).len(), 1);
    }

    #[test]
    fn reports_refusals_and_truncation() {
        let qs = questions();
        let batch: Vec<_> = qs
            .iter()
            .map(|q| (q, Wanted::Either.slots_for(q)))
            .collect();
        let refused = json!({ "stop_reason": "refusal", "stop_details": { "category": "cyber" }, "content": [] });
        assert!(
            matches!(parse_reply(&refused, &batch), Err(GenerateError::Refused(c)) if c == "cyber")
        );
        let cut = json!({ "stop_reason": "max_tokens", "content": [] });
        assert!(matches!(
            parse_reply(&cut, &batch),
            Err(GenerateError::Truncated)
        ));
        let junk =
            json!({ "stop_reason": "end_turn", "content": [{ "type": "text", "text": "nope" }] });
        assert!(matches!(
            parse_reply(&junk, &batch),
            Err(GenerateError::Format(_))
        ));
    }
}
