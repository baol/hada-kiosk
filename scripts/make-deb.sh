#!/usr/bin/env bash
# Package a hada binary as a .deb for Debian, Ubuntu and Raspberry Pi OS.
# Usage: scripts/make-deb.sh <binary> <version> <amd64|arm64> <output.deb>
set -euo pipefail
BIN="$1"; VERSION="${2#v}"; ARCH="$3"; OUT="$4"
ROOT="$(mktemp -d)"
chmod 755 "$ROOT"
trap 'rm -rf "$ROOT"' EXIT
install -D -m 755 "$BIN" "$ROOT/usr/bin/hada"
# The binary contains Slint under GPL-3.0, so the package is GPL-3.0 as a whole;
# hada's own code is MIT OR Apache-2.0. All three texts go into the copyright file.
TOP="$(dirname "$0")/.."
install -d "$ROOT/usr/share/doc/hada"
{
  echo "hada: https://github.com/baol/hada-kiosk"
  echo
  echo "This package is distributed under GPL-3.0-only: it contains the Slint UI"
  echo "toolkit under that licence. hada's own code is licensed under MIT OR"
  echo "Apache-2.0, at your option. See LICENSE-NOTICES.md in the source repository."
  echo
  for f in LICENSES/GPL-3.0-only.txt LICENSE-MIT LICENSE-APACHE; do
    echo "===== $f"
    echo
    cat "$TOP/$f"
    echo
  done
} > "$ROOT/usr/share/doc/hada/copyright"
chmod 644 "$ROOT/usr/share/doc/hada/copyright"
install -D -m 755 "$(dirname "$0")/../deploy/install-kiosk.sh" "$ROOT/usr/share/hada/install-kiosk.sh"
mkdir -p "$ROOT/DEBIAN"
cat > "$ROOT/DEBIAN/control" <<CONTROL
Package: hada
Version: $VERSION
Architecture: $ARCH
Maintainer: hada contributors <https://github.com/baol/hada-kiosk/issues>
Homepage: https://github.com/baol/hada-kiosk
Section: misc
Priority: optional
Depends: libc6 (>= 2.36), libinput10, libudev1, libxkbcommon0, libseat1, libfontconfig1
Recommends: libxkbcommon-x11-0, libx11-6, libx11-xcb1, libxcursor1, libxi6, libwayland-client0, fonts-noto-core | fonts-dejavu-core
Suggests: seatd
Installed-Size: $(du -sk "$ROOT/usr" | cut -f1)
Description: native Home Assistant dashboard for wall panels and kiosks
 hada reads a Home Assistant Lovelace dashboard and renders it natively,
 in a desktop window or full screen on the console (DRM/KMS), without a
 browser.
CONTROL
dpkg-deb --root-owner-group --build "$ROOT" "$OUT" > /dev/null
echo "$OUT"
