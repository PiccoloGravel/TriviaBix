//! Loads the question bank (content/packs, imports included, plus content/scripts) into the
//! database, so games and the admin panel work from one place. Runs at startup and when the
//! admin panel asks for a reload.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::Context;
use serde::Serialize;
use sqlx::Row;
use triviabix_questions::{Question as BankQuestion, QuestionBank, QuestionKind, normalize};

use crate::db::Db;

/// What a reload changed.
#[derive(Debug, Default, Serialize, PartialEq)]
pub struct SyncReport {
    pub added: usize,
    pub updated: usize,
    /// Questions no longer in any pack. They stop being played but stay in the database.
    pub retired: usize,
    pub lines: usize,
    /// Problems in the content that didn't stop it from loading.
    pub warnings: Vec<String>,
}

/// Reads the content folder and loads it into the database, with the host lines of the persona
/// in use (crate::personas).
pub async fn load(db: &Db, dir: &Path) -> anyhow::Result<SyncReport> {
    let owned = dir.to_path_buf();
    let persona = crate::personas::active(db).await;
    let bank = tokio::task::spawn_blocking(move || QuestionBank::load_dir_as(&owned, &persona))
        .await?
        .with_context(|| format!("loading questions from {}", dir.display()))?;
    db.sync_bank(&bank).await
}

/// How a pack question is stored in the `questions` table.
struct Stored {
    correct: String,
    wrong: Vec<String>,
    choices: Option<Vec<String>>,
    keep_order: bool,
}

fn stored(q: &BankQuestion) -> Stored {
    match &q.kind {
        QuestionKind::MultipleChoice {
            choices,
            answer,
            keep_order,
        } => Stored {
            correct: answer.trim().to_string(),
            wrong: choices
                .iter()
                .filter(|c| c.trim() != answer.trim())
                .cloned()
                .collect(),
            choices: Some(choices.clone()),
            keep_order: *keep_order,
        },
        QuestionKind::TrueFalse { answer } => {
            let (right, wrong) = if *answer {
                ("True", "False")
            } else {
                ("False", "True")
            };
            Stored {
                correct: right.into(),
                wrong: vec![wrong.into()],
                choices: Some(vec!["True".into(), "False".into()]),
                keep_order: true,
            }
        }
        _ => Stored {
            correct: q.answer_text(),
            wrong: vec![],
            choices: None,
            keep_order: false,
        },
    }
}

impl Db {
    /// Makes the `questions` table match the bank. Pack questions are matched by id, or by prompt
    /// the first time (which adopts the old seeded rows), and their content lines are rewritten.
    /// Questions that left the bank are retired. Lines edited in the admin panel are kept.
    pub async fn sync_bank(&self, bank: &QuestionBank) -> anyhow::Result<SyncReport> {
        let mut report = SyncReport {
            warnings: bank.warnings().iter().map(|w| w.to_string()).collect(),
            ..Default::default()
        };
        let names: HashMap<&str, &str> = bank
            .categories()
            .iter()
            .map(|c| (c.slug.as_str(), c.name.as_str()))
            .collect();
        let current: HashSet<&str> = bank.questions().iter().map(|q| q.id.as_str()).collect();

        let mut tx = self.pool.begin().await?;
        let mut by_bank_id = HashMap::new();
        let mut by_prompt = HashMap::new();
        for row in sqlx::query("SELECT id, bank_id, prompt FROM questions")
            .fetch_all(&mut *tx)
            .await?
        {
            let id: i64 = row.get("id");
            let bank_id: Option<String> = row.get("bank_id");
            match bank_id {
                Some(b) if current.contains(b.as_str()) => {
                    by_bank_id.insert(b, id);
                }
                // Rows the bank doesn't claim can be adopted by a question with the same prompt.
                _ => {
                    by_prompt.insert(normalize(row.get("prompt")), id);
                }
            }
        }

        let mut kept = HashSet::new();
        for q in bank.questions() {
            let s = stored(q);
            let existing = by_bank_id
                .get(&q.id)
                .or_else(|| by_prompt.get(&normalize(&q.prompt)))
                .copied();
            let source = q.source.as_ref().map_or("pack", |s| s.name.as_str());
            let category = names
                .get(q.category.as_str())
                .copied()
                .unwrap_or(&q.category);
            let query = match existing {
                Some(_) => {
                    "UPDATE questions SET bank_id = ?1, category = ?2, prompt = ?3, correct = ?4,
                         wrong = ?5, rating = ?6, difficulty = ?7, kind = ?8, tags = ?9,
                         choices = ?10, keep_order = ?11, data = ?12, source = ?13, active = 1
                     WHERE id = ?14 RETURNING id"
                }
                None => {
                    "INSERT INTO questions (bank_id, category, prompt, correct, wrong, rating,
                         difficulty, kind, tags, choices, keep_order, data, source)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13) RETURNING id"
                }
            };
            let id: i64 = sqlx::query(query)
                .bind(&q.id)
                .bind(category)
                .bind(&q.prompt)
                .bind(&s.correct)
                .bind(serde_json::to_string(&s.wrong)?)
                .bind(q.rating.as_str())
                .bind(q.difficulty.as_str())
                .bind(q.question_type().as_str())
                .bind(serde_json::to_string(&q.tags)?)
                .bind(s.choices.map(|c| serde_json::to_string(&c)).transpose()?)
                .bind(s.keep_order as i64)
                .bind(serde_json::to_string(q)?)
                .bind(source)
                .bind(existing)
                .fetch_one(&mut *tx)
                .await
                .with_context(|| format!("saving question {}", q.id))?
                .get("id");
            if existing.is_some() {
                report.updated += 1;
            } else {
                report.added += 1;
            }
            kept.insert(id);

