#!/usr/bin/env bash
# hada kiosk installer: hada starts at boot, full screen, INSTEAD of the desktop.
# Runs on the device (Raspberry Pi OS, Debian or Ubuntu, 64-bit) as a normal
# user with sudo. Read it before running it; every step is listed below.
#
#   Install:    HA_URL=http://homeassistant.local:8123 bash install-kiosk.sh
#   Uninstall:  bash install-kiosk.sh uninstall      (restores the previous boot setup)
#
# Settings (environment variables):
#   HA_URL          required    Home Assistant address as seen from the device. A dashboard
#                               URL such as http://ha.local:8123/lovelace/kiosk also selects
#                               the dashboard and the view to open.
#   TOKEN_FILE      ha-token    file holding a Home Assistant long-lived access token, alone on
#                               one line (create it under Profile > Security). "" = log in
#                               through the trusted_networks auth provider instead.
#   ROTATION        0           0 or 180 = landscape, 90 or 270 = portrait
#   TOUCH_ROTATION  =ROTATION   change it if touches land in the wrong place
#   LANG_CODE       from $LANG  language for state texts: en, it, de, fr, ...
#   HA_USER                     user to log in as through trusted_networks, if several
#   HADA_VERSION    latest      release to install, e.g. v0.1.0. A hada-<arch>-linux-gnu
#                               binary and its .sha256 in the current folder are used instead.
set -euo pipefail

USER="${USER:-$(id -un)}"                            # not set in every shell
REPO="${HADA_REPO:-https://github.com/baol/hada-kiosk}"   # where releases are downloaded from
HADA_VERSION="${HADA_VERSION:-latest}"
STATE=/etc/hada/install-state

# ------------------------------------------------------------------ uninstall --
if [ "${1:-}" = uninstall ]; then
  echo "== removing hada and restoring the previous boot setup"
  sudo systemctl disable --now hada 2> /dev/null || true
  sudo rm -f /etc/systemd/system/hada.service /etc/udev/rules.d/99-hada-touch-rotation.rules \
             /usr/local/sbin/hada-probe-display /usr/local/bin/hada /etc/hada.token
  sudo udevadm control --reload-rules
  if [ -f "$STATE" ]; then
    # shellcheck source=/dev/null
    . "$STATE"
    sudo systemctl set-default "$DEFAULT_TARGET"
    for dm in $DISPLAY_MANAGERS; do sudo systemctl enable "$dm"; done
    [ "$SEATD_WAS_ENABLED" = yes ] || sudo systemctl disable --now seatd
    for g in $ADDED_GROUPS; do sudo gpasswd -d "$INSTALL_USER" "$g" > /dev/null || true; done
    echo "restored: boot target $DEFAULT_TARGET${DISPLAY_MANAGERS:+, display manager $DISPLAY_MANAGERS}"
  else
    echo "no record of the previous setup ($STATE); to boot into the desktop again:"
    echo "  sudo systemctl set-default graphical.target && sudo systemctl enable lightdm"
  fi
  sudo rm -rf /etc/hada
  sudo systemctl daemon-reload
  echo "Done. Packages installed by hada (seatd, libinput, ...) were left in place. Reboot to finish."
  exit 0
fi

# ------------------------------------------------------------------- settings --
HA_URL="${HA_URL:?set HA_URL, e.g.  HA_URL=http://homeassistant.local:8123 bash install-kiosk.sh}"
TOKEN_FILE="${TOKEN_FILE-ha-token}"
ROTATION="${ROTATION:-0}"
TOUCH_ROTATION="${TOUCH_ROTATION:-$ROTATION}"
LANG_CODE="${LANG_CODE:-${LANG:-en}}"
HA_USER="${HA_USER:-}"

case "$ROTATION" in 0|90|180|270) ;; *) echo "ROTATION must be 0, 90, 180 or 270"; exit 1 ;; esac
case "$TOUCH_ROTATION" in   # libinput calibration: Slint rotates the picture, not the touch input
  0)   MATRIX="1 0 0 0 1 0" ;;
  90)  MATRIX="0 -1 1 1 0 0" ;;
  180) MATRIX="-1 0 1 0 -1 1" ;;
  270) MATRIX="0 1 0 -1 0 1" ;;
  *) echo "TOUCH_ROTATION must be 0, 90, 180 or 270"; exit 1 ;;
esac
case "$(uname -m)" in
  aarch64|arm64) BINARY="hada-aarch64-linux-gnu" ;;
  x86_64)        BINARY="hada-x86_64-linux-gnu" ;;
  *) echo "unsupported architecture $(uname -m): hada needs a 64-bit OS (aarch64 or x86_64)"; exit 1 ;;
esac
if [ -n "$TOKEN_FILE" ] && [ ! -f "$TOKEN_FILE" ]; then
  echo "token file '$TOKEN_FILE' not found (TOKEN_FILE=\"\" uses trusted networks instead)"; exit 1
fi
HA_HOST="$(echo "$HA_URL" | sed -E 's|^[a-z]+://([^/:]+).*|\1|')"

