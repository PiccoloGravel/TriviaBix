//! Name That Tune's songs: files from the operator's music library (see `library.rs`), and
//! previews from the iTunes Search API (no key needed) for the curated hits.
//!
//! Apple hosts a 30-second preview for most tracks and allows apps to stream it to promote
//! the track, as long as the preview is played as-is (not downloaded or re-hosted) and the
//! app links back to the store. So we only keep track ids and links; the TV streams the
//! preview straight from Apple and the reveal shows where to hear the full song.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use rand::Rng;
use rand::seq::SliceRandom;
use serde::Deserialize;

use crate::db::{Db, Question, Rating, Song, SongClip};
use crate::library::{Library, Media};
use crate::protocol::SongSource;

/// Previews are about 30 seconds; clips start somewhere in this window so it isn't always the intro.
const CLIP_START_MS: std::ops::RangeInclusive<u64> = 2_000..=20_000;
/// Searches per game are capped because the iTunes API allows roughly 20 calls a minute.
const MAX_SEARCHES: usize = 16;

/// One playable track.
#[derive(Debug, Clone)]
pub struct Track {
    pub itunes_id: i64,
    pub preview_url: String,
    pub artwork_url: Option<String>,
    pub store_url: Option<String>,
    pub explicit: bool,
}

#[derive(Clone)]
pub struct Itunes {
    client: reqwest::Client,
    base: String,
    country: String,
}

#[derive(Deserialize)]
struct Response {
    results: Vec<ApiTrack>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiTrack {
    kind: Option<String>,
    track_id: Option<i64>,
    track_name: Option<String>,
    artist_name: Option<String>,
    preview_url: Option<String>,
    artwork_url100: Option<String>,
    track_view_url: Option<String>,
    track_explicitness: Option<String>,
}

impl ApiTrack {
    fn into_track(self) -> Option<Track> {
        if self.kind.as_deref().is_some_and(|k| k != "song") {
            return None;
        }
        Some(Track {
            itunes_id: self.track_id?,
            preview_url: self.preview_url.filter(|u| !u.is_empty())?,
            // Bigger cover art for the TV; Apple serves any size from the same path.
            artwork_url: self
                .artwork_url100
                .map(|u| u.replace("100x100bb", "600x600bb")),
            store_url: self.track_view_url,
            explicit: self.track_explicitness.as_deref() == Some("explicit"),
        })
    }
}

impl Itunes {
    pub fn new(base: &str, country: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .user_agent("TriviaBix/0.1")
            .build()
            .expect("building HTTP client");
        Self {
            client,
            base: base.trim_end_matches('/').to_string(),
            country: country.to_string(),
        }
    }

    async fn get(&self, path: &str, query: &[(&str, &str)]) -> anyhow::Result<Vec<ApiTrack>> {
        let res: Response = self
            .client
            .get(format!("{}/{path}", self.base))
            .query(query)
            .query(&[("country", self.country.as_str())])
            .send()
            .await
            .context("calling the iTunes API")?
            .error_for_status()
            .context("the iTunes API said no")?
            .json()
            .await
            .context("reading the iTunes API response")?;
        Ok(res.results)
    }

    /// Finds the track for a title and artist. Results whose artist doesn't match are skipped
    /// so a cover version or karaoke track doesn't sneak in. With `clean_only`, explicit
    /// versions are skipped too.
    pub async fn search(
        &self,
        title: &str,
        artist: &str,
        clean_only: bool,
    ) -> anyhow::Result<Option<Track>> {
        let term = format!("{artist} {title}");
        let results = self
            .get(
                "search",
                &[
                    ("term", &term),
                    ("media", "music"),
                    ("entity", "song"),
                    ("limit", "10"),
                ],
            )
            .await?;
        let want_artist = simplify(artist);
        let want_title = simplify(title);
        Ok(results
            .into_iter()
            .filter(|t| {
                let a = simplify(t.artist_name.as_deref().unwrap_or(""));
                let n = simplify(t.track_name.as_deref().unwrap_or(""));
                a.contains(&want_artist) && n.contains(&want_title)
            })
            .filter_map(ApiTrack::into_track)
            .find(|t| !(clean_only && t.explicit)))
    }

