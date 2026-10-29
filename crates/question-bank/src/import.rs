//! Importers that turn questions from free trivia sites into TriviaBix questions.
//!
//! Parsing is separate from fetching so it can be tested without the network.

use std::collections::HashSet;
use std::time::Duration;

use base64::Engine;
use serde::Deserialize;

pub use crate::dupes::{screen, Screened};
use crate::model::*;
use crate::pack::generated_id;

pub const OPENTDB_SOURCE: (&str, &str, &str) =
    ("Open Trivia DB", "https://opentdb.com", "CC BY-SA 4.0");
pub const TRIVIA_API_SOURCE: (&str, &str, &str) = (
    "The Trivia API",
    "https://the-trivia-api.com",
    "Free for non-commercial use; see the-trivia-api.com",
);

fn source(s: (&str, &str, &str)) -> Source {
    Source {
        name: s.0.into(),
        url: Some(s.1.into()),
        license: Some(s.2.into()),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("request failed: {0}")]
    Http(String),
    #[error("unexpected response: {0}")]
    Format(String),
    #[error("Open Trivia DB returned code {0}: {1}")]
    OpenTdb(u32, &'static str),
}

// ---------- Open Trivia DB ----------

/// Open Trivia DB category ids, for `--category`. Run `import opentdb --list-categories`.
pub const OPENTDB_CATEGORIES: &[(u32, &str)] = &[
    (9, "General Knowledge"),
    (10, "Entertainment: Books"),
    (11, "Entertainment: Film"),
    (12, "Entertainment: Music"),
    (13, "Entertainment: Musicals & Theatres"),
    (14, "Entertainment: Television"),
    (15, "Entertainment: Video Games"),
    (16, "Entertainment: Board Games"),
    (17, "Science & Nature"),
    (18, "Science: Computers"),
    (19, "Science: Mathematics"),
    (20, "Mythology"),
    (21, "Sports"),
    (22, "Geography"),
    (23, "History"),
    (24, "Politics"),
    (25, "Art"),
    (26, "Celebrities"),
    (27, "Animals"),
    (28, "Vehicles"),
    (29, "Entertainment: Comics"),
    (30, "Science: Gadgets"),
    (31, "Entertainment: Japanese Anime & Manga"),
    (32, "Entertainment: Cartoon & Animations"),
];

/// Map an Open Trivia DB category name to a TriviaBix category and an extra tag.
fn map_opentdb_category(name: &str) -> (&'static str, Option<String>) {
    let tag = name.rsplit(": ").next().map(crate::slugify);
    let cat = match name {
        "Entertainment: Film" => "movies",
        "Entertainment: Music" | "Entertainment: Musicals & Theatres" => "music",
        "Entertainment: Television" | "Entertainment: Cartoon & Animations" => "tv",
        "Entertainment: Video Games" | "Entertainment: Board Games" => "video-games",
        "Celebrities" | "Entertainment: Comics" | "Entertainment: Japanese Anime & Manga" => {
            "pop-culture"
        }
        "Science & Nature"
        | "Science: Computers"
        | "Science: Mathematics"
        | "Science: Gadgets"
        | "Animals" => "science",
        "Geography" => "geography",
        "History" | "Politics" | "Mythology" => "history",
        "Sports" | "Vehicles" => "sports",
        _ => "general",
    };
    (cat, tag)
}

#[derive(Deserialize)]
struct OpenTdbResponse {
    response_code: u32,
    #[serde(default)]
    results: Vec<OpenTdbQuestion>,
}

#[derive(Deserialize)]
struct OpenTdbQuestion {
    #[serde(rename = "type")]
    kind: String,
    difficulty: String,
    category: String,
    question: String,
    correct_answer: String,
    incorrect_answers: Vec<String>,
}

fn opentdb_error(code: u32) -> ImportError {
    let msg = match code {
        1 => "not enough questions for that query; ask for fewer",
        2 => "invalid parameter",
        3 => "session token not found",
        4 => "session token has returned every question; reset it",
        5 => "rate limited; wait 5 seconds between requests",
        _ => "unknown error",
    };
    ImportError::OpenTdb(code, msg)
}

/// Parse an Open Trivia DB response fetched with `encode=base64`.
pub fn parse_opentdb(json: &str) -> Result<Vec<Question>, ImportError> {
    let resp: OpenTdbResponse =
        serde_json::from_str(json).map_err(|e| ImportError::Format(e.to_string()))?;
    if resp.response_code != 0 {
        return Err(opentdb_error(resp.response_code));
    }
    let b64 = |s: &str| -> Result<String, ImportError> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(s)
            .map_err(|e| ImportError::Format(format!("bad base64: {e}")))?;
        String::from_utf8(bytes).map_err(|e| ImportError::Format(e.to_string()))
    };
    let mut out = Vec::new();
    for r in resp.results {
        let category_name = b64(&r.category)?;
        let prompt = b64(&r.question)?;
        let correct = b64(&r.correct_answer)?;
        let kind = match b64(&r.kind)?.as_str() {
            "boolean" => QuestionKind::TrueFalse {
                answer: correct.eq_ignore_ascii_case("true"),
            },
            _ => {
                let mut choices = vec![correct.clone()];
                for w in &r.incorrect_answers {
                    choices.push(b64(w)?);
                }
                QuestionKind::MultipleChoice {
                    choices,
                    answer: correct,
                    keep_order: false,
                }
            }
        };
        let (category, extra_tag) = map_opentdb_category(&category_name);
        let mut tags = vec!["opentdb".to_string()];
        tags.extend(extra_tag.filter(|t| t != category));
        out.push(Question {
            id: generated_id("opentdb", &prompt),
            category: category.into(),
            tags,
            difficulty: parse_difficulty(&b64(&r.difficulty)?),
            rating: Rating::Pg,
            prompt,
            kind,
            fun_fact: None,
            host_quip: None,
            script: vec![],
            source: Some(source(OPENTDB_SOURCE)),
        });
    }
    Ok(out)
}

