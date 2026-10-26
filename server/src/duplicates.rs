//! Questions in the database that ask the same thing, whichever pack, import or seed they came
//! from, for the admin panel's duplicates page. An admin hides one of a pair (hidden questions
//! aren't played, and stay hidden across reloads) or marks the pair as different questions.
//! Also remembers which questions games dealt, so a new game skips recently played ones.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
use serde::Serialize;
use sqlx::Row;
use triviabix_questions::{
    DuplicateIndex, NOT_DUPLICATES, NotDuplicates, Question as BankQuestion, QuestionKind, Rating,
    Verdict,
};

use crate::db::Db;

/// One side of a look-alike pair, as the admin page shows it.
#[derive(Debug, Serialize, Clone)]
pub struct DupeQuestion {
    pub id: i64,
    pub bank_id: Option<String>,
    pub category: String,
    pub prompt: String,
    pub answer: String,
    pub kind: String,
    pub source: String,
    pub rating: String,
}

#[derive(Debug, Serialize)]
pub struct DupePair {
    pub verdict: Verdict,
    pub score: f32,
    pub a: DupeQuestion,
    pub b: DupeQuestion,
}

#[derive(Debug, Serialize)]
pub struct DupeReport {
    /// Surest first.
    pub pairs: Vec<DupePair>,
    /// Questions hidden as duplicates, so they can be brought back.
    pub hidden: Vec<DupeQuestion>,
}

fn row_question(r: &sqlx::sqlite::SqliteRow) -> DupeQuestion {
    DupeQuestion {
        id: r.get("id"),
        bank_id: r.get("bank_id"),
        category: r.get("category"),
        prompt: r.get("prompt"),
        answer: r.get("correct"),
        kind: r.get("kind"),
        source: r.get("source"),
        rating: r.get("rating"),
    }
}

/// The question a row holds, keyed by its database id. Rows from packs keep the whole pack
/// question in `data`; older seeded rows only have their choices.
fn bank_question(r: &sqlx::sqlite::SqliteRow) -> BankQuestion {
    let id: i64 = r.get("id");
    let data: Option<&str> = r.get("data");
    let mut q = data
        .and_then(|d| serde_json::from_str::<BankQuestion>(d).ok())
        .unwrap_or_else(|| {
            let correct: String = r.get("correct");
            let mut choices: Vec<String> =
                serde_json::from_str(r.get::<&str, _>("wrong")).unwrap_or_default();
            choices.push(correct.clone());
            BankQuestion {
                id: String::new(),
                category: r.get("category"),
                tags: vec![],
                difficulty: Default::default(),
                rating: Rating::parse(r.get("rating")).unwrap_or_default(),
                prompt: r.get("prompt"),
                kind: QuestionKind::MultipleChoice {
                    choices,
                    answer: correct,
                    keep_order: false,
                },
                fun_fact: None,
                host_quip: None,
                script: vec![],
                source: None,
            }
        });
    q.id = id.to_string();
    q
}

impl Db {
    /// Every look-alike pair among the questions that can still be played, skipping pairs kept
    /// in the admin panel or in `<content>/not-duplicates.toml`.
    pub async fn duplicate_report(&self, content_dir: &Path) -> anyhow::Result<DupeReport> {
        let rows = sqlx::query(
            "SELECT id, bank_id, category, prompt, correct, wrong, rating, kind, source, data, hidden
             FROM questions WHERE active = 1 ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut shown = HashMap::new();
        let mut by_bank_id = HashMap::new();
        let mut hidden = Vec::new();
        let mut index = DuplicateIndex::new();
        for r in &rows {
            let q = row_question(r);
            if r.get::<i64, _>("hidden") != 0 {
                hidden.push(q);
                continue;
            }
            index.add(&bank_question(r));
            if let Some(b) = &q.bank_id {
                by_bank_id.insert(b.clone(), q.id);
            }
            shown.insert(q.id, q);
        }
        for r in sqlx::query("SELECT a, b FROM question_kept_pairs")
            .fetch_all(&self.pool)
            .await?
        {
            index.keep_pair(
                &r.get::<i64, _>("a").to_string(),
                &r.get::<i64, _>("b").to_string(),
            );
        }
        let file = content_dir.join(NOT_DUPLICATES);
        let kept = NotDuplicates::read(&file)
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("reading {}", file.display()))?;
        for (a, b) in kept.ids() {
            if let (Some(a), Some(b)) = (by_bank_id.get(a), by_bank_id.get(b)) {
                index.keep_pair(&a.to_string(), &b.to_string());
            }
        }

        let pairs = index
            .pairs()
            .into_iter()
            .filter_map(|p| {
                let a = shown.get(&p.a.parse().ok()?)?.clone();
                let b = shown.get(&p.b.parse().ok()?)?.clone();
                Some(DupePair {
                    verdict: p.verdict,
                    score: p.score,
                    a,
                    b,
                })
            })
            .collect();
        Ok(DupeReport { pairs, hidden })
    }

