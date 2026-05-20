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

### Runtime packages

Install the native runtime libraries before installing GameEase.

On Arch-based systems:

```sh
sudo pacman -S gtk4 gtk4-layer-shell libpulse libxkbcommon systemd-libs networkmanager bluez
```

On Debian/Ubuntu-based systems, install the equivalent runtime packages:

```sh
sudo apt install libgtk-4-1 libgtk4-layer-shell0 libpulse0 libudev1 libxkbcommon0 network-manager bluez
```

Package names vary by distribution.

### Install from a release

Download the latest `gameease-vX.Y.Z-linux-x86_64.tar.gz` and matching
`.sha256` file from the GitHub Releases page, then verify and extract it:

```sh
sha256sum -c gameease-vX.Y.Z-linux-x86_64.tar.gz.sha256
tar -xzf gameease-vX.Y.Z-linux-x86_64.tar.gz
cd gameease-vX.Y.Z-linux-x86_64
```

Install the bundled binary, udev rule, and systemd user service:

```sh
bash dist/install.sh
```

The installer copies the bundled `gameease` binary to `/usr/local/bin/gameease`,
installs the udev rule, reloads udev, and enables `gameease.service` as a
systemd user unit.

The service defaults to:

```ini
Environment=WAYLAND_DISPLAY=wayland-1
```

Adjust `~/.config/systemd/user/gameease.service` if your compositor uses a
different Wayland socket.

### Verify the install

From a supported Wayland compositor such as KDE Plasma/KWin, Sway, or Hyprland:

```sh
systemctl --user status gameease.service
journalctl --user -u gameease.service -e
```

Open a text field in another application, show the OSK, and activate a key. The
text should appear in the focused application, not in GameEase.

To manually run the installed binary for testing:

```sh
/usr/local/bin/gameease
```

### Uninstall

From the extracted release directory:

```sh
bash dist/uninstall.sh
```

## Controls

- Select + South toggles the OSK.
- Select + North toggles the side menu.
- Select + Start toggles Desktop Mode.
- East closes the OSK and side menu.

When the OSK is visible:

- D-pad moves the selected key.
- South activates the selected key.
- North inputs Space.
- West inputs Backspace.
- Right Trigger inputs Enter.
- Left Trigger holds Shift.
- Left Stick Click toggles Caps Lock.
- Select moves the OSK between the bottom and top of the screen.

When the side menu is visible:

- D-pad Up/Down moves the selected row or item.
- D-pad Left/Right adjusts sliders such as volume and configuration values.
- South activates the selected row or item.
- East closes the active panel or side menu.
- North toggles scanning in Wi-Fi and Bluetooth panels.
- West terminates the selected app in the task switcher when supported.

When Desktop Mode is enabled:

- Right Stick moves the pointer.
- Right Trigger holds left click.
- Left Trigger holds right click.
- Left Stick scrolls.
- D-pad sends arrow keys.
- Right Stick Click sends middle click.

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
