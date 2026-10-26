-- Bix's voice packs: a named voice and speaking speed. Each voice's lines stay in its own folder
-- of the asset store (assets/voice/<style>/), so switching packs and back loses and re-pays nothing.
-- The pack in use is the one whose voice is BIX_VOICE.
CREATE TABLE voice_packs (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    name       TEXT NOT NULL UNIQUE,
    voice      TEXT NOT NULL UNIQUE,
    speed      TEXT NOT NULL DEFAULT '1.0',
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    used_at    TEXT
);
