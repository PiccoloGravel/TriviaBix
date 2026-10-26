//! The registry of live rooms, keyed by room code.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rand::Rng;

use crate::db::Db;
use crate::host::Script;
use crate::music::Itunes;
use crate::room::{Room, RoomHandle, Settings, Timing};
use crate::voice::Voice;

/// No vowels, so random codes can't spell anything rude, and no letters that
/// look alike on a TV across the room.
const CODE_ALPHABET: &[u8] = b"BCDFGHJKLMNPQRSTVWXZ";
const CODE_LEN: usize = 4;

#[derive(Clone)]
pub struct Rooms {
    inner: Arc<Mutex<HashMap<String, RoomHandle>>>,
    db: Db,
    script: Arc<Script>,
    itunes: Itunes,
    voice: Voice,
    timing: Timing,
    max_players: usize,
    max_rooms: usize,
}

impl Rooms {
    pub fn new(
        db: Db,
        script: Arc<Script>,
        itunes: Itunes,
        voice: Voice,
        timing: Timing,
        max_players: usize,
        max_rooms: usize,
    ) -> Self {
        Self {
            inner: Default::default(),
            db,
            script,
            itunes,
            voice,
            timing,
            max_players,
            max_rooms,
        }
    }

    /// Creates a room and returns its code and the secret host token. `max_players` is set
    /// from the admin panel; otherwise the room takes the server's `MAX_PLAYERS`.
    pub fn create(
        &self,
        settings: Settings,
        max_players: Option<usize>,
    ) -> Result<(String, String), String> {
        let mut rooms = self.inner.lock().unwrap();
        if rooms.len() >= self.max_rooms {
            return Err("Too many games running right now. Try again in a bit.".into());
        }
        let mut rng = rand::rng();
        let code = loop {
            let code: String = (0..CODE_LEN)
                .map(|_| CODE_ALPHABET[rng.random_range(0..CODE_ALPHABET.len())] as char)
                .collect();
            if !rooms.contains_key(&code) {
                break code;
            }
        };
        let host_token = uuid::Uuid::new_v4().to_string();
        let registry = self.inner.clone();
        let closing = code.clone();
        let handle = Room::spawn(
            code.clone(),
            host_token.clone(),
            settings,
            self.timing.clone(),
            clamp_players(max_players.unwrap_or(self.max_players)),
            self.db.clone(),
            self.script.clone(),
            self.itunes.clone(),
            self.voice.clone(),
            move || {
                registry.lock().unwrap().remove(&closing);
            },
        );
        rooms.insert(code.clone(), handle);
        tracing::info!(%code, "room created");
        Ok((code, host_token))
    }

    pub fn get(&self, code: &str) -> Option<RoomHandle> {
        self.inner
            .lock()
            .unwrap()
            .get(&normalize_code(code))
            .cloned()
    }

    /// Every live room, for the admin panel.
    pub fn all(&self) -> Vec<RoomHandle> {
        self.inner.lock().unwrap().values().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub fn normalize_code(code: &str) -> String {
    code.trim().to_ascii_uppercase()
}

/// One character each, so a room can't hold more players than there are characters.
pub fn clamp_players(n: usize) -> usize {
    n.clamp(1, crate::characters::CHARACTERS.len())
}
