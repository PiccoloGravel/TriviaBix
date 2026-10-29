# Playing away from home

TriviaBix runs on the Unraid server at home. To play somewhere else, like at your parents' house,
the TV there and everyone's phones need an address that reaches the server over the internet. This
guide gives the server a public **https** address without opening any ports on your router, so
guests just scan the QR code as usual and don't install anything.

There are two ways to do it. Both are free to run, both carry the game's WebSockets, and the game
works the same with either.

| | Tailscale Funnel (recommended) | Cloudflare Tunnel |
|---|---|---|
| Cost | Free | Free, plus a domain (about $10 a year) |
| Address | `https://triviabix.tail1234.ts.net` (long, but nobody types it) | `https://trivia.yourname.com` (short) |
| Setup | About 10 minutes | About 20 minutes |
| Extra lock on `/admin` | No (the admin password does it) | Optional: Cloudflare Access email codes |
| Art and audio | Streamed from home | Cached by Cloudflare near your parents |

Pick Funnel unless you already have a domain or want the short address. You can switch later; the
game doesn't care which one is in front of it.

## Before you start (both ways)

1. **Admin password.** Anyone on the internet can now reach `/admin`, so make the **Admin password**
   long: 12 characters or more, e.g. three or four random words. After a few wrong passwords in a row
   the server pauses all logins, from 2 seconds up to a minute, so guessing isn't practical. The
   container log shows each failed login and where it came from.
2. **Keep Public URL as your LAN address** (`http://<unraid IP>:8080`). At home the TV's QR code sends
   phones straight to the server over Wi-Fi. When a TV opens the game on its https address instead,
   its QR code shows that address on its own, so nothing needs changing when you leave the house.
3. **Upload speed.** At your parents' place everything streams from your home connection's upload:
   pages, Bix's voice lines, cutscene art and music library clips. 10 Mbps or more of upload is plenty.
   The TV loads the art in the background while people join, so give it a minute in the lobby the
   first time.

## Option A: Tailscale Funnel

You need a free Tailscale account (<https://login.tailscale.com>, sign in with Google, GitHub or
Microsoft).

### A1. Turn on Funnel for your tailnet (once)

In the Tailscale admin console:

1. **DNS** page: make sure **MagicDNS** is on, then under **HTTPS Certificates** click **Enable HTTPS**.
2. **Access controls** page: new tailnets already allow Funnel. If yours is older, the first time the
   container starts its log prints a link to turn Funnel on; open it and click **Enable**.

### A2. Put TriviaBix on Funnel

Pick the one that matches how you installed TriviaBix.

**Docker tab template, Unraid 7.0 or newer** (no extra container):

1. **Docker** tab → **TriviaBix** → **Edit**.
2. Turn on **Use Tailscale**. Set **Tailscale Hostname** to `triviabix`.
3. Set **Tailscale Serve** to **Funnel** and **Tailscale Serve Port** to `8080`.
4. **Apply**. Open the container's **Logs**: the first time it prints a Tailscale login link. Open
   it and approve the device.
5. On the Tailscale admin **Machines** page you now have `triviabix`. Your address is
   `https://triviabix.<your tailnet>.ts.net` (the **DNS** page shows the tailnet name).

If your Unraid is older than 7.0, or the log shows Tailscale errors, use the compose way below.

**Docker Compose Manager stack:**

1. Tailscale admin → **Settings** → **Keys** → **Generate auth key**. Leave it single use and not
   ephemeral. Copy it (`tskey-auth-…`).
2. In the Unraid terminal, put the Funnel settings where the stack expects them:
   ```sh
   mkdir -p /mnt/user/appdata/triviabix-tailscale
   wget -O /mnt/user/appdata/triviabix-tailscale/serve.json \
     https://raw.githubusercontent.com/PiccoloGravel/TriviaBix/main/deploy/unraid/serve.json
   ```
   (If the repo is private, copy `deploy/unraid/serve.json` there instead.)
3. Update the stack's compose file from [`compose.yaml`](compose.yaml) (it now has a `funnel`
   service), and in its `.env` set `TS_AUTHKEY=` to the key.
