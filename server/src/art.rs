//! The admin Art page: art themes, category styles, and adding or changing pieces of art.
//!
//! What the page changes is saved in one JSON file in appdata (ART_STUDIO, next to the art
//! folder), which `art/generate.py --studio` reads on top of art/catalog.toml and art/themes.toml:
//!
//! - `active`: the theme the TV shows. Every theme's TV files are kept in their own folder
//!   (the default theme in the art folder, others in themes/<id>), and its originals under their
//!   own style in the asset store, so switching back and forth never makes anything twice.
//! - `themes`: themes made on the page (a look, a palette, optional extras).
//! - `category_styles`: slug -> words added to that category's title card in every theme.
//! - `entries`: new pieces, and changes to the catalog's (prompt, seed, tags, rating, hidden).
//!   One with a `theme` applies only there, which is how a new take (a new seed) of a piece in
//!   one theme leaves the others alone.
//!
//! Every change is checked by having generate.py read it back, and undone if it can't. Making the
//! TV's files from what the asset store already has is free and runs straight after; anything
//! new is a paid art run on the Pipeline's terms (price check, then confirm).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::process::Command;

use crate::render::Render;

pub const DEFAULT_THEME: &str = "bix";
const RATINGS: &[&str] = &["PG", "PG-13", "MA", "NC-17"];
const KINDS: &[&str] = &["backdrop", "sticker", "category", "clip"];
const MAX_PROMPT: usize = 600;
const MAX_TAGS: usize = 12;
/// Reading the catalog is quick; making TV files from the asset store takes a little longer.
const TOOL_TIMEOUT: Duration = Duration::from_secs(120);

/// One writer at a time, so two quick saves can't lose one another.
static WRITING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A theme or piece id: lower-case letters, digits and dashes, as generate.py's ID_RE.
pub fn valid_id(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty()
        && b.len() <= 40
        && b[0] != b'-'
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

fn valid_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag.len() <= 40
        && tag
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"-_:".contains(&c))
}

pub fn theme_dir(art_dir: &Path, theme: &str) -> PathBuf {
    if theme == DEFAULT_THEME {
        art_dir.to_path_buf()
    } else {
        art_dir.join("themes").join(theme)
    }
}

fn read(path: &Path) -> Value {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}))
}

fn write(path: &Path, studio: &Value) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_vec_pretty(studio).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, body).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// The theme in use.
pub fn active(path: &Path) -> String {
    read(path)["active"]
        .as_str()
        .filter(|t| valid_id(t))
        .unwrap_or(DEFAULT_THEME)
        .to_string()
}

/// The TV's manifest for `theme`: its own art over the default theme's, so pieces it hasn't made
/// yet still show (in the default look) instead of falling back to the drawn backgrounds.
pub fn merged_manifest(art_dir: &Path, shipped: &Path, theme: &str) -> Option<Value> {
    let load = |dir: &Path| -> Option<Value> {
        serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).ok()?).ok()
    };
    let mut base = load(art_dir).or_else(|| load(shipped))?;
    let Some(own) = load(&theme_dir(art_dir, theme)) else {
        return Some(base);
    };
    for key in ["files", "categories"] {
        if let (Some(into), Some(from)) = (
            base.get_mut(key).and_then(Value::as_object_mut),
            own.get(key).and_then(Value::as_object),
        ) {
            for (k, v) in from {
                into.insert(k.clone(), v.clone());
            }
        }
    }
    Some(base)
}

