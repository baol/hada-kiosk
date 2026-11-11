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
adb logcat -c
adb shell am start -W -n "$PKG/android.app.NativeActivity"
sleep 30   # first start on a software-rendered emulator is slow

adb exec-out screencap -p > "$OUT/first-start.png"
adb logcat -d > "$OUT/logcat.txt"
echo "--- hada log"
grep -E "[ /]hada *:" "$OUT/logcat.txt" || true

PID="$(adb shell pidof "$PKG" | tr -d '\r' || true)"
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
