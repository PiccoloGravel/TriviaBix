# Putting the game on the TV

The TV page (`/tv`, the same page as `/host`) runs itself: it opens a room, shows the code and QR, and opens a new room if
that one closes. Phones do everything else. So the TV only needs a web browser at `http://<server>:8080/tv`, opened once
and saved. **📺 Put the game on the TV** on the phone page shows the exact address and these steps.

## Fire TV, Google TV, Android TV (recommended)

1. Install a browser app on the TV: **Silk** on Fire TV, or **Fully Kiosk Browser** (Fire TV and Android TV; the free
   version is enough).
2. Open `http://<server>:8080/tv` and make it the start page or a bookmark.
3. In Fully Kiosk, turn on *Autoplay videos* (Web Content Settings) so Bix talks without the first click, and *Keep
   Screen On* (Device Management) so the TV doesn't go to the screensaver. In Silk, press OK on the page once for sound,
   and set the Fire TV screensaver to *Never* (Settings → Display & Sounds → Screensaver) for long games.

## Smart TV with a browser (LG webOS, Samsung, ...)

Open the TV's browser, type the address, and bookmark it. Press OK once on the page to turn sound on.

## Chromecast, Apple TV, Roku, or a console

These have no browser to point at the page. Use a spare phone or tablet as the TV: open the phone page on it, tap
**📺 Put the game on the TV → Use this device as the TV**, then mirror its screen (Google Home → *Cast my screen* on
Android to a Chromecast, *Screen Mirroring* on an iPhone to an Apple TV). Keep it sideways and plugged in. Everyone else
plays on their own phones.

## Why not a "Cast" button?

Google Cast doesn't cast a web page from a phone. A Cast receiver is a web page that runs on the Chromecast itself, so a
Cast button would need the server on HTTPS with a real certificate, a registered Cast app (a $5 Google Cast developer
account, plus each Chromecast's serial number while the app is unpublished), and it would only work from Android phones,
not iPhones. An Android phone app doesn't avoid any of that. It is possible later, once the server sits behind HTTPS (for
example a reverse proxy on Unraid with a Let's Encrypt certificate), but a browser app on the TV is simpler and works
with every phone.

Browsers also only let a page keep the screen awake over HTTPS or on `localhost`, which is why the screensaver settings
above matter on a plain LAN address.