4. Start the stack with the `funnel` profile (in Compose Manager, the stack's **Edit Stack** →
   **UI Labels/Profiles**, or `docker compose --profile funnel up -d` in the stack folder).
5. The `triviabix-funnel` container's log prints the address once it's up:
   `https://triviabix.<your tailnet>.ts.net`. The key is used once; the login is kept in appdata.

### A3. Check it

On your phone with Wi-Fi **off**, open `https://triviabix.<your tailnet>.ts.net`. You should see the
join screen. Then open `/tv` on a laptop the same way: the QR code should show the ts.net address.

## Option B: Cloudflare Tunnel

You need a domain whose DNS is on Cloudflare. If you don't have one, buy one at
<https://dash.cloudflare.com> → **Domain Registration** (it's set up for you), or move an existing
domain's nameservers to Cloudflare.

1. Cloudflare dashboard → **Zero Trust** → **Networks** → **Tunnels** → **Create a tunnel** →
   **Cloudflared**. Name it `triviabix`.
2. It shows an install command with a long token after `--token`. Copy just the token.
3. **Public hostname**: subdomain `trivia`, your domain, **Service** `HTTP` and URL:
   - `triviabix:8080` with the Compose Manager stack (the tunnel runs next to the game), or
   - `<unraid IP>:8080` with the Cloudflared app from Community Applications.
   WebSockets are on by default; leave them on.
4. Run the tunnel, either way:
   - **Compose Manager stack:** set `CLOUDFLARE_TUNNEL_TOKEN=` in the stack's `.env` and start it
     with the `tunnel` profile.
   - **Docker tab:** install **Cloudflared** from Community Applications and paste the token.
5. Your address is `https://trivia.yourdomain.com`.

**Optional second lock on the admin panel.** Zero Trust → **Access** → **Applications** → **Add an
application** → **Self-hosted**. Domain `trivia.yourdomain.com`, path `admin`, then add a second one
with path `api/admin`. Policy: **Allow**, include **Emails** → your email. Now `/admin` asks for a
code sent to your email before the game's own password. Players, the TV and the QR code aren't
affected. GPU jobs talk to the server from inside the container, so they're not affected either.

## At your parents' house

**The TV.** Pick whichever is easiest there:

- **Samsung TV browser:** open **Internet**, type `https://<your address>/tv` once, and add it to
  bookmarks so next time it's one click. Click the screen once (OK on the remote) to turn on sound.
- **Cast app on your S22:** open the TriviaBix app, tap **⚙**, enter `https://<your address>` and
  **Save**. Then turn on **Smart View** and pick the TV: it gets `/tv` with sound.
  The public address works at home too, so you can leave the app set to it.
- **Fire stick** (if you bring it): Fully Kiosk Browser with the start URL
  `https://<your address>/tv`.

**Phones.** Everyone scans the QR code on the TV, on Wi-Fi or mobile data. Over https their screens
stay on during the game, and a phone that locks or drops signal for a moment reconnects as soon as
it's back.

**Settings.** Open `https://<your address>/admin` on your phone to set the rating, length and teams
for the room, the same as at home.

## Troubleshooting

- **"Can't reach this page" away from home:** check the tunnel container is running and its log has
  no errors. With Funnel, the address only works after HTTPS certificates are enabled (A1). The very
  first visit can take 10 to 20 seconds while the certificate is made.
- **The QR code shows your home LAN address:** the TV opened the game on the LAN address. Open
  `https://<your address>/tv` on it instead.
- **The TV says it lost connection now and then:** it reconnects by itself. If it keeps happening,
  the internet at either end is dropping out; Wi-Fi close to the router helps.
- **"Too many wrong passwords" on the admin login:** someone (maybe you) typed it wrong several
  times. Wait the seconds it says; the pause resets after 15 quiet minutes.
- **Slow first load of art:** the TV fetches the cutscene art in the background while people join.
  With Cloudflare, the second game is faster because the art is cached near you.
