//! Plans a trivia game as rounds that get harder: easy, medium, hard, and with four rounds a final
//! round of hard questions from every category worth triple points. Each of the first rounds is
//! themed on one category when the question bank has enough of it.

use std::collections::{HashMap, HashSet};

use rand::Rng;
use rand::seq::SliceRandom;

use crate::db::{Difficulty, PoolEntry};

pub const MIN_ROUNDS: u8 = 3;
pub const MAX_ROUNDS: u8 = 4;
pub const MIN_PER_ROUND: usize = 1;
pub const MAX_PER_ROUND: usize = 10;

/// The theme of a round that mixes every category.
pub const MIXED: &str = "Anything Goes";

/// One round of a planned game.
#[derive(Debug, Clone)]
pub struct RoundPlan {
    pub difficulty: Difficulty,
    /// The category every question comes from, or [`MIXED`].
    pub category: String,
    /// Points are scaled by this percentage.
    pub points_pct: u32,
    pub final_round: bool,
    pub ids: Vec<i64>,
}

/// What each round of an `n`-round game is: (difficulty, points percentage, final round).
pub fn ladder(rounds: u8) -> Vec<(Difficulty, u32, bool)> {
    let mut out = vec![
        (Difficulty::Easy, 100, false),
        (Difficulty::Medium, 150, false),
        (Difficulty::Hard, 200, false),
    ];
    if rounds >= 4 {
        out.push((Difficulty::Hard, 300, true));
    }
    out.truncate(rounds.max(1) as usize);
    out
}

/// How the host says a points multiplier out loud.
pub fn points_phrase(pct: u32) -> &'static str {
    match pct {
        ..=100 => "regular points",
        101..=150 => "time and a half",
        151..=200 => "double points",
        _ => "triple points",
    }
}

/// Difficulties to fall back on when a round runs short, closest first.
fn near(d: Difficulty, rng: &mut impl Rng) -> Vec<Difficulty> {
    match d {
        Difficulty::Easy => vec![Difficulty::Medium],
        Difficulty::Hard => vec![Difficulty::Medium],
        Difficulty::Medium => {
            let mut v = vec![Difficulty::Easy, Difficulty::Hard];
            v.shuffle(rng);
            v
        }
    }
}

/// Splits `pool` into rounds of up to `per_round` questions, skipping ids in `exclude`. If that
/// leaves too few questions for a full game, repeats are allowed and the second value is true so
/// the caller can forget what it has already played.
pub fn plan(
    pool: &[PoolEntry],
    rounds: u8,
    per_round: usize,
    exclude: &HashSet<i64>,
    rng: &mut impl Rng,
) -> (Vec<RoundPlan>, bool) {
    let ladder = ladder(rounds);
    let fresh: Vec<&PoolEntry> = pool.iter().filter(|q| !exclude.contains(&q.id)).collect();
    let reset = fresh.len() < ladder.len() * per_round;
    let mut left: Vec<&PoolEntry> = if reset { pool.iter().collect() } else { fresh };
    left.shuffle(rng);

    let mut used_categories = HashSet::new();
    let mut out = Vec::new();
    for (difficulty, points_pct, final_round) in ladder {
        let near = near(difficulty, rng);
        let category = if final_round {
            None
        } else {
            pick_category(&left, difficulty, &near, per_round, &used_categories, rng)
        };
        if let Some(c) = &category {
            used_categories.insert(c.clone());
        }

        // Exact difficulty first, then the nearest ones, in the round's category if it has one.
        let mut ids = Vec::new();
        let mut tiers: Vec<Option<Difficulty>> = vec![Some(difficulty)];
        tiers.extend(near.into_iter().map(Some));
        // A short mixed round takes any difficulty rather than coming up empty.
        if category.is_none() {
            tiers.push(None);
        }
        for tier in tiers {
            let mut i = 0;
            while i < left.len() && ids.len() < per_round {
                let q = left[i];
                let fits = tier.is_none_or(|d| q.difficulty == d)
                    && category.as_ref().is_none_or(|c| q.category == *c);
                if fits {
                    ids.push(left.remove(i).id);
                } else {
                    i += 1;
                }
            }
        }
        if ids.is_empty() {
            continue;
        }
        out.push(RoundPlan {
            difficulty,
            category: category.unwrap_or_else(|| MIXED.to_string()),
            points_pct,
            final_round,
            ids,
        });
    }
    (out, reset)
}

