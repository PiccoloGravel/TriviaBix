# Running TriviaBix on Unraid

TriviaBix runs as one container. Unraid pulls a ready-made image from GitHub Container Registry
(`ghcr.io/piccologravel/triviabix`), which GitHub Actions builds every time `main` changes.
The TV page is then opened from any laptop or phone and cast to the TV.

## 1. Let Unraid pull the image (once)

The first push to `main` after this is merged runs **Actions → Publish image** and creates the
package. GitHub makes new packages private, so either:

- **Make it public (simplest):** on GitHub open your profile → **Packages** → **triviabix** →
  **Package settings** → **Change visibility** → **Public**. Nothing secret is in the image.
- **Or keep it private:** create a classic personal access token with only `read:packages`, then in
  the Unraid terminal run `docker login ghcr.io -u PiccoloGravel` and paste the token as the password.

## 2. Install the container

Pick one of these.

### Option A: Docker tab template (no plugins)

1. In the Unraid terminal, fetch the template:
   ```sh
   wget -O /boot/config/plugins/dockerMan/templates-user/my-TriviaBix.xml \
     https://raw.githubusercontent.com/PiccoloGravel/TriviaBix/main/deploy/unraid/triviabix.xml
   ```
   (If the repo is private, copy `deploy/unraid/triviabix.xml` to that path instead.)
2. **Docker** tab → **Add Container** → **Template** → pick **TriviaBix**.
3. Fill in:
   - **Public URL**: `http://<your Unraid IP>:8080`, e.g. `http://192.168.1.20:8080`.
     Find the IP on the Unraid **Dashboard**. Give the server a fixed IP (DHCP reservation in your
     router) so the QR code keeps working.
   - **Admin password**: something long. Leave empty to keep `/admin` switched off.
   - **Web port**: keep `8080` unless something else uses it (then change Public URL to match).
   - **Appdata**: keep `/mnt/user/appdata/triviabix`.
4. **Apply**.

### Option B: Docker Compose Manager plugin

1. Install **Docker Compose Manager** from Community Applications.
2. **Docker** tab → **Add New Stack**, name it `triviabix`.
3. Edit the stack's compose file and paste [`compose.yaml`](compose.yaml).
4. Edit the stack's env file and paste [`.env.example`](.env.example), filling in `PUBLIC_URL` and
   `ADMIN_PASSWORD`.
5. **Compose Up**.

## 3. Play

| Page | Address | Open it on |
|---|---|---|
| TV display | `http://192.168.1.20:8080/host` | A laptop on HDMI, or a laptop/phone casting a Chrome tab to the TV |
| Players | `http://192.168.1.20:8080` (or scan the QR on the TV) | Phones |
| Admin panel | `http://192.168.1.20:8080/admin` | Any laptop or phone |

The **WebUI** entry in the container's menu on the Docker tab opens the TV page.

## Your data

