use std::collections::HashMap;
use std::str::FromStr;

use anyhow::Context;
use rand::seq::SliceRandom;
use sqlx::Row;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};

/// Movie-style content rating. Ordered from tamest to wildest.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum Rating {
    #[default]
    #[serde(rename = "PG")]
    Pg,
    #[serde(rename = "PG-13")]
    Pg13,
    #[serde(rename = "MA")]
    Ma,
    #[serde(rename = "NC-17")]
    Nc17,
}

impl Rating {
    pub fn level(self) -> i64 {
        self as i64
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Rating::Pg => "PG",
            Rating::Pg13 => "PG-13",
            Rating::Ma => "MA",
            Rating::Nc17 => "NC-17",
        }
    }

    pub fn parse(s: &str) -> Option<Rating> {
        [Rating::Pg, Rating::Pg13, Rating::Ma, Rating::Nc17]
            .into_iter()
            .find(|r| r.as_str() == s)
    }
}

/// SQL that is true for questions the game can play: still in a pack, and a type the phones
/// support (multiple choice with up to four choices, or true/false).
macro_rules! playable {
    () => {
        "active = 1 AND kind IN ('multiple_choice', 'true_false')
         AND json_array_length(wrong) BETWEEN 1 AND 3"
    };
}

/// How hard a question is. Trivia rounds get harder as the game goes on.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Difficulty {
    Easy,
    #[default]
    Medium,
    Hard,
}

impl Difficulty {
    pub fn name(self) -> &'static str {
        match self {
            Difficulty::Easy => "easy",
            Difficulty::Medium => "medium",
            Difficulty::Hard => "hard",
        }
    }

    /// Unknown values count as medium, like the column's default.
    pub fn parse(s: &str) -> Self {
        match s {
            "easy" => Difficulty::Easy,
            "hard" => Difficulty::Hard,
            _ => Difficulty::Medium,
        }
    }
}

/// Just enough about a question to plan a game's rounds before loading the questions themselves.
#[derive(Debug, Clone)]
pub struct PoolEntry {
    pub id: i64,
    pub category: String,
    pub difficulty: Difficulty,
}

/// A question ready to be played, with its choices already shuffled.
#[derive(Debug, Clone)]
pub struct Question {
    pub id: i64,
    pub category: String,
    pub prompt: String,
    pub choices: Vec<String>,
    pub answer: usize,
    /// Set for Name That Tune rounds.
    pub song: Option<SongClip>,
    /// What the host can say about this question, already limited to the room's rating.
    pub lines: QuestionLines,
}

/// A question's own host lines, by when they're said. Empty means the host sticks to his
/// generic lines.
#[derive(Debug, Clone, Default)]
pub struct QuestionLines {
    pub lead_in: Vec<(Rating, String)>,
    pub reveal: Vec<(Rating, String)>,
}

/// The clip the TV plays for a Name That Tune round.
#[derive(Debug, Clone)]
pub struct SongClip {
    pub title: String,
    pub artist: String,
    pub year: i64,
    pub preview_url: String,
    /// Where in the 30-second preview the clip starts.
    pub start_ms: u64,
    pub artwork_url: Option<String>,
    pub store_url: Option<String>,
}

/// A row from the `songs` table.
#[derive(Debug, Clone)]
pub struct Song {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub year: i64,
    pub itunes_id: Option<i64>,
    pub explicit: bool,
}

/// A speed hangman phrase.
#[derive(Debug, Clone)]
pub struct Phrase {
    pub id: i64,
    pub category: String,
    pub phrase: String,
}

/// One player's final standing, written to the database when a game ends.
pub struct Standing {
    pub name: String,
    pub score: i64,
    pub rank: usize,
}

#[derive(Clone)]
pub struct Db {
    pub(crate) pool: SqlitePool,
}

impl Db {
    pub async fn connect(url: &str) -> anyhow::Result<Self> {
        let opts = SqliteConnectOptions::from_str(url)
            .with_context(|| format!("bad DATABASE_URL {url}"))?
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await
            .context("opening database")?;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .context("running migrations")?;
        Ok(Self { pool })
    }

    pub async fn question_count(&self) -> anyhow::Result<i64> {
        let row = sqlx::query(concat!(
            "SELECT COUNT(*) AS n FROM questions WHERE ",
            playable!()
        ))
        .fetch_one(&self.pool)
        .await?;
        Ok(row.get("n"))
    }

