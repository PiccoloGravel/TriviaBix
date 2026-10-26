#!/bin/sh
# Builds the TriviaBix cast app into web/downloads/triviabix-cast.apk, which the server offers on
# the admin Settings page. Needs only Docker: the Android SDK comes from a pinned image, and the
# build uses the SDK tools directly (no Gradle), so the same sources give the same app.
# Usage: android/cast/build.sh     (bump VERSION first so phones take it as an update)
set -eu
cd "$(dirname "$0")"
IMAGE="cimg/android:2025.04@sha256:7a5db59d820a1fdc72fe12a315422223b4d003a80fd588850eb614f04529af2e"
mkdir -p ../../web/downloads
docker run --rm -u "$(id -u):$(id -g)" -e HOME=/tmp -v "$PWD:/app" -v "$PWD/../../web/downloads:/out" -w /app "$IMAGE" sh -eu -c '
  BT=$ANDROID_HOME/build-tools/35.0.1
  JAR=$ANDROID_HOME/platforms/android-35/android.jar
  read CODE NAME < VERSION
  rm -rf /tmp/b && mkdir -p /tmp/b/res /tmp/b/classes /tmp/b/dex
  $BT/aapt2 compile --dir res -o /tmp/b/res.zip
  $BT/aapt2 link -o /tmp/b/app.apk -I $JAR --manifest AndroidManifest.xml /tmp/b/res.zip \
    --min-sdk-version 26 --target-sdk-version 35 --version-code "$CODE" --version-name "$NAME"
  javac -source 11 -target 11 -nowarn -cp $JAR -d /tmp/b/classes $(find src -name "*.java")
  $BT/d8 --release --min-api 26 --lib $JAR --output /tmp/b/dex $(find /tmp/b/classes -name "*.class")
  (cd /tmp/b/dex && TZ=UTC touch -t 198001010000 classes.dex && TZ=UTC zip -q -X /tmp/b/app.apk classes.dex)
  $BT/zipalign -f -p 4 /tmp/b/app.apk /tmp/b/aligned.apk
  # The key is in the repo on purpose: anyone rebuilding signs the same way, so phones update in place.
  $BT/apksigner sign --ks triviabix-cast.keystore --ks-pass pass:triviabix --out /out/triviabix-cast.apk /tmp/b/aligned.apk
  $BT/apksigner verify /out/triviabix-cast.apk
  rm -f /out/triviabix-cast.apk.idsig
  echo "built web/downloads/triviabix-cast.apk, version $NAME ($CODE)"
'
