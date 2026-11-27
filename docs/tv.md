# Putting the game on the TV

The TV page (`/tv`, the same page as `/host`) runs itself: it opens a room, shows the code and QR, and opens a new room if
that one closes. Phones do everything else. So the TV only needs a web browser at `http://<server>:8080/tv`, opened once
and saved. **📺 Put the game on the TV** on the phone page shows the exact address and these steps.

## The TriviaBix app (Android phones and Fire TV / Google TV sticks)

One APK does both jobs. It finds the server on the Wi-Fi by itself (it asks every address on the local network for
`/healthz` on port 8080), so nobody types an address; **⚙** (phone) or the remote's back button (TV) changes it, and
a Tailscale or other `https` address works too. It is a thin shell around the server's own pages, so game updates
never need a new app. Download it from **/admin → Settings → TriviaBix app**, `/downloads/triviabix-cast.apk`, or
the short address `/app`.

### On a phone: cast to the TV and keep playing

1. On the phone, open **/admin → Settings** (or scan its QR code from a laptop), or tap **📺 Put the game on the TV →
   Get the TriviaBix app** on the phone page. Tap the downloaded APK.
2. Android asks once to allow installs from your browser (Chrome or Samsung Internet): allow it, then install.
   Play Protect may warn that it doesn't know the app; choose **Install anyway**.
3. Open TriviaBix, tap **📺 Cast**, then turn on **Smart View** from the quick panel and pick the TV.
4. The bar at the top says **On the TV** when the TV is showing the game. If the TV shows a copy of the phone
   instead, that TV's Smart View doesn't offer a second screen. Tap **🖥 Be the TV**: the phone turns sideways and
   shows the game screen full size, and everyone else plays on their own phones. Back returns to playing.

### On a Fire TV or Google TV stick: the TV runs itself

1. On the stick, install the free **Downloader** app (Fire TV app store, or Google Play on Google TV).
2. Allow it to install apps: on Fire TV, **Settings → My Fire TV → Developer options → Install unknown apps →
   Downloader**. If there are no Developer options, open **Settings → My Fire TV → About** and press OK on the
   device name 7 times first.
3. In Downloader, type `<server>/app` (for example `trivia.jegan.online/app`, or a LAN address like `10.0.0.68:8090/app`) and install TriviaBix.
4. Open TriviaBix from the apps row. It opens straight into the game screen with sound, keeps the screensaver off,
   and OK on the remote works like Enter (skip a scene, move on). Back or ☰ opens a menu to reload, find the server
   again, type its address, or close the app.

### Building the app

The source is [`android/cast/`](../android/cast/); `android/cast/build.sh` rebuilds `web/downloads/triviabix-cast.apk`
with only Docker (pinned Android SDK image, no Gradle; the same sources give the same file). Its signing key is in the
repo on purpose so any rebuild installs over the last one; bump `android/cast/VERSION` when the app changes. The APK
is part of the server image, like the web pages, so a server update brings the newest app with it.

## Samsung or other smart TV browser

Open the TV's **Internet** app, type `http://<server>:8080/tv`, and bookmark it (Samsung also lets you add it to the
home screen). Press OK once on the page to turn sound on. The page avoids newer browser features so it runs on
Samsung TVs back to about 2019.

## Fire TV, Google TV, Android TV without the app

The TriviaBix app above is the easy way. Without it:

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