pub struct OpenTdbQuery {
    pub amount: usize,
    pub category: Option<u32>,
    pub difficulty: Option<Difficulty>,
    /// `multiple` or `boolean`.
    pub kind: Option<String>,
}

/// Open Trivia DB answers each IP at most once every 5 seconds, counting the token request.
const OPENTDB_GAP: Duration = Duration::from_secs(6);

/// Fetch questions from Open Trivia DB, 50 per request, waiting between requests to
/// respect its rate limit. A session token keeps it from repeating questions.
pub fn fetch_opentdb(q: &OpenTdbQuery) -> Result<Vec<Question>, ImportError> {
    fetch_opentdb_with(q, OPENTDB_GAP, |url| get_text(&agent(), url))
}

fn fetch_opentdb_with(
    q: &OpenTdbQuery,
    gap: Duration,
    mut get: impl FnMut(&str) -> Result<String, ImportError>,
) -> Result<Vec<Question>, ImportError> {
    // The token only stops repeats, so the import goes ahead without one.
    let token = get("https://opentdb.com/api_token.php?command=request")
        .ok()
        .and_then(|body| serde_json::from_str::<serde_json::Value>(&body).ok())
        .and_then(|v| v["token"].as_str().map(str::to_string));

    let mut out = Vec::new();
    let mut remaining = q.amount;
    let mut batch_cap = 50;
    let mut rate_limited = 0;
    while remaining > 0 {
        std::thread::sleep(gap);
        let batch = remaining.min(batch_cap);
        let mut url = format!("https://opentdb.com/api.php?amount={batch}&encode=base64");
        if let Some(c) = q.category {
            url.push_str(&format!("&category={c}"));
        }
        if let Some(d) = q.difficulty {
            url.push_str(&format!("&difficulty={}", d.as_str()));
        }
        if let Some(k) = &q.kind {
            url.push_str(&format!("&type={k}"));
        }
        if let Some(t) = &token {
            url.push_str(&format!("&token={t}"));
        }
        let got = match get(&url).and_then(|body| parse_opentdb(&body)) {
            Ok(got) => got,
            // Too soon after the last request: wait and ask again.
            Err(e) if is_rate_limited(&e) && rate_limited < 3 => {
                rate_limited += 1;
                continue;
            }
            // Not that many left for this category and difficulty: ask for fewer, and keep
            // what earlier batches found.
            Err(ImportError::OpenTdb(1 | 4, _)) if batch > 1 => {
                batch_cap = batch / 2;
                continue;
            }
            Err(ImportError::OpenTdb(1 | 4, _)) if !out.is_empty() => break,
            Err(e) => return Err(e),
        };
        rate_limited = 0;
        if got.is_empty() {
            break;
        }
        remaining -= got.len().min(remaining);
        out.extend(got);
    }
    Ok(out)
}

fn is_rate_limited(e: &ImportError) -> bool {
    match e {
        ImportError::OpenTdb(5, _) => true,
        ImportError::Http(msg) => msg.contains("429"),
        _ => false,
    }
}

