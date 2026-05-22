# GameEase

GameEase is a Linux overlay for gamepad-first desktop control.

On Wayland it creates a `wlr-layer-shell` overlay window. On X11 it uses a
fullscreen, transparent, click-through overlay window with X11 window-manager
hints for Cinnamon and other EWMH-compatible desktops. A background `gilrs`
thread reads gamepad input independently of window focus, and the OSK injects
keys through Linux uinput.

## Target platform

- Linux
- Wayland compositor with layer-shell support, including KDE Plasma/KWin, Sway,
  and Hyprland
- X11 desktop with an EWMH-compatible window manager, including Cinnamon
- GTK4
- `wlr-layer-shell` for the Wayland backend

GNOME Wayland is not supported because it does not expose `wlr-layer-shell`.
Gamescope compatibility is untested.

## Installation

### Runtime packages

Install the native runtime libraries before installing GameEase.

On Arch-based systems:

```sh
sudo pacman -S gtk4 libpulse libxkbcommon systemd-libs networkmanager bluez
```

Install `gtk4-layer-shell` as well if you want to use the Wayland layer-shell
backend from the portable archive.

On Debian/Ubuntu-based systems, install the equivalent runtime packages:

```sh
sudo apt install libgtk-4-1 libpulse0 libudev1 libxkbcommon0 network-manager bluez
```

Package names vary by distribution. GameEase only loads `gtk4-layer-shell` when
running on Wayland, so Cinnamon/X11 does not require that library. Debian and
Linux Mint users should prefer the `.deb` release package for Wayland sessions
because it bundles the `gtk4-layer-shell` runtime library that is missing from
some standard repositories.

### Install from a release

On Debian, Ubuntu, Linux Mint, and related systems, download the latest
`gameease_X.Y.Z_amd64.deb` from the GitHub Releases page and install it:

```sh
sudo apt install ./gameease_X.Y.Z_amd64.deb
systemctl --user import-environment DISPLAY WAYLAND_DISPLAY XAUTHORITY XDG_CURRENT_DESKTOP XDG_SESSION_DESKTOP XDG_SESSION_TYPE
systemctl --user enable --now gameease.service
```

To use the AppImage, download `GameEase-vX.Y.Z-x86_64.AppImage`,
`install-appimage.sh`, and their matching `.sha256` files. Verify the downloads,
then install the AppImage, udev rule, and user service:

```sh
sha256sum -c GameEase-vX.Y.Z-x86_64.AppImage.sha256
sha256sum -c install-appimage.sh.sha256
chmod +x GameEase-vX.Y.Z-x86_64.AppImage install-appimage.sh
bash install-appimage.sh ./GameEase-vX.Y.Z-x86_64.AppImage
```

The AppImage bundles GameEase and the `gtk4-layer-shell` runtime library used by
Wayland layer-shell sessions. It still requires the native runtime packages
listed above, and the installer script is still needed for uinput permissions
and autostart. The AppImage installer checks `/dev/uinput` before starting the
service; if it reports that your user cannot write to `/dev/uinput`, add your
user to the `input` group and log out completely before trying again:

```sh
sudo usermod -aG input "$USER"
```

The installer writes a log to `~/.cache/gameease/install-appimage.log`, so you
can inspect the result even if a file-manager-launched terminal closes.

For the portable archive, download `gameease-vX.Y.Z-linux-x86_64.tar.gz` and
the matching `.sha256` file, then verify and extract it:

```sh
sha256sum -c gameease-vX.Y.Z-linux-x86_64.tar.gz.sha256
tar -xzf gameease-vX.Y.Z-linux-x86_64.tar.gz
cd gameease-vX.Y.Z-linux-x86_64
```

Install the bundled binary, udev rule, and systemd user service:

```sh
bash dist/install.sh
```

The archive installer copies the bundled `gameease` binary to
`/usr/local/bin/gameease`, installs the udev rule, reloads udev, imports the
active display environment, and enables `gameease.service` as a systemd user
unit. The portable archive expects `gtk4-layer-shell` to be available only for
Wayland layer-shell sessions; Cinnamon/X11 can run without it.

### Verify the install

From a supported session such as Cinnamon on X11, KDE Plasma/KWin, Sway, or
Hyprland:

```sh
systemctl --user status gameease.service
journalctl --user -u gameease.service -e
```

Open a text field in another application, show the OSK, and activate a key. The
text should appear in the focused application, not in GameEase.

To manually run the installed binary for testing:

```sh
gameease
```

### Uninstall

From the extracted release directory:

```sh
bash dist/uninstall.sh
```

For AppImage installs, download `uninstall-appimage.sh` from the release and run:

```sh
bash uninstall-appimage.sh
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

- Gamescope is untested.
- GNOME Wayland is not supported.
- X11 overlay stacking depends on the window manager and compositor. Cinnamon is
  supported; unusual override-redirect fullscreen windows may still draw above
  the overlay.
- The task switcher currently supports X11/EWMH, KDE Plasma/KWin, Sway, and
  Hyprland.

## Troubleshooting

### `/dev/uinput` permission denied

Install the udev rule:

```sh
sudo install -Dm644 dist/99-gameease.rules /etc/udev/rules.d/99-gameease.rules
sudo udevadm control --reload
sudo modprobe uinput
sudo udevadm trigger --subsystem-match=misc --attr-match=name=uinput
sudo udevadm settle
```

You can also add your user to the `input` group:

```sh
sudo usermod -aG input $USER
```

Log out and back in after changing group membership.

Verify that the current login session can use uinput:

```sh
test -w /dev/uinput && echo ok
```

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

Confirm that the service has the right display environment for your session:

```sh
echo "$XDG_SESSION_TYPE"
echo "$DISPLAY"
echo "$WAYLAND_DISPLAY"
systemctl --user import-environment DISPLAY WAYLAND_DISPLAY XAUTHORITY XDG_SESSION_TYPE
systemctl --user edit gameease.service
systemctl --user restart gameease.service
```

Check logs:

```sh
journalctl --user -u gameease.service -e
```
