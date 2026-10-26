# TriviaBix questions

Every question lives in a pack file under `packs/`. A pack is a TOML file (JSON works
too) with some shared settings at the top and a list of questions. Drop a new file in
`packs/` and the game picks it up the next time the server starts.

## Adding a question

```toml
[pack]
name = "Totally Rad 80s"
category = "80s"          # default category for this pack
tags = ["80s", "retro"]   # added to every question in the pack
rating = "PG"             # default rating (see below)

[[question]]
type = "multiple_choice"
prompt = "Which band sang \"Take On Me\"?"
choices = ["a-ha", "Duran Duran", "Tears for Fears", "Eurythmics"]
answer = "a-ha"                       # the text of the right choice
difficulty = "easy"                   # easy, medium (default), or hard
tags = ["music"]                      # extra tags for this question
fun_fact = "The video took months of hand-drawn rotoscoping."
host_quip = "Taaake ON MEEE."         # a cheeky line for the host after the reveal
```

Anything you leave off a question falls back to the `[pack]` settings. You don't need
to write an `id`: one is made from the pack's file name and the prompt. Add `id = "..."`
only if you want to fix a typo in a prompt without it counting as a new question.

Then check your work:

```sh
cargo run -p triviabix-questions --features cli -- validate
cargo run -p triviabix-questions --features cli -- sample --topic 80s
```

`validate` lists every mistake with the question it's in: an answer that isn't one of
the choices, an unknown category, a duplicate question, and so on.

## Question types

