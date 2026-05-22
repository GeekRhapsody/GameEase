#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_SRC="$ROOT_DIR/gameease"
BIN_DST="/usr/local/bin/gameease"
UDEV_SRC="$ROOT_DIR/dist/99-gameease.rules"
UDEV_DST="/etc/udev/rules.d/99-gameease.rules"
SERVICE_SRC="$ROOT_DIR/dist/gameease.service"
SERVICE_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
SERVICE_DST="$SERVICE_DIR/gameease.service"

step() {
    printf '\n==> %s\n' "$1"
}

if [[ ! -x "$BIN_SRC" ]]; then
    printf 'Error: release binary not found at %s\n' "$BIN_SRC" >&2
    printf 'Run this installer from an extracted GameEase release archive.\n' >&2
    exit 1
fi

step "Installing binary to $BIN_DST"
sudo install -Dm755 "$BIN_SRC" "$BIN_DST"
printf 'Installed %s\n' "$BIN_DST"

step "Installing udev rule to $UDEV_DST"
sudo install -Dm644 "$UDEV_SRC" "$UDEV_DST"
sudo udevadm control --reload
sudo udevadm trigger --subsystem-match=misc --attr-match=name=uinput || true
printf 'Installed udev rule and reloaded udev\n'

step "Installing systemd user unit to $SERVICE_DST"
install -Dm644 "$SERVICE_SRC" "$SERVICE_DST"
systemctl --user import-environment \
    DISPLAY \
    WAYLAND_DISPLAY \
    XAUTHORITY \
    XDG_CURRENT_DESKTOP \
    XDG_SESSION_DESKTOP \
    XDG_SESSION_TYPE || true
systemctl --user daemon-reload
systemctl --user enable --now gameease.service
printf 'Enabled and started gameease.service\n'

cat <<'MSG'

GameEase installation complete.

If the overlay does not appear, check:
  systemctl --user status gameease.service
  journalctl --user -u gameease.service -e

If the overlay does not appear because the service cannot see your display, edit:
  ~/.config/systemd/user/gameease.service
or import your session environment with:
  systemctl --user import-environment DISPLAY WAYLAND_DISPLAY XAUTHORITY XDG_SESSION_TYPE
MSG
