# TriviaBix

A Jackbox-style party trivia game: the host screen goes on the TV and players answer on their phones.

## Playing

1. Open `http://<server>:8080/host` on the TV (a smart TV browser, a cast Chrome tab, or a laptop on HDMI).
   Pick how many questions and the rating (PG, PG-13, MA, NC-17), then press **Start a room**.
2. Players open `http://<server>:8080` on their phones (or scan the QR code on the TV), type the 4-letter room code and a name.
   Each player picks a character (24 to choose from, one per player per room) and can change their name or character in the lobby.
   Up to 12 players per room by default (`MAX_PLAYERS`).
3. The first player to join is the VIP and can start the game from their phone. The TV can also start it with **Enter**.

After the game starts the TV needs no mouse: questions, reveals and scoreboards advance on timers, and the game moves on
as soon as everyone has answered. On the TV, **Enter**/**Space** skips ahead, **M** mutes the host voice, **V** tries the next
voice and **F** toggles fullscreen.

## The host

Bix, the host, runs the show from a script: an intro, banter before each question, the answer and a reaction (lone geniuses,
clean sweeps, roasts of wrong answers, streaks of three or more and broken streaks), score updates with new leaders and last
place, and a finale. The TV shows each line in a speech bubble, reads it out, and pops matching graphics: a title card before
each question, stamps and banners for roasts and streaks, scoreboard rows sliding to their new rank, and confetti for the winner.
Players who get called out see the badge on their phone too.

The script is [`server/host-script.json`](server/host-script.json). Lines are grouped by rating; a room uses every line at or
below its rating and leans heavily toward the spiciest tier allowed, so PG rooms stay friendly and NC-17 rooms get filthy. Edit
it and rebuild, or point `HOST_SCRIPT` at your own copy. The server checks the script on startup (known events, a PG line for
each, and only placeholders that event fills in). Reveals and scoreboards stretch so the host can finish talking.

The voice is the browser's built-in text-to-speech, picking the best free voice it finds (Edge's "Natural" voices and Chrome's
Google voices sound much better than the defaults). Press **V** on the TV to cycle voices; the choice is remembered.

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
| `QUESTION_SECS` / `REVEAL_SECS` / `SCOREBOARD_SECS` | `20` / `7` / `6` | Minimums; the host's lines can stretch them |
| `INTRO_SECS` | `6` | The host's opening, before question 1 |
| `HOST_TALK_CPS` | `13` | Voice speed estimate in characters per second, for pacing; `0` stops waiting for the host |
| `HOST_SCRIPT` | | Path to a host script to use instead of the built-in one |
| `ROOM_IDLE_SECS` | `900` | Close rooms with nobody connected |

## Layout

- `server/` is the Rust game server (axum + WebSockets + SQLite via sqlx). Each room is its own task that owns the game state.
  - `src/room.rs` holds the game rules, `src/host.rs` the host, `src/protocol.rs` the JSON messages, `src/db.rs` the database.
  - `host-script.json` is everything the host says.
  - `migrations/` creates the tables and seeds a starter question bank.
- `web/` is the TV page (`host.html`) and the phone page (`index.html`), plain HTML/JS with no build step.

### WebSocket protocol

The TV connects to `/ws/host?code=ABCD&token=...` (token from `POST /api/rooms`), phones to `/ws/play?code=ABCD&name=Sam`
(or `&token=...` to rejoin). The server sends `welcome`, then a full `state` snapshot on every change, and `error` messages.
Phones can pass `&character=fox` when joining (see `GET /api/characters`). Clients send `start`, `answer {choice}`, `next`, `play_again`, `profile {name, character}` (lobby only), and from the TV `settings {rounds, max_rating}` and `kick {player_id}`.
The correct answer is never sent until the question closes. Phases are `lobby`, `intro`, `question`, `reveal`, `scoreboard`
and `final`; each state carries `host {name, id, text, callouts}`, the current host line and the graphics that go with it.
