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

## Using the bank from the server

```rust
use triviabix_questions::{Filter, PlayerAnswer, QuestionBank, Rating};

let bank = QuestionBank::load_dir("content")?;
let filter = Filter { topics: vec!["80s".into()], max_rating: Rating::Pg13, ..Default::default() };
let round = bank.pick(&filter, 10, &mut rand::thread_rng());
let to_phones = round[0].public_view(&mut rand::thread_rng()); // no answer inside
let result = round[0].check(&PlayerAnswer::Choice("a-ha".into()));
```

`export --out questions.json` writes the whole bank as one JSON array if you'd rather
seed a database.
