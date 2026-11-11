#!/usr/bin/env bash
# Install the APK on a running emulator (or device), start it and check that it
# stays up without crashing. Saves a screenshot and the log next to each other.
#
#   scripts/android-smoke-test.sh [apk] [output-dir]
set -euo pipefail
APK="${1:-target/debug/apk/hada.apk}"
OUT="${2:-android-smoke}"
PKG=io.github.baol.hada
mkdir -p "$OUT"

adb install -r "$APK"
# Only to shorten the log; some Android 8 emulators refuse to clear it.
adb logcat -c || echo "(log not cleared)"
adb shell am start -W -n "$PKG/android.app.NativeActivity"
sleep 30   # first start on a software-rendered emulator is slow

adb exec-out screencap -p > "$OUT/first-start.png"
adb logcat -d > "$OUT/logcat.txt"
echo "--- hada log"
grep -E "[ /]hada *:" "$OUT/logcat.txt" || true

PID="$(adb shell pidof "$PKG" 2> /dev/null | tr -d '\r' || true)"
if [ -z "$PID" ]; then   # older toybox without pidof
  PID="$(adb shell ps -A 2> /dev/null | awk -v p="$PKG" '$NF == p { print $2 }' | tr -d '\r' || true)"
fi
if [ -z "$PID" ]; then
  echo "hada is not running after start"
  grep -E -A20 "FATAL|Fatal signal|$PKG" "$OUT/logcat.txt" | head -80 || true
  exit 1
fi
if grep -E "panic:|FATAL EXCEPTION|Fatal signal" "$OUT/logcat.txt" | grep -q -e "$PID" -e "hada"; then
  echo "hada crashed"
  exit 1
fi
grep -q "hada [0-9]" "$OUT/logcat.txt" || { echo "no startup line from hada in the log"; exit 1; }
echo "hada is running (pid $PID)"
