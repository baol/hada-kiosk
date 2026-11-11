#!/usr/bin/env bash
# Install the APK on a running emulator (or device) and check the first start:
# with nothing configured the preferences screen opens; Back returns to the
# dashboard, drawn by Slint's software renderer. The app must not crash.
# Screenshots and the log are saved in the output folder; small JPEG copies are
# also printed (base64) so they can be read from the CI log.
#
#   scripts/android-smoke-test.sh [apk] [output-dir]
set -euo pipefail
APK="${1:-target/android/hada.apk}"
OUT="${2:-android-smoke}"
PKG=io.github.baol.hada
mkdir -p "$OUT"

shot() {   # shot <name>
  adb exec-out screencap -p > "$OUT/$1.png"
  if command -v convert > /dev/null; then
    echo "::group::screenshot $1 (base64 JPEG)"
    convert "$OUT/$1.png" -resize 360x -quality 60 jpg:- | base64 -w0
    echo
    echo "::endgroup::"
  fi
}

front() {  # the activity in front
  adb shell dumpsys activity activities | grep -E "mResumedActivity|mFocusedActivity" | head -1
}

pid() {
  local p
  p="$(adb shell pidof "$PKG" 2> /dev/null | tr -d '\r' || true)"
  if [ -z "$p" ]; then   # older toybox without pidof
    p="$(adb shell ps 2> /dev/null | awk -v n="$PKG" '$NF == n { print $2 }' | tr -d '\r' || true)"
  fi
  echo "$p"
}

fail() {
  echo "FAILED: $*"
  adb logcat -d > "$OUT/logcat.txt" || true
  grep -E "[ /]hada *:|FATAL|Fatal signal|AndroidRuntime" "$OUT/logcat.txt" | tail -60 || true
  exit 1
}

adb install -r "$APK"
# Only to shorten the log; some Android 5-8 emulators refuse to clear it.
adb logcat -c || echo "(log not cleared)"
adb shell am start -W -n "$PKG/android.app.NativeActivity"
sleep 15

echo "front: $(front)"
front | grep -q "PrefsActivity" || fail "the preferences screen did not open on first start"
shot preferences

adb shell input keyevent KEYCODE_BACK
sleep 5
if front | grep -q "PrefsActivity"; then   # the first Back may only close a keyboard
  adb shell input keyevent KEYCODE_BACK
fi
sleep 10
echo "front: $(front)"
front | grep -q "NativeActivity" || fail "Back did not return to the dashboard"
shot dashboard

adb logcat -d > "$OUT/logcat.txt"
echo "--- hada log"
grep -E "[ /]hada *:" "$OUT/logcat.txt" || true
[ -n "$(pid)" ] || fail "hada is not running"
if grep -E "panic:|FATAL EXCEPTION|Fatal signal" "$OUT/logcat.txt" | grep -q -e "hada"; then
  fail "hada crashed"
fi
grep -q "preferences opened" "$OUT/logcat.txt" || fail "no 'preferences opened' in the log"
grep -q "first frame drawn" "$OUT/logcat.txt" || fail "the dashboard was never drawn"
echo "OK: preferences opened, dashboard drawn, hada running (pid $(pid))"
