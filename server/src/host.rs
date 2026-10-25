//! The game show host: picks lines from a script for each moment of the game and fills in names,
//! answers and scores. The script lives in `server/host-script.json`, grouped by rating so higher
//! rated rooms get sassier material.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, bail};
use rand::Rng;
use serde::Deserialize;

use crate::db::Rating;

/// Every event the game can call for, and the placeholders it fills in. `{host}` works everywhere.
pub const EVENTS: &[(&str, &[&str])] = &[
    ("room_open", &["code"]),
    ("join", &["name"]),
    ("rematch", &[]),
    ("intro", &["count", "rounds", "names"]),
    ("question_first", &["round", "total", "category"]),
    ("question", &["round", "total", "category"]),
    ("question_last", &["round", "total", "category"]),
    ("reveal_answer", &["answer"]),
    ("nobody", &["answer"]),
    ("everyone", &[]),
    ("only_one", &["name"]),
    ("fastest", &["name"]),
    ("streak", &["name", "streak"]),
    ("streak_broken", &["name", "streak"]),
    ("roast", &["name", "wrong"]),
    ("asleep", &["name"]),
    ("leader", &["name", "gap"]),
    ("new_leader", &["name"]),
    ("tie", &["names"]),
    ("last_place", &["name"]),
    ("one_left", &[]),
    (
        "round_intro",
        &["round", "total", "category", "difficulty", "points"],
    ),
    ("round_final", &["category", "points"]),
    ("recap", &["round", "name", "points"]),
    ("recap_nobody", &["round"]),
    ("finale_win", &["name", "score"]),
    ("finale_tie", &["names"]),
    ("finale_last", &["name"]),
    ("sign_off", &[]),
];

const RATINGS: [(&str, Rating); 4] = [
    ("PG", Rating::Pg),
    ("PG-13", Rating::Pg13),
    ("MA", Rating::Ma),
    ("NC-17", Rating::Nc17),
];

#[derive(Deserialize)]
struct RawScript {
    host_name: String,
    lines: BTreeMap<String, BTreeMap<String, Vec<String>>>,
}

#[derive(Debug)]
pub struct Script {
    pub host_name: String,
    lines: HashMap<&'static str, Vec<(Rating, String)>>,
}

impl Script {
    /// The script that ships with the server.
    pub fn builtin() -> Self {
        Self::parse(include_str!("../host-script.json")).expect("built-in host script is valid")
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("in host script {}", path.display()))
    }

    /// Parses and checks a script: known events and ratings, a PG line for every event so any room
    /// has something to say, and only placeholders the event actually fills in.
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let raw: RawScript = serde_json::from_str(text)?;
        let mut lines = HashMap::new();
        for (event, tiers) in &raw.lines {
            let Some(&(key, vars)) = EVENTS.iter().find(|(k, _)| k == event) else {
                bail!("unknown event {event:?}");
            };
            let mut out = Vec::new();
            for (rating, texts) in tiers {
                let Some(&(_, rating)) = RATINGS.iter().find(|(name, _)| name == rating) else {
                    bail!("{event}: unknown rating {rating:?} (use PG, PG-13, MA or NC-17)");
                };
                for text in texts {
                    check_placeholders(text, vars).with_context(|| format!("{event}: {text:?}"))?;
                    out.push((rating, text.clone()));
                }
            }
            if !out.iter().any(|(r, _)| *r == Rating::Pg) {
                bail!("{event}: needs at least one PG line");
            }
            lines.insert(key, out);
        }
        if let Some((missing, _)) = EVENTS.iter().find(|(k, _)| !lines.contains_key(k)) {
            bail!("missing event {missing:?}");
        }
        Ok(Script {
            host_name: raw.host_name,
            lines,
        })
    }
}

fn check_placeholders(text: &str, vars: &[&str]) -> anyhow::Result<()> {
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        let Some(len) = rest[start..].find('}') else {
            bail!("unclosed {{")
        };
        let name = &rest[start + 1..start + len];
        if name != "host" && !vars.contains(&name) {
            bail!("unknown placeholder {{{name}}}; this event has {vars:?}");
        }
        rest = &rest[start + len + 1..];
    }
    Ok(())
}

/// A room's host. Remembers the last line used for each event so it doesn't repeat itself.
pub struct Host {
    script: std::sync::Arc<Script>,
    last: HashMap<&'static str, usize>,
}

impl Host {
    pub fn new(script: std::sync::Arc<Script>) -> Self {
        Host {
            script,
            last: HashMap::new(),
        }
    }

    pub fn name(&self) -> &str {
        &self.script.host_name
    }

    /// Picks a line for `event` rated at most `rating`. Spicier tiers are weighted heavier
    /// (1, 3, 9, 27), so an NC-17 room mostly hears NC-17 lines with tamer ones mixed in.
    pub fn say(&mut self, event: &'static str, rating: Rating, vars: &[(&str, &str)]) -> String {
        let lines = &self.script.lines[event];
        let pick = pick_line(lines, rating, self.last.get(event).copied())
            .expect("every event has a PG line");
        self.last.insert(event, pick);

        let mut text = lines[pick].1.replace("{host}", &self.script.host_name);
        for (k, v) in vars {
            text = text.replace(&format!("{{{k}}}"), v);
        }
        text
    }
}