    /// Refreshes known tracks in one request. Preview links can change, so we look them up
    /// again at the start of each game rather than storing them.
    pub async fn lookup(&self, ids: &[i64]) -> anyhow::Result<HashMap<i64, Track>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let ids = ids
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let results = self
            .get("lookup", &[("id", &ids), ("entity", "song")])
            .await?;
        Ok(results
            .into_iter()
            .filter_map(ApiTrack::into_track)
            .map(|t| (t.itunes_id, t))
            .collect())
    }
}

/// Everything a room needs to find songs: Apple's previews and the operator's own library.
#[derive(Clone)]
pub struct Music {
    pub itunes: Itunes,
    pub library: Library,
    pub media: Media,
}

/// Builds `n` Name That Tune questions: picks songs not in `exclude` from `source`, finds a
/// playable clip for each (a library file, or else an Apple preview), and adds three wrong answers
/// from songs of the same genre released around the same time.
pub async fn build_round(
    db: &Db,
    music: &Music,
    n: usize,
    max_rating: Rating,
    exclude: &[i64],
    source: SongSource,
) -> anyhow::Result<Vec<Question>> {
    let itunes = &music.itunes;
    let pool = db.songs(max_rating).await?;
    let fresh_pool: Vec<&Song> = pool.iter().filter(|s| !exclude.contains(&s.id)).collect();
    let candidates = order_candidates(
        if fresh_pool.len() < n {
            // Played through them all; repeats beat a short game.
            pool.iter().collect()
        } else {
            fresh_pool
        },
        source,
        &mut rand::rng(),
    );

    let clean_only = max_rating == Rating::Pg;
    let known: Vec<i64> = candidates
        .iter()
        .filter(|s| s.file_path.is_none())
        .take(n + 6)
        .filter_map(|s| s.itunes_id)
        .collect();
    // With a music library, Apple being unreachable just means fewer songs to pick from.
    let mut apple_error = None;
    let mut fresh = match itunes.lookup(&known).await {
        Ok(found) => found,
        Err(e) => {
            tracing::warn!("Apple lookup failed: {e:#}");
            apple_error = Some(e);
            HashMap::new()
        }
    };

    let mut picked: Vec<(&Song, Clip)> = Vec::with_capacity(n);
    let mut searches = 0;
    for song in candidates {
        if picked.len() == n {
            break;
        }
        if let Some(path) = local_file(song) {
            picked.push((song, Clip::Local(music.media.url(&path))));
            continue;
        }
        let cached = song
            .itunes_id
            .and_then(|id| fresh.remove(&id))
            .filter(|t| !(clean_only && t.explicit));
        let track = match cached {
            Some(t) => t,
            None if searches < MAX_SEARCHES && apple_error.is_none() => {
                searches += 1;
                let found = match itunes.search(&song.title, &song.artist, clean_only).await {
                    Ok(found) => found,
                    // Probably rate limited. Play what we have (and library songs) rather than nothing.
                    Err(e) => {
                        tracing::warn!("song search failed, skipping Apple for this round: {e:#}");
                        apple_error = Some(e);
                        continue;
                    }
                };
                match found {
                    Some(t) => {
                        if let Err(e) = db.save_track(song.id, &t).await {
                            tracing::warn!("saving track for song {}: {e:#}", song.id);
                        }
                        t
                    }
                    None => {
                        tracing::info!(title = %song.title, artist = %song.artist, "no playable preview");
                        // A clean-only miss may still have an explicit version for wilder rooms.
                        if !clean_only {
                            let _ = db.mark_unplayable(song.id).await;
                        }
                        continue;
                    }
                }
            }
            None => continue,
        };
        picked.push((song, Clip::Apple(track)));
    }
    if picked.is_empty()
        && let Some(e) = apple_error
    {
        return Err(e);
    }

    // ThreadRng can't be held across an await, so it's only made once the lookups are done.
    let mut rng = rand::rng();
    Ok(picked
        .into_iter()
        .map(|(song, clip)| {
            let mut choices: Vec<String> = decoys(&pool, song, &mut rng)
                .into_iter()
                .map(label)
                .collect();
            choices.push(label(song));
            choices.shuffle(&mut rng);
            let answer = choices.iter().position(|c| *c == label(song)).unwrap();
            let category = match &song.genre {
                Some(g) => format!("Name That Tune · {} · {g}", decade(song.year)),
                None => format!("Name That Tune · {}", decade(song.year)),
            };
            let song_clip = match clip {
                Clip::Apple(track) => SongClip {
                    title: song.title.clone(),
                    artist: song.artist.clone(),
                    year: song.year,
                    preview_url: track.preview_url,
                    start_ms: rng.random_range(CLIP_START_MS),
                    artwork_url: track.artwork_url,
                    store_url: track.store_url,
                    album: None,
                    from_library: false,
                },
                Clip::Local(url) => SongClip {
                    title: song.title.clone(),
                    artist: song.artist.clone(),
                    year: song.year,
                    preview_url: url,
                    start_ms: library_start_ms(song.duration_ms, &mut rng),
                    artwork_url: song.artwork_url.clone(),
                    store_url: None,
                    album: song.album.clone(),
                    from_library: true,
                },
            };
            Question {
                id: song.id,
                category,
                prompt: "Name that tune!".into(),
                choices,
                answer,
                song: Some(song_clip),
                lines: Default::default(),
            }
        })
        .collect())
}