    /// Picks up to `n` random questions rated at most `max_rating`, skipping ids in `exclude` so a
    /// "play again" in the same room does not repeat itself.
    pub async fn random_questions(
        &self,
        n: usize,
        max_rating: Rating,
        exclude: &[i64],
    ) -> anyhow::Result<Vec<Question>> {
        let rows = sqlx::query(concat!(
            "SELECT id, category, prompt, correct, wrong, choices, keep_order FROM questions
             WHERE CASE rating WHEN 'PG' THEN 0 WHEN 'PG-13' THEN 1 WHEN 'MA' THEN 2 ELSE 3 END <= ?1
               AND ",
            playable!(),
            " ORDER BY RANDOM() LIMIT ?2"
        ))
        .bind(max_rating.level())
        .bind((n + exclude.len()) as i64)
        .fetch_all(&self.pool)
        .await?;

        let mut out = Vec::with_capacity(n);
        for row in rows {
            let id: i64 = row.get("id");
            if exclude.contains(&id) {
                continue;
            }
            if let Some(q) = question_from_row(&row)? {
                out.push(q);
            }
            if out.len() == n {
                break;
            }
        }
        let ids: Vec<i64> = out.iter().map(|q| q.id).collect();
        let mut lines = self.question_lines(&ids, max_rating).await?;
        for q in &mut out {
            q.lines = lines.remove(&q.id).unwrap_or_default();
        }
        Ok(out)
    }

