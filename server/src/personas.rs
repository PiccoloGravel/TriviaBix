//! Bix's personas: who he is when Claude writes his lines (HOST_PERSONA), and the lines written
//! for each. The original persona speaks the shared lines (content/scripts outside `personas/`);
//! every other one keeps its own in content/scripts/personas/<name>/, so switching to another
//! persona and back later loses nothing and writes (and pays for) nothing twice.
//!
//! Voice lines are kept by their text, so every persona's lines keep their audio in every voice
//! pack (crate::packs), and the two can be mixed freely.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sqlx::Row;
use triviabix_questions::{PERSONAS_DIR, Persona, ScriptFile};

use crate::db::Db;
use crate::settings::Settings;

const MAX_NAME: usize = 40;
const ORIGINAL: &str = "Original Bix";
/// Where the original persona's lines are written, as before personas (crate::render).
const SHARED_FOLDER: &str = "generated";

/// The persona in use, as the question bank loads it. The original one if there's none yet.
pub async fn active(db: &Db) -> Persona {
    let row = sqlx::query("SELECT folder, fill_gaps FROM persona_packs WHERE active = 1")
        .fetch_optional(&db.pool)
        .await
        .ok()
        .flatten();
    match row {
        Some(r) => Persona {
            folder: r
                .get::<Option<String>, _>("folder")
                .map(PathBuf::from)
                .filter(|f| Persona::is_folder(f)),
            fill_gaps: r.get("fill_gaps"),
        },
        None => Persona::default(),
    }
}

#[derive(Clone)]
pub struct Personas {
    db: Db,
    settings: Settings,
    render: crate::render::Render,
    content_dir: PathBuf,
}

impl Personas {
    pub fn new(
        db: Db,
        settings: Settings,
        render: crate::render::Render,
        content_dir: &Path,
    ) -> Self {
        Self {
            db,
            settings,
            render,
            content_dir: content_dir.to_path_buf(),
        }
    }

    fn scripts(&self) -> PathBuf {
        self.content_dir.join("scripts")
    }

    /// Makes sure there's the original persona and one for every persona folder, that one is in
    /// use, and that it has the text on the Settings page. Run at startup and when HOST_PERSONA
    /// changes. Doesn't reload the content; switching does.
    pub async fn sync(&self) -> Result<(), String> {
        let original: Option<i64> =
            sqlx::query_scalar("SELECT id FROM persona_packs WHERE folder IS NULL")
                .fetch_optional(&self.db.pool)
                .await
                .map_err(err)?;
        if original.is_none() {
            self.insert(ORIGINAL, None, None, true).await?;
        }
        // A folder of lines with no persona, such as after the database was restored from an
        // older backup: keep its lines reachable.
        for folder in self.folders() {
            let have: Option<i64> =
                sqlx::query_scalar("SELECT id FROM persona_packs WHERE folder = ?")
                    .bind(&folder)
                    .fetch_optional(&self.db.pool)
                    .await
                    .map_err(err)?;
            if have.is_none() {
                let name = folder
                    .trim_start_matches(&format!("{PERSONAS_DIR}/"))
                    .replace('-', " ");
                self.insert(&name, None, Some(&folder), true).await?;
            }
        }
        let in_use: Option<i64> =
            sqlx::query_scalar("SELECT id FROM persona_packs WHERE active = 1")
                .fetch_optional(&self.db.pool)
                .await
                .map_err(err)?;
        if in_use.is_none() {
            sqlx::query("UPDATE persona_packs SET active = 1 WHERE folder IS NULL")
                .execute(&self.db.pool)
                .await
                .map_err(err)?;
        }
        // The Settings page's text is the persona in use.
        sqlx::query("UPDATE persona_packs SET persona = ? WHERE active = 1")
            .bind(self.settings.get("HOST_PERSONA"))
            .execute(&self.db.pool)
            .await
            .map_err(err)?;
        self.render.set_persona(active(&self.db).await).await;
        Ok(())
    }

