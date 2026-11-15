#!/usr/bin/env bash
# Builds libwarp for macOS (arm64 + x86_64), iOS and the iOS simulator,
# packages WarpCoreFFI.xcframework and regenerates the Swift bindings into the
# local Swift package apple/WarpCore.
#   MACOS_ONLY=1  build only the macOS slice (CI .dmg job)
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
PROFILE="${PROFILE:-release}"
FLAG=$([ "$PROFILE" = release ] && echo --release || echo "")
OUT=apple/WarpCore
GEN=target/uniffi-swift
export IPHONEOS_DEPLOYMENT_TARGET=17.0 MACOSX_DEPLOYMENT_TARGET=14.0

TARGETS="aarch64-apple-darwin x86_64-apple-darwin aarch64-apple-ios aarch64-apple-ios-sim"
[ "${MACOS_ONLY:-0}" = 1 ] && TARGETS="aarch64-apple-darwin x86_64-apple-darwin"
for t in $TARGETS; do
  echo "▸ cargo build $t"
  cargo build -q $FLAG -p libwarp --target "$t"
done

rm -rf "$GEN" && mkdir -p "$GEN/headers" target/universal-macos
cargo run -q -p uniffi-bindgen -- generate \
  --library "target/aarch64-apple-darwin/$PROFILE/libwarp.dylib" \
  --language swift --out-dir "$GEN" --no-format
cp "$GEN/warpFFI.h" "$GEN/headers/"
cat > "$GEN/headers/module.modulemap" <<MAP
module warpFFI {
    header "warpFFI.h"
    export *
}
MAP

lipo -create \
  "target/aarch64-apple-darwin/$PROFILE/libwarp.a" \
  "target/x86_64-apple-darwin/$PROFILE/libwarp.a" \
  -output target/universal-macos/libwarp.a

rm -rf "$OUT/Frameworks/WarpCoreFFI.xcframework"
mkdir -p "$OUT/Frameworks" "$OUT/Sources/WarpCore"
LIBS=(-library target/universal-macos/libwarp.a -headers "$GEN/headers")
if [ "${MACOS_ONLY:-0}" != 1 ]; then
  LIBS+=(-library "target/aarch64-apple-ios/$PROFILE/libwarp.a" -headers "$GEN/headers")
  LIBS+=(-library "target/aarch64-apple-ios-sim/$PROFILE/libwarp.a" -headers "$GEN/headers")
fi
xcodebuild -create-xcframework "${LIBS[@]}" -output "$OUT/Frameworks/WarpCoreFFI.xcframework" >/dev/null
cp "$GEN/warp.swift" "$OUT/Sources/WarpCore/warp.swift"
echo "✓ $OUT/Frameworks/WarpCoreFFI.xcframework + Swift bindings"
