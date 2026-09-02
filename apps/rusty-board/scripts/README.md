# RustyBoard helper scripts

Quick reference for what each script does and when to run it. All
scripts are idempotent — safe to re-run.

## First-time device setup

Order matters once, after that everything is fire-and-forget.

```bash
# 1. Build and install the binary (the egui v2 keyboard, --features wayland).
./scripts/install.sh --system          # or: ./scripts/install.sh --gtk --system

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

Equivalent to (see `frontend.sh`, which both install.sh and redeploy.sh source
so they cannot disagree about which frontend is built or where its binary is):

```bash
( cd egui-frontend && cargo build --release --features wayland,layer )
sudo install -m755 egui-frontend/target/release/rusty-board-egui /usr/local/bin/rusty-board
pkill -f rusty-board
qdbus6 org.kde.KWin /KWin reconfigure
sleep 3
tail -40 /run/user/$(id -u)/rusty-board.log
```

Options:
- `--gtk` — build and install the GTK4 frontend instead of the egui v2 one
- `--no-wayland` — egui without the input-method backend (types into its own preview)
- `--skip-build` — reinstall existing binary without rebuilding
- `--wait N` — seconds to wait for KWin to re-spawn (default: 3)
- `--log-lines N` — log tail length (default: 40)

## The egui (v2) keyboard: docking

Built with `--features layer` (the default), the keyboard is a
`zwlr_layer_shell_v1` surface: bottom edge, exclusive zone, never focused,
mapped on Activate and unmapped on Deactivate — the same protocol the GTK build
uses through gtk4-layer-shell and Maliit through layer-shell-qt. Phosh, Sway
and KWin all honour it. `RUSTY_BOARD_LAYER=overlay` puts it above fullscreen
windows and the lock screen. If the compositor has no layer shell the binary
says so on stderr and runs as a toplevel window instead, which is where the
rule below matters.

### The KWin window rule (fallback for the toplevel window)

`plasma-setup.sh` also writes a KWin window rule (`~/.config/kwinrulesrc`, rule id
`rusty-board-egui-keyboard`, matched on the app id `org.rustyboard.RustyBoard`)
that forces: accept focus = no, keep above, no titlebar, skip taskbar/switcher/
pager, and — when `kscreen-doctor` can report the primary output — a size of
`<width>x340` at the bottom edge (`RUSTY_BOARD_HEIGHT` overrides 340). The
keyboard's status strip says which placement it believes it has
(`toplevel + window rule` on Wayland); `RUSTY_BOARD_PLACEMENT=floating`
forces the dev-mode window with decorations.

Without the rule, clicking a key gives the keyboard window focus and the
virtual-keyboard protocol then types into the keyboard itself. If keys visibly
press but nothing arrives in the app: check the rule exists, then
`qdbus6 org.kde.KWin /KWin reconfigure`.

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

- `frontend.sh` — sourced by install.sh and redeploy.sh: which
  frontend (egui v2 by default, `--gtk` for the original), how it is
  built, where its binary lands, which system deps it needs.
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
