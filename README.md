# TriviaBix

A Jackbox-style party trivia game: the host screen goes on the TV and players answer on their phones.

## Playing

1. Open `http://<server>:8080/host` on the TV (a smart TV browser, a cast Chrome tab, or a laptop on HDMI).
   Pick how many questions and the rating (PG, PG-13, MA, NC-17), then press **Start a room**.
2. Players open `http://<server>:8080` on their phones (or scan the QR code on the TV), type the 4-letter room code and a name.
   Up to 12 players per room by default (`MAX_PLAYERS`).
3. The first player to join is the VIP and can start the game from their phone. The TV can also start it with **Enter**.

After the game starts the TV needs no mouse: questions, reveals and scoreboards advance on timers, and the game moves on
as soon as everyone has answered. On the TV, **Enter**/**Space** skips ahead, **M** mutes the host voice and **F** toggles fullscreen.
The host voice uses the browser's built-in text-to-speech.

### Speed Hangman

Pick **Speed Hangman** on the setup screen instead of Trivia. Each round puts one phrase on the TV with its category, and
everyone races to crack it from their phone at the same time:

- Tap a letter to guess it (one guess per second each). Every tile it reveals is worth 75 points.
- A wrong letter costs you 50 points and adds a strike for the **whole room**. Six strikes and he's hanged and nobody solves it.
- Know the answer? Type it in. Solving pays 250, plus 50 for every letter still blank, plus up to 250 for time left.
  A wrong solve is a strike, costs 50, and locks you out of solving for the rest of that puzzle.
- Rounds are 45 seconds (`HANGMAN_SECS`). Phrases follow the room's rating, from PG up to NC-17.

Phones that lock or refresh rejoin automatically as the same player. A room closes after 15 minutes with nobody connected.

## Running it

With Docker:

```sh
PUBLIC_URL=http://192.168.1.20:8080 docker compose up --build
```

`PUBLIC_URL` is the address phones should use for the join link and QR code. Leave it out if you open the TV page on that
same address anyway (not `localhost`).

Without Docker (needs a recent stable Rust):

```sh
cargo run -p triviabix        # serves on http://localhost:8080
cargo test                    # unit tests plus full games over WebSockets
```

### Settings

| Variable | Default | |
|---|---|---|
| `PORT` | `8080` | |
| `DATABASE_URL` | `sqlite://data/triviabix.db` | SQLite file; created and migrated on startup |
| `STATIC_DIR` | `web` | The TV and phone pages |
| `PUBLIC_URL` | | Join address shown on the TV |
| `MAX_PLAYERS` | `12` | Per room |
| `MAX_ROOMS` | `500` | |
| `QUESTION_SECS` / `REVEAL_SECS` / `SCOREBOARD_SECS` | `20` / `7` / `6` | |
| `HANGMAN_SECS` | `45` | Time to crack one Speed Hangman phrase |
| `ROOM_IDLE_SECS` | `900` | Close rooms with nobody connected |

## Layout

- `server/` is the Rust game server (axum + WebSockets + SQLite via sqlx). Each room is its own task that owns the game state.
  - `src/room.rs` holds the game rules, `src/protocol.rs` the JSON messages, `src/db.rs` the database.
  - `src/hangman.rs` holds the Speed Hangman puzzle rules.
  - `migrations/` creates the tables and seeds a starter question bank and hangman phrases.
- `web/` is the TV page (`host.html`) and the phone page (`index.html`), plain HTML/JS with no build step.

### WebSocket protocol

The TV connects to `/ws/host?code=ABCD&token=...` (token from `POST /api/rooms`), phones to `/ws/play?code=ABCD&name=Sam`
(or `&token=...` to rejoin). The server sends `welcome`, then a full `state` snapshot on every change, and `error` messages.
Clients send `start`, `answer {choice}`, `next`, `play_again`, and from the TV `settings {rounds, max_rating, mode}` and `kick {player_id}`.
In Speed Hangman, phones send `guess {letter}` and `solve {text}`, and state carries a `hangman` object instead of `question`.
The correct answer (or the hangman phrase) is never sent until the round closes.
