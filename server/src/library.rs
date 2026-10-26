//! The operator's own music library, synced from Lidarr.
//!
//! Lidarr knows every artist, album and track that has a file on disk. We ask it for that list,
//! map each file's path (as Lidarr sees it) to where the same folder is mounted in this container,
//! and store the tracks in the `songs` table. The TV then streams clips straight from the files
//! through `/media/...` links (see [`Media`]).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use futures_util::{StreamExt, TryStreamExt, stream};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::{Db, Rating};

/// File types TV browsers can play. Anything else (ALAC is often .m4a too, but most libraries are
/// AAC there) is left out of the game.
const PLAYABLE: &[&str] = &[
    "mp3", "flac", "m4a", "aac", "ogg", "oga", "opus", "wav", "webm",
];
/// Album kinds that make poor Name That Tune clips: crowd noise, remixes, talking.
const SKIPPED_ALBUM_TYPES: &[&str] = &[
    "live",
    "remix",
    "spokenword",
    "interview",
    "audiobook",
    "audio drama",
    "dj-mix",
    "demo",
    "field recording",
];
/// Intros, skits and interludes are too short to be worth guessing.
const MIN_TRACK_MS: i64 = 60_000;
/// Lidarr calls in flight at once while syncing.
const PARALLEL_CALLS: usize = 4;

/// Where Lidarr is and how its files map into this container. Set from the environment.
#[derive(Debug, Clone, Default)]
pub struct LibraryConfig {
    /// Lidarr's address, e.g. http://192.168.1.20:8686. Empty turns the library off.
    pub lidarr_url: Option<String>,
    pub api_key: Option<String>,
    /// Where the music folder is mounted in this container (read-only is fine).
    pub music_dir: PathBuf,
    /// The same folder as Lidarr sees it. Empty means "use Lidarr's root folders".
    pub lidarr_music_path: Option<String>,
    /// Rating for library songs that Lidarr doesn't mark explicit. Explicit ones are MA.
    pub rating: Rating,
    /// How often to sync again; zero only syncs at startup and from the admin panel.
    pub sync_every: Duration,
}

/// One library track, ready for the `songs` table.
#[derive(Debug, Clone)]
pub struct LibraryTrack {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub year: i64,
    pub genre: Option<String>,
    pub duration_ms: i64,
    pub explicit: bool,
    pub rating: Rating,
    /// The file inside this container.
    pub path: String,
    pub artwork_url: Option<String>,
}