async fn run_tool(render: &Render, args: &[String]) -> Result<(i32, String, String), String> {
    let c = render.config();
    let mut cmd = Command::new(&c.python);
    cmd.arg(c.tools_dir.join("art/generate.py"))
        .args(args)
        .arg("--library")
        .arg(&c.assets_dir)
        .arg("--studio")
        .arg(&c.studio)
        .current_dir(&c.tools_dir)
        .env("PYTHONUNBUFFERED", "1")
        .env("ASSETS_DIR", &c.assets_dir)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    render.with_settings(&mut cmd);
    let out = tokio::time::timeout(TOOL_TIMEOUT, cmd.output())
        .await
        .map_err(|_| "The art tool took too long.".to_string())?
        .map_err(|e| format!("Couldn't run the art tool: {e}"))?;
    Ok((
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
}

fn last_words(stderr: &str, stdout: &str) -> String {
    stderr
        .lines()
        .chain(stdout.lines())
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("the art tool failed")
        .to_string()
}

/// Everything the page shows for `theme`: the themes with how much of each is made, every piece
/// with its prompt, tags, state and earlier takes, and the categories with their styles.
pub async fn listing(render: &Render, theme: Option<&str>) -> Result<Value, String> {
    let c = render.config();
    let active = render.art_theme();
    let theme = theme.filter(|t| valid_id(t)).unwrap_or(&active).to_string();
    let (code, stdout, stderr) = run_tool(
        render,
        &[
            "--backend".into(),
            "offline".into(),
            "--theme".into(),
            theme,
            "--list".into(),
            c.art_dir.display().to_string(),
        ],
    )
    .await?;
    if code != 0 {
        return Err(last_words(&stderr, &stdout));
    }
    let mut v: Value = serde_json::from_str(stdout.trim())
        .map_err(|e| format!("The art tool's answer didn't make sense: {e}"))?;
    v["active"] = json!(active);
    v["busy"] = json!(render.art_busy());
    Ok(v)
}

/// Makes `theme`'s TV files from what the asset store has, for free, and copies tag and rating
/// changes on. Returns how many pieces still need a GPU.
pub async fn apply(render: &Render, theme: &str) -> Result<usize, String> {
    if render.art_busy().is_some() {
        return Ok(0); // the run in progress finishes with this same step
    }
    // Before the first file is written there, so the art the server ships isn't hidden.
    render.seed_art_dir();
    let out = render.theme_dir(theme);
    let (code, stdout, stderr) = run_tool(
        render,
        &[
            "--backend".into(),
            "offline".into(),
            "--theme".into(),
            theme.into(),
            "--out".into(),
            out.display().to_string(),
        ],
    )
    .await?;
    render.forget_missing().await;
    match code {
        0 => Ok(0),
        // offline: some pieces need a GPU, listed in .needs-gpu.
        3 => Ok(std::fs::read_to_string(out.join(".needs-gpu"))
            .map(|s| s.split_whitespace().count())
            .unwrap_or(0)),
        _ => Err(last_words(&stderr, &stdout)),
    }
}

/// Changes the saved file with `f`, then has generate.py read it back for `theme`; if it can't,
/// the change is undone and its complaint returned.
async fn change(
    render: &Render,
    theme: Option<&str>,
    f: impl FnOnce(&mut Value) -> Result<(), String>,
) -> Result<(), String> {
    let _w = WRITING.lock().await;
    let path = &render.config().studio;
    let before = read(path);
    let mut studio = before.clone();
    f(&mut studio)?;
    write(path, &studio)?;
    if let Err(e) = listing(render, theme).await {
        if let Err(u) = write(path, &before) {
            tracing::warn!("art studio: couldn't undo a change: {u}");
        }
        return Err(e.trim_start_matches("SystemExit: ").to_string());
    }
    render.forget_missing().await;
    Ok(())
}

fn array<'a>(studio: &'a mut Value, key: &str) -> &'a mut Vec<Value> {
    if !studio[key].is_array() {
        studio[key] = json!([]);
    }
    studio[key].as_array_mut().unwrap()
}

#[derive(Debug, Deserialize)]
pub struct EntryChange {
    pub kind: String,
    pub id: String,
    /// Only in this theme (a new take). Without it, every theme.
    #[serde(default)]
    pub theme: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub seed: Option<u32>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub rating: Option<String>,
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub hidden: Option<bool>,
    /// Forget this entry's changes: the piece goes back to the catalog's (or, if it was new, away).
    #[serde(default)]
    pub reset: bool,
}