// ---------- The Trivia API ----------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TriviaApiQuestion {
    id: String,
    category: String,
    correct_answer: String,
    incorrect_answers: Vec<String>,
    question: TriviaApiText,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    difficulty: String,
}

#[derive(Deserialize)]
struct TriviaApiText {
    text: String,
}

fn map_trivia_api_category(name: &str) -> &'static str {
    match name {
        "music" => "music",
        "film_and_tv" => "movies",
        "sport_and_leisure" => "sports",
        "history" => "history",
        "geography" => "geography",
        "science" => "science",
        "food_and_drink" => "food-drink",
        "society_and_culture" | "arts_and_literature" => "pop-culture",
        _ => "general",
    }
}

/// Turn a Trivia API tag into a TriviaBix tag: `1980's` becomes `80s`, `pop_music` becomes `pop-music`.
fn map_trivia_api_tag(tag: &str) -> String {
    let t = tag.trim().to_lowercase();
    if let Some(decade) = t
        .strip_prefix("19")
        .and_then(|r| r.strip_suffix("0's").or(r.strip_suffix("0s")))
    {
        if decade.len() == 1 {
            return format!("{decade}0s");
        }
    }
    if let Some(decade) = t
        .strip_prefix("20")
        .and_then(|r| r.strip_suffix("0's").or(r.strip_suffix("0s")))
    {
        if decade.len() == 1 {
            return format!("20{decade}0s");
        }
    }
    crate::slugify(&t)
}

/// Parse a response from `https://the-trivia-api.com/v2/questions`.
pub fn parse_trivia_api(json: &str) -> Result<Vec<Question>, ImportError> {
    let items: Vec<TriviaApiQuestion> =
        serde_json::from_str(json).map_err(|e| ImportError::Format(e.to_string()))?;
    Ok(items
        .into_iter()
        .map(|r| {
            let category = map_trivia_api_category(&r.category);
            let mut tags = vec!["trivia-api".to_string()];
            for t in &r.tags {
                let t = map_trivia_api_tag(t);
                if !t.is_empty() && t != category && !tags.contains(&t) {
                    tags.push(t);
                }
            }
            // Film and TV share a category upstream; send obvious TV questions to tv.
            let category =
                if category == "movies" && tags.iter().any(|t| t == "television" || t == "tv") {
                    "tv"
                } else {
                    category
                };
            let mut choices = vec![r.correct_answer.clone()];
            choices.extend(r.incorrect_answers);
            Question {
                id: format!("trivia-api-{}", r.id),
                category: category.into(),
                tags,
                difficulty: parse_difficulty(&r.difficulty),
                rating: Rating::Pg,
                prompt: r.question.text,
                kind: QuestionKind::MultipleChoice {
                    choices,
                    answer: r.correct_answer,
                    keep_order: false,
                },
                fun_fact: None,
                host_quip: None,
                script: vec![],
                source: Some(source(TRIVIA_API_SOURCE)),
            }
        })
        .collect())
}

pub struct TriviaApiQuery {
    pub amount: usize,
    /// Upstream category names, such as `music` or `film_and_tv`.
    pub categories: Vec<String>,
    /// Upstream tags, such as `1980's` or `pop_music`.
    pub tags: Vec<String>,
    pub difficulty: Option<Difficulty>,
}

pub fn fetch_trivia_api(q: &TriviaApiQuery) -> Result<Vec<Question>, ImportError> {
    let agent = agent();
    let mut out: Vec<Question> = Vec::new();
    let mut seen = HashSet::new();
    let mut misses = 0;
    while out.len() < q.amount && misses < 3 {
        let mut url = format!(
            "https://the-trivia-api.com/v2/questions?limit={}",
            (q.amount - out.len()).min(50)
        );
        if !q.categories.is_empty() {
            url.push_str(&format!("&categories={}", q.categories.join(",")));
        }
        if !q.tags.is_empty() {
            url.push_str(&format!("&tags={}", q.tags.join(",").replace('\'', "%27")));
        }
        if let Some(d) = q.difficulty {
            url.push_str(&format!("&difficulties={}", d.as_str()));
        }
        let before = out.len();
        for question in parse_trivia_api(&get_text(&agent, &url)?)? {
            if seen.insert(question.id.clone()) {
                out.push(question);
            }
        }
        if out.len() == before {
            misses += 1;
        }
    }
    out.truncate(q.amount);
    Ok(out)
}

// ---------- shared ----------

