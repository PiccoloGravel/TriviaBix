-- Bix's personas: who he is (the persona Claude writes his lines in) and the lines written for
-- it. The original persona's lines are the shared ones (content/scripts outside personas/), so
-- its folder is NULL; every other persona keeps its own in content/scripts/personas/<name>/.
-- Exactly one is active; its text is HOST_PERSONA.
CREATE TABLE persona_packs (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    name       TEXT NOT NULL UNIQUE,
    -- NULL is the built-in default persona.
    persona    TEXT,
    folder     TEXT UNIQUE,
    -- Where this persona has no line of its own, use the shared one instead of none.
    fill_gaps  INTEGER NOT NULL DEFAULT 1,
    active     INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    used_at    TEXT
);
