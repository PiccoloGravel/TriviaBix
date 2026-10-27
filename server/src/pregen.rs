//! Host lines worth voicing ahead of time beyond the fixed ones: lines with a category, an
//! answer, a round number or a regular player's name in them. Made in a batch (say on a rented
//! GPU), they let a whole game play in Bix's voice with no TTS service up, since the host
//! prefers lines that already have audio (`Host::say_voiced`).

use crate::db::Db;
use crate::host::Script;
use crate::rounds;

/// How many of the most frequent players' names to voice lines for.
const REGULARS: usize = 12;
/// Streaks the host calls out (from 3 in a row) up to this.
const MAX_STREAK: usize = 8;
/// Question counts to voice "question 3 of 10" lines for: the defaults for a trivia round and for
/// the other games. Other lengths still get lines without the count in them.
const TOTALS: [usize; 2] = [5, 10];

/// Every expanded line, for the voice's background queue. `extra_names` (VOICE_NAMES) are voiced
/// along with the regulars, so names can be ready before anyone's played.
pub async fn lines(script: &Script, db: &Db, extra_names: &[String]) -> Vec<String> {
    let (categories, answers) = match db.voice_values().await {
        Ok(values) => values,
        Err(e) => {
            tracing::error!("listing categories and answers for the voice: {e:#}");
            return Vec::new();
        }
    };
    let mut names = extra_names.to_vec();
    match db.regulars(REGULARS).await {
        Ok(regulars) => names.extend(regulars),
        Err(e) => tracing::error!("listing regular players for the voice: {e:#}"),
    }
    names.sort();
    names.dedup();
    let mut categories = categories;
    categories.push(rounds::MIXED.to_string());
    let values = Values {
        names,
        categories,
        answers,
    };
    script.expand(|event| values.combos(event), per_tier)
}

/// Lines a regular hears every game get more variety.
fn per_tier(event: &str) -> usize {
    match event {
        "join" | "fastest" | "only_one" | "asleep" | "new_leader" | "last_place" => 3,
        _ => 1,
    }
}

struct Values {
    names: Vec<String>,
    categories: Vec<String>,
    answers: Vec<String>,
}

type Combo = Vec<(&'static str, String)>;

impl Values {
    /// An event's placeholders filled in every way a game is likely to fill them.
    fn combos(&self, event: &str) -> Vec<Combo> {
        let one = |key: &'static str, values: &[String]| -> Vec<Combo> {
            values.iter().map(|v| vec![(key, v.clone())]).collect()
        };
        match event {
            "join" | "asleep" | "fastest" | "only_one" | "new_leader" | "last_place"
            | "finale_last" | "finale_win" | "mini_win" => one("name", &self.names),
            "streak" | "streak_broken" => self
                .names
                .iter()
                .flat_map(|n| {
                    (3..=MAX_STREAK)
                        .map(move |s| vec![("name", n.clone()), ("streak", s.to_string())])
                })
                .collect(),
            "reveal_answer" | "nobody" => one("answer", &self.answers),
            "question_first" | "question" | "question_last" => {
                let mut out = Vec::new();
                for total in TOTALS {
                    let rounds: Vec<usize> = match event {
                        "question_first" => vec![1],
                        "question_last" => vec![total],
                        _ => (2..total).collect(),
                    };
                    for round in rounds {
                        for c in &self.categories {
                            out.push(vec![
                                ("category", c.clone()),
                                ("round", round.to_string()),
                                ("total", total.to_string()),
                            ]);
                        }
                    }
                }
                out
            }
            "round_intro" => {
                let mut out = Vec::new();
                for n in rounds::MIN_ROUNDS..=rounds::MAX_ROUNDS {
                    for (i, (difficulty, pct, final_round)) in
                        rounds::ladder(n).into_iter().enumerate()
                    {
                        if final_round {
                            continue;
                        }
                        for c in &self.categories {
                            out.push(vec![
                                ("category", c.clone()),
                                ("round", (i + 1).to_string()),
                                ("total", n.to_string()),
                                ("difficulty", difficulty.name().to_string()),
                                ("points", rounds::points_phrase(pct).to_string()),
                            ]);
                        }
                    }
                }
                out
            }
            "round_final" => rounds::ladder(rounds::MAX_ROUNDS)
                .into_iter()
                .filter(|r| r.2)
                .map(|(_, pct, _)| {
                    vec![
                        ("category", rounds::MIXED.to_string()),
                        ("points", rounds::points_phrase(pct).to_string()),
                    ]
                })
                .collect(),
            "recap_nobody" => (1..=rounds::MAX_ROUNDS as usize)
                .map(|r| vec![("round", r.to_string())])
                .collect(),
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expansions_are_stable_and_cover_each_value() {
        let script = Script::builtin();
        let values = Values {
            names: vec!["Zelda".into(), "Link".into()],
            categories: vec!["Pop Culture".into()],
            answers: vec!["Paris".into()],
        };
        let lines = script.expand(|e| values.combos(e), per_tier);
        assert_eq!(lines, script.expand(|e| values.combos(e), per_tier));
        assert!(
            lines.iter().all(|l| !l.contains('{')),
            "every placeholder is filled"
        );
        for needle in ["Zelda", "Link", "Pop Culture", "Paris"] {
            assert!(lines.iter().any(|l| l.contains(needle)), "{needle}");
        }
    }
}
