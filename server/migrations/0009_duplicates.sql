-- Duplicate questions. `hidden` is set from the admin panel when a question asks the same thing
-- as another; unlike `active` it survives reloading the packs, and hidden questions aren't played.
ALTER TABLE questions ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0;

-- Look-alike questions an admin checked and decided are different. `a` is the lower id.
CREATE TABLE question_kept_pairs (
    a          INTEGER NOT NULL REFERENCES questions(id) ON DELETE CASCADE,
    b          INTEGER NOT NULL REFERENCES questions(id) ON DELETE CASCADE,
    created_at TEXT    NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (a, b),
    CHECK (a < b)
);

-- Every question a game dealt, so new games (in any room) can skip the recently played ones.
CREATE TABLE question_plays (
    question_id INTEGER NOT NULL REFERENCES questions(id) ON DELETE CASCADE,
    played_at   TEXT    NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX question_plays_played_at ON question_plays(played_at);