/// Adds a piece, or changes or hides one, everywhere or in one theme.
pub async fn set_entry(render: &Render, e: EntryChange) -> Result<(), String> {
    if !KINDS.contains(&e.kind.as_str()) {
        return Err(format!("Art is one of {}.", KINDS.join(", ")));
    }
    if !valid_id(&e.id) {
        return Err(
            "Give it an id of lower-case letters, digits and dashes, like party-hat.".into(),
        );
    }
    let theme = e.theme.clone().filter(|t| !t.is_empty());
    if theme.as_deref().is_some_and(|t| !valid_id(t)) {
        return Err("That isn't an art theme.".into());
    }
    let prompt = e
        .prompt
        .as_ref()
        .map(|p| p.split_whitespace().collect::<Vec<_>>().join(" "));
    if let Some(p) = &prompt
        && !(3..=MAX_PROMPT).contains(&p.chars().count())
    {
        return Err(format!(
            "Describe it in 3 to {MAX_PROMPT} characters. The theme's look is added for you."
        ));
    }
    let tags = e.tags.as_ref().map(|t| {
        let mut t: Vec<String> = t.iter().map(|t| t.trim().to_lowercase()).collect();
        t.retain(|t| !t.is_empty());
        t.dedup();
        t
    });
    if let Some(t) = &tags
        && (t.len() > MAX_TAGS || !t.iter().all(|t| valid_tag(t)))
    {
        return Err(format!(
            "Up to {MAX_TAGS} tags, each lower-case letters, digits, - _ or :."
        ));
    }
    if e.rating.as_deref().is_some_and(|r| !RATINGS.contains(&r)) {
        return Err(format!("Rate it {}.", RATINGS.join(", ")));
    }
    if e.from.as_deref().is_some_and(|f| !valid_id(f)) {
        return Err("A clip animates one of the backdrops; pick one.".into());
    }
    let view = theme.clone();
    change(render, view.as_deref(), move |studio| {
        let entries = array(studio, "entries");
        let same = |v: &Value| {
            v["kind"] == e.kind.as_str()
                && v["id"] == e.id.as_str()
                && v["theme"].as_str() == theme.as_deref()
        };
        let at = entries.iter().position(same);
        if e.reset {
            if let Some(i) = at {
                entries.remove(i);
            }
            return Ok(());
        }
        let entry = match at {
            Some(i) => &mut entries[i],
            None => {
                let mut v = json!({ "kind": e.kind, "id": e.id });
                if let Some(t) = &theme {
                    v["theme"] = json!(t);
                }
                entries.push(v);
                entries.last_mut().unwrap()
            }
        };
        if let Some(p) = prompt {
            entry["prompt"] = json!(p);
        }
        if let Some(s) = e.seed {
            entry["seed"] = json!(s);
        }
        if let Some(t) = tags {
            entry["tags"] = json!(t);
        }
        if let Some(r) = e.rating {
            entry["rating"] = json!(r);
        }
        if let Some(f) = e.from {
            entry["from"] = json!(f);
        }
        match e.hidden {
            Some(true) => entry["hidden"] = json!(true),
            Some(false) => {
                entry.as_object_mut().unwrap().remove("hidden");
            }
            None => {}
        }
        Ok(())
    })
    .await
}

#[derive(Debug, Deserialize)]
pub struct ThemeChange {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub palette: String,
    #[serde(default)]
    pub sticker: String,
    #[serde(default)]
    pub backdrop: String,
    /// Remove this theme (only one made on the page, and not the one in use).
    #[serde(default)]
    pub delete: bool,
}

/// Makes or changes a theme made on the page. Changing its look or palette starts a new version,
/// so new art is made for it while the old version's stays in the asset store.
pub async fn set_theme(render: &Render, t: ThemeChange) -> Result<(), String> {
    if !valid_id(&t.id) || t.id == DEFAULT_THEME {
        return Err(
            "Give the theme an id of lower-case letters, digits and dashes, like spooky.".into(),
        );
    }
    if t.delete {
        if render.art_theme() == t.id {
            return Err("That theme is in use. Switch to another one first.".into());
        }
        return change(render, None, move |studio| {
            array(studio, "themes").retain(|v| v["id"] != t.id.as_str());
            Ok(())
        })
        .await;
    }
    let tidy = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let (name, prompt) = (tidy(&t.name), tidy(&t.prompt));
    if !(1..=40).contains(&name.chars().count()) {
        return Err("Name the theme in up to 40 characters.".into());
    }
    if !(10..=MAX_PROMPT).contains(&prompt.chars().count()) {
        return Err(format!(
            "Describe the look in 10 to {MAX_PROMPT} characters, e.g. \"watercolour storybook illustration, soft paper texture\"."
        ));
    }
    let extras = [&t.description, &t.palette, &t.sticker, &t.backdrop];
    if extras.iter().any(|s| s.chars().count() > 300) {
        return Err("Keep the description, palette and extras under 300 characters each.".into());
    }
    let id = t.id.clone();
    change(render, Some(&id), move |studio| {
        let themes = array(studio, "themes");
        let old = themes.iter().position(|v| v["id"] == t.id.as_str());
        let look = |v: &Value| {
            [
                v["prompt"].as_str().unwrap_or(""),
                v["palette"].as_str().unwrap_or(""),
                v["sticker"].as_str().unwrap_or(""),
                v["backdrop"].as_str().unwrap_or(""),
            ]
            .map(String::from)
        };
        let mut theme = json!({
            "id": t.id,
            "name": name,
            "description": tidy(&t.description),
            "prompt": prompt,
            "palette": tidy(&t.palette),
            "sticker": tidy(&t.sticker),
            "backdrop": tidy(&t.backdrop),
            "version": "v1",
        });
        if let Some(i) = old {
            let was = &themes[i];
            let version = was["version"].as_str().unwrap_or("v1").to_string();
            theme["version"] = json!(if look(was) == look(&theme) {
                version
            } else {
                bump(&version)
            });
            if let Some(styles) = was.get("category_styles") {
                theme["category_styles"] = styles.clone();
            }
            themes[i] = theme;
        } else {
            themes.push(theme);
        }
        Ok(())
    })
    .await
}

