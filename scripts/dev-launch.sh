#!/usr/bin/env bash
# WarpDrop live-device dev loop (macOS host).
#
#   ./scripts/dev-launch.sh                     start warp-signal on this Mac + launch the macOS app
#   ./scripts/dev-launch.sh --signal wss://…    use an existing signaling/relay server instead
#   ./scripts/dev-launch.sh --deploy-ios --team ABCDE12345   also build, install and launch on a connected iPhone
#   ./scripts/dev-launch.sh --deploy-android    also build, install and launch on connected Android device(s)
#
# Options:
#   --signal URL       signaling server URL (default: start ./target/release/warp-signal on :$PORT)
#   --port N           port for the local warp-signal (default 47900, TCP + UDP)
#   --team ID          Apple development team for device builds (or env DEVELOPMENT_TEAM)
#   --no-app           don't build/launch the macOS app
#   --rebuild          rebuild the macOS app even if a build exists
#   --deploy-ios       build + install + launch on every connected iOS device (needs --team)
#   --deploy-android   build + install + launch on every `adb devices` entry
#   --check            start everything, print the instructions, then shut down (smoke test)
#
# Ctrl+C stops the local warp-signal and the macOS app.
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$(pwd)
export PATH="$HOME/.cargo/bin:$PATH"

PORT=47900 SIGNAL_URL="" TEAM="${DEVELOPMENT_TEAM:-}" APP=1 REBUILD=0 DEPLOY_IOS=0 DEPLOY_ANDROID=0 CHECK=0
while [ $# -gt 0 ]; do
  case "$1" in
    --signal) SIGNAL_URL="$2"; shift 2 ;;
    --port) PORT="$2"; shift 2 ;;
    --team) TEAM="$2"; shift 2 ;;
    --no-app) APP=0; shift ;;
    --rebuild) REBUILD=1; shift ;;
    --deploy-ios) DEPLOY_IOS=1; shift ;;
    --deploy-android) DEPLOY_ANDROID=1; shift ;;
    --check) CHECK=1; shift ;;
    -h|--help) sed -n '2,22p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $1 (see --help)" >&2; exit 2 ;;
  esac
done

LOGS="$ROOT/target/dev-launch"
mkdir -p "$LOGS"
B=$(tput bold 2>/dev/null || true); D=$(tput dim 2>/dev/null || true); R=$(tput sgr0 2>/dev/null || true)
step() { printf '\n%s▸ %s%s\n' "$B" "$*" "$R"; }
note() { printf '  %s\n' "$*"; }
cmd()  { printf '    %s$%s %s\n' "$D" "$R" "$*"; }

PIDS=""
cleanup() {
  trap - EXIT INT TERM
  for p in $PIDS; do kill "$p" 2>/dev/null || true; done
  pkill -P $$ 2>/dev/null || true   # anything else we started (e.g. the log watcher)
}
trap cleanup EXIT
trap 'exit 130' INT TERM

lan_ip() {
  local ifc ip
  ifc=$(route -n get default 2>/dev/null | awk '/interface:/{print $2}')
  for i in $ifc en0 en1; do
    ip=$(ipconfig getifaddr "$i" 2>/dev/null || true)
    [ -n "$ip" ] && { echo "$ip"; return; }
  done
  echo 127.0.0.1
}
IP=$(lan_ip)

# ------------------------------------------------------------ signaling ----
if [ -z "$SIGNAL_URL" ]; then
  step "Starting warp-signal on 0.0.0.0:$PORT (TCP + UDP)"
  # Always (re)build: an outdated server would mis-handle resume-ticket listens.
  cargo build --release -q -p warp-signal -p warp-cli
  if lsof -nP -iTCP:"$PORT" -sTCP:LISTEN >/dev/null 2>&1; then
    echo "  port $PORT is already in use — pass --port N or --signal URL" >&2; exit 1
  fi
  ./target/release/warp-signal "0.0.0.0:$PORT" >"$LOGS/warp-signal.log" 2>&1 &
  PIDS="$PIDS $!"
  for _ in $(seq 1 50); do
    curl -fsS "http://127.0.0.1:$PORT/healthz" >/dev/null 2>&1 && break
    sleep 0.1
  done
  curl -fsS "http://127.0.0.1:$PORT/healthz" >/dev/null || { echo "  warp-signal did not start (see $LOGS/warp-signal.log)" >&2; exit 1; }
  SIGNAL_URL="ws://$IP:$PORT"
  note "ready — log: $LOGS/warp-signal.log"
  note "devices on other networks need a public URL (port-forward $PORT TCP+UDP, or deploy rendezvous-relay/Dockerfile)"