            sqlx::query("DELETE FROM question_lines WHERE question_id = ?1 AND origin = 'content'")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            for line in q.host_lines() {
                sqlx::query(
                    "INSERT INTO question_lines (question_id, slot, rating, text, origin)
                     VALUES (?1, ?2, ?3, ?4, 'content')",
                )
                .bind(id)
                .bind(line.slot.as_str())
                .bind(line.rating.as_str())
                .bind(&line.text)
                .execute(&mut *tx)
                .await?;
                report.lines += 1;
            }
        }

        for row in sqlx::query("SELECT id FROM questions WHERE bank_id IS NOT NULL AND active = 1")
            .fetch_all(&mut *tx)
            .await?
        {
            let id: i64 = row.get("id");
            if !kept.contains(&id) {
                sqlx::query("UPDATE questions SET active = 0 WHERE id = ?1")
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
                report.retired += 1;
            }
        }
        tx.commit().await?;
        Ok(report)
    }
}

/// A question as the admin panel lists it, with every host line it has.
#[derive(Debug, Serialize)]
pub struct AdminQuestion {
    pub id: i64,
    pub bank_id: Option<String>,
    pub category: String,
    pub prompt: String,
    pub answer: String,
    pub rating: String,
    pub difficulty: String,
    pub kind: String,
    pub source: String,
    pub active: bool,
    pub lines: Vec<AdminLine>,
}

#[derive(Debug, Serialize, serde::Deserialize, Clone, PartialEq)]
pub struct AdminLine {
    #[serde(default)]
    pub slot: String,
    pub rating: String,
    pub text: String,
    /// `content` (from the packs and script files) or `edited` (from the admin panel).
    #[serde(default)]
    pub origin: String,
}