/// v1 -> v2; anything else gets -2 on the end.
fn bump(version: &str) -> String {
    match version
        .strip_prefix('v')
        .and_then(|n| n.parse::<u32>().ok())
    {
        Some(n) => format!("v{}", n + 1),
        None => format!("{version}-2"),
    }
}

/// Puts a theme on the TV. Its art that the asset store has is made into TV files first, for free.
pub async fn use_theme(render: &Render, id: &str) -> Result<usize, String> {
    if !valid_id(id) {
        return Err("That isn't an art theme.".into());
    }
    let id = id.to_string();
    let want = id.clone();
    change(render, Some(&id), move |studio| {
        studio["active"] = json!(want);
        Ok(())
    })
    .await?;
    apply(render, &id).await
}

/// Words added to one category's title card, in every theme. Empty clears it.
pub async fn set_category_style(render: &Render, slug: &str, style: &str) -> Result<(), String> {
    let style = style.split_whitespace().collect::<Vec<_>>().join(" ");
    if style.chars().count() > 300 {
        return Err("Keep a category's style under 300 characters.".into());
    }
    if slug.is_empty() || slug.len() > 60 || slug.contains(['/', '\\', '"']) {
        return Err("That isn't a category.".into());
    }
    let slug = slug.to_string();
    change(render, None, move |studio| {
        if !studio["category_styles"].is_object() {
            studio["category_styles"] = json!({});
        }
        let styles = studio["category_styles"].as_object_mut().unwrap();
        if style.is_empty() {
            styles.remove(&slug);
        } else {
            styles.insert(slug, json!(style));
        }
        Ok(())
    })
    .await
}

/// A stored original from the asset store, for the page to show an earlier take:
/// image/<12 hex>/<32 hex>.png. Anything else is refused.
pub fn original(assets: &Path, rel: &str) -> Option<PathBuf> {
    let hex = |s: &str, n: usize| s.len() == n && s.bytes().all(|b| b.is_ascii_hexdigit());
    let parts: Vec<&str> = rel.split('/').collect();
    match parts.as_slice() {
        ["image", style, file] if hex(style, 12) => {
            let key = file.strip_suffix(".png")?;
            hex(key, 32).then(|| assets.join("image").join(style).join(file))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_paths() {
        assert!(valid_id("retro-arcade"));
        assert!(valid_id("a1"));
        assert!(!valid_id("-x"));
        assert!(!valid_id("Big"));
        assert!(!valid_id("../x"));
        assert!(!valid_id(""));
        assert_eq!(
            theme_dir(Path::new("/data/art"), "bix"),
            Path::new("/data/art")
        );
        assert_eq!(
            theme_dir(Path::new("/data/art"), "comic-book"),
            Path::new("/data/art/themes/comic-book")
        );
        assert_eq!(bump("v1"), "v2");
        assert_eq!(bump("v9"), "v10");
        assert_eq!(bump("x"), "x-2");
        let a = Path::new("/a");
        let k = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            original(a, &format!("image/0123456789ab/{k}.png")),
            Some(PathBuf::from(format!("/a/image/0123456789ab/{k}.png")))
        );
        assert_eq!(original(a, &format!("image/../{k}.png")), None);
        assert_eq!(original(a, &format!("voice/0123456789ab/{k}.png")), None);
        assert_eq!(original(a, "image/0123456789ab/x.png"), None);
    }

    #[test]
    fn a_theme_shows_over_the_default_art() {
        let dir = std::env::temp_dir().join(format!("tbx-art-{}", std::process::id()));
        let shipped = dir.join("shipped");
        std::fs::create_dir_all(dir.join("themes/pixel")).unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            r#"{"files":{"bg/a.webp":{"id":"a"},"bg/b.webp":{"id":"b"}},"categories":{"x":"category/x.webp"}}"#,
        )
        .unwrap();
        assert_eq!(
            merged_manifest(&dir, &shipped, "pixel").unwrap()["files"]["bg/a.webp"]["id"],
            "a",
            "a theme with nothing made yet shows the default art"
        );
        std::fs::write(
            dir.join("themes/pixel/manifest.json"),
            r#"{"files":{"bg/a.webp":{"id":"a","theme":"pixel"}},"categories":{}}"#,
        )
        .unwrap();
        let m = merged_manifest(&dir, &shipped, "pixel").unwrap();
        assert_eq!(m["files"]["bg/a.webp"]["theme"], "pixel");
        assert_eq!(m["files"]["bg/b.webp"]["id"], "b");
        assert_eq!(m["categories"]["x"], "category/x.webp");
        std::fs::write(dir.join("studio.json"), r#"{"active":"pixel"}"#).unwrap();
        assert_eq!(active(&dir.join("studio.json")), "pixel");
        assert_eq!(active(&dir.join("nothing.json")), DEFAULT_THEME);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