else
  step "Using signaling server $SIGNAL_URL"
fi

# ------------------------------------------------------------ macOS app ----
MAC_APP="$ROOT/apple/build/DerivedData/Build/Products/Debug/WarpDrop.app"
ensure_core() {
  if [ ! -d apple/WarpCore/Frameworks/WarpCoreFFI.xcframework ]; then
    step "Building the Rust core for Apple platforms (first run, ~8 min)"
    ./scripts/build-apple.sh
  fi
}
if [ "$APP" = 1 ]; then
  if [ ! -d "$MAC_APP" ] || [ "$REBUILD" = 1 ]; then
    ensure_core
    step "Building WarpDrop for macOS"
    xcodebuild -quiet -project apple/WarpDrop.xcodeproj -scheme WarpDrop-macOS -configuration Debug \
      -destination 'generic/platform=macOS' -derivedDataPath apple/build/DerivedData CODE_SIGNING_ALLOWED=NO build \
      >"$LOGS/xcodebuild-macos.log" 2>&1 || { echo "  build failed — see $LOGS/xcodebuild-macos.log" >&2; exit 1; }
  fi
  step "Launching WarpDrop for macOS"
  "$MAC_APP/Contents/MacOS/WarpDrop" -warp.relayURL "$SIGNAL_URL" -WarpDebugPrintCode YES >"$LOGS/macos-app.log" 2>&1 &
  PIDS="$PIDS $!"
  CODE=""
  for _ in $(seq 1 300); do   # up to 30 s (first launch creates the identity)
    CODE=$(grep -m1 -oE 'WARP_CODE [0-9]{6}' "$LOGS/macos-app.log" 2>/dev/null | awk '{print $2}' || true)
    [ -n "$CODE" ] && break
    sleep 0.1
  done
  if [ -n "$CODE" ]; then
    note "this Mac's code: ${B}${CODE:0:3}-${CODE:3:3}${R}   (log: $LOGS/macos-app.log)"
    note "test from a terminal:  ./target/release/warp send ${CODE} <file> --relay $SIGNAL_URL"
  else
    note "app started (log: $LOGS/macos-app.log)"
  fi
fi

# ---------------------------------------------------------------- iOS ------
step "iPhone / iPad (physical device)"
IOS_DEVICES=""
if xcrun devicectl list devices --json-output "$LOGS/devices.json" >/dev/null 2>&1; then
  IOS_DEVICES=$(python3 -I - "$LOGS/devices.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
for dev in d.get("result", {}).get("devices", []):
    hw = dev.get("hardwareProperties", {})
    conn = dev.get("connectionProperties", {})
    # Physical devices only (devicectl also lists Simulators); USB or network.
    if hw.get("platform") == "iOS" and hw.get("reality") == "physical" and conn.get("pairingState") == "paired":
        via = {"wired": "USB", "localNetwork": "Wi-Fi"}.get(conn.get("transportType"), "offline")
        name = dev.get("deviceProperties", {}).get("name", "iPhone") + " [" + via + "]"
        print(dev.get("identifier", ""), name.replace(" ", "_"))
PY
)
fi
IOS_DD="apple/build/DerivedData-ios"
IOS_APP="$IOS_DD/Build/Products/Debug-iphoneos/WarpDrop.app"
IOS_BUILD="xcodebuild -project apple/WarpDrop.xcodeproj -scheme WarpDrop-iOS -configuration Debug -destination 'generic/platform=iOS' -derivedDataPath $IOS_DD -allowProvisioningUpdates DEVELOPMENT_TEAM=${TEAM:-<TEAM_ID>} build"
if [ -z "$IOS_DEVICES" ]; then
  note "no iOS device connected (plug it in, unlock it, trust this Mac; enable Developer Mode)"
fi
note "1. build (signing team required — set it once in Xcode or pass --team):"
cmd "$IOS_BUILD"
echo "$IOS_DEVICES" | while read -r udid name; do
  [ -z "$udid" ] && continue
  note "→ ${name//_/ } ($udid)"
  note "2. install:"; cmd "xcrun devicectl device install app --device $udid $IOS_APP"
  note "3. launch with this signaling server:"; cmd "xcrun devicectl device process launch --device $udid com.warpdrop.WarpDrop -warp.relayURL $SIGNAL_URL"