enum Clip {
    Apple(Track),
    /// A `/media/...` link to a library file.
    Local(String),
}

/// The song's library file, if it has one this container can still see.
fn local_file(song: &Song) -> Option<PathBuf> {
    let path = PathBuf::from(song.file_path.as_deref()?);
    path.is_file().then_some(path)
}

/// Shuffles the songs and orders them by `source`: library songs are the ones with a file, hits
/// are the curated list. Whatever the source, the other kind follows as a fallback.
fn order_candidates<'a>(
    mut songs: Vec<&'a Song>,
    source: SongSource,
    rng: &mut impl Rng,
) -> Vec<&'a Song> {
    songs.shuffle(rng);
    let (library, hits): (Vec<&Song>, Vec<&Song>) =
        songs.into_iter().partition(|s| s.source == "library");
    // Curated songs that are also in the library count as hits here.
    match source {
        SongSource::Library => {
            let (with_file, without): (Vec<&Song>, Vec<&Song>) =
                hits.into_iter().partition(|s| s.file_path.is_some());
            let mut mine = library;
            mine.extend(with_file);
            mine.shuffle(rng);
            mine.extend(without);
            mine
        }
        SongSource::Hits => {
            let mut out = hits;
            out.extend(library);
            out
        }
        SongSource::Mix => {
            let mut out = Vec::with_capacity(library.len() + hits.len());
            let (mut a, mut b) = (hits.into_iter(), library.into_iter());
            loop {
                match (a.next(), b.next()) {
                    (None, None) => break,
                    (x, y) => out.extend(x.into_iter().chain(y)),
                }
            }
            out
        }
    }
}

/// Somewhere between a fifth and half way into the track, where the chorus usually is, leaving
/// room for the clip and the reveal to play.
fn library_start_ms(duration_ms: Option<i64>, rng: &mut impl Rng) -> u64 {
    match duration_ms.filter(|d| *d >= 60_000) {
        Some(d) => {
            let d = d as u64;
            rng.random_range(d / 5..=(d / 2).min(d.saturating_sub(20_000)).max(d / 5))
        }
        None => rng.random_range(20_000..=45_000),
    }
}