| type | fields | how it plays |
| --- | --- | --- |
| `multiple_choice` | `choices` (2 to 6), `answer`, optional `keep_order = true` | Choices are shuffled on phones unless `keep_order` is set. |
| `true_false` | `answer = true` or `false` | Two big buttons. |
| `type_answer` | `answers` (every spelling you'll accept; the first is shown) | Players type. Case, punctuation, accents, a leading "the", and small typos are forgiven. Numbers must be exact. |
| `closest_number` | `answer`, optional `unit`, optional `tolerance` | Closest guess wins. Guesses within `tolerance` count as right. |
| `put_in_order` | `items` (3 to 6, written in the correct order), optional `first_label`, `last_label` | Items are shuffled; partial credit for each one in the right spot. |

## Ratings

Every question has a movie-style rating. When setting up a room, the host picks the
highest rating allowed, and the room gets every question at or below it.

| rating | what goes here |
| --- | --- |
| `PG` | Fine for kids and grandparents. The default. |
| `PG-13` | Mild innuendo, drinking, cheeky jokes. |
| `MA` | Adults only. Frank about sex, drugs, and swearing, but not graphic. |
| `NC-17` | The filthiest stuff, for rooms that opt in. |

A category can set a minimum rating in `categories.toml`. After Dark is at least PG-13,
so nothing in it shows up on a family night even if a question forgets its rating.

## Categories and tags

Categories are listed in `categories.toml` with a name, emoji, and color for the TV.
Each question belongs to one category, and can have any number of tags. A themed round
asks for topics, and matches a question if its category or any of its tags is listed.
So an "80s night" gets the whole 80s category plus the 80s-tagged music, movie, and
video game questions.

Decade tags used so far: `60s`, `70s`, `80s`, `90s`, `2000s`, `2020s`.

## Importing from trivia sites

```sh
# Open Trivia DB (CC BY-SA 4.0). List its category ids first:
cargo run -p triviabix-questions --features cli -- import opentdb --list-categories
cargo run -p triviabix-questions --features cli -- import opentdb --amount 100 --category 12

# The Trivia API (free for non-commercial use). Tags like 1980's work well for themes:
cargo run -p triviabix-questions --features cli -- import trivia-api --amount 50 --tag "1980's"
cargo run -p triviabix-questions --features cli -- import trivia-api --amount 50 --category food_and_drink
```

Imports land in `packs/imported/`, mapped onto our categories and tagged with their
source (`opentdb`, `trivia-api`) and decade where the site gives one. Questions already in
the bank are skipped, and running the same import again adds to the same file. Imported
questions are rated PG, and each keeps its source and license so attribution travels with
it. Skim a fresh import before a party: these sites are crowd-sourced.

## Host scripts

Bix can say something about each question: a **lead-in** right after he announces it, and a **reveal** line after the
answer (a joke, an anecdote, or a fun fact). Lines are rated like questions, so a PG room hears the clean one and an MA
room usually gets the spicy one. A question with no lines of its own gets Bix's generic banter instead.

Lines can go straight into a pack:

```toml
[[question]]
type = "true_false"
prompt = "A group of crows is called a murder."
answer = true
lead_in = "Bird question. It sounds ominous."   # rated like the question
reveal = { PG = "Crows remember faces and hold grudges.", MA = "Crows remember faces. Yours. Forever." }
host_quip = "..."   # host_quip and fun_fact are reveal lines too
```

`lead_in` and `reveal` take one line, a list of lines, or a table of ratings to lines. Keep them under 280 characters,
don't give the answer away in a lead-in, and use `{host}` if Bix should say his own name.

Lines written in bulk live in `scripts/`, one file per pack, matched to questions by id, so packs stay tidy:

```toml
[[script]]
question = "general-1a2b3c4d"
prompt = "How many sides does a hexagon have?"   # for humans; validate warns if the question changed
lead_in = "Shapes time. Count the sides."
reveal = "Bees build honeycomb this way because it packs with no wasted space."
```

To write lines for many questions at once, for example after an import:

```sh
# 1. Export the questions that still need lines, with answers and existing lines for context.
cargo run -p triviabix-questions --features cli -- scripts todo --out todo.json
# 2. Fill in lead_in and reveal in todo.json (by hand, or have an assistant do it).
# 3. File them under content/scripts next to each question's pack.
cargo run -p triviabix-questions --features cli -- scripts add todo.json
```

Or let Claude write them. This uses the paid Anthropic API, so it only runs with an API key from
[console.anthropic.com](https://console.anthropic.com):

```sh
export ANTHROPIC_API_KEY=sk-ant-...
# See what it would send, without calling the API.
cargo run -p triviabix-questions --features cli -- scripts generate --limit 20 --dry-run
# Write lines for up to 50 questions that are missing them, 10 per request.
cargo run -p triviabix-questions --features cli -- scripts generate --topic 80s
```

It only fills slots a question doesn't have yet, writes a few lines per slot (including spicier MA ones for tamer
questions), and saves after every request into the same `scripts/` files `scripts add` uses. Options: `--missing`,
`--topic`, `--limit` (default 50), `--batch` (default 10), `--model` (default `claude-opus-5-5`), and `--effort`
(`low` to `max`, default `medium`). It prints the tokens used at the end; at Opus 5.5 prices ($4 in, $20 out per million
tokens) a batch of 10 questions costs roughly 10 to 20 cents, mostly for Claude's thinking. Read what it wrote before a party: it's good, but it's a comedian, not
an encyclopedia.

`scripts todo` takes `--missing lead-in|reveal|either`, `--topic`, and `--limit`. `stats` shows how many questions have
lines. The server loads packs and scripts into its database at startup (or on **Reload packs** in the admin panel), and
lines edited in the admin panel override the files for that question.

## Using the bank from the server

```rust
use triviabix_questions::{Filter, PlayerAnswer, QuestionBank, Rating};

let bank = QuestionBank::load_dir("content")?;
let filter = Filter { topics: vec!["80s".into()], max_rating: Rating::Pg13, ..Default::default() };
let round = bank.pick(&filter, 10, &mut rand::thread_rng());
let to_phones = round[0].public_view(&mut rand::thread_rng()); // no answer inside
let result = round[0].check(&PlayerAnswer::Choice("a-ha".into()));
```

The game server loads every pack (imports included) and script file into its SQLite
database at startup, matching questions by id, and plays from there. Questions removed from a
pack stop being played. The game plays multiple choice and true/false questions so far; the
other types are stored and wait for phone support. `export --out questions.json` writes the
whole bank as one JSON array.