/// A category for a themed round: any that can fill it at exactly this difficulty, otherwise the
/// one with the most at this difficulty that can fill it with nearby ones. None means mix them all.
fn pick_category(
    left: &[&PoolEntry],
    difficulty: Difficulty,
    near: &[Difficulty],
    n: usize,
    used: &HashSet<String>,
    rng: &mut impl Rng,
) -> Option<String> {
    let mut counts: HashMap<&str, (usize, usize)> = HashMap::new();
    for q in left.iter().filter(|q| !used.contains(&q.category)) {
        let c = counts.entry(&q.category).or_default();
        if q.difficulty == difficulty {
            c.0 += 1;
        } else if near.contains(&q.difficulty) {
            c.1 += 1;
        }
    }
    let mut full: Vec<&str> = counts
        .iter()
        .filter(|(_, (exact, _))| *exact >= n)
        .map(|(c, _)| *c)
        .collect();
    full.sort();
    if !full.is_empty() {
        return Some(full[rng.random_range(0..full.len())].to_string());
    }
    let mut close: Vec<(&str, usize)> = counts
        .iter()
        .filter(|(_, (exact, near))| *exact > 0 && exact + near >= n)
        .map(|(c, (exact, _))| (*c, *exact))
        .collect();
    close.sort();
    let best = close.iter().map(|c| c.1).max()?;
    close.retain(|c| c.1 == best);
    Some(close[rng.random_range(0..close.len())].0.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(id: i64, category: &str, difficulty: Difficulty) -> PoolEntry {
        PoolEntry {
            id,
            category: category.into(),
            difficulty,
        }
    }

    fn pool() -> Vec<PoolEntry> {
        let mut v = Vec::new();
        let mut id = 0;
        for (cat, d, n) in [
            ("Movies", Difficulty::Easy, 5),
            ("Science", Difficulty::Medium, 5),
            ("History", Difficulty::Hard, 3),
            ("History", Difficulty::Medium, 2),
            ("Sports", Difficulty::Hard, 4),
            ("Food", Difficulty::Easy, 2),
        ] {
            for _ in 0..n {
                id += 1;
                v.push(q(id, cat, d));
            }
        }
        v
    }

    #[test]
    fn rounds_get_harder_and_stay_on_theme() {
        let pool = pool();
        let (plan, reset) = plan(&pool, 4, 3, &HashSet::new(), &mut rand::rng());
        assert!(!reset);
        let diffs: Vec<_> = plan.iter().map(|r| r.difficulty).collect();
        assert_eq!(
            diffs,
            [
                Difficulty::Easy,
                Difficulty::Medium,
                Difficulty::Hard,
                Difficulty::Hard
            ]
        );
        assert_eq!(
            plan.iter().map(|r| r.points_pct).collect::<Vec<_>>(),
            [100, 150, 200, 300]
        );
        assert!(plan[3].final_round && plan[3].category == MIXED);
        let by_id: HashMap<i64, &PoolEntry> = pool.iter().map(|q| (q.id, q)).collect();
        for r in &plan[..3] {
            assert_eq!(r.ids.len(), 3);
            assert_ne!(r.category, MIXED, "{r:?}");
            assert!(r.ids.iter().all(|id| by_id[id].category == r.category));
            assert!(r.ids.iter().all(|id| by_id[id].difficulty == r.difficulty));
        }
        // No question is asked twice.
        let all: HashSet<i64> = plan.iter().flat_map(|r| r.ids.clone()).collect();
        assert_eq!(all.len(), plan.iter().map(|r| r.ids.len()).sum::<usize>());
    }

    #[test]
    fn short_categories_borrow_nearby_difficulties() {
        // No category has 5 hard questions; History has 3, plus 2 medium ones to top up with.
        let pool = pool();
        let (plan, _) = plan(&pool, 3, 5, &HashSet::new(), &mut rand::rng());
        let hard = plan
            .iter()
            .find(|r| r.difficulty == Difficulty::Hard)
            .unwrap();
        assert_eq!(hard.category, "History");
        assert_eq!(hard.ids.len(), 5);
    }

    #[test]
    fn played_questions_are_skipped_until_they_run_out() {
        let pool = pool();
        let (first, _) = plan(&pool, 3, 3, &HashSet::new(), &mut rand::rng());
        let played: HashSet<i64> = first.iter().flat_map(|r| r.ids.clone()).collect();
        let (second, reset) = plan(&pool, 3, 3, &played, &mut rand::rng());
        assert!(!reset);
        assert!(
            second
                .iter()
                .flat_map(|r| &r.ids)
                .all(|id| !played.contains(id))
        );
        // Asking for more than is left allows repeats.
        let (_, reset) = plan(&pool, 4, 10, &played, &mut rand::rng());
        assert!(reset);
    }

    #[test]
    fn three_rounds_have_no_final() {
        assert_eq!(ladder(3).len(), 3);
        assert!(ladder(3).iter().all(|r| !r.2));
        assert!(ladder(4)[3].2);
    }
}
