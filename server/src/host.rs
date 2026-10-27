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
    ("mini_mash", &[]),
    ("mini_closest", &[]),
    ("mini_quickdraw", &[]),
    ("mini_win", &["name"]),
    ("mini_nobody", &[]),
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

impl Script {
    /// Every line that reads the same in any game (no names or scores in it), so its audio can be
    /// made ahead of time.
    pub fn fixed_lines(&self) -> Vec<String> {
        self.lines
            .values()
            .flatten()
            .map(|(_, text)| text.replace("{host}", &self.host_name))
            .filter(|text| !text.contains('{'))
            .collect()
    }

    /// Lines with placeholders filled in from `combos`, to be voiced ahead of time. `combos` gives
    /// an event's likely values (all of its placeholders together, as they occur in a game). For
    /// each event, rating and set of placeholders, every distinct filling gets `per_tier` of the
    /// lines, chosen by a hash of the filling so the same ones come back every time. Lines with a
    /// placeholder `combos` has no values for are left out.
    pub fn expand(
        &self,
        combos: impl Fn(&str) -> Vec<Vec<(&'static str, String)>>,
        per_tier: impl Fn(&str) -> usize,
    ) -> Vec<String> {
        use std::collections::BTreeSet;
        let mut out = BTreeSet::new();
        for (event, lines) in &self.lines {
            let combos = combos(event);
            if combos.is_empty() {
                continue;
            }
            // (rating, placeholders) -> lines
            let mut groups: BTreeMap<(Rating, Vec<&str>), Vec<&str>> = BTreeMap::new();
            for (rating, text) in lines {
                let vars = placeholders(text);
                if vars.is_empty() || !vars.iter().all(|v| combos[0].iter().any(|(k, _)| k == v)) {
                    continue;
                }
                groups.entry((*rating, vars)).or_default().push(text);
            }
            for ((_, vars), texts) in groups {
                let fillings: BTreeSet<Vec<&str>> = combos
                    .iter()
                    .map(|c| {
                        let get = |v: &&str| c.iter().find(|(k, _)| k == v).unwrap().1.as_str();
                        vars.iter().map(get).collect()
                    })
                    .collect();
                for filling in fillings {
                    let mut ranked = texts.clone();
                    let seed = filling.join("\u{1f}");
                    ranked.sort_by_cached_key(|t| stable_hash(&format!("{seed}\u{1f}{t}")));
                    for text in ranked.into_iter().take(per_tier(event)) {
                        let mut line = text.replace("{host}", &self.host_name);
                        for (var, value) in vars.iter().zip(&filling) {
                            line = line.replace(&format!("{{{var}}}"), value);
                        }
                        out.insert(line);
                    }
                }
            }
        }
        out.into_iter().collect()
    }
}

/// The placeholders in `text` other than `{host}`, sorted.
fn placeholders(text: &str) -> Vec<&str> {
    let mut out: Vec<&str> = text
        .split('{')
        .skip(1)
        .filter_map(|rest| rest.split_once('}').map(|(name, _)| name))
        .filter(|name| *name != "host")
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// A hash that's the same on every run and machine (unlike `std`'s), so pre-made lines are stable.
fn stable_hash(text: &str) -> u64 {
    // FNV-1a
    text.bytes().fold(0xcbf29ce484222325, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    })
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

/// A room's host. Remembers which lines it has used for each event and works through all of
/// them before repeating any, so a long night doesn't get stale.
pub struct Host {
    script: std::sync::Arc<Script>,
    /// Event -> lines said since the event last ran out of fresh material.
    used: HashMap<&'static str, Vec<usize>>,
}

impl Host {
    pub fn new(script: std::sync::Arc<Script>) -> Self {
        Host {
            script,
            used: HashMap::new(),
        }
    }

    pub fn name(&self) -> &str {
        &self.script.host_name
    }

    /// Picks a line for `event` rated at most `rating` that the room hasn't heard lately.
    /// Spicier tiers are weighted heavier (1, 3, 9, 27), so an NC-17 room hears the NC-17 lines
    /// early and often, with tamer ones mixed in once those are used up.
    pub fn say(&mut self, event: &'static str, rating: Rating, vars: &[(&str, &str)]) -> String {
        self.say_voiced(event, rating, vars, |_| false)
    }

    /// Like `say`, but prefers lines for which `voiced` says Bix's own voice is ready, so a game
    /// sticks to pre-made audio wherever it can rather than falling back to the browser's voice.
    pub fn say_voiced(
        &mut self,
        event: &'static str,
        rating: Rating,
        vars: &[(&str, &str)],
        voiced: impl Fn(&str) -> bool,
    ) -> String {
        let lines = &self.script.lines[event];
        let render = |i: usize| {
            let mut text = lines[i].1.replace("{host}", &self.script.host_name);
            for (k, v) in vars {
                text = text.replace(&format!("{{{k}}}"), v);
            }
            text
        };
        let ready: Vec<usize> = (0..lines.len())
            .filter(|&i| lines[i].0 <= rating && voiced(&render(i)))
            .collect();
        let used = self.used.entry(event).or_default();
        if !ready.is_empty() {
            // Fresh voiced lines first; once those are used up, any voiced line but the last one.
            let fresh: Vec<usize> = ready
                .iter()
                .copied()
                .filter(|i| !used.contains(i))
                .collect();
            let pool = if !fresh.is_empty() {
                fresh
            } else if ready.len() > 1 {
                ready
                    .into_iter()
                    .filter(|i| used.last() != Some(i))
                    .collect()
            } else {
                ready
            };
            let pick = weighted(lines, &pool).expect("pool isn't empty");
            used.push(pick);
            return render(pick);
        }
        let pick = match pick_line(lines, rating, used) {
            Some(pick) => pick,
            None => {
                // Everything's been said: start over, but not with the line just used.
                let last = used.pop();
                used.clear();
                used.extend(last);
                pick_line(lines, rating, used)
                    .or(last)
                    .expect("every event has a PG line")
            }
        };
        used.push(pick);
        render(pick)
    }
}

impl Host {
    /// One of a question's own lines for a room rated `rating`, or `None` if it has none, in
    /// which case the host sticks to his generic material.
    pub fn say_own(&self, lines: &[(Rating, String)], rating: Rating) -> Option<String> {
        let pick = pick_line(lines, rating, &[])?;
        Some(lines[pick].1.replace("{host}", &self.script.host_name))
    }
}

/// Picks a line rated at most `rating` that isn't in `skip`, or `None` if there's none left.
/// Spicier tiers are weighted heavier (1, 3, 9, 27).
fn pick_line(lines: &[(Rating, String)], rating: Rating, skip: &[usize]) -> Option<usize> {
    let pool: Vec<usize> = (0..lines.len())
        .filter(|&i| lines[i].0 <= rating && !skip.contains(&i))
        .collect();
    weighted(lines, &pool)
}

/// One of `pool`, spicier lines weighted heavier (1, 3, 9, 27).
fn weighted(lines: &[(Rating, String)], pool: &[usize]) -> Option<usize> {
    let weight = |i: usize| 3u32.pow(lines[i].0 as u32);
    let total: u32 = pool.iter().map(|&i| weight(i)).sum();
    if total == 0 {
        return None;
    }
    let mut roll = rand::rng().random_range(0..total);
    for &i in pool {
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

    /// The built-in script with `change` applied, as JSON text.
    fn edited(change: impl FnOnce(&mut serde_json::Value)) -> String {
        let mut v: serde_json::Value =
            serde_json::from_str(include_str!("../host-script.json")).unwrap();
        change(&mut v["lines"]);
        v.to_string()
    }

    #[test]
    fn lines_with_audio_come_first() {
        let script = std::sync::Arc::new(Script::builtin());
        let mut host = Host::new(script.clone());
        let vars = [("name", "Zelda")];
        let made: Vec<String> = (0..3)
            .map(|_| host.say("join", Rating::Pg, &vars))
            .collect();
        let mut host = Host::new(script);
        for _ in 0..10 {
            let line = host.say_voiced("join", Rating::Pg, &vars, |t| made.iter().any(|m| m == t));
            assert!(made.contains(&line), "{line}");
        }
    }

    #[test]
    fn fixed_lines_have_no_placeholders() {
        let lines = Script::builtin().fixed_lines();
        assert!(!lines.is_empty());
        assert!(lines.iter().all(|l| !l.contains('{')));
    }

    #[test]
    fn bad_scripts_are_rejected() {
        let ok = include_str!("../host-script.json");
        assert!(Script::parse(&ok.replace("\"sign_off\"", "\"sign_of\"")).is_err());
        assert!(Script::parse(&ok.replace("{name} has entered", "{nmae} has entered")).is_err());
        let bad_rating = edited(|lines| {
            let tiers = lines["rematch"].as_object_mut().unwrap();
            let pg13 = tiers.remove("PG-13").unwrap();
            tiers.insert("R".into(), pg13);
        });
        assert!(Script::parse(&bad_rating).is_err());
    }

    #[test]
    fn ratings_cap_the_lines() {
        let text = edited(|lines| {
            for tier in ["MA", "NC-17"] {
                for line in lines["rematch"][tier].as_array_mut().unwrap() {
                    *line = format!("SPICY. {}", line.as_str().unwrap()).into();
                }
            }
        });
        let script = std::sync::Arc::new(Script::parse(&text).unwrap());
        let mut host = Host::new(script.clone());
        for _ in 0..50 {
            assert!(!host.say("rematch", Rating::Pg13, &[]).contains("SPICY"));
        }
        // A fresh NC-17 room usually opens with the spicy material.
        let spicy = (0..200)
            .filter(|_| {
                Host::new(script.clone())
                    .say("rematch", Rating::Nc17, &[])
                    .contains("SPICY")
            })
            .count();
        assert!(
            spicy > 150,
            "spicy tiers should come first in an NC-17 room, got {spicy}/200"
        );
    }

    #[test]
    fn every_line_is_used_before_any_repeats() {
        let script = std::sync::Arc::new(Script::builtin());
        let available = script.lines["question"].len();
        let mut host = Host::new(script);
        let vars = [("round", "1"), ("total", "9"), ("category", "Music")];
        let mut said: Vec<String> = (0..available)
            .map(|_| host.say("question", Rating::Nc17, &vars))
            .collect();
        said.sort();
        said.dedup();
        assert_eq!(said.len(), available, "repeated a line too soon");
        // And the next round of lines doesn't open with the one just said.
        let last = host.say("question", Rating::Nc17, &vars);
        let next = host.say("question", Rating::Nc17, &vars);
        assert_ne!(last, next);
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
