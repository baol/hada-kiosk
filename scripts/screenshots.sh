#!/usr/bin/env bash
# Render the demo dashboard to PNG without a display (software renderer).
# Usage: scripts/screenshots.sh [output-dir]   (default: docs/screenshots)
# Needs a build with the `screenshot` feature; the clock is pinned so the
# calendar and relative times look the same on every run.
set -euo pipefail
cd "$(dirname "$0")/.."
OUT="${1:-docs/screenshots}"
mkdir -p "$OUT"
cargo build --quiet -p hada --features screenshot
BIN=target/debug/hada
export HADA_NOW=1794211200   # 2026-11-09 08:00 UTC, matches tests/fixtures/events.json
F=(--dashboard tests/fixtures/dashboard.yaml --states tests/fixtures/states.json
   --icons tests/fixtures/icons.json --events tests/fixtures/events.json)
"$BIN" "${F[@]}" --lang en --translations tests/fixtures/translations_en.json --view home    --size 1024x600 --screenshot "$OUT/landscape-home.png"
"$BIN" "${F[@]}" --lang en --translations tests/fixtures/translations_en.json --view devices --size 1024x600 --screenshot "$OUT/landscape-devices.png"
"$BIN" "${F[@]}" --lang en --translations tests/fixtures/translations_en.json --view kiosk   --size 1024x600 --screenshot "$OUT/landscape-kiosk.png"
"$BIN" "${F[@]}" --lang en --translations tests/fixtures/translations_en.json --view kiosk   --size 600x1024 --screenshot "$OUT/portrait-kiosk.png"
"$BIN" "${F[@]}" --lang en --translations tests/fixtures/translations_en.json --view home    --size 600x1024 --screenshot "$OUT/portrait-home.png"
# The same portrait view in Italian, showing Home Assistant's own translations.
"$BIN" "${F[@]}" --lang it --view kiosk   --size 600x1024 \
  --translations tests/fixtures/translations_it.json --screenshot "$OUT/portrait-kiosk-it.png"
ls -l "$OUT"
