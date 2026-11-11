#!/usr/bin/env bash
# Build the Android app (Android 5.0 and newer) as an APK with the Android SDK and
# NDK tools only (no Gradle): Rust → lib/<abi>/libhada_android.so (NDK clang),
# Java → classes.dex (javac + d8), manifest and icons (aapt2), zipalign, apksigner.
# Installs the NDK, platform, build-tools and Rust targets it needs.
#
#   scripts/build-apk.sh                                   arm64 + armv7
#   scripts/build-apk.sh --target x86_64-linux-android     e.g. for an emulator
#
# Signing: HADA_APK_KEYSTORE (path) and HADA_APK_KEYSTORE_PASSWORD. Without them a
# throwaway key in target/android/ is used: fine for testing, but Android refuses
# to update an app signed with a different key.
#
# Needs ANDROID_HOME (an Android SDK with cmdline-tools) and a JDK 17 or newer.
# Output: target/android/hada.apk
set -euo pipefail
cd "$(dirname "$0")/.."

NDK_VERSION=27.2.12479018
PLATFORM=android-34
BUILD_TOOLS=35.0.1
MIN_SDK=21
TARGET_SDK=34

: "${ANDROID_HOME:?set ANDROID_HOME to the Android SDK}"
if [ ! -d "$ANDROID_HOME/ndk/$NDK_VERSION" ] || [ ! -d "$ANDROID_HOME/platforms/$PLATFORM" ] \
   || [ ! -d "$ANDROID_HOME/build-tools/$BUILD_TOOLS" ]; then
  # `yes` answers the licence prompts; it ends with a broken pipe, which is fine.
  { yes 2> /dev/null || true; } | "$ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager" \
    --sdk_root="$ANDROID_HOME" "ndk;$NDK_VERSION" "platforms;$PLATFORM" "build-tools;$BUILD_TOOLS" > /dev/null
fi
NDK_BIN="$ANDROID_HOME/ndk/$NDK_VERSION/toolchains/llvm/prebuilt/linux-x86_64/bin"
BT="$ANDROID_HOME/build-tools/$BUILD_TOOLS"
JAR="$ANDROID_HOME/platforms/$PLATFORM/android.jar"

TARGETS=()
while [ $# -gt 0 ]; do
  case "$1" in
    --target) TARGETS+=("$2"); shift 2 ;;
    *) echo "unknown option $1"; exit 1 ;;
  esac
done
[ ${#TARGETS[@]} -gt 0 ] || TARGETS=(aarch64-linux-android armv7-linux-androideabi)
rustup target add "${TARGETS[@]}"

OUT=target/android
rm -rf "$OUT/stage" "$OUT/classes"
mkdir -p "$OUT/stage/lib" "$OUT/classes"

# --- native code, linked against the API $MIN_SDK libraries ----------------------
for t in "${TARGETS[@]}"; do
  case "$t" in
    aarch64-linux-android)   abi=arm64-v8a;   cc="aarch64-linux-android$MIN_SDK-clang" ;;
    armv7-linux-androideabi) abi=armeabi-v7a; cc="armv7a-linux-androideabi$MIN_SDK-clang" ;;
    x86_64-linux-android)    abi=x86_64;      cc="x86_64-linux-android$MIN_SDK-clang" ;;
    *) echo "unsupported target $t"; exit 1 ;;
  esac
  var="$(echo "$t" | tr 'a-z-' 'A-Z_')"
  env "CARGO_TARGET_${var}_LINKER=$NDK_BIN/$cc" "CC_${t//-/_}=$NDK_BIN/$cc" \
      "CXX_${t//-/_}=$NDK_BIN/$cc++" "AR_${t//-/_}=$NDK_BIN/llvm-ar" \
      cargo build -p hada-android --lib --release --target "$t"
  mkdir -p "$OUT/stage/lib/$abi"
  "$NDK_BIN/llvm-strip" -o "$OUT/stage/lib/$abi/libhada_android.so" "target/$t/release/libhada_android.so"
done

# --- Java: preferences screen and boot receiver ----------------------------------
mapfile -t JAVA < <(find android/java -name '*.java' | sort)
javac --release 8 -Xlint:-options -encoding UTF-8 -classpath "$JAR" -d "$OUT/classes" "${JAVA[@]}"
mapfile -t CLASSES < <(find "$OUT/classes" -name '*.class' | sort)
"$BT/d8" --release --min-api "$MIN_SDK" --lib "$JAR" --output "$OUT/stage" "${CLASSES[@]}"

# --- manifest, icons; version code as cargo-apk had it (1<<24 | major<<16 | minor<<8 | patch)
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
IFS=. read -r MAJOR MINOR PATCH <<< "${VERSION%%[-+]*}"
CODE=$(( (1 << 24) | (MAJOR << 16) | (MINOR << 8) | PATCH ))
"$BT/aapt2" compile --dir crates/hada-android/res -o "$OUT/res.zip"
"$BT/aapt2" link -I "$JAR" --manifest android/AndroidManifest.xml \
  --min-sdk-version "$MIN_SDK" --target-sdk-version "$TARGET_SDK" \
  --version-code "$CODE" --version-name "$VERSION" -o "$OUT/unsigned.apk" "$OUT/res.zip"
(cd "$OUT/stage" && zip -q -r ../unsigned.apk classes.dex lib)
"$BT/zipalign" -f -p 4 "$OUT/unsigned.apk" "$OUT/aligned.apk"

# --- signing ---------------------------------------------------------------------
if [ -z "${HADA_APK_KEYSTORE:-}" ]; then
  echo "No HADA_APK_KEYSTORE: signing with a throwaway test key."
  HADA_APK_KEYSTORE="$OUT/test.keystore"
  export HADA_APK_KEYSTORE_PASSWORD=android
  [ -f "$HADA_APK_KEYSTORE" ] || keytool -genkeypair -keystore "$HADA_APK_KEYSTORE" \
    -storepass android -keypass android -alias test -keyalg RSA -keysize 2048 \
    -validity 3650 -dname "CN=hada test" -noprompt > /dev/null 2>&1
fi
: "${HADA_APK_KEYSTORE_PASSWORD:?set HADA_APK_KEYSTORE_PASSWORD}"
"$BT/apksigner" sign --ks "$HADA_APK_KEYSTORE" --ks-pass env:HADA_APK_KEYSTORE_PASSWORD \
  --out "$OUT/hada.apk" "$OUT/aligned.apk"
"$BT/apksigner" verify "$OUT/hada.apk"
rm -f "$OUT/unsigned.apk" "$OUT/aligned.apk" "$OUT/hada.apk.idsig"
echo "$OUT/hada.apk: version $VERSION ($CODE), ${TARGETS[*]}"
