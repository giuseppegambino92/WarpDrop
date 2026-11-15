#!/usr/bin/env bash
# Builds libwarp.so for Android ABIs with cargo-ndk and regenerates the
# Kotlin bindings (package `uniffi.warp`) into the app module.
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
SDK="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$(ls -d "$SDK"/ndk/* | sort -V | tail -1)}"
JNI=android/app/src/main/jniLibs
echo "▸ NDK $ANDROID_NDK_HOME"
cargo ndk -t arm64-v8a -t armeabi-v7a -t x86_64 -P 26 -o "$JNI" build --release -p libwarp
cargo run -q -p uniffi-bindgen -- generate \
  --library target/aarch64-linux-android/release/libwarp.so \
  --language kotlin --out-dir android/app/src/main/java --no-format
echo "✓ $JNI + android/app/src/main/java/uniffi/warp/warp.kt"
