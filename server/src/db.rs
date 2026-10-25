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

/// One player's final standing, written to the database when a game ends.
pub struct Standing {
    pub name: String,
    pub score: i64,
    pub rank: usize,
}

#[derive(Clone)]
pub struct Db {
    pool: SqlitePool,
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
        let row = sqlx::query("SELECT COUNT(*) AS n FROM questions")
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
        let rows = sqlx::query(
            "SELECT id, category, prompt, correct, wrong FROM questions
             WHERE CASE rating WHEN 'PG' THEN 0 WHEN 'PG-13' THEN 1 WHEN 'MA' THEN 2 ELSE 3 END <= ?1
             ORDER BY RANDOM() LIMIT ?2",
        )
        .bind(max_rating.level())
        .bind((n + exclude.len()) as i64)
        .fetch_all(&self.pool)
        .await?;

        let mut rng = rand::rng();
        let mut out = Vec::with_capacity(n);
        for row in rows {
            let id: i64 = row.get("id");
            if exclude.contains(&id) {
                continue;
            }
            let correct: String = row.get("correct");
            let wrong: Vec<String> = serde_json::from_str(row.get::<&str, _>("wrong"))
                .with_context(|| format!("question {id} has bad `wrong` JSON"))?;
            let mut choices = wrong;
            choices.push(correct.clone());
            choices.shuffle(&mut rng);
            let answer = choices.iter().position(|c| *c == correct).unwrap();
            out.push(Question {
                id,
                category: row.get("category"),
                prompt: row.get("prompt"),
                choices,
                answer,
                song: None,
            });
            if out.len() == n {
                break;
            }
        }
        Ok(out)
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