/// What the admin panel shows about the library.
#[derive(Debug, Clone, Default, Serialize)]
pub struct LibraryStatus {
    /// LIDARR_URL and LIDARR_API_KEY are both set.
    pub configured: bool,
    pub syncing: bool,
    /// Unix time in milliseconds of the last finished sync.
    pub last_sync: Option<u64>,
    /// Songs that can play from the library right now.
    pub songs: i64,
    /// Lidarr tracks with a file this container can't see (usually a volume mapping problem).
    pub missing_files: usize,
    /// Tracks left out on purpose: live albums, short interludes, formats browsers can't play.
    pub skipped: usize,
    pub last_report: Option<crate::db::LibrarySync>,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct Library {
    config: LibraryConfig,
    /// Lidarr's address and key, which the Settings page can change while the server runs.
    lidarr: Arc<Mutex<(Option<String>, Option<String>)>>,
    changed: Arc<tokio::sync::Notify>,
    client: reqwest::Client,
    status: Arc<Mutex<LibraryStatus>>,
    /// Only one sync at a time.
    running: Arc<tokio::sync::Mutex<()>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiArtist {
    id: i64,
    artist_name: String,
    #[serde(default)]
    genres: Vec<String>,
    statistics: Option<ApiStatistics>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiStatistics {
    #[serde(default)]
    track_file_count: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiAlbum {
    id: i64,
    title: String,
    release_date: Option<String>,
    #[serde(default)]
    genres: Vec<String>,
    album_type: Option<String>,
    /// Strings in some Lidarr versions, `{ "name": ... }` objects in others.
    #[serde(default)]
    secondary_types: Vec<Value>,
    #[serde(default)]
    images: Vec<ApiImage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiImage {
    cover_type: Option<String>,
    remote_url: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiTrack {
    album_id: i64,
    #[serde(default)]
    track_file_id: i64,
    title: String,
    /// Milliseconds.
    #[serde(default)]
    duration: i64,
    #[serde(default)]
    explicit: bool,
    #[serde(default)]
    has_file: bool,
}

#[derive(Deserialize)]
struct ApiTrackFile {
    id: i64,
    path: String,
}

#[derive(Deserialize)]
struct ApiRootFolder {
    path: String,
}

impl ApiAlbum {
    fn year(&self) -> Option<i64> {
        self.release_date.as_deref()?.get(..4)?.parse().ok()
    }

    /// Live albums, remixes and the like.
    fn skipped(&self) -> bool {
        self.secondary_types.iter().any(|t| {
            let name = t
                .as_str()
                .or_else(|| t.get("name").and_then(Value::as_str))
                .unwrap_or("");
            SKIPPED_ALBUM_TYPES.contains(&name.to_ascii_lowercase().as_str())
        })
    }

    /// Prefer the studio album over a single, EP or compilation when a song is on several.
    fn preference(&self) -> u8 {
        let compilation = self.secondary_types.iter().any(|t| {
            let name = t
                .as_str()
                .or_else(|| t.get("name").and_then(Value::as_str))
                .unwrap_or("");
            name.eq_ignore_ascii_case("compilation") || name.eq_ignore_ascii_case("soundtrack")
        });
        match (self.album_type.as_deref(), compilation) {
            (Some("Album"), false) => 0,
            (Some("Single") | Some("EP"), false) => 1,
            _ => 2,
        }
    }

    fn cover(&self) -> Option<String> {
        self.images
            .iter()
            .find(|i| i.cover_type.as_deref() == Some("cover"))
            .and_then(|i| i.remote_url.clone())
            .filter(|u| u.starts_with("https://") || u.starts_with("http://"))
    }
}

impl Library {
    pub fn new(config: LibraryConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .user_agent("TriviaBix/0.1")
            .build()
            .expect("building HTTP client");
        let status = LibraryStatus {
            configured: config.lidarr_url.is_some() && config.api_key.is_some(),
            ..Default::default()
        };
        Self {
            lidarr: Arc::new(Mutex::new((
                config.lidarr_url.clone(),
                config.api_key.clone(),
            ))),
            changed: Default::default(),
            config,
            client,
            status: Arc::new(Mutex::new(status)),
            running: Default::default(),
        }
    }

    /// Points the library at another Lidarr (from the Settings page) and syncs from it.
    pub fn set_lidarr(&self, url: Option<String>, api_key: Option<String>) {
        let configured = url.is_some() && api_key.is_some();
        *self.lidarr.lock().unwrap() = (url, api_key);
        let mut status = self.status.lock().unwrap();
        status.configured = configured;
        status.error = None;
        drop(status);
        self.changed.notify_one();
    }

    pub fn configured(&self) -> bool {
        self.status.lock().unwrap().configured
    }

    pub fn rating(&self) -> Rating {
        self.config.rating
    }

    pub async fn status(&self, db: &Db) -> LibraryStatus {
        let mut status = self.status.lock().unwrap().clone();
        status.songs = db.library_count().await.unwrap_or(0);
        status
    }

    async fn get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> anyhow::Result<T> {
        let (base, key) = self.lidarr.lock().unwrap().clone();
        let (base, key) = (base.unwrap_or_default(), key.unwrap_or_default());
        self.client
            .get(format!("{}/api/v1/{path}", base.trim_end_matches('/')))
            .header("X-Api-Key", &key)
            .query(query)
            .send()
            .await
            .with_context(|| format!("calling Lidarr at {base}"))?
            .error_for_status()
            .context("Lidarr said no (check the Lidarr API key in Settings)")?
            .json()
            .await
            .with_context(|| format!("reading Lidarr's {path} list"))
    }

    /// Syncs the library now. Returns the new status; a failure is in `status.error`.
    pub async fn sync(&self, db: &Db) -> LibraryStatus {
        if !self.configured() {
            return self.status(db).await;
        }
        let Ok(_guard) = self.running.try_lock() else {
            // Someone else is already syncing; report what they'll report.
            return self.status(db).await;
        };
        self.status.lock().unwrap().syncing = true;
        let started = Instant::now();
        let result = self.fetch().await;
        let mut status = self.status.lock().unwrap().clone();
        status.syncing = false;
        match result {
            Ok(fetched) => {
                status.missing_files = fetched.missing;
                status.skipped = fetched.skipped;
                if fetched.tracks.is_empty() && fetched.missing > 0 {
                    // Lidarr has files but we can't see any: a mount problem, not an empty
                    // library. Keep what we had rather than wiping it.
                    status.error = Some(format!(
                        "Lidarr has {} tracks with files, but none of them are under {}. Check that the music folder is mounted there, and set LIDARR_MUSIC_PATH to the folder as Lidarr sees it.",
                        fetched.missing,
                        self.config.music_dir.display()
                    ));
                } else {
                    match db.sync_library(&fetched.tracks).await {
                        Ok(report) => {
                            tracing::info!(
                                ?report,
                                missing = fetched.missing,
                                skipped = fetched.skipped,
                                "music library synced from Lidarr in {:.1}s",
                                started.elapsed().as_secs_f32()
                            );
                            status.last_report = Some(report);
                            status.last_sync = Some(now_ms());
                            status.error = None;
                        }
                        Err(e) => status.error = Some(format!("Saving the library failed: {e:#}")),
                    }
                }
                if fetched.missing > 0 {
                    tracing::warn!(
                        "{} Lidarr tracks have files this container can't see under {}",
                        fetched.missing,
                        self.config.music_dir.display()
                    );
                }
            }
            Err(e) => {
                tracing::warn!("music library sync failed: {e:#}");
                status.error = Some(format!("{e:#}"));
            }
        }
        if let Some(e) = &status.error {
            tracing::warn!("music library: {e}");
        }
        *self.status.lock().unwrap() = status;
        self.status(db).await
    }

    /// Starts a sync in the background (unless one is running) and returns the status right away.
    pub async fn start_sync(&self, db: &Db) -> LibraryStatus {
        if self.configured() && !self.status.lock().unwrap().syncing {
            self.status.lock().unwrap().syncing = true;
            let (this, db) = (self.clone(), db.clone());
            tokio::spawn(async move { this.sync(&db).await });
        }
        self.status(db).await
    }

    async fn fetch(&self) -> anyhow::Result<Fetched> {
        let artists: Vec<ApiArtist> = self.get("artist", &[]).await?;
        let albums: Vec<ApiAlbum> = self.get("album", &[]).await?;
        let albums: HashMap<i64, ApiAlbum> = albums.into_iter().map(|a| (a.id, a)).collect();
        let roots: Vec<String> = match &self.config.lidarr_music_path {
            Some(p) => vec![p.clone()],
            None => self
                .get::<Vec<ApiRootFolder>>("rootfolder", &[])
                .await?
                .into_iter()
                .map(|r| r.path)
                .collect(),
        };

        let with_files: Vec<ApiArtist> = artists
            .into_iter()
            .filter(|a| a.statistics.as_ref().is_none_or(|s| s.track_file_count > 0))
            .collect();
        let per_artist: Vec<(ApiArtist, Vec<ApiTrack>, Vec<ApiTrackFile>)> =
            stream::iter(with_files)
                .map(|artist| {
                    let this = self.clone();
                    async move {
                        let q = [("artistId", artist.id.to_string())];
                        let tracks = this.get::<Vec<ApiTrack>>("track", &q).await?;
                        let files = this.get::<Vec<ApiTrackFile>>("trackfile", &q).await?;
                        anyhow::Ok((artist, tracks, files))
                    }
                })
                .buffer_unordered(PARALLEL_CALLS)
                .try_collect()
                .await?;

        let mut fetched = Fetched::default();
        // song key -> (album preference, track), so each song is in the game once.
        let mut best: HashMap<String, (u8, LibraryTrack)> = HashMap::new();
        let mut checked_dirs: HashSet<PathBuf> = HashSet::new();
        for (artist, tracks, files) in per_artist {
            let files: HashMap<i64, String> = files.into_iter().map(|f| (f.id, f.path)).collect();
            for t in tracks {
                if !t.has_file || t.track_file_id == 0 {
                    continue;
                }
                let Some(lidarr_path) = files.get(&t.track_file_id) else {
                    continue;
                };
                let Some(album) = albums.get(&t.album_id) else {
                    continue;
                };
                let playable = Path::new(lidarr_path)
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| PLAYABLE.contains(&e.to_ascii_lowercase().as_str()));
                let Some(year) = album.year().filter(|y| *y > 0) else {
                    fetched.skipped += 1;
                    continue;
                };
                if !playable || album.skipped() || (t.duration > 0 && t.duration < MIN_TRACK_MS) {
                    fetched.skipped += 1;
                    continue;
                }
                let Some(path) = self.local_path(lidarr_path, &roots) else {
                    fetched.missing += 1;
                    continue;
                };
                // Checking the folder once is enough to catch a bad mount without touching
                // every file on a sleeping array disk.
                let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
                if !checked_dirs.contains(&dir) {
                    if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
                        fetched.missing += 1;
                        continue;
                    }
                    checked_dirs.insert(dir);
                }
                let track = LibraryTrack {
                    title: t.title.trim().to_string(),
                    artist: artist.artist_name.trim().to_string(),
                    album: Some(album.title.clone()),
                    year,
                    genre: album
                        .genres
                        .first()
                        .or(artist.genres.first())
                        .map(|g| title_case(g)),
                    duration_ms: t.duration,
                    explicit: t.explicit,
                    rating: if t.explicit {
                        Rating::Ma.max(self.config.rating)
                    } else {
                        self.config.rating
                    },
                    path: path.to_string_lossy().into_owned(),
                    artwork_url: album.cover(),
                };
                let key = song_key(&track.title, &track.artist);
                let pref = album.preference();
                match best.get(&key) {
                    Some((p, old)) if (*p, old.year) <= (pref, track.year) => {}
                    _ => {
                        best.insert(key, (pref, track));
                    }
                }
            }
        }
        fetched.tracks = best.into_values().map(|(_, t)| t).collect();
        fetched.tracks.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(fetched)
    }

    /// Maps a path as Lidarr sees it to the same file in this container. None if it isn't
    /// under any of Lidarr's music folders.
    fn local_path(&self, lidarr_path: &str, roots: &[String]) -> Option<PathBuf> {
        let mut roots: Vec<&str> = roots.iter().map(|r| r.trim_end_matches('/')).collect();
        roots.sort_by_key(|r| std::cmp::Reverse(r.len()));
        for root in roots {
            if let Some(rel) = lidarr_path.strip_prefix(root)
                && (rel.is_empty() || rel.starts_with('/'))
            {
                let rel = rel.trim_start_matches('/');
                // No escaping the music folder with "..".
                if Path::new(rel)
                    .components()
                    .any(|c| !matches!(c, std::path::Component::Normal(_)))
                {
                    return None;
                }
                return Some(self.config.music_dir.join(rel));
            }
        }
        None
    }

    /// Syncs at startup, then every `sync_every`.
    pub async fn run(self, db: Db) {
        if !self.configured() {
            tracing::info!("Lidarr isn't set up, so the music library is off until it is");
        }
        loop {
            if self.configured() {
                self.sync(&db).await;
            }
            // Again on schedule, or straight away when the Settings page changes Lidarr.
            if self.config.sync_every.is_zero() || !self.configured() {
                self.changed.notified().await;
            } else {
                tokio::select! {
                    _ = tokio::time::sleep(self.config.sync_every) => {}
                    _ = self.changed.notified() => {}
                }
            }
        }
    }
}

#[derive(Default)]
struct Fetched {
    tracks: Vec<LibraryTrack>,
    missing: usize,
    skipped: usize,
}

/// The same song whatever the punctuation or case: "Don't Stop Believin'" by Journey matches
/// "Don’t Stop Believin’" by JOURNEY.
pub fn song_key(title: &str, artist: &str) -> String {
    let simplify = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    };
    format!("{}|{}", simplify(title), simplify(artist))
}

fn title_case(s: &str) -> String {
    s.split(' ')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().chain(c).collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Hands out unguessable `/media/<token>` links for library files, so the TV can stream a clip
/// without the link giving away the song, and nobody can walk the whole library by guessing.
#[derive(Clone, Default)]
pub struct Media {
    inner: Arc<Mutex<MediaInner>>,
}

#[derive(Default)]
struct MediaInner {
    by_token: HashMap<String, (PathBuf, Instant)>,
    by_path: HashMap<PathBuf, String>,
}

/// Links last a good while so a long game (or a TV left on the lobby) keeps working.
const MEDIA_TTL: Duration = Duration::from_secs(12 * 60 * 60);

impl Media {
    /// A link to `path`, reusing the existing one so the browser can cache it.
    pub fn url(&self, path: &Path) -> String {
        let mut inner = self.inner.lock().unwrap();
        let now = Instant::now();
        if let Some(token) = inner.by_path.get(path).cloned()
            && let Some(entry) = inner.by_token.get_mut(&token)
        {
            entry.1 = now + MEDIA_TTL;
            return format!("/media/{token}");
        }
        // Forget expired links now and then.
        if inner.by_token.len() > 1000 {
            inner.by_token.retain(|_, (_, expires)| *expires > now);
            let live: HashSet<String> = inner.by_token.keys().cloned().collect();
            inner.by_path.retain(|_, t| live.contains(t));
        }
        let token = uuid::Uuid::new_v4().simple().to_string();
        inner
            .by_token
            .insert(token.clone(), (path.to_path_buf(), now + MEDIA_TTL));
        inner.by_path.insert(path.to_path_buf(), token.clone());
        format!("/media/{token}")
    }

    pub fn path(&self, token: &str) -> Option<PathBuf> {
        let inner = self.inner.lock().unwrap();
        inner
            .by_token
            .get(token)
            .filter(|(_, expires)| *expires > Instant::now())
            .map(|(p, _)| p.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library(root: &str) -> Library {
        Library::new(LibraryConfig {
            music_dir: PathBuf::from("/music"),
            lidarr_music_path: Some(root.into()),
            ..Default::default()
        })
    }

    #[test]
    fn maps_lidarr_paths_into_the_container() {
        let lib = library("/data/media/music/");
        let roots = vec!["/data/media/music/".to_string()];
        assert_eq!(
            lib.local_path(
                "/data/media/music/Queen/A Night at the Opera/11.flac",
                &roots
            ),
            Some(PathBuf::from("/music/Queen/A Night at the Opera/11.flac"))
        );
        // A sibling folder that merely starts with the same letters isn't inside it.
        assert_eq!(lib.local_path("/data/media/music2/x.flac", &roots), None);
        assert_eq!(
            lib.local_path("/data/media/music/../../etc/passwd", &roots),
            None
        );
        assert_eq!(lib.local_path("/elsewhere/x.flac", &roots), None);
    }

    #[test]
    fn song_keys_ignore_punctuation_and_case() {
        assert_eq!(
            song_key("Don't Stop Believin'", "Journey"),
            song_key("Don’t Stop Believin’", "JOURNEY")
        );
        assert_ne!(
            song_key("Hey Jude", "The Beatles"),
            song_key("Hey", "Jude The Beatles")
        );
    }

    #[test]
    fn album_kinds() {
        let album: ApiAlbum = serde_json::from_value(serde_json::json!({
            "id": 1, "title": "Alive!", "releaseDate": "1975-09-10T00:00:00Z",
            "albumType": "Album", "secondaryTypes": [{"id": 6, "name": "Live"}]
        }))
        .unwrap();
        assert!(album.skipped());
        assert_eq!(album.year(), Some(1975));
        let album: ApiAlbum = serde_json::from_value(serde_json::json!({
            "id": 2, "title": "Hits", "albumType": "Album", "secondaryTypes": ["Compilation"]
        }))
        .unwrap();
        assert!(!album.skipped());
        assert_eq!(album.preference(), 2);
        assert_eq!(album.year(), None);
    }

    #[test]
    fn media_links_are_stable_and_unguessable() {
        let media = Media::default();
        let a = media.url(Path::new("/music/a.mp3"));
        assert_eq!(a, media.url(Path::new("/music/a.mp3")));
        assert_ne!(a, media.url(Path::new("/music/b.mp3")));
        let token = a.trim_start_matches("/media/");
        assert_eq!(media.path(token), Some(PathBuf::from("/music/a.mp3")));
        assert_eq!(media.path("nope"), None);
    }
}
