#!/usr/bin/env bash
# Signed + notarized macOS release, built on this Mac.
#
# Uses the Apple ID signed into Xcode (Settings → Accounts): Xcode signs with the team's
# cloud-managed Developer ID certificate and submits the app to Apple's notary service, so
# no certificate file or app-specific password is needed. CI cannot do this (cloud-managed
# certificates cannot be exported), which is why the release workflow only signs macOS
# when the MACOS_* secrets hold an exportable Developer ID certificate.
#
#   ./scripts/release-macos.sh 1.2.0                   → dist/WarpDrop-1.2.0-macOS.dmg
#   ./scripts/release-macos.sh 1.2.0 --upload v1.2.0   → also replaces the asset and SHA256SUMS
#                                                         in that GitHub release
set -euo pipefail

VERSION="${1:?usage: release-macos.sh <version> [--upload <tag>]}"
TAG=""
[[ "${2:-}" == "--upload" ]] && TAG="${3:?missing tag}"
TEAM="${WARPDROP_TEAM_ID:-62XX6A3AR9}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$ROOT/build/release-macos"
DMG="$ROOT/dist/WarpDrop-$VERSION-macOS.dmg"

rm -rf "$WORK"; mkdir -p "$WORK" "$ROOT/dist"
cd "$ROOT"

echo "▸ Rust core (macOS, universal)"
MACOS_ONLY=1 ./scripts/build-apple.sh

echo "▸ Archive"
xcodebuild -project apple/WarpDrop.xcodeproj -scheme WarpDrop-macOS -configuration Release \
  -destination 'generic/platform=macOS' -archivePath "$WORK/WarpDrop.xcarchive" \
  -derivedDataPath "$WORK/dd" ARCHS="arm64 x86_64" ONLY_ACTIVE_ARCH=NO \
  DEVELOPMENT_TEAM="$TEAM" MARKETING_VERSION="$VERSION" CURRENT_PROJECT_VERSION="$(date +%Y%m%d%H)" \
  -allowProvisioningUpdates -quiet archive

echo "▸ Developer ID export + notarization upload"
cat > "$WORK/export.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>method</key><string>developer-id</string>
  <key>destination</key><string>upload</string>
  <key>signingStyle</key><string>automatic</string>
  <key>teamID</key><string>$TEAM</string>
</dict></plist>
EOF
xcodebuild -exportArchive -archivePath "$WORK/WarpDrop.xcarchive" -exportOptionsPlist "$WORK/export.plist" \
  -exportPath "$WORK/export" -allowProvisioningUpdates -quiet

echo "▸ Waiting for Apple's notary service"
for _ in $(seq 1 60); do
  if xcodebuild -exportNotarizedApp -archivePath "$WORK/WarpDrop.xcarchive" -exportPath "$WORK/notarized" >/dev/null 2>&1; then
    break
  fi
  sleep 30
done
APP="$WORK/notarized/WarpDrop.app"
[[ -d "$APP" ]] || { echo "notarization did not finish in 30 minutes" >&2; exit 1; }
spctl -a -t exec "$APP"
xcrun stapler validate "$APP" >/dev/null

echo "▸ DMG"
mkdir -p "$WORK/dmgroot"
ditto "$APP" "$WORK/dmgroot/WarpDrop.app"
ln -s /Applications "$WORK/dmgroot/Applications"
hdiutil create -quiet -volname WarpDrop -srcfolder "$WORK/dmgroot" -ov -format UDZO "$DMG"
echo "✓ $DMG"
shasum -a 256 "$DMG"

if [[ -n "$TAG" ]]; then
  echo "▸ Upload to release $TAG"
  cd "$WORK"
  gh release download "$TAG" -p SHA256SUMS.txt --clobber
  NAME="$(basename "$DMG")"
  grep -v "  \*\{0,1\}$NAME\$" SHA256SUMS.txt > SHA256SUMS.new || true
  (cd "$(dirname "$DMG")" && shasum -a 256 "$NAME") >> SHA256SUMS.new
  mv SHA256SUMS.new SHA256SUMS.txt
  gh release upload "$TAG" "$DMG" SHA256SUMS.txt --clobber
  echo "✓ uploaded"
fi