    /// The host lines a room rated `max_rating` may hear for each question. Lines edited in the
    /// admin panel replace the content lines for their slot, unless none of them suit the room.
    /// An edited line with empty text silences the slot.
    pub async fn question_lines(
        &self,
        ids: &[i64],
        max_rating: Rating,
    ) -> anyhow::Result<HashMap<i64, QuestionLines>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let rows = sqlx::query(
            "SELECT question_id, slot, rating, text, origin FROM question_lines
             WHERE question_id IN (SELECT value FROM json_each(?1)) ORDER BY id",
        )
        .bind(serde_json::to_string(ids)?)
        .fetch_all(&self.pool)
        .await?;
        // (question, slot) -> (edited lines, content lines)
        type Found = (Vec<(Rating, String)>, Vec<(Rating, String)>);
        let mut found: HashMap<(i64, String), Found> = HashMap::new();
        for row in rows {
            let Some(rating) = Rating::parse(row.get("rating")) else {
                continue;
            };
            if rating > max_rating {
                continue;
            }
            let entry = found
                .entry((row.get("question_id"), row.get("slot")))
                .or_default();
            let line = (rating, row.get("text"));
            match row.get::<&str, _>("origin") {
                "edited" => entry.0.push(line),
                _ => entry.1.push(line),
            }
        }
        let mut out: HashMap<i64, QuestionLines> = HashMap::new();
        for ((id, slot), (edited, content)) in found {
            // An edited line with no text mutes the slot.
            let mut chosen = if edited.is_empty() { content } else { edited };
            chosen.retain(|(_, text)| !text.trim().is_empty());
            let q = out.entry(id).or_default();
            match slot.as_str() {
                "lead_in" => q.lead_in = chosen,
                _ => q.reveal = chosen,
            }
        }
        Ok(out)
    }

    /// Every question a room rated `max_rating` may play, for planning rounds.
    pub async fn question_pool(&self, max_rating: Rating) -> anyhow::Result<Vec<PoolEntry>> {
        let rows = sqlx::query(concat!(
            "SELECT id, category, difficulty FROM questions
             WHERE CASE rating WHEN 'PG' THEN 0 WHEN 'PG-13' THEN 1 WHEN 'MA' THEN 2 ELSE 3 END <= ?1
               AND ",
            playable!()
        ))
        .bind(max_rating.level())
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| PoolEntry {
                id: row.get("id"),
                category: row.get("category"),
                difficulty: Difficulty::parse(row.get("difficulty")),
            })
            .collect())
    }

    /// Loads questions by id, in the order given, with their choices shuffled and the host lines a
    /// room rated `max_rating` may hear. Ids that no longer exist are skipped.
    pub async fn questions_by_ids(
        &self,
        ids: &[i64],
        max_rating: Rating,
    ) -> anyhow::Result<Vec<Question>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(
            "SELECT id, category, prompt, correct, wrong, choices, keep_order FROM questions
             WHERE id IN (SELECT value FROM json_each(?1))",
        )
        .bind(serde_json::to_string(ids)?)
        .fetch_all(&self.pool)
        .await?;
        let mut by_id = HashMap::new();
        for row in rows {
            if let Some(q) = question_from_row(&row)? {
                by_id.insert(q.id, q);
            }
        }
        let mut lines = self.question_lines(ids, max_rating).await?;
        for q in by_id.values_mut() {
            q.lines = lines.remove(&q.id).unwrap_or_default();
        }
        Ok(ids.iter().filter_map(|id| by_id.remove(id)).collect())
    }

    /// Picks up to `n` random hangman phrases rated at most `max_rating`, skipping ids in `exclude`.
    pub async fn random_phrases(
        &self,
        n: usize,
        max_rating: Rating,
        exclude: &[i64],
    ) -> anyhow::Result<Vec<Phrase>> {
        let rows = sqlx::query(
            "SELECT id, category, phrase FROM hangman_phrases
             WHERE CASE rating WHEN 'PG' THEN 0 WHEN 'PG-13' THEN 1 WHEN 'MA' THEN 2 ELSE 3 END <= ?1
             ORDER BY RANDOM() LIMIT ?2",
        )
        .bind(max_rating.level())
        .bind((n + exclude.len()) as i64)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| Phrase {
                id: row.get("id"),
                category: row.get("category"),
                phrase: row.get("phrase"),
            })
            .filter(|p| !exclude.contains(&p.id))
            .take(n)
            .collect())
    }

    /// Every playable song rated at most `max_rating`.
    pub async fn songs(&self, max_rating: Rating) -> anyhow::Result<Vec<Song>> {
        let rows = sqlx::query(
            "SELECT id, title, artist, year, itunes_id, explicit FROM songs
             WHERE unplayable = 0
               AND CASE rating WHEN 'PG' THEN 0 WHEN 'PG-13' THEN 1 WHEN 'MA' THEN 2 ELSE 3 END <= ?1",
        )
        .bind(max_rating.level())
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| Song {
                id: row.get("id"),
                title: row.get("title"),
                artist: row.get("artist"),
                year: row.get("year"),
                itunes_id: row.get("itunes_id"),
                explicit: row.get::<i64, _>("explicit") != 0,
            })
            .collect())
    }

    /// Remembers which store track a song is, so later games need one lookup instead of a search.
    pub async fn save_track(
        &self,
        song_id: i64,
        track: &crate::music::Track,
    ) -> anyhow::Result<()> {
        sqlx::query("UPDATE songs SET itunes_id = ?2, explicit = ?3, artwork_url = ?4, store_url = ?5 WHERE id = ?1")
            .bind(song_id)
            .bind(track.itunes_id)
            .bind(track.explicit as i64)
            .bind(&track.artwork_url)
            .bind(&track.store_url)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn mark_unplayable(&self, song_id: i64) -> anyhow::Result<()> {
        sqlx::query("UPDATE songs SET unplayable = 1 WHERE id = ?1")
            .bind(song_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn start_game(
        &self,
        room_code: &str,
        rounds: usize,
        players: usize,
    ) -> anyhow::Result<i64> {
        let row = sqlx::query(
            "INSERT INTO games (room_code, rounds, player_count) VALUES (?1, ?2, ?3) RETURNING id",
        )
        .bind(room_code)
        .bind(rounds as i64)
        .bind(players as i64)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.get("id"))
    }

    pub async fn finish_game(&self, game_id: i64, standings: &[Standing]) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE games SET finished_at = CURRENT_TIMESTAMP WHERE id = ?1")
            .bind(game_id)
            .execute(&mut *tx)
            .await?;
        for s in standings {
            sqlx::query(
                "INSERT INTO game_players (game_id, name, score, rank) VALUES (?1, ?2, ?3, ?4)",
            )
            .bind(game_id)
            .bind(&s.name)
            .bind(s.score)
            .bind(s.rank as i64)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn finished_games(&self) -> anyhow::Result<i64> {
        let row = sqlx::query("SELECT COUNT(*) AS n FROM games WHERE finished_at IS NOT NULL")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.get("n"))
    }
}

/// Builds a playable question from a `questions` row, shuffling the right answer in with the wrong ones.
/// Builds a question from a row with `id, category, prompt, correct, wrong, choices, keep_order`.
/// Choices are shuffled unless the pack fixed their order. None if the right answer is missing.
fn question_from_row(row: &sqlx::sqlite::SqliteRow) -> anyhow::Result<Option<Question>> {
    let id: i64 = row.get("id");
    let correct: String = row.get("correct");
    let wrong: Vec<String> = serde_json::from_str(row.get::<&str, _>("wrong"))
        .with_context(|| format!("question {id} has bad `wrong` JSON"))?;
    let authored: Option<Vec<String>> = row
        .get::<Option<&str>, _>("choices")
        .and_then(|c| serde_json::from_str(c).ok());
    let choices = match authored {
        Some(choices) if row.get::<i64, _>("keep_order") != 0 => choices,
        _ => {
            let mut choices = wrong;
            choices.push(correct.clone());
            choices.shuffle(&mut rand::rng());
            choices
        }
    };
    let Some(answer) = choices.iter().position(|c| *c == correct) else {
        tracing::warn!("question {id} has no correct choice; skipping it");
        return Ok(None);
    };
    Ok(Some(Question {
        id,
        category: row.get("category"),
        prompt: row.get("prompt"),
        choices,
        answer,
        song: None,
        lines: QuestionLines::default(),
    }))
}