fn parse_difficulty(s: &str) -> Difficulty {
    match s {
        "easy" => Difficulty::Easy,
        "hard" => Difficulty::Hard,
        _ => Difficulty::Medium,
    }
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(20))
        .user_agent("triviabix-questions")
        .build()
}

fn get_text(agent: &ureq::Agent, url: &str) -> Result<String, ImportError> {
    agent
        .get(url)
        .call()
        .map_err(|e| ImportError::Http(e.to_string()))?
        .into_string()
        .map_err(|e| ImportError::Http(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(s: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(s)
    }

    #[test]
    fn parses_opentdb_base64() {
        let json = format!(
            r#"{{"response_code":0,"results":[
              {{"type":"{}","difficulty":"{}","category":"{}","question":"{}","correct_answer":"{}","incorrect_answers":["{}","{}","{}"]}},
              {{"type":"{}","difficulty":"{}","category":"{}","question":"{}","correct_answer":"{}","incorrect_answers":["{}"]}}
            ]}}"#,
            enc("multiple"),
            enc("easy"),
            enc("Entertainment: Music"),
            enc("Who sang \"Thriller\"?"),
            enc("Michael Jackson"),
            enc("Prince"),
            enc("Madonna"),
            enc("Whitney Houston"),
            enc("boolean"),
            enc("hard"),
            enc("Geography"),
            enc("Canberra is Australia's capital."),
            enc("True"),
            enc("False"),
        );
        let qs = parse_opentdb(&json).unwrap();
        assert_eq!(qs.len(), 2);
        assert_eq!(qs[0].category, "music");
        assert_eq!(qs[0].prompt, "Who sang \"Thriller\"?");
        assert!(
            matches!(&qs[0].kind, QuestionKind::MultipleChoice { choices, answer, .. }
            if choices.len() == 4 && answer == "Michael Jackson")
        );
        assert_eq!(qs[1].difficulty, Difficulty::Hard);
        assert_eq!(qs[1].kind, QuestionKind::TrueFalse { answer: true });
        assert!(crate::validate_question(&qs[0]).is_empty());
    }

    #[test]
    fn opentdb_error_codes_surface() {
        assert!(matches!(
            parse_opentdb(r#"{"response_code":5,"results":[]}"#),
            Err(ImportError::OpenTdb(5, _))
        ));
    }

    fn opentdb_body(prompts: &[&str]) -> String {
        let results: Vec<String> = prompts
            .iter()
            .map(|p| {
                format!(
                    r#"{{"type":"{}","difficulty":"{}","category":"{}","question":"{}","correct_answer":"{}","incorrect_answers":["{}"]}}"#,
                    enc("multiple"), enc("easy"), enc("Geography"), enc(p), enc("Yes"), enc("No")
                )
            })
            .collect();
        format!(r#"{{"response_code":0,"results":[{}]}}"#, results.join(","))
    }

    fn query(amount: usize) -> OpenTdbQuery {
        OpenTdbQuery {
            amount,
            category: None,
            difficulty: None,
            kind: None,
        }
    }

    #[test]
    fn opentdb_waits_out_the_rate_limit() {
        // The token request counts against the limit, so the first fetch can be refused.
        let mut calls = Vec::new();
        let mut replies = vec![
            Ok(r#"{"response_code":0,"token":"abc"}"#.to_string()),
            Err(ImportError::Http(
                "https://opentdb.com/api.php: status code 429".into(),
            )),
            Ok(r#"{"response_code":5,"results":[]}"#.to_string()),
            Ok(opentdb_body(&["Is it one?", "Is it two?"])),
        ]
        .into_iter();
        let got = fetch_opentdb_with(&query(2), Duration::ZERO, |url| {
            calls.push(url.to_string());
            replies.next().unwrap()
        })
        .unwrap();
        assert_eq!(got.len(), 2);
        assert!(calls[3].contains("amount=2") && calls[3].contains("token=abc"));
    }

    #[test]
    fn opentdb_asks_for_fewer_when_a_category_runs_short() {
        let mut calls = Vec::new();
        let mut replies = vec![
            Err(ImportError::Http("token down".into())),
            Ok(r#"{"response_code":1,"results":[]}"#.to_string()),
            Ok(opentdb_body(&["Is it one?"])),
            Ok(r#"{"response_code":1,"results":[]}"#.to_string()),
        ]
        .into_iter();
        let got = fetch_opentdb_with(&query(2), Duration::ZERO, |url| {
            calls.push(url.to_string());
            replies
                .next()
                .unwrap_or_else(|| Ok(r#"{"response_code":1,"results":[]}"#.into()))
        })
        .unwrap();
        assert_eq!(got.len(), 1);
        assert!(calls[1].contains("amount=2") && !calls[1].contains("token="));
        assert!(calls[2].contains("amount=1"));
    }

    #[test]
    fn parses_trivia_api_and_maps_decade_tags() {
        let json = r#"[{"category":"music","id":"622a1c357cc59eab6f94fd6f","correctAnswer":"Bon Jovi",
          "incorrectAnswers":["Def Leppard","Poison","Whitesnake"],"question":{"text":"Which band released 'Livin' on a Prayer'?"},
          "tags":["music","1980's","rock_music"],"type":"text_choice","difficulty":"medium","regions":[],"isNiche":false}]"#;
        let qs = parse_trivia_api(json).unwrap();
        assert_eq!(qs.len(), 1);
        assert_eq!(qs[0].category, "music");
        assert_eq!(qs[0].tags, vec!["trivia-api", "80s", "rock-music"]);
        assert_eq!(qs[0].id, "trivia-api-622a1c357cc59eab6f94fd6f");
    }

    #[test]
    fn decade_tags() {
        assert_eq!(map_trivia_api_tag("1980's"), "80s");
        assert_eq!(map_trivia_api_tag("1990s"), "90s");
        assert_eq!(map_trivia_api_tag("2000's"), "2000s");
        assert_eq!(map_trivia_api_tag("pop_music"), "pop-music");
    }
}

#[cfg(test)]
mod pack_tests {
    use super::*;
    use crate::{DuplicateIndex, Match, PackFile, PackMeta, PackQuestion};

    #[test]
    fn screening_skips_reworded_questions_already_in_the_bank() {
        let bank = parse_trivia_api(
            r#"[{"category":"music","id":"a","correctAnswer":"Bon Jovi","incorrectAnswers":["Poison"],
              "question":{"text":"Which band sang 'Livin' on a Prayer'?"},"tags":[],"difficulty":"easy"}]"#,
        )
        .unwrap();
        let mut index = DuplicateIndex::of(&bank);
        let fetched = parse_trivia_api(
            r#"[{"category":"music","id":"b","correctAnswer":"Bon Jovi","incorrectAnswers":["Poison"],
                 "question":{"text":"'Livin' on a Prayer' was a hit for which band?"},"tags":[],"difficulty":"easy"},
                {"category":"music","id":"c","correctAnswer":"Queen","incorrectAnswers":["Poison"],
                 "question":{"text":"Which band sang 'Bohemian Rhapsody'?"},"tags":[],"difficulty":"easy"},
                {"category":"music","id":"d","correctAnswer":"Queen","incorrectAnswers":["Poison"],
                 "question":{"text":"Bohemian Rhapsody was sung by which band?"},"tags":[],"difficulty":"easy"},
                {"category":"music","id":"e","correctAnswer":"Queen","incorrectAnswers":["Queen"],
                 "question":{"text":"A broken one"},"tags":[],"difficulty":"easy"}]"#,
        )
        .unwrap();
        let out = screen(fetched, &mut index, false);
        let ids = |v: &[(Question, Match)]| v.iter().map(|(q, _)| q.id.clone()).collect::<Vec<_>>();
        assert_eq!(
            out.fresh.iter().map(|q| q.id.as_str()).collect::<Vec<_>>(),
            vec!["trivia-api-c"]
        );
        // One matches the bank, the other an earlier question in the same batch.
        assert_eq!(ids(&out.skipped), vec!["trivia-api-b", "trivia-api-d"]);
        assert_eq!(out.skipped[0].1.id, "trivia-api-a");
        assert_eq!(out.invalid, 1);
    }

    #[test]
    fn imported_questions_write_and_reload_as_a_pack() {
        let json = r#"[{"category":"music","id":"abc","correctAnswer":"Bon Jovi","incorrectAnswers":["Poison","Whitesnake","Def Leppard"],
          "question":{"text":"Who sang 'Livin' on a Prayer'?"},"tags":["1980's"],"difficulty":"easy"}]"#;
        let qs = parse_trivia_api(json).unwrap();
        let pack = PackFile {
            pack: PackMeta {
                name: "Imported".into(),
                ..Default::default()
            },
            questions: qs.iter().map(PackQuestion::from).collect(),
        };
        let text = pack.to_toml().unwrap();
        let again = PackFile::from_toml(&text).unwrap().resolve("ignored");
        assert_eq!(again, qs);
    }
}