impl Host {
    /// One of a question's own lines for a room rated `rating`, or `None` if it has none, in
    /// which case the host sticks to his generic material.
    pub fn say_own(&self, lines: &[(Rating, String)], rating: Rating) -> Option<String> {
        let pick = pick_line(lines, rating, None)?;
        Some(lines[pick].1.replace("{host}", &self.script.host_name))
    }
}

/// Picks a line rated at most `rating`, avoiding `last` when there's a choice. Spicier tiers
/// are weighted heavier (1, 3, 9, 27).
fn pick_line(lines: &[(Rating, String)], rating: Rating, last: Option<usize>) -> Option<usize> {
    let weight = |i: usize| 3u32.pow(lines[i].0 as u32);
    let mut pool: Vec<usize> = (0..lines.len()).filter(|&i| lines[i].0 <= rating).collect();
    if pool.len() > 1 {
        pool.retain(|&i| last != Some(i));
    }
    let total: u32 = pool.iter().map(|&i| weight(i)).sum();
    if total == 0 {
        return None;
    }
    let mut roll = rand::rng().random_range(0..total);
    for &i in &pool {
        if roll < weight(i) {
            return Some(i);
        }
        roll -= weight(i);
    }
    pool.last().copied()
}

/// "Ann", "Ann and Bo", "Ann, Bo and Cy", or "Ann, Bo, Cy and 4 others" for big groups.
pub fn name_list(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => one.to_string(),
        _ if names.len() > 5 => format!("{} and {} others", names[..3].join(", "), names.len() - 3),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Roughly how long the TV's voice takes to say `text`, so the game can wait for the host to finish.
pub fn talk_time(text: &str, chars_per_sec: f64) -> Duration {
    if chars_per_sec <= 0.0 {
        return Duration::ZERO;
    }
    Duration::from_secs_f64(text.chars().count() as f64 / chars_per_sec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_script_is_valid() {
        let script = Script::builtin();
        assert_eq!(script.lines.len(), EVENTS.len());
    }

    #[test]
    fn bad_scripts_are_rejected() {
        let ok = include_str!("../host-script.json");
        assert!(Script::parse(&ok.replace("\"sign_off\"", "\"sign_of\"")).is_err());
        assert!(Script::parse(&ok.replace("{name} has entered", "{nmae} has entered")).is_err());
        assert!(Script::parse(&ok.replace("\"PG-13\": [\"Rematch", "\"R\": [\"Rematch")).is_err());
    }

    #[test]
    fn ratings_cap_the_lines() {
        let script = std::sync::Arc::new(
            Script::parse(
                &include_str!("../host-script.json")
                    .replace("\"NC-17\": [\"Round two, baby.", "\"NC-17\": [\"SPICY.")
                    .replace("\"MA\": [\"Again?", "\"MA\": [\"SPICY. Again?"),
            )
            .unwrap(),
        );
        let mut host = Host::new(script);
        for _ in 0..50 {
            assert!(!host.say("rematch", Rating::Pg13, &[]).contains("SPICY"));
        }
        let spicy = (0..200)
            .filter(|_| host.say("rematch", Rating::Nc17, &[]).contains("SPICY"))
            .count();
        assert!(
            spicy > 100,
            "spicy tiers should dominate an NC-17 room, got {spicy}/200"
        );
    }

    #[test]
    fn question_lines_respect_the_room() {
        let host = Host::new(std::sync::Arc::new(Script::builtin()));
        let lines = vec![
            (Rating::Pg, "Tame, says {host}.".to_string()),
            (Rating::Ma, "Spicy.".to_string()),
        ];
        assert_eq!(host.say_own(&[], Rating::Nc17), None);
        for _ in 0..30 {
            assert_eq!(
                host.say_own(&lines, Rating::Pg13).as_deref(),
                Some("Tame, says Bix.")
            );
        }
        assert_eq!(host.say_own(&lines[1..], Rating::Pg), None);
        let spicy = (0..200)
            .filter(|_| host.say_own(&lines, Rating::Ma).as_deref() == Some("Spicy."))
            .count();
        assert!(spicy > 100, "{spicy}/200");
    }

    #[test]
    fn placeholders_are_filled() {
        let mut host = Host::new(std::sync::Arc::new(Script::builtin()));
        let line = host.say(
            "roast",
            Rating::Nc17,
            &[("name", "Sam"), ("wrong", "Paris")],
        );
        assert!(!line.contains('{'), "{line}");
        assert_eq!(name_list(&["A", "B", "C"]), "A, B and C");
        assert_eq!(
            name_list(&["A", "B", "C", "D", "E", "F"]),
            "A, B, C and 3 others"
        );
    }
}
