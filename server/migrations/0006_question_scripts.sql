-- Questions now come from the content packs (content/packs, including imported ones). The
-- server loads them at startup and matches rows by `bank_id`, the pack question's stable id.
-- Rows from the old seed migration are adopted by prompt the first time.
ALTER TABLE questions ADD COLUMN bank_id TEXT;
CREATE UNIQUE INDEX questions_bank_id ON questions(bank_id);
-- multiple_choice, true_false, type_answer, closest_number or put_in_order. The game plays
-- multiple choice and true/false; the rest wait in the bank for phone support.
ALTER TABLE questions ADD COLUMN kind TEXT NOT NULL DEFAULT 'multiple_choice';
ALTER TABLE questions ADD COLUMN tags TEXT NOT NULL DEFAULT '[]';
-- The choices in authored order, used instead of shuffling when keep_order is set.
ALTER TABLE questions ADD COLUMN choices TEXT;
ALTER TABLE questions ADD COLUMN keep_order INTEGER NOT NULL DEFAULT 0;
-- 0 once a question is removed from its pack, so it stops showing up without losing history.
ALTER TABLE questions ADD COLUMN active INTEGER NOT NULL DEFAULT 1;
-- The whole pack question as JSON, for question types the game can't play yet.
ALTER TABLE questions ADD COLUMN data TEXT;

-- What the host says about a question: a lead-in before it, or a joke, anecdote or fun fact
-- after the reveal. A room only hears lines at or below its rating. `content` lines are
-- reloaded from the packs and script files at every start; `edited` lines come from the admin
-- panel, survive reloads, and replace the content lines for that question and slot.
CREATE TABLE question_lines (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    question_id INTEGER NOT NULL REFERENCES questions(id) ON DELETE CASCADE,
    slot        TEXT    NOT NULL CHECK (slot IN ('lead_in', 'reveal')),
    rating      TEXT    NOT NULL DEFAULT 'PG' CHECK (rating IN ('PG', 'PG-13', 'MA', 'NC-17')),
    text        TEXT    NOT NULL,
    origin      TEXT    NOT NULL CHECK (origin IN ('content', 'edited')),
    created_at  TEXT    NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX question_lines_question ON question_lines(question_id);