Everything TriviaBix keeps lives in `/mnt/user/appdata/triviabix` (the container's `/data`), never inside
the image, so updating or recreating the container loses nothing. Back it up with the **Appdata Backup**
plugin like any other container.

| What | Where in appdata | Kept as |
|---|---|---|
| Players, scores, game history, admin edits to Bix's lines, the music library from Lidarr, rendering jobs and their costs | `triviabix.db` | SQLite database |
| Question packs | `content/packs/` | TOML files you can edit |
| Host lines Claude wrote (from the Pipeline page or `triviabix-questions scripts generate`) | `content/scripts/generated/` | TOML files |
| Bix's voice lines | `assets/voice/` | audio files plus a `.json` each, in the asset store ([docs/assets.md](../../docs/assets.md)) |
| Generated art originals (pictures, cut-outs, clips) | `assets/image/`, `assets/clip/` | files plus a `.json` each with prompt, seed, model, tags and rating |
| The TV's copies of the art | `art/` | small WebP/MP4 files and `manifest.json` |

Files are used for anything big (audio, art) or anything you might edit by hand (packs); the database holds
everything else. Generated content is looked up in the asset store before anything is made, so nothing is
paid for twice.

The image only holds the program and the built-in content it starts from. The built-in `starter` folders
under `content/packs` and `content/scripts` are refreshed from the image on every start, so don't edit
them; put your own packs in `content/packs/mine/` (any folder name other than `starter` works) and restart
the container, or press reload in the admin panel. See `content/README.md` for the pack format.

Not kept across restarts, on purpose: live rooms, admin logins, and a voice server the Pipeline page
pointed at a rented GPU (it goes back to `TTS_URL`). API keys and addresses are container settings.

## Updating

GitHub builds a new image on every merge to `main`. On the Docker tab, **Check for Updates**, then
**Apply Update** (or **Update Stack** with Compose Manager). The CA Auto Update plugin can do this
nightly.

## Playing away from home (optional)

The game only needs your LAN. To let people join from elsewhere:

- **Tailscale** (easiest, built into Unraid 7): players on your tailnet open
  `http://<unraid tailscale name>:8080`. Set Public URL to that address for the QR code.
- **Reverse proxy** (Nginx Proxy Manager, SWAG, Cloudflare Tunnel): point a hostname at
  `http://<unraid IP>:8080` and **turn on WebSocket support** (the game runs over `/ws/...`).
  Set Public URL to the public `https://` address. Use a strong admin password, or block `/admin`
  at the proxy, since anyone who finds the address can reach it.

## Your music library (Lidarr)

Name That Tune can play songs from your own library instead of (or as well as) Apple's previews,
and the TV plays your music quietly in the lobby and between rounds. TriviaBix asks Lidarr which
tracks have files, then streams them from your music share, which it only reads.

1. In Lidarr: **Settings → General → Security**, copy the **API Key**. Note the root folder under
   **Settings → Media Management → Root Folders** (e.g. `/music` or `/data/media/music`).
2. In the TriviaBix template (or the stack's `.env`):
   - **Music library**: the Unraid share holding your music, the same one mapped into Lidarr,
     e.g. `/mnt/user/data/media/music`. It's mounted read-only at `/music`.
   - **Lidarr URL**: `http://<your Unraid IP>:8686`.
   - **Lidarr API key**: the key from step 1.
   - **Lidarr music path** (advanced): only needed if Lidarr has more than one root folder, or the
     share you mapped above is a parent of it. Put the folder *as Lidarr sees it* that matches the
     share you mounted, e.g. `/data/media/music`.
   - **Library rating** (advanced): rating for library songs, `PG-13` by default. Tracks marked
     explicit are always at least `MA`, so a PG room skips your library unless you set this to `PG`.
3. **Apply**. The library syncs at startup and every 6 hours (`LIDARR_SYNC_HOURS`). The admin
   panel shows how many songs are ready and has **Sync from Lidarr** for after you add albums.

Per room, the admin panel's **Songs from** picks **Library + hits** (half and half, the default),
**Your library**, or **Hits** (the built-in list, played from your files when you have them and
from Apple otherwise). Live albums, remixes, tracks under a minute and files TV browsers can't play
(e.g. `.wma`, `.ape`) are left out. Wrong answers are songs of the same genre from around the same
year. The **🎵 Music** button (or **B**) on the TV turns the background music and sound effects off.

If the admin panel says files aren't found, the music share isn't mounted at `/music` or Lidarr
sees it under a different path: set **Lidarr music path**.

## Bix's voice

Out of the box, Bix speaks with the browser's built-in text-to-speech on the TV laptop or phone, and the
server needs nothing extra. With an NVIDIA GPU, he can have a real voice from the self-hosted
Ming-3.0-tts model instead (see [tts/README.md](../../tts/README.md) for how it works and the voices).

1. Install the **Nvidia Driver** plugin from Community Applications, reboot, and note the GPU's UUID on its
   settings page (or use `all`). An RTX 30-series or newer is the safe bet; the model is only published as
   tested on data-center cards.
2. Start the voice container:
   - **Compose Manager:** in the stack's `.env`, set `TTS_URL=http://tts:8000`, then start the stack with
     the `voice` profile.
   - **Without compose:** add a container from `ghcr.io/piccologravel/triviabix-tts:latest` with
     Extra Parameters `--runtime=nvidia`, variable `NVIDIA_VISIBLE_DEVICES` set to the GPU UUID (or
     `all`), port 8000, and paths `/models` → `/mnt/user/appdata/triviabix-tts/models` and `/voices` →
     `/mnt/user/appdata/triviabix-tts/voices`. Then set TriviaBix's **Voice server** to
     `http://<unraid-ip>:8000`.
3. The first start downloads the model (about 3 GB). `http://<unraid-ip>:8000/healthz` says
   `"ready": true` when it's loaded. TriviaBix then makes every question and fixed line in the background,
   once, and keeps them in `appdata/triviabix/assets/voice`. Until a line is ready, the browser voice reads it.

No GPU in the Unraid box? Leave `TTS_URL` empty and rent one on vast.ai when there are lines to make:
see [gpu/README.md](../../gpu/README.md). Everything it makes stays in `appdata/triviabix/assets`.

For a custom, cheekier Bix, run `python make_voice.py --count 4` in the voice container's console, listen
to the clips in `appdata/triviabix-tts/voices`, rename the one you like to `bix.wav` (and its `.txt` to
`bix.txt`), and set `BIX_VOICE=clone:/voices/bix.wav`.

## Rendering from the admin panel

Bix's voice lines, the cutscene art and the host's per-question lines can all be made from the website:
open **`/admin` → Pipeline** (it works from a phone). Each job shows how much is missing, then
**Check the price** does a dry run that rents nothing and shows the GPU, its price per hour, the time
limit and the most the job can cost. Nothing starts until you press **Start** and confirm. While it runs
you see the log and the progress, and **Cancel** stops it and gives the GPU back. Past jobs and what they
cost are listed underneath.

1. Add the keys in the template (or the stack's `.env`); they're masked and never shown in the panel:
   - **vast.ai API key** (`VAST_API_KEY`) for voice and art. Make it at
     <https://cloud.vast.ai/manage-keys/>. vast.ai is prepaid: keep $5 to $10 of credit with auto-reload
     off, and that's the most that can ever be spent.
   - **Anthropic API key** (`ANTHROPIC_API_KEY`) for host lines written by Claude, billed to that account.
2. Make the `triviabix-tts` and `triviabix-art` packages on GitHub public (see
   [gpu/README.md](../../gpu/README.md)), or fill in **Private image login**.
3. **Apply**, then open the Pipeline page.

The limits are the scripts' own: at most $0.50/hr for voice and $0.60/hr for art, a voice run stops
after 3 hours at most, an art run after its estimate plus half, and only one GPU at a time. A job
rents at no more than a little over the price you confirmed. Whatever happens (it finishes, fails, is
cancelled, or the container restarts mid-job) the instance is destroyed; **Destroy leftover GPUs**
clears anything started from the command line.

Everything made is kept in appdata: voice lines in `assets/voice`, art originals in `assets/`, the
TV's copies of the art in `art/` (served to the TV straight away), and Claude's lines in
`content/scripts/generated/` (loaded and queued for Bix's voice as soon as the job finishes). Nothing
is made twice.

## Troubleshooting

- **Phones can't connect / QR points at the wrong place:** Public URL must be the Unraid LAN IP,
  not `localhost`, and phones must be on the same Wi-Fi (not a guest network that isolates devices).
- **Permission errors in the log:** the container fixes appdata ownership on start using PUID/PGID
  (99/100 by default on Unraid). Check those weren't changed.
- **"Lidarr has N tracks with files, but none of them are under /music":** the **Music library**
  path points at a different folder than Lidarr's root folder, or **Lidarr music path** is wrong.
  Compare a file's path in Lidarr (an album → the file icon) with what's in your share.
- **Logs:** Docker tab → TriviaBix icon → **Logs**.
