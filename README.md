# GameEase

GameEase is a Linux Wayland overlay for gamepad-first desktop control.

It creates a `wlr-layer-shell` overlay window with an on-screen keyboard and a
slide-in side menu. A background `gilrs` thread reads gamepad input independently
of window focus, and the OSK injects keys through Linux uinput.

## Target platform

- Linux
- Wayland compositor with layer-shell support, including KDE Plasma/KWin, Sway,
  and Hyprland
- GTK4
- `wlr-layer-shell`

GNOME is not supported because it does not expose `wlr-layer-shell`. Gamescope
compatibility is untested.

## Installation

### Prerequisites

Install:

- Rust toolchain
- `libgtk-4-dev`
- `libgtk4-layer-shell-dev`
- `libudev-dev`
- `libxkbcommon-dev`

Package names vary by distribution. On Arch-based systems the native packages
are typically:

```sh
sudo pacman -S rust gtk4 gtk4-layer-shell systemd-libs libxkbcommon
```

### One-line install

```sh
bash dist/install.sh
```

The installer builds the release binary, installs it to `/usr/local/bin/gameease`,
installs the udev rule, and enables `gameease.service` as a systemd user unit.

The service defaults to:

```ini
Environment=WAYLAND_DISPLAY=wayland-1
```

Adjust `~/.config/systemd/user/gameease.service` if your compositor uses a
different Wayland socket.

### Manual testing

From a supported Wayland compositor such as KDE Plasma/KWin, Sway, or Hyprland:

```sh
cargo run
```

The overlay is anchored to the bottom-left edge, uses the overlay layer, does
not reserve compositor space, and does not request keyboard focus. This keeps
the currently focused application as the target for injected uinput key events.

To verify installation:

```sh
systemctl --user status gameease.service
journalctl --user -u gameease.service -e
```

Open a text field in another application, show the OSK, and click a key. The
text should appear in the focused application, not in GameEase.

## Controls

- South + Select toggles the OSK.
- Start + Select toggles the side menu.
- Select moves the visible OSK between the bottom and top of the screen.
- D-pad moves the selected OSK key.
- South activates the selected OSK key when the OSK is visible.
- When the side menu is open, D-pad moves the selected row and South activates it.

`gilrs` reads from `/dev/input`, so gamepad events continue to arrive even when
the overlay window has no focus.

When the OSK or side menu is visible, GameEase uses Linux evdev `EVIOCGRAB` on
the active gamepad devices so the focused game does not also receive navigation
input. The grab is released again when both overlay surfaces are hidden.

## Known Limitations

- GNOME is not supported.
- Gamescope is untested.
- The systemd unit assumes `WAYLAND_DISPLAY=wayland-1`.
- The task switcher currently supports KDE Plasma/KWin, Sway, and Hyprland.

## Troubleshooting

### `/dev/uinput` permission denied

Install the udev rule:

```sh
sudo install -Dm644 dist/99-gameease.rules /etc/udev/rules.d/99-gameease.rules
sudo udevadm control --reload
sudo udevadm trigger --subsystem-match=misc --attr-match=name=uinput
```

You can also add your user to the `input` group:

```sh
sudo usermod -aG input $USER
```

Log out and back in after changing group membership.

### Game still receives gamepad input while the overlay is visible

Exclusive gamepad mode needs read access to the controller's `/dev/input/event*`
node. Add your user to the `input` group, then log out and back in:

```sh
sudo usermod -aG input $USER
```

Check logs for failed grab messages:

```sh
journalctl --user -u gameease.service -e
```

### Overlay does not appear

Confirm that you are running a wlroots-based Wayland compositor and that the
service has the right Wayland socket:

```sh
echo "$WAYLAND_DISPLAY"
systemctl --user edit gameease.service
systemctl --user restart gameease.service
```

Check logs:

```sh
journalctl --user -u gameease.service -e
```

If the linker reports undefined `gtk_layer_*` symbols while building, force Cargo
to rerun the native layer-shell build script:

```sh
cargo clean -p gtk4-layer-shell-sys
cargo build
```