    /// Makes a persona, without switching to it. Its lines go in their own folder.
    pub async fn create(&self, name: &str, persona: &str, fill_gaps: bool) -> Result<i64, String> {
        let name = check_name(name)?;
        let persona = check_persona(persona)?;
        let slug = triviabix_questions::slugify(&name);
        let slug = if slug.is_empty() {
            "persona".to_string()
        } else {
            slug
        };
        // A folder no persona has used, so lines are never mixed up with another's.
        let taken = self.folders();
        let mut folder = format!("{PERSONAS_DIR}/{slug}");
        for n in 2.. {
            let used: Option<i64> =
                sqlx::query_scalar("SELECT id FROM persona_packs WHERE folder = ?")
                    .bind(&folder)
                    .fetch_optional(&self.db.pool)
                    .await
                    .map_err(err)?;
            if used.is_none() && !taken.contains(&folder) {
                break;
            }
            folder = format!("{PERSONAS_DIR}/{slug}-{n}");
        }
        self.insert(&name, Some(&persona), Some(&folder), fill_gaps)
            .await
    }

    /// Renames a persona, rewrites who it is (for lines written from now on), or changes whether
    /// it borrows the shared lines where it has none. Returns whether the lines in play changed.
    pub async fn update(
        &self,
        id: i64,
        name: Option<&str>,
        persona: Option<&str>,
        fill_gaps: Option<bool>,
    ) -> Result<bool, String> {
        let row = self.row(id).await?;
        let is_active: bool = row.get("active");
        if let Some(name) = name {
            let name = check_name(name)?;
            sqlx::query("UPDATE persona_packs SET name = ? WHERE id = ?")
                .bind(&name)
                .bind(id)
                .execute(&self.db.pool)
                .await
                .map_err(unique("There's already a persona with that name."))?;
        }
        if let Some(persona) = persona {
            let persona = check_persona(persona)?;
            sqlx::query("UPDATE persona_packs SET persona = ? WHERE id = ?")
                .bind(&persona)
                .bind(id)
                .execute(&self.db.pool)
                .await
                .map_err(err)?;
            if is_active {
                self.settings.set("HOST_PERSONA", Some(&persona)).await?;
            }
        }
        let mut changed = false;
        if let Some(fill) = fill_gaps {
            sqlx::query("UPDATE persona_packs SET fill_gaps = ? WHERE id = ?")
                .bind(fill)
                .bind(id)
                .execute(&self.db.pool)
                .await
                .map_err(err)?;
            if is_active && fill != row.get::<bool, _>("fill_gaps") {
                self.render.set_persona(active(&self.db).await).await;
                changed = true;
            }
        }
        Ok(changed)
    }

