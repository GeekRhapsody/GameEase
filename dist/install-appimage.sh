#!/usr/bin/env bash
set -euo pipefail

APPIMAGE_SRC="${1:-${APPIMAGE:-}}"
APPIMAGE_DST_DIR="${XDG_BIN_HOME:-$HOME/.local/bin}"
APPIMAGE_DST="$APPIMAGE_DST_DIR/GameEase.AppImage"
UDEV_DST="/etc/udev/rules.d/99-gameease.rules"
SERVICE_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
SERVICE_DST="$SERVICE_DIR/gameease.service"

step() {
    printf '\n==> %s\n' "$1"
}

if [[ -z "$APPIMAGE_SRC" ]]; then
    printf 'Usage: %s /path/to/GameEase-x86_64.AppImage\n' "$0" >&2
    exit 1
fi

if [[ ! -f "$APPIMAGE_SRC" ]]; then
    printf 'Error: AppImage not found at %s\n' "$APPIMAGE_SRC" >&2
    exit 1
fi

step "Installing AppImage to $APPIMAGE_DST"
install -Dm755 "$APPIMAGE_SRC" "$APPIMAGE_DST"
printf 'Installed %s\n' "$APPIMAGE_DST"

step "Installing udev rule to $UDEV_DST"
tmp_rule="$(mktemp)"
cat > "$tmp_rule" <<'RULE'
# GameEase uinput access rules.
#
# This creates a stable /dev/uinput node and grants members of the input group,
# plus active local sessions via uaccess, permission to create the virtual
# keyboard and mouse devices used by GameEase.

KERNEL=="uinput", SUBSYSTEM=="misc", OPTIONS+="static_node=uinput"
KERNEL=="uinput", MODE="0660", GROUP="input", TAG+="uaccess"
RULE
sudo install -Dm644 "$tmp_rule" "$UDEV_DST"
rm -f "$tmp_rule"
sudo udevadm control --reload
sudo udevadm trigger --subsystem-match=misc --attr-match=name=uinput || true
printf 'Installed udev rule and reloaded udev\n'

step "Installing systemd user unit to $SERVICE_DST"
install -d "$SERVICE_DIR"
cat > "$SERVICE_DST" <<SERVICE
[Unit]
Description=GameEase overlay
After=graphical-session.target
Wants=graphical-session.target

[Service]
ExecStart=$APPIMAGE_DST
Restart=on-failure
RestartSec=3

[Install]
WantedBy=default.target
SERVICE

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

cat <<MSG

GameEase AppImage installation complete.

Installed AppImage:
  $APPIMAGE_DST

If the overlay does not appear, check:
  systemctl --user status gameease.service
  journalctl --user -u gameease.service -e
MSG
