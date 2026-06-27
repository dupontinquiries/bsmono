# RustyBoard helper scripts

Quick reference for what each script does and when to run it. All
scripts are idempotent — safe to re-run.

## First-time device setup

Order matters once, after that everything is fire-and-forget.

```bash
# 1. Build and install the binary.
cargo build --release
sudo install -m755 target/release/rusty-board /usr/local/bin/

# 2. Register rusty-board as a virtual keyboard with Plasma/KWin.
#    Drops the .desktop file in /usr/share/applications/, writes
#    the kwinrc setting that selects rusty-board, and pokes KWin
#    to reload its config.
./scripts/plasma-setup.sh
```

After step 2, focusing a text field anywhere on the system should
summon RustyBoard. If it doesn't, see "Troubleshooting" below.

## Day-to-day: redeploy after a code change

```bash
./scripts/redeploy.sh
```

Equivalent to:

```bash
cargo build --release
sudo install -m755 target/release/rusty-board /usr/local/bin/
pkill -f rusty-board
qdbus6 org.kde.KWin /KWin reconfigure
sleep 3
tail -40 /run/user/$(id -u)/rusty-board.log
```

Options:
- `--skip-build` — reinstall existing binary without rebuilding
- `--wait N` — seconds to wait for KWin to re-spawn (default: 3)
- `--log-lines N` — log tail length (default: 40)

## Troubleshooting

**RustyBoard doesn't show up when tapping a text field.** Check:

```bash
# Verify the desktop file exists.
ls -l /usr/share/applications/io.rustyboard.Keyboard.desktop

# Verify kwinrc points at it.
grep -A1 '\[Wayland\]' ~/.config/kwinrc
# (or ~/.config/plasma-mobile/kwinrc on Plasma Mobile)

# Check the log.
cat /run/user/$(id -u)/rusty-board.log
```

If the log doesn't exist, KWin never launched rusty-board.
That points to either a missing desktop file, a wrong path in
kwinrc, or KWin not having reloaded its config — re-run
`plasma-setup.sh`.

**RustyBoard disappeared from SystemSettings → Virtual Keyboard
after a re-install.** A known gotcha: `install.sh` writes a
`.desktop` file to `~/.local/share/applications/`, which under
XDG search order **shadows** the system-level `.desktop` file at
`/usr/share/applications/`. If the user-level one lacks
`X-KDE-Wayland-VirtualKeyboard=true`, KWin filters us out even
though the system-level file is correct.

```bash
# See if you have a shadowing user-level .desktop file:
ls -l ~/.local/share/applications/io.rustyboard.Keyboard.desktop

# Verify it has the magic line:
grep X-KDE-Wayland-VirtualKeyboard ~/.local/share/applications/io.rustyboard.Keyboard.desktop

# If missing, either re-run install.sh from this build (which now
# writes the correct content) or just delete the user-level file:
rm ~/.local/share/applications/io.rustyboard.Keyboard.desktop
./scripts/plasma-setup.sh   # writes the correct system-level file
```

The current versions of `install.sh` and `plasma-setup.sh` both
write the correct `.desktop` content, so a fresh install
shouldn't hit this. The bug existed in earlier versions of
`install.sh` whose template lacked the virtual-keyboard line.

**Switch back to the system default (Maliit) keyboard:**

```bash
kwriteconfig6 --file ~/.config/kwinrc --group Wayland \
    --key InputMethod /usr/share/applications/com.github.maliit.keyboard.desktop
qdbus6 org.kde.KWin /KWin reconfigure
```

## Other scripts in this directory

- `install.sh` — full-featured installer with packaging,
  systemd user service, optional Whisper model download.
  Use this when packaging for distribution; for personal dev
  use, `cargo build` + `plasma-setup.sh` is faster.
- `sanity-check.sh` — diagnose registration issues. Run when
  rusty-board doesn't appear in SystemSettings, KWin isn't
  launching it, or something else feels off. Doesn't modify
  anything; just walks the registration chain and reports.
- `fix-registration.sh` — one-shot recovery. Removes any stale
  user-level .desktop file (which shadows the correct system-level
  one) and re-runs plasma-setup.sh. Use when sanity-check.sh
  reports FAILs.
- `uninstall.sh` — removes installed files.
- `download-model.sh` — fetches a Whisper model for voice
  input. Not needed for typing.
