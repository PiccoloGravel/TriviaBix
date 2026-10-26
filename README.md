# TriviaBix

A Jackbox-style party trivia game: the host screen goes on the TV and players answer on their phones.

## Playing

Everything is played from phones. The TV is display only.

1. Open `http://<server>:8080/host` on the TV (a smart TV browser, a cast Chrome tab, or a laptop on HDMI).
   It opens a room straight away and shows the room code and a QR code. Browsers keep a page silent until it has been
   clicked once, so click the TV page once if you want the host's voice and Name That Tune clips.
2. Players open `http://<server>:8080` on their phones (or scan the QR code on the TV), type the 4-letter room code and a name.
   Each player picks a character (24 to choose from, one per player per room) and can change their name or character in the lobby.
   Up to 12 players per room by default (`MAX_PLAYERS`); the admin panel can change it per room.
3. The first player to join is the VIP. On their phone the VIP picks the game (Trivia, Speed Hangman or Name That Tune)
   and starts it. The TV shows the choice as it changes.
4. During the game the VIP's phone has **Next** to skip ahead and **End game** to go back to the lobby. After the final
   scores the VIP taps **Pick the next game** to return to the lobby with everyone still in. The VIP can also remove a
   player from the lobby.
5. Only the admin changes a room's settings: rating (PG, PG-13, MA, NC-17), length, teams, clip length, late joins and
   the maximum number of players (up to 24, one per character).
   Rooms start at PG. Sign in to the [admin panel](#admin-panel) at `/admin`, on a phone if you like. The lobby on every
   phone links to it, and that room is listed first.

Questions, reveals and scoreboards advance on timers, and the game moves on as soon as everyone has answered. A keyboard
on the TV is optional: **Enter**/**Space** skips ahead, **M** mutes the host voice, **V** tries the next voice and **F**
toggles fullscreen.

## The host

Bix, the host, runs the show from a script: an intro, banter before each question, the answer and a reaction (lone geniuses,
clean sweeps, roasts of wrong answers, streaks of three or more and broken streaks), score updates with new leaders and last
place, and a finale. The TV shows each line in a speech bubble, reads it out, and pops matching graphics: stamps and banners for
roasts and streaks, scoreboard rows sliding to their new rank, and confetti for the winner. Players who get called out see the
badge on their phone too.

Bix is an animated cartoon microphone ([`web/bix.js`](web/bix.js)) whose face follows the moment (excited, smug, shocked, sad,
laughing, or in shades for the champion) and whose mouth moves while the voice talks. Big moments get Jackbox-style
**cutscenes**: curtains for the opening, a card for each question, a fire storm for streaks, a snail for last place, a crown
for the champion. Each line from the server carries a `cue`, and the scenes are JSON files in
[`web/cutscenes/`](web/cutscenes/), so adding one means dropping in a file and a line in `index.json`. The guide is
[`web/cutscenes/README.md`](web/cutscenes/README.md), and `/cutscenes.html` previews every scene.

The script is [`server/host-script.json`](server/host-script.json). Lines are grouped by rating; a room uses every line at or
below its rating and leans heavily toward the spiciest tier allowed, so PG rooms stay friendly and NC-17 rooms get filthy. Edit
it and rebuild, or point `HOST_SCRIPT` at your own copy. The server checks the script on startup (known events, a PG line for
each, and only placeholders that event fills in). Reveals and scoreboards stretch so the host can finish talking.

Every question can also have its own lines: a **lead-in** Bix says right after announcing the question, and a **reveal**
line (a joke, anecdote, or fun fact) right after the answer, before his usual reactions and roasts. They're rated too, so a PG room
hears the clean version and an MA room gets the spicy one. A question without its own lines falls back to the generic
script. Every question that ships has both; see [content/README.md](content/README.md#host-scripts) for writing more, or edit
them in the admin panel.

The voice is the browser's built-in text-to-speech, picking the best free voice it finds (Edge's "Natural" voices and Chrome's
Google voices sound much better than the defaults). Press **V** on the TV to cycle voices; the choice is remembered.
With an NVIDIA GPU, the optional voice service in [`tts/`](tts/README.md) gives Bix a real voice with Ming-3.0-tts: lines are
made once, cached, and mostly made ahead of time, and the browser voice fills in for anything that isn't ready.

### Trivia rounds

A trivia game is 3 or 4 rounds (3, 5 or 7 questions each, set in the admin panel) that climb in difficulty:

| Round | Questions | Points |
|---|---|---|
| 1 | Easy | ×1 |
| 2 | Medium | ×1.5 |
| 3 | Hard | ×2 |
| Final (4-round games) | Hardest, from every category | ×3 |

Each of the first rounds is themed on one category when the question bank has enough of it at that difficulty (it tops up
with the nearest difficulty, and falls back to "Anything Goes"). Every round opens with a title card showing its theme,
difficulty and points, plus a staircase of the whole game where later themes stay hidden. After each round the TV shows a
recap: the scoreboard with what everyone won that round, and every question with its answer and who got it right. Phones
get their own round summary. Each question in the database has a `difficulty` of `easy`, `medium` or `hard` (default
`medium`).

### Speed Hangman

The VIP picks **Speed Hangman** instead of Trivia. Each round puts one phrase on the TV with its category, and
everyone races to crack it from their phone at the same time:

- Tap a letter to guess it (one guess per second each). Every tile it reveals is worth 75 points.
- A wrong letter costs you 50 points and adds a strike for the **whole room**. Six strikes and he's hanged and nobody solves it.
- Know the answer? Type it in. Solving pays 250, plus 50 for every letter still blank, plus up to 250 for time left.
  A wrong solve is a strike, costs 50, and locks you out of solving for the rest of that puzzle.
- Rounds are 45 seconds (`HANGMAN_SECS`). Phrases follow the room's rating, from PG up to NC-17.

### Name That Tune

The VIP picks **🎵 Name That Tune**, and the admin panel sets a 1, 2 or 3 second clip. Each round the TV plays a slice of a
song (once at the start and once more halfway through), and players pick the title and artist from four choices on their
phones. Wrong answers are songs from around the same year. At the reveal the TV shows the cover art and plays the song.
**R** replays the clip. If the TV browser blocked sound (for example after reopening the page), the first press of
**Enter** turns sound on instead of skipping.

The songs are a list of titles and artists in `server/migrations/0004_songs.sql`, rated like the questions. The audio is
Apple's 30-second preview from the [iTunes Search API](https://performance-partners.apple.com/search-api), streamed by the
TV straight from Apple; nothing is downloaded or stored. Apple allows previews to be streamed this way to promote the music,
provided they're played as-is and link back to the store, so the reveal credits Apple Music and phones get a link to the
full song. That suits a home game; check Apple's terms again before running TriviaBix as a public or paid service.
The server needs internet access for this mode, and it looks every song up once in the background at startup
(about 4 minutes, to stay under Apple's limit of roughly 20 requests a minute).

Phones that lock or refresh rejoin automatically as the same player. A room closes after 15 minutes with nobody connected.

**Running several games.** Every room is its own game, so several TVs can each run one at the same time.

**Joining late.** Players can join a game that has already started. They wait on a "You're in!" screen and play from the
next question with a score of 0. Turn this off per room in the admin panel.

**Teams.** Set 2 to 4 teams in the admin panel. Players are dealt onto the smallest team and can
switch from their phone in the lobby. A team's score is the average of its players' scores, so a team of 3 can beat a team of 5.

## Admin panel

Set `ADMIN_PASSWORD` and open `http://<server>:8080/admin`. After logging in you see every live room with its players,
scores, phase, and the current question and answer. From there you can:

- create rooms and open a room's TV page (handy for casting a room to a particular TV),
- start, skip, end a game, or send a room back to the lobby,
- kick players, move them between teams, and change a room's settings,
- turn late joins on or off, and close a room.

**Bix's lines** (`/admin/questions`) lists every question in the database with what Bix says about it. Search, filter to
questions missing a lead-in or reveal, and edit, add, or mute lines per rating. Edits are saved in the database, override
the pack's lines for that question, and survive restarts; "Use pack lines" undoes them. **Reload packs** loads the content
folder again without a restart, e.g. after an import.

Without `ADMIN_PASSWORD` the panel stays locked. The login lasts 12 hours. Serve the panel over HTTPS if anyone outside
your home network can reach the server.

## Running it

With Docker:

```sh
PUBLIC_URL=http://192.168.1.20:8080 docker compose up --build
```

`PUBLIC_URL` is the address phones should use for the join link and QR code. Leave it out if you open the TV page on that
same address anyway (not `localhost`).

On Unraid, use the published image `ghcr.io/piccologravel/triviabix`: see [deploy/unraid](deploy/unraid/README.md).

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
| `MAX_PLAYERS` | `12` | Default room size; the admin panel can set each room from 1 to 24 |
| `MAX_ROOMS` | `500` | |
| `QUESTION_SECS` / `REVEAL_SECS` / `SCOREBOARD_SECS` | `20` / `7` / `6` | Minimums; the host's lines can stretch them |
| `INTRO_SECS` | `6` | The host's opening, before question 1 |
| `ROUND_INTRO_SECS` / `RECAP_SECS` | `5` / `14` | Trivia round title card, and the scores and recap after each round |
| `HOST_TALK_CPS` | `13` | Voice speed estimate in characters per second, for pacing; `0` stops waiting for the host |
| `HOST_SCRIPT` | | Path to a host script to use instead of the built-in one |
| `CONTENT_DIR` | `content` | Question packs and host scripts, loaded into the database at startup |
| `HANGMAN_SECS` | `45` | Time to crack one Speed Hangman phrase |
| `SONG_SECS` | `15` | Answer time in Name That Tune |
| `MUSIC_COUNTRY` | `US` | Store country for song previews |
| `MUSIC_WARMUP` | `true` | Look up every song at startup |
| `ITUNES_API_URL` | `https://itunes.apple.com` | |
| `ROOM_IDLE_SECS` | `900` | Close rooms with nobody connected |
| `PUID` / `PGID` | `10001` | Docker only: who owns the files in `/data` (Unraid: `99` / `100`) |
| `ADMIN_PASSWORD` | | Turns on the admin panel at `/admin` |
| `TTS_URL` | | Bix's voice service, e.g. `http://tts:8000` (see [tts/README.md](tts/README.md)); empty means browser text-to-speech |
| `TTS_CACHE_DIR` | `data/tts` | Where his lines are cached (`/data/tts` in Docker) |

## Layout

- `server/` is the Rust game server (axum + WebSockets + SQLite via sqlx). Each room is its own task that owns the game state.
  - `src/room.rs` holds the game rules, `src/host.rs` the host, `src/protocol.rs` the JSON messages, `src/db.rs` the database,
    `src/music.rs` the song lookups for Name That Tune, `src/voice.rs` the cache in front of the voice service.
  - `host-script.json` is everything the host says.
  - `src/hangman.rs` holds the Speed Hangman puzzle rules.
  - `src/rounds.rs` plans a trivia game's rounds: difficulty, theme and points.
  - `migrations/` creates the tables and seeds hangman phrases and songs.
  - `src/content.rs` loads the question packs into the database at startup.
- `content/` is the question bank: packs (hand-written and imported) and per-question host scripts. See
  [content/README.md](content/README.md). `crates/question-bank` reads it and has the command line tool.
- `tts/` is the optional voice service (Python, Ming-3.0-tts on a GPU).
- `web/` is the TV page (`host.html`), the phone page (`index.html`) and the admin panel (`admin.html`), plain HTML/JS with no build step.
  - `cutscenes.js` plays the scenes in `cutscenes/`; `bix.js` draws the host; `cutscenes.html` previews them.
- `server/src/admin.rs` is the admin login and API (`/api/admin/*`).

### WebSocket protocol

The TV connects to `/ws/host?code=ABCD&token=...` (token from `POST /api/rooms`), phones to `/ws/play?code=ABCD&name=Sam`
(or `&token=...` to rejoin). The server sends `welcome`, then a full `state` snapshot on every change, and `error` messages.
Phones can pass `&character=fox` when joining (see `GET /api/characters`). Clients send `start`, `answer {choice}`, `next`,
`play_again`, `profile {name, character}` and `team {team}` (phones, lobby only), and from the TV
`settings {rounds, max_rating, mode, clip_ms, teams, late_join, trivia_rounds, per_round}` and `kick {player_id}`.
In Speed Hangman, phones send `guess {letter}` and `solve {text}`, and state carries a `hangman` object instead of `question`.
In Name That Tune only the TV gets the audio (`question.clip` and `next_clip`); phones get the choices and, at the reveal, `reveal.song`.
The correct answer (or the hangman phrase) is never sent until the round closes. Phases are `lobby`, `intro`, `round_intro`, `question`, `reveal`,
`scoreboard`, `recap` and `final`. Trivia goes `round_intro`, then question and reveal for each question in the round, then
`recap`; state carries `stages` (the round ladder), `stage` and `stage_question`, and `recap` between rounds. Other modes
use `scoreboard` after each reveal; each state carries `host {name, id, text, callouts, cue}`, the current host line, the graphics that go with it, and the cue that picks its cutscene.