/// Which questions the admin panel lists.
#[derive(Debug, Default, serde::Deserialize)]
pub struct QuestionSearch {
    /// Words in the prompt, answer, category, or a host line.
    #[serde(default)]
    pub q: String,
    /// `lead_in` or `reveal` to list only questions missing those lines.
    #[serde(default)]
    pub missing: Option<String>,
    #[serde(default)]
    pub offset: i64,
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct QuestionPage {
    pub total: i64,
    pub questions: Vec<AdminQuestion>,
    /// Across every active question: how many have their own lead-in and reveal lines.
    pub active: i64,
    pub with_lead_in: i64,
    pub with_reveal: i64,
}

impl Db {
    pub async fn admin_questions(&self, search: &QuestionSearch) -> anyhow::Result<QuestionPage> {
        let like = format!("%{}%", search.q.trim());
        let missing = search.missing.as_deref().unwrap_or("");
        let limit = search.limit.unwrap_or(50).clamp(1, 200);
        // `has(slot)`: the question has a line for the slot that isn't muted.
        const FILTER: &str = "q.active = 1
            AND (?1 = '%%' OR q.prompt LIKE ?1 OR q.correct LIKE ?1 OR q.category LIKE ?1
                 OR q.bank_id LIKE ?1
                 OR EXISTS (SELECT 1 FROM question_lines l WHERE l.question_id = q.id AND l.text LIKE ?1))
            AND (?2 = '' OR NOT EXISTS (SELECT 1 FROM question_lines l
                 WHERE l.question_id = q.id AND l.slot = ?2 AND l.text <> ''))";
        let total: i64 = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*) FROM questions q WHERE {FILTER}"
        )))
        .bind(&like)
        .bind(missing)
        .fetch_one(&self.pool)
        .await?
        .get(0);
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT id, bank_id, category, prompt, correct, rating, difficulty, kind, source, active
             FROM questions q WHERE {FILTER} ORDER BY category, prompt LIMIT ?3 OFFSET ?4"
        )))
        .bind(&like)
        .bind(missing)
        .bind(limit)
        .bind(search.offset.max(0))
        .fetch_all(&self.pool)
        .await?;
        let mut questions: Vec<AdminQuestion> = rows
            .into_iter()
            .map(|r| AdminQuestion {
                id: r.get("id"),
                bank_id: r.get("bank_id"),
                category: r.get("category"),
                prompt: r.get("prompt"),
                answer: r.get("correct"),
                rating: r.get("rating"),
                difficulty: r.get("difficulty"),
                kind: r.get("kind"),
                source: r.get("source"),
                active: r.get::<i64, _>("active") != 0,
                lines: vec![],
            })
            .collect();
        let ids: Vec<i64> = questions.iter().map(|q| q.id).collect();
        let lines = sqlx::query(
            "SELECT question_id, slot, rating, text, origin FROM question_lines
             WHERE question_id IN (SELECT value FROM json_each(?1)) ORDER BY origin DESC, slot, id",
        )
        .bind(serde_json::to_string(&ids)?)
        .fetch_all(&self.pool)
        .await?;
        for l in lines {
            let qid: i64 = l.get("question_id");
            if let Some(q) = questions.iter_mut().find(|q| q.id == qid) {
                q.lines.push(AdminLine {
                    slot: l.get("slot"),
                    rating: l.get("rating"),
                    text: l.get("text"),
                    origin: l.get("origin"),
                });
            }
        }
        let counts = sqlx::query(
            "SELECT COUNT(*) AS active,
                SUM(EXISTS (SELECT 1 FROM question_lines l WHERE l.question_id = q.id
                    AND l.slot = 'lead_in' AND l.text <> '')) AS lead_in,
                SUM(EXISTS (SELECT 1 FROM question_lines l WHERE l.question_id = q.id
                    AND l.slot = 'reveal' AND l.text <> '')) AS reveal
             FROM questions q WHERE active = 1",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(QuestionPage {
            total,
            questions,
            active: counts.get("active"),
            with_lead_in: counts.get::<Option<i64>, _>("lead_in").unwrap_or(0),
            with_reveal: counts.get::<Option<i64>, _>("reveal").unwrap_or(0),
        })
    }

    /// Replaces a question's hand-edited lines for one slot. They take over from the pack's
    /// lines and survive reloads. An empty list goes back to the pack's lines; a single line
    /// with no text keeps the host quiet for that slot.
    pub async fn set_edited_lines(
        &self,
        question_id: i64,
        slot: &str,
        lines: &[AdminLine],
    ) -> Result<(), String> {
        if !matches!(slot, "lead_in" | "reveal") {
            return Err("Lines go in lead_in or reveal.".into());
        }
        for l in lines {
            if crate::db::Rating::parse(&l.rating).is_none() {
                return Err(format!("Unknown rating {:?}.", l.rating));
            }
            let line = triviabix_questions::ScriptLine {
                slot: triviabix_questions::ScriptSlot::LeadIn,
                rating: triviabix_questions::Rating::Pg,
                text: l.text.clone(),
            };
            let problems = triviabix_questions::validate_line(&line);
            if let Some(p) = problems
                .iter()
                .find(|p| !(lines.len() == 1 && p.contains("empty")))
            {
                return Err(p.clone());
            }
        }
        let db_err = |e: sqlx::Error| {
            tracing::error!("saving host lines: {e}");
            "Couldn't save the lines.".to_string()
        };
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let exists = sqlx::query("SELECT 1 FROM questions WHERE id = ?1")
            .bind(question_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_err)?;
        if exists.is_none() {
            return Err("No question with that id.".into());
        }
        sqlx::query(
            "DELETE FROM question_lines WHERE question_id = ?1 AND slot = ?2 AND origin = 'edited'",
        )
        .bind(question_id)
        .bind(slot)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        for l in lines {
            sqlx::query(
                "INSERT INTO question_lines (question_id, slot, rating, text, origin)
                 VALUES (?1, ?2, ?3, ?4, 'edited')",
            )
            .bind(question_id)
            .bind(slot)
            .bind(&l.rating)
            .bind(l.text.trim())
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use triviabix_questions::{Category, Rating, ScriptLine, ScriptSlot};

    fn bank(questions: Vec<BankQuestion>) -> QuestionBank {
        let cat = Category {
            slug: "geography".into(),
            name: "Geography".into(),
            emoji: String::new(),
            color: String::new(),
            description: String::new(),
            rating: Rating::Pg,
        };
        QuestionBank::new(vec![cat], questions).unwrap()
    }

    fn question(id: &str, prompt: &str) -> BankQuestion {
        BankQuestion {
            id: id.into(),
            category: "geography".into(),
            tags: vec![],
            difficulty: Default::default(),
            rating: Rating::Pg,
            prompt: prompt.into(),
            kind: QuestionKind::MultipleChoice {
                choices: vec!["Canberra".into(), "Sydney".into()],
                answer: "Canberra".into(),
                keep_order: false,
            },
            fun_fact: Some("It was built as a compromise.".into()),
            host_quip: None,
            script: vec![ScriptLine {
                slot: ScriptSlot::LeadIn,
                rating: Rating::Pg,
                text: "Down under we go.".into(),
            }],
            source: None,
        }
    }

    async fn db() -> Db {
        let path = std::env::temp_dir().join(format!("tbx-content-{}.db", uuid::Uuid::new_v4()));
        Db::connect(&format!("sqlite://{}", path.display()))
            .await
            .unwrap()
    }

    async fn count(db: &Db, sql: &'static str) -> i64 {
        sqlx::query(sql).fetch_one(&db.pool).await.unwrap().get(0)
    }

    #[tokio::test]
    async fn adopts_seeds_then_updates_and_retires() {
        let db = db().await;
        let seeds = count(&db, "SELECT COUNT(*) FROM questions").await;
        // A seeded prompt, with different punctuation, is adopted rather than duplicated.
        let q = question("geo-1", "what is the capital of australia");
        let report = db.sync_bank(&bank(vec![q.clone()])).await.unwrap();
        assert_eq!((report.added, report.updated, report.lines), (0, 1, 2));
        assert_eq!(count(&db, "SELECT COUNT(*) FROM questions").await, seeds);
        assert_eq!(
            count(&db, "SELECT COUNT(*) FROM questions WHERE bank_id = 'geo-1' AND category = 'Geography' AND source = 'pack'").await,
            1
        );

        // Edited lines survive a reload; content lines are rewritten, not duplicated.
        sqlx::query("INSERT INTO question_lines (question_id, slot, rating, text, origin) SELECT id, 'reveal', 'PG', 'Mine.', 'edited' FROM questions WHERE bank_id = 'geo-1'")
            .execute(&db.pool)
            .await
            .unwrap();
        let mut q2 = question("geo-2", "Which ocean is the biggest? (new)");
        q2.kind = QuestionKind::TrueFalse { answer: false };
        let report = db.sync_bank(&bank(vec![q.clone(), q2])).await.unwrap();
        assert_eq!((report.added, report.updated, report.retired), (1, 1, 0));
        assert_eq!(count(&db, "SELECT COUNT(*) FROM question_lines").await, 5);
        assert_eq!(
            count(&db, "SELECT COUNT(*) FROM questions WHERE bank_id = 'geo-2' AND correct = 'False' AND keep_order = 1").await,
            1
        );

        let report = db.sync_bank(&bank(vec![q])).await.unwrap();
        assert_eq!(report.retired, 1);
        assert_eq!(
            count(&db, "SELECT active FROM questions WHERE bank_id = 'geo-2'").await,
            0
        );
    }

    #[tokio::test]
    async fn starter_content_loads_and_adopts_every_seed() {
        let db = db().await;
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../content");
        let report = load(&db, &dir).await.unwrap();
        assert!(report.added > 100, "{report:?}");
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert_eq!(
            count(&db, "SELECT COUNT(*) FROM questions WHERE bank_id IS NULL").await,
            0,
            "every seeded question should be in a pack"
        );
        // Loading again changes nothing.
        let again = load(&db, &dir).await.unwrap();
        assert_eq!((again.added, again.retired), (0, 0));
        assert_eq!(again.lines, report.lines);
    }
}
