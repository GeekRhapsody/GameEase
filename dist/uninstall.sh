#!/usr/bin/env bash
set -euo pipefail

BIN_DST="/usr/local/bin/gameease"
UDEV_DST="/etc/udev/rules.d/99-gameease.rules"
SERVICE_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
SERVICE_DST="$SERVICE_DIR/gameease.service"

step() {
    printf '\n==> %s\n' "$1"
}

step "Stopping and disabling systemd user unit"
systemctl --user disable --now gameease.service 2>/dev/null || true
systemctl --user daemon-reload
printf 'Stopped and disabled gameease.service if it was installed\n'

step "Removing systemd user unit"
rm -f "$SERVICE_DST"
systemctl --user daemon-reload
printf 'Removed %s\n' "$SERVICE_DST"

step "Removing installed binary"
sudo rm -f "$BIN_DST"
printf 'Removed %s\n' "$BIN_DST"

step "Removing udev rule"
sudo rm -f "$UDEV_DST"
sudo udevadm control --reload
sudo udevadm trigger --subsystem-match=misc --attr-match=name=uinput || true
printf 'Removed udev rule and reloaded udev\n'

cat <<'MSG'

GameEase uninstallation complete.
MSG