/// Three other songs from around the same year (and the same genre, when we know it), so the era
/// or the style alone doesn't give it away.
fn decoys<'a>(pool: &'a [Song], answer: &Song, rng: &mut impl Rng) -> Vec<&'a Song> {
    let mut others: Vec<&Song> = pool
        .iter()
        .filter(|s| s.id != answer.id && !s.title.eq_ignore_ascii_case(&answer.title))
        .collect();
    others.shuffle(rng);
    // A different genre counts like being eight years apart.
    let off_genre = |s: &Song| match (&answer.genre, &s.genre) {
        (Some(a), Some(b)) if a.eq_ignore_ascii_case(b) => 0,
        (Some(_), _) => 8,
        (None, _) => 0,
    };
    others.sort_by_key(|s| (s.year - answer.year).abs() + off_genre(s));
    let mut close: Vec<&Song> = others.into_iter().take(8).collect();
    close.shuffle(rng);
    close.truncate(3);
    close
}

fn label(song: &Song) -> String {
    format!("{} — {}", song.title, song.artist)
}

fn decade(year: i64) -> String {
    let d = year / 10 * 10;
    if d >= 2000 {
        format!("{d}s")
    } else {
        format!("{}s", d % 100)
    }
}

/// Looks up every song once, slowly, so the first Name That Tune game starts fast and stays
/// under the API's rate limit.
pub async fn warm_up(db: Db, itunes: Itunes) {
    let songs = match db.songs(Rating::Nc17).await {
        Ok(s) => s,
        Err(e) => return tracing::warn!("music warm-up: {e:#}"),
    };
    let mut found = 0;
    // Library songs play from their files, so only the curated hits need looking up.
    let curated = songs
        .iter()
        .filter(|s| s.source == "hits" && s.file_path.is_none() && s.itunes_id.is_none());
    for song in curated {
        tokio::time::sleep(Duration::from_secs(4)).await;
        match itunes.search(&song.title, &song.artist, false).await {
            Ok(Some(t)) => {
                found += 1;
                let _ = db.save_track(song.id, &t).await;
            }
            Ok(None) => tracing::info!(title = %song.title, "no preview found during warm-up"),
            Err(e) => return tracing::warn!("music warm-up stopped: {e:#}"),
        }
    }
    tracing::info!("music warm-up found {found} new tracks");
}

/// Lowercase letters and digits only, so "Guns N' Roses" matches "Guns N’ Roses".
fn simplify(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decades_read_naturally() {
        assert_eq!(decade(1985), "80s");
        assert_eq!(decade(1999), "90s");
        assert_eq!(decade(2004), "2000s");
        assert_eq!(decade(2021), "2020s");
    }

    #[test]
    fn simplify_ignores_punctuation() {
        assert_eq!(simplify("Guns N' Roses"), simplify("Guns N’ Roses"));
        assert_eq!(simplify("...Baby One More Time"), "babyonemoretime");
    }

    #[test]
    fn tracks_need_a_preview() {
        let t: ApiTrack = serde_json::from_value(serde_json::json!({
            "kind": "song", "trackId": 7, "trackName": "x", "artistName": "y",
            "artworkUrl100": "https://a/100x100bb.jpg", "trackExplicitness": "notExplicit"
        }))
        .unwrap();
        assert!(t.into_track().is_none());
        let t: ApiTrack = serde_json::from_value(serde_json::json!({
            "kind": "song", "trackId": 7, "previewUrl": "https://p/7.m4a",
            "artworkUrl100": "https://a/100x100bb.jpg", "trackExplicitness": "explicit"
        }))
        .unwrap();
        let t = t.into_track().unwrap();
        assert!(t.explicit);
        assert_eq!(t.artwork_url.as_deref(), Some("https://a/600x600bb.jpg"));
    }
}
