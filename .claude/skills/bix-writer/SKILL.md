---
name: bix-writer
description: Write, punch up, or review lines for Bix, the TriviaBix host. Use when asked to write host lines or jokes, add to server/host-script.json or content/scripts, change Bix's persona, deepen his character, add a running gag or cameo, or check that lines sound like him.
---

# Writing for Bix

Bix's character bible is a Story Skills project in `bible/`. It is the source of truth for who he is. Everything he says comes from it:

- `server/host-script.json`: the generic lines for every moment of a game, by event and rating.
- `content/scripts/`: each question's own lead-in and reveal lines.
- `DEFAULT_PERSONA` in `crates/question-bank/src/lib.rs`: the summary of the bible that Claude writes new per-question lines from (`triviabix-questions scripts generate`, and the admin panel's Pipeline). Keep it under `MAX_PERSONA_CHARS` and in step with the bible.
- The cameo characters in `crates/question-bank/src/cameo.rs`. Each is a job Bix took on the way down; their backstory is in `bible/characters/bix.md`.

## Before writing

1. Read `bible/characters/bix.md` in full: the comedy toolkit, the running-gag ladders, the tone line, and the ratings ladder.
2. Read `bible/style-sheet.md`: these lines are spoken by text-to-speech, so its rules on length, punctuation and numbers are hard rules.
3. For new lines in `host-script.json`, check `EVENTS` in `server/src/host.rs` for the placeholders each event fills in, and read the event's existing lines so the new ones don't repeat a joke.

If the Story Skills skills are installed (`npx skills add danjdewhurst/story-skills`, or cloned into `~/.claude/skills`), use them:

- `genre-craft` (its `references/comedy.md`): escalation, call-backs, running gags that move, the comic flaw, straight man, tone line.
- `character-management`: to add or change a character in `bible/characters/` (Marty, Brenda, Gary, Doreen, and anyone new).
- `voice-style`: to update the style sheet or check voice drift.
- `line-editing` (its read-aloud pass): comic timing is easier to hear than see.

## Writing rules

- Every line should sound like it could only come from Bix. A generic game-show line ("Great job!") is a missed joke.
- One big comic move per line. Vary the moves across an event: crack and snap, callback, fake-out, mock hype, escalation, status flip, stealth compliment.
- Running gags climb their ladder; don't write the same rung twice. The top rungs (sincere) belong to finale and sign-off lines.
- Career cracks in about one line in four at most, per event. The rest is pure energy.
- Roast what players do, never who they are, at every rating. Nothing hateful toward real groups of people.
- Every event needs at least one PG line out of character. A room favours its spiciest allowed tier, so the higher tiers need enough lines not to repeat in a night.
- A cameo line starts with its tag (`[as:noir]`) and only uses slugs from `cameo.rs`.

## After writing

```sh
cargo test -p triviabix --lib host          # the script parses: events, ratings, placeholders, cameos
story check bible                           # if the bible changed (or: node <skills>/story-maintenance/scripts/story.js check bible)
```

With Bix's own voice set up, a game prefers lines whose audio is ready, so new lines join the rotation as the voice service makes them. Lines with no placeholders are queued at startup; the rest are made ahead of time per player name.
