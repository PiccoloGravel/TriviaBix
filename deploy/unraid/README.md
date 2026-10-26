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

Everything lives in `/mnt/user/appdata/triviabix`:

- `triviabix.db` holds rooms, scores, and anything edited in the admin panel. Back up appdata
  with the **Appdata Backup** plugin like any other container.
- `content/` holds the question packs and Bix's host scripts. The built-in `starter` folders are
  refreshed from the image on every start, so don't edit them; put your own packs in
  `content/packs/mine/` (any folder name other than `starter` works) and restart the container,
  or press reload in the admin panel. See `content/README.md` for the pack format.

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

## Bix's voice

Bix currently speaks with the browser's built-in text-to-speech, so the TV laptop/phone does the
talking and the server needs nothing extra. A self-hosted voice model is being added separately.
It runs as a second container (needs an NVIDIA GPU and the Nvidia Driver plugin), and the game
finds it through the **Voice server** setting (`TTS_URL`). The Compose Manager stack has it
ready to uncomment; voice lines are cached in `appdata/triviabix/tts`.

## Troubleshooting

- **Phones can't connect / QR points at the wrong place:** Public URL must be the Unraid LAN IP,
  not `localhost`, and phones must be on the same Wi-Fi (not a guest network that isolates devices).
- **Permission errors in the log:** the container fixes appdata ownership on start using PUID/PGID
  (99/100 by default on Unraid). Check those weren't changed.
- **Logs:** Docker tab → TriviaBix icon → **Logs**.