    /// Hides a question as a duplicate, or brings it back.
    pub async fn set_question_hidden(&self, id: i64, hidden: bool) -> anyhow::Result<bool> {
        let done = sqlx::query("UPDATE questions SET hidden = ?1 WHERE id = ?2")
            .bind(hidden as i64)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(done.rows_affected() == 1)
    }

    /// Records that two look-alike questions are different, so they stop being reported.
    pub async fn keep_question_pair(&self, a: i64, b: i64) -> anyhow::Result<()> {
        anyhow::ensure!(a != b, "a question can't duplicate itself");
        sqlx::query("INSERT OR IGNORE INTO question_kept_pairs (a, b) VALUES (?1, ?2)")
            .bind(a.min(b))
            .bind(a.max(b))
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Remembers that a game dealt these questions.
    pub async fn record_plays(&self, ids: &[i64]) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO question_plays (question_id)
             SELECT value FROM json_each(?1) WHERE value IN (SELECT id FROM questions)",
        )
        .bind(serde_json::to_string(ids)?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The `n` questions played most recently in any room, newest first.
    pub async fn recently_played(&self, n: usize) -> anyhow::Result<Vec<i64>> {
        let rows = sqlx::query(
            "SELECT question_id FROM question_plays GROUP BY question_id
             ORDER BY MAX(rowid) DESC LIMIT ?1",
        )
        .bind(n as i64)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(|r| r.get("question_id")).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn db() -> Db {
        let path = std::env::temp_dir().join(format!("tbx-dupes-{}.db", uuid::Uuid::new_v4()));
        Db::connect(&format!("sqlite://{}", path.display()))
            .await
            .unwrap()
    }

    async fn add(db: &Db, prompt: &str, answer: &str) -> i64 {
        sqlx::query(
            "INSERT INTO questions (category, prompt, correct, wrong, source)
             VALUES ('General', ?1, ?2, '[\"Nope\"]', 'test') RETURNING id",
        )
        .bind(prompt)
        .bind(answer)
        .fetch_one(&db.pool)
        .await
        .unwrap()
        .get(0)
    }

    #[tokio::test]
    async fn reports_reworded_questions_and_acts_on_them() {
        let db = db().await;
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../content");
        crate::content::load(&db, &dir).await.unwrap();
        // The starter content has no duplicates of its own.
        let report = db.duplicate_report(&dir).await.unwrap();
        assert!(report.pairs.is_empty(), "{:?}", report.pairs);

        let a = add(&db, "Which bird lays the largest eggs?", "Ostrich").await;
        let b = add(
            &db,
            "The largest eggs of any bird are laid by which bird?",
            "Ostrich",
        )
        .await;
        let report = db.duplicate_report(&dir).await.unwrap();
        assert_eq!(report.pairs.len(), 1);
        let pair = &report.pairs[0];
        assert_eq!(pair.verdict, Verdict::Duplicate);
        assert_eq!((pair.a.id, pair.b.id), (a, b));

        // Kept pairs stop being reported.
        db.keep_question_pair(b, a).await.unwrap();
        assert!(db.duplicate_report(&dir).await.unwrap().pairs.is_empty());

        // A hidden question isn't played, survives a reload, and is listed so it can come back.
        let c = add(&db, "Which bird lays the biggest eggs of all?", "Ostrich").await;
        let report = db.duplicate_report(&dir).await.unwrap();
        assert_eq!(report.pairs.len(), 1);
        assert_eq!((report.pairs[0].a.id, report.pairs[0].b.id), (a, c));
        assert!(db.set_question_hidden(c, true).await.unwrap());
        crate::content::load(&db, &dir).await.unwrap();
        let report = db.duplicate_report(&dir).await.unwrap();
        assert!(report.pairs.is_empty());
        assert_eq!(
            report.hidden.iter().map(|q| q.id).collect::<Vec<_>>(),
            vec![c]
        );
        let pool = db.question_pool(crate::db::Rating::Nc17).await.unwrap();
        assert!(pool.iter().any(|q| q.id == a));
        assert!(!pool.iter().any(|q| q.id == c));
    }

    #[tokio::test]
    async fn remembers_recent_plays_across_rooms() {
        let db = db().await;
        let ids: Vec<i64> = sqlx::query("SELECT id FROM questions ORDER BY id LIMIT 5")
            .fetch_all(&db.pool)
            .await
            .unwrap()
            .iter()
            .map(|r| r.get(0))
            .collect();
        db.record_plays(&ids[..3]).await.unwrap();
        db.record_plays(&[ids[3], ids[0], 999_999]).await.unwrap();
        assert_eq!(
            db.recently_played(3).await.unwrap(),
            vec![ids[0], ids[3], ids[2]]
        );
    }
}
