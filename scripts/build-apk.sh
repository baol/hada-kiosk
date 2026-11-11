#!/usr/bin/env bash
# Build the Android app (crates/hada-android) as an APK with cargo-apk.
# Needs ANDROID_HOME (an Android SDK with cmdline-tools) and a JDK 17 or newer;
# installs the NDK, platform, build-tools, Rust targets and cargo-apk it needs.
#
#   scripts/build-apk.sh                              debug APK, arm64 + armv7
#   scripts/build-apk.sh --target x86_64-linux-android   debug APK for an emulator
#   scripts/build-apk.sh --release                    release APK, signed with
#       CARGO_APK_RELEASE_KEYSTORE and CARGO_APK_RELEASE_KEYSTORE_PASSWORD
#
# Output: target/debug/apk/hada.apk or target/release/apk/hada.apk
set -euo pipefail
cd "$(dirname "$0")/.."

NDK_VERSION=27.2.12479018
PLATFORM=android-34
BUILD_TOOLS=35.0.1
CARGO_APK_VERSION=0.10.0

: "${ANDROID_HOME:?set ANDROID_HOME to the Android SDK}"
if [ ! -d "$ANDROID_HOME/ndk/$NDK_VERSION" ] || [ ! -d "$ANDROID_HOME/platforms/$PLATFORM" ] \
   || [ ! -d "$ANDROID_HOME/build-tools/$BUILD_TOOLS" ]; then
  # `yes` answers the licence prompts; it ends with a broken pipe, which is fine.
  { yes 2> /dev/null || true; } | "$ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager" \
    --sdk_root="$ANDROID_HOME" "ndk;$NDK_VERSION" "platforms;$PLATFORM" "build-tools;$BUILD_TOOLS" > /dev/null
fi
export ANDROID_NDK_ROOT="$ANDROID_HOME/ndk/$NDK_VERSION"
export ANDROID_NDK="$ANDROID_NDK_ROOT"   # skia-bindings builds Skia for armv7 itself (no prebuilt)

rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
if [ "$(cargo apk version 2> /dev/null | grep -o '[0-9.]*$')" != "$CARGO_APK_VERSION" ]; then
  cargo install cargo-apk --version "$CARGO_APK_VERSION" --locked
fi

cargo apk build -p hada-android --lib "$@"