echo "== 1/7 record the current boot setup (first install only; used by 'uninstall')"
if [ ! -f "$STATE" ]; then
  if [ -f /etc/systemd/system/hada.service ]; then
    echo "an older hada install without a record exists; uninstall will print manual steps"
  else
    DMS=""; for dm in lightdm gdm3 sddm; do systemctl is-enabled "$dm" > /dev/null 2>&1 && DMS="$DMS $dm"; done
    ADDED=""; for g in seat video input render; do
      getent group "$g" > /dev/null && ! id -nG "$USER" | tr ' ' '\n' | grep -qx "$g" && ADDED="$ADDED $g"
    done
    SEATD="no"; systemctl is-enabled seatd > /dev/null 2>&1 && SEATD="yes"
    sudo mkdir -p /etc/hada
    printf 'DEFAULT_TARGET=%s\nDISPLAY_MANAGERS="%s"\nADDED_GROUPS="%s"\nSEATD_WAS_ENABLED=%s\nINSTALL_USER=%s\n' \
      "$(systemctl get-default)" "${DMS# }" "${ADDED# }" "$SEATD" "$USER" | sudo tee "$STATE"
  fi
fi

echo "== 2/7 runtime packages"
sudo apt-get install -y --no-install-recommends curl seatd libinput10 libudev1 libxkbcommon0 libseat1 fontconfig fonts-noto-core

echo "== 3/7 binary -> /usr/local/bin/hada"
if [ ! -f "$BINARY" ]; then
  if [ "$HADA_VERSION" = latest ]; then URL="$REPO/releases/latest/download"; else URL="$REPO/releases/download/$HADA_VERSION"; fi
  curl -fL --progress-bar -o "$BINARY" "$URL/$BINARY"
  curl -fsSL -o "$BINARY.sha256" "$URL/$BINARY.sha256"
fi
sha256sum -c "$BINARY.sha256"                       # refuse a damaged or different file
chmod +x "$BINARY"
"./$BINARY" --version                               # stops here if it does not run on this OS
sudo install -m 755 "$BINARY" /usr/local/bin/hada

echo "== 4/7 credentials"
AUTH_ENV=""
sudo mkdir -p /etc/hada
if [ -n "$TOKEN_FILE" ]; then
  sudo install -m 640 -o root -g "$(id -gn "$USER")" "$TOKEN_FILE" /etc/hada/token
  AUTH_ENV="Environment=HADA_TOKEN_FILE=/etc/hada/token"
else
  echo "no token: hada logs in through the trusted_networks auth provider"
  if [ -n "$HA_USER" ]; then AUTH_ENV="Environment=HADA_USER=$HA_USER"; fi
fi

echo "== 5/7 seatd + groups, so hada can use display and input without root"
sudo systemctl enable --now seatd
GROUPS_OK=""
for g in seat video input render; do
  if getent group "$g" > /dev/null; then sudo usermod -aG "$g" "$USER"; GROUPS_OK="$GROUPS_OK $g"; fi
done

echo "== 6/7 display probe helper, systemd unit, touch rotation rule"
# Some panels answer their EDID late at boot, so the kernel only offers generic
# modes. This helper asks for a fresh probe before hada starts and logs the result.
sudo tee /usr/local/sbin/hada-probe-display > /dev/null <<'PROBE'
#!/bin/sh
for c in /sys/class/drm/card*-*; do
  [ -w "$c/status" ] || continue
  echo detect > "$c/status"; sleep 1; echo detect > "$c/status"
  [ "$(cat "$c/status")" = connected ] && echo "$(basename "$c"): first mode $(head -n 1 "$c/modes")"
done
exit 0
PROBE
sudo chmod 755 /usr/local/sbin/hada-probe-display
sudo tee /etc/systemd/system/hada.service > /dev/null <<EOF
[Unit]
Description=hada - native Home Assistant dashboard
After=network-online.target seatd.service
Wants=network-online.target

[Service]
User=$USER
SupplementaryGroups=$GROUPS_OK
# The network may come up after network-online.target: wait (max 60 s) for the HA host.
ExecStartPre=-/usr/bin/timeout 60 /bin/sh -c 'until getent hosts $HA_HOST >/dev/null; do sleep 1; done'
ExecStartPre=+/usr/local/sbin/hada-probe-display
ExecStart=/usr/local/bin/hada
Restart=always
RestartSec=3
Environment=SLINT_BACKEND=linuxkms-software
Environment=SLINT_KMS_ROTATION=$ROTATION
Environment=HADA_URL=$HA_URL
Environment=HADA_LANG=$LANG_CODE
Environment=RUST_LOG=info
$AUTH_ENV

[Install]
WantedBy=multi-user.target
EOF
echo "ENV{ID_INPUT_TOUCHSCREEN}==\"1\", ENV{LIBINPUT_CALIBRATION_MATRIX}=\"$MATRIX\"" \
  | sudo tee /etc/udev/rules.d/99-hada-touch-rotation.rules > /dev/null
sudo rm -f /etc/hada.token                          # location used by older versions
sudo udevadm control --reload-rules
sudo udevadm trigger

echo "== 7/7 boot into hada instead of the desktop"
for dm in lightdm gdm3 sddm; do
  if systemctl is-enabled "$dm" > /dev/null 2>&1; then sudo systemctl disable --now "$dm"; fi
done
sudo systemctl set-default multi-user.target
sudo systemctl daemon-reload
sudo systemctl enable hada
sudo systemctl restart hada                          # restart: a re-run loads the new binary and settings

cat <<EOF

Done. Reboot once so the group memberships apply, then check:  journalctl -u hada -b
  Picture upside down?        run again with another ROTATION
  Touches in the wrong place? run again with another TOUCH_ROTATION
  Back to the desktop:        bash install-kiosk.sh uninstall
EOF
