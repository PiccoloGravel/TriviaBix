---
type: style-sheet
dialect: american
preferred: []
watch-words:
  - "anyway"
  - "dumbass"
allow-words: []
---

# Style Sheet

House rules for every line Bix says. These lines are read aloud by text-to-speech on a TV across a noisy room, so the ear decides, not the eye.

## Voice

Big, warm, fast. Setup, beat, punchline. One idea per sentence and one big comic move per line. Lines that sound exactly right:

> "Oh, geography! Grab your passports, people, we are going places. Unlike my career. Anyway! Let's GO!"

> "I can see some of you tapping before I've finished reading. Bold. Wrong, probably, but bold."

> "It's Jupiter! Huge, gassy, and spinning out of control. Just like my last contract negotiation."

## Spelling And Usage

American spelling. Contractions always. "Okay," not "OK" (TTS reads "OK" as letters on some voices).

## Capitalisation

At most one ALL-CAPS stress word per line ("Let's GO!"). Show titles are plain in host lines: Brain Blast, Bix After Dark.

## Numbers, Dates, And Time

Placeholders like `{points}` and `{streak}` are filled with numerals by the server; write around them so they read naturally ("{streak} in a row", not "{streak}-question-long streak"). Write other numbers as words: "twenty-two million," "nineteen ninety-eight."

## Dialogue And Punctuation

- No stage directions, asterisks, emoji, or parentheses. TTS reads them or chokes on them.
- Ellipses only for a real spoken pause, at most one per line.
- Lines are three to forty words. Most land under twenty-five.
- The laugh word goes last. Cut anything after the punchline.
- Placeholders: only the ones the event fills in (see `EVENTS` in `server/src/host.rs`). `{host}` works everywhere.
- A cameo line starts with its tag, `[as:noir]`, and stays fully in that character's voice.

## Character Voices

- **Bix:** see `characters/bix.md`. Reaches for "people," "folks," "anyway," "let's go," "oh, I love this," "big one." Never says "literally," "vibes," "lol," "awesome," or "no cap" unless he's mangling slang on purpose and the mangling is the joke.
- **Cameo characters:** see `crates/question-bank/src/cameo.rs`. Each one is a job Bix took on the way down; the backstory is in his file.

## Watch List

- **anyway:** the snap-back after a career crack. Precious. More than one in five lines per event and it stops being a snap.
- **dumbass:** the old script leaned on it at MA and NC-17. Prefer a specific roast of what the player did.
