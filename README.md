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
as soon as everyone has answered. On the TV, **Enter**/**Space** skips ahead, **M** mutes the host voice and **F** toggles fullscreen.
The host voice uses the browser's built-in text-to-speech.

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
| `ROOM_IDLE_SECS` | `900` | Close rooms with nobody connected |

## Layout

- `server/` is the Rust game server (axum + WebSockets + SQLite via sqlx). Each room is its own task that owns the game state.
  - `src/room.rs` holds the game rules, `src/protocol.rs` the JSON messages, `src/db.rs` the database.
  - `migrations/` creates the tables and seeds a starter question bank.
- `web/` is the TV page (`host.html`) and the phone page (`index.html`), plain HTML/JS with no build step.

### WebSocket protocol

The TV connects to `/ws/host?code=ABCD&token=...` (token from `POST /api/rooms`), phones to `/ws/play?code=ABCD&name=Sam`
(or `&token=...` to rejoin). The server sends `welcome`, then a full `state` snapshot on every change, and `error` messages.
Phones can pass `&character=fox` when joining (see `GET /api/characters`). Clients send `start`, `answer {choice}`, `next`, `play_again`, `profile {name, character}` (lobby only), and from the TV `settings {rounds, max_rating}` and `kick {player_id}`.
The correct answer is never sent until the question closes.