    /// Makes a persona the one in use. The caller reloads the content so its lines play, and
    /// queues them for the voice.
    pub async fn switch(&self, id: i64) -> Result<(), String> {
        let row = self.row(id).await?;
        let persona: Option<String> = row.get("persona");
        let mut tx = self.db.pool.begin().await.map_err(err)?;
        sqlx::query("UPDATE persona_packs SET active = (id = ?)")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(err)?;
        sqlx::query("UPDATE persona_packs SET used_at = CURRENT_TIMESTAMP WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(err)?;
        tx.commit().await.map_err(err)?;
        self.settings
            .set("HOST_PERSONA", persona.as_deref())
            .await?;
        self.render.set_persona(active(&self.db).await).await;
        Ok(())
    }

    /// Every persona, the one in use first, with how many questions it has lines for.
    pub async fn list(&self) -> Result<Vec<Value>, String> {
        let rows = sqlx::query(
            "SELECT id, name, persona, folder, fill_gaps, active, created_at, used_at
             FROM persona_packs ORDER BY active DESC, used_at IS NULL, used_at DESC, id",
        )
        .fetch_all(&self.db.pool)
        .await
        .map_err(err)?;
        let scripts = self.scripts();
        let out = rows
            .iter()
            .map(|r| {
                let folder: Option<String> = r.get("folder");
                let persona: Option<String> = r.get("persona");
                let lines = match &folder {
                    Some(f) => questions_with_lines(&scripts.join(f), None),
                    None => questions_with_lines(&scripts, Some(&scripts.join(PERSONAS_DIR))),
                };
                json!({
                    "id": r.get::<i64, _>("id"),
                    "name": r.get::<String, _>("name"),
                    "persona": persona.clone().unwrap_or_else(|| triviabix_questions::DEFAULT_PERSONA.into()),
                    "default_persona": persona.is_none(),
                    "original": folder.is_none(),
                    "folder": folder.unwrap_or_else(|| SHARED_FOLDER.into()),
                    "fill_gaps": r.get::<bool, _>("fill_gaps"),
                    "active": r.get::<bool, _>("active"),
                    "lines": lines,
                    "created": r.get::<String, _>("created_at"),
                    "used": r.get::<Option<String>, _>("used_at"),
                })
            })
            .collect();
        Ok(out)
    }

    async fn insert(
        &self,
        name: &str,
        persona: Option<&str>,
        folder: Option<&str>,
        fill_gaps: bool,
    ) -> Result<i64, String> {
        let base: String = name.chars().take(MAX_NAME - 4).collect();
        for n in 1.. {
            let name = if n == 1 {
                base.clone()
            } else {
                format!("{base} {n}")
            };
            let res = sqlx::query(
                "INSERT INTO persona_packs (name, persona, folder, fill_gaps) VALUES (?, ?, ?, ?)",
            )
            .bind(&name)
            .bind(persona)
            .bind(folder)
            .bind(fill_gaps)
            .execute(&self.db.pool)
            .await;
            match res {
                Ok(r) => {
                    tracing::info!(
                        "persona {name:?} with lines in {}",
                        folder.unwrap_or(SHARED_FOLDER)
                    );
                    return Ok(r.last_insert_rowid());
                }
                Err(e) if e.to_string().contains("persona_packs.name") => continue,
                Err(e) => return Err(err(e)),
            }
        }
        unreachable!()
    }

    async fn row(&self, id: i64) -> Result<sqlx::sqlite::SqliteRow, String> {
        sqlx::query("SELECT * FROM persona_packs WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.db.pool)
            .await
            .map_err(err)?
            .ok_or_else(|| "There's no such persona.".to_string())
    }

    /// The persona folders on disk, as `personas/<name>`.
    fn folders(&self) -> HashSet<String> {
        std::fs::read_dir(self.scripts().join(PERSONAS_DIR))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().into_string().ok())
            .map(|n| format!("{PERSONAS_DIR}/{n}"))
            .filter(|f| Persona::is_folder(Path::new(f)))
            .collect()
    }
}

/// How many questions the script files under `dir` (but not under `skip`) have lines for.
fn questions_with_lines(dir: &Path, skip: Option<&Path>) -> usize {
    fn walk(dir: &Path, skip: Option<&Path>, ids: &mut HashSet<String>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if skip.is_some_and(|s| path.starts_with(s)) {
                continue;
            }
            if path.is_dir() {
                walk(&path, skip, ids);
            } else if path.extension().is_some_and(|e| e == "toml" || e == "json")
                && let Ok(file) = ScriptFile::read(&path)
            {
                ids.extend(
                    file.scripts
                        .into_iter()
                        .filter(|s| s.lead_in.is_some() || s.reveal.is_some())
                        .map(|s| s.question),
                );
            }
        }
    }
    let mut ids = HashSet::new();
    walk(dir, skip, &mut ids);
    ids.len()
}

fn check_name(name: &str) -> Result<String, String> {
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    match name.chars().count() {
        0 => Err("Give the persona a name.".into()),
        n if n > MAX_NAME => Err(format!("Keep the name under {MAX_NAME} characters.")),
        _ => Ok(name),
    }
}

fn check_persona(persona: &str) -> Result<String, String> {
    let persona = persona.replace("\r\n", "\n").replace('\r', "\n");
    let persona = persona.trim();
    let max = triviabix_questions::MAX_PERSONA_CHARS;
    match persona.chars().count() {
        0 => Err("Say who Bix is in this persona.".into()),
        n if n > max => Err(format!("Keep it under {max} characters; that's {n}.")),
        _ if persona.contains('\0') => Err("That doesn't look like a persona.".into()),
        _ => Ok(persona.to_string()),
    }
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn unique(message: &'static str) -> impl Fn(sqlx::Error) -> String {
    move |e| match e.to_string().contains("UNIQUE") {
        true => message.to_string(),
        false => e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn personas_are_checked() {
        assert_eq!(check_name("  Late   night ").unwrap(), "Late night");
        assert!(check_name("").is_err());
        assert!(check_persona("   ").is_err());
        assert_eq!(
            check_persona(" Bix is a pirate.\r\n").unwrap(),
            "Bix is a pirate."
        );
        let long = "x".repeat(triviabix_questions::MAX_PERSONA_CHARS + 1);
        assert!(check_persona(&long).is_err());
    }
}
