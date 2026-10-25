-- Question bank. `wrong` is a JSON array of incorrect answers; choices are
-- shuffled when a game loads them so the correct answer moves around.
CREATE TABLE questions (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    category    TEXT    NOT NULL,
    prompt      TEXT    NOT NULL,
    correct     TEXT    NOT NULL,
    wrong       TEXT    NOT NULL,
    -- Movie-style content rating; a room plays everything at or below its max.
    rating      TEXT    NOT NULL DEFAULT 'PG' CHECK (rating IN ('PG', 'PG-13', 'MA', 'NC-17')),
    source      TEXT    NOT NULL DEFAULT 'seed',
    created_at  TEXT    NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE UNIQUE INDEX questions_prompt ON questions(prompt);

CREATE TABLE games (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    room_code    TEXT    NOT NULL,
    rounds       INTEGER NOT NULL,
    player_count INTEGER NOT NULL,
    started_at   TEXT    NOT NULL DEFAULT CURRENT_TIMESTAMP,
    finished_at  TEXT
);

CREATE TABLE game_players (
    game_id INTEGER NOT NULL REFERENCES games(id),
    name    TEXT    NOT NULL,
    score   INTEGER NOT NULL,
    rank    INTEGER NOT NULL
);
CREATE INDEX game_players_game ON game_players(game_id);