done
[ -z "$IOS_DEVICES" ] && { note "2. install:"; cmd "xcrun devicectl device install app --device <UDID> $IOS_APP"; note "3. launch:"; cmd "xcrun devicectl device process launch --device <UDID> com.warpdrop.WarpDrop -warp.relayURL $SIGNAL_URL"; }
note "   UDIDs: xcrun devicectl list devices"

if [ "$DEPLOY_IOS" = 1 ]; then
  [ -n "$TEAM" ] || { echo "  --deploy-ios needs --team TEAM_ID (or DEVELOPMENT_TEAM)" >&2; exit 2; }
  [ -n "$IOS_DEVICES" ] || { echo "  no connected iOS device" >&2; exit 1; }
  ensure_core
  step "Building for iOS devices (team $TEAM)"
  eval "$IOS_BUILD" >"$LOGS/xcodebuild-ios.log" 2>&1 || { echo "  build failed — see $LOGS/xcodebuild-ios.log" >&2; exit 1; }
  echo "$IOS_DEVICES" | while read -r udid name; do
    [ -z "$udid" ] && continue
    step "Installing on ${name//_/ }"
    xcrun devicectl device install app --device "$udid" "$IOS_APP"
    xcrun devicectl device process launch --device "$udid" com.warpdrop.WarpDrop -warp.relayURL "$SIGNAL_URL"
  done
fi

# ------------------------------------------------------------- Android -----
step "Android (physical device)"
ADB=$(command -v adb || true)
[ -z "$ADB" ] && [ -x "${ANDROID_HOME:-$HOME/Library/Android/sdk}/platform-tools/adb" ] && ADB="${ANDROID_HOME:-$HOME/Library/Android/sdk}/platform-tools/adb"
APK="android/app/build/outputs/apk/debug/app-debug.apk"
ANDROID_SERIALS=""
if [ -n "$ADB" ]; then
  ANDROID_SERIALS=$("$ADB" devices | awk 'NR>1 && $2=="device"{print $1}')
else
  note "adb not found (install Android platform-tools)"
fi
[ -z "$ANDROID_SERIALS" ] && note "no Android device in 'adb devices' (enable USB debugging and accept the prompt)"
note "1. build:"
cmd "[ -f android/app/src/main/jniLibs/arm64-v8a/libwarp.so ] || ./scripts/build-android.sh"
cmd "(cd android && ./gradlew :app:assembleDebug)"
for s in ${ANDROID_SERIALS:-<SERIAL>}; do
  note "→ $s"
  note "2. install + launch:"; cmd "adb -s $s install -r $APK"; cmd "adb -s $s shell am start -n app.warpdrop.android/.ui.MainActivity"
  note "3. relay: in the app, Settings → Internet relay server → $SIGNAL_URL"
  note "4. ⚡ USB cable (Tier 0), optional — the desktop app does this automatically:"; cmd "adb -s $s forward tcp:47821 tcp:47821"
done

if [ "$DEPLOY_ANDROID" = 1 ]; then
  [ -n "$ANDROID_SERIALS" ] || { echo "  no Android device connected" >&2; exit 1; }
  [ -f android/app/src/main/jniLibs/arm64-v8a/libwarp.so ] || ./scripts/build-android.sh
  step "Building the Android APK"
  if [ -z "${JAVA_HOME:-}" ] && [ -d "/Applications/Android Studio.app/Contents/jbr/Contents/Home" ]; then
    export JAVA_HOME="/Applications/Android Studio.app/Contents/jbr/Contents/Home"
  fi
  (cd android && ./gradlew -q :app:assembleDebug) >"$LOGS/gradle.log" 2>&1 || { echo "  build failed — see $LOGS/gradle.log" >&2; exit 1; }
  for s in $ANDROID_SERIALS; do
    step "Installing on $s"
    "$ADB" -s "$s" install -r "$APK"
    "$ADB" -s "$s" shell am start -n app.warpdrop.android/.ui.MainActivity
  done
fi

# ---------------------------------------------------------------- wait -----
step "Signaling server: $SIGNAL_URL"
note "Set the same URL on every device (Settings → Internet relay server) so codes work across networks."
if [ "$CHECK" = 1 ]; then
  step "--check: shutting down"
  exit 0
fi
case "$PIDS" in
  *[0-9]*)
    note "Watching codes and received files — Ctrl+C to stop."
    tail -n 0 -F "$LOGS/macos-app.log" 2>/dev/null | grep --line-buffered -E 'WARP_(CODE|SAVED|FAILED)' &
    PIDS="$PIDS $!"
    wait
    ;;
esac
