# Putting the game on the TV

The TV page (`/tv`, the same page as `/host`) runs itself: it opens a room, shows the code and QR, and opens a new room if
that one closes. Phones do everything else. So the TV only needs a web browser at `http://<server>:8080/tv`, opened once
and saved. **📺 Put the game on the TV** on the phone page shows the exact address and these steps.

## Cast from an Android phone (the TriviaBix app)

The app puts the game on the TV and keeps the same phone as a player. When the phone is cast with Smart View
(or any screen Android offers as a second display), the TV gets the game screen with sound, not a copy of the
phone. Nobody needs to press anything on the TV.

1. On the phone, open **/admin → Settings → TV cast app** (or scan its QR code from a laptop), or tap
   **📺 Put the game on the TV → Get the TriviaBix app** on the phone page. Tap the downloaded APK.
2. Android asks once to allow installs from your browser (Chrome or Samsung Internet): allow it, then install.
   Play Protect may warn that it doesn't know the app; choose **Install anyway**.
3. Open TriviaBix, check the server address (⚙, it starts at `http://10.0.0.123:8080`), tap **📺 Cast**, then
   turn on **Smart View** from the quick panel and pick the TV.
4. The bar at the top says **On the TV** when the TV is showing the game. If the TV shows a copy of the phone
   instead, that TV's Smart View doesn't offer a second screen; use its browser (below).

The app is a thin shell: two web views pointed at the server, so game updates never need a new app. The source is
[`android/cast/`](../android/cast/); `android/cast/build.sh` rebuilds `web/downloads/triviabix-cast.apk` with only
Docker (pinned Android SDK image, no Gradle; the same sources give the same file). Its signing key is in the repo on
purpose so any rebuild installs over the last one; bump `android/cast/VERSION` when the app changes.

## Samsung or other smart TV browser

Open the TV's **Internet** app, type `http://<server>:8080/tv`, and bookmark it (Samsung also lets you add it to the
home screen). Press OK once on the page to turn sound on. The page avoids newer browser features so it runs on
Samsung TVs back to about 2019.

## Fire TV, Google TV, Android TV

1. Install a browser app on the TV: **Silk** on Fire TV, or **Fully Kiosk Browser** (Fire TV and Android TV; the free
   version is enough).
2. Open `http://<server>:8080/tv` and make it the start page or a bookmark.
3. In Fully Kiosk, turn on *Autoplay videos* (Web Content Settings) so Bix talks without the first click, and *Keep
   Screen On* (Device Management) so the TV doesn't go to the screensaver. In Silk, press OK on the page once for sound,
   and set the Fire TV screensaver to *Never* (Settings → Display & Sounds → Screensaver) for long games.

## Chromecast, Apple TV, Roku, or a console

These have no browser to point at the page. Use a spare phone or tablet as the TV: open the phone page on it, tap
**📺 Put the game on the TV → Use this device as the TV**, then mirror its screen (Google Home → *Cast my screen* on
Android to a Chromecast, *Screen Mirroring* on an iPhone to an Apple TV). Keep it sideways and plugged in. Everyone else
plays on their own phones.

## Why not a Google Cast button?

Google Cast doesn't cast a web page from a phone. A Cast receiver is a web page that runs on the Chromecast itself, so a
Cast button would need the server on HTTPS with a real certificate, a registered Cast app (a $5 Google Cast developer
account, plus each Chromecast's serial number while the app is unpublished), and it would only work from Android phones,
not iPhones. An Android phone app doesn't avoid any of that. It is possible later, once the server sits behind HTTPS (for
example a reverse proxy on Unraid with a Let's Encrypt certificate), but a browser app on the TV is simpler and works
with every phone.

Browsers also only let a page keep the screen awake over HTTPS or on `localhost`, which is why the screensaver settings
above matter on a plain LAN address.
