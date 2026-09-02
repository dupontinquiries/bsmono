#!/usr/bin/env bash
# ============================================================
# RustyBoard — Plasma / Plasma Mobile registration script.
#
# Run this ONCE per device after installing the binary. It:
#
#   1. Drops a `.desktop` file at /usr/share/applications that
#      includes `X-KDE-Wayland-VirtualKeyboard=true`. This is
#      what makes RustyBoard show up in:
#        SystemSettings → Keyboard → Virtual Keyboard (desktop)
#        SystemSettings → Virtual Keyboard           (mobile)
#
#   2. Writes the kwinrc setting that selects RustyBoard as the
#      active input method. Plasma desktop uses ~/.config/kwinrc;
#      Plasma Mobile uses ~/.config/plasma-mobile/kwinrc.
#
#   3. Pokes KWin via D-Bus so it reloads the [Wayland] config
#      group without a full session restart.
#
# After this, focusing a text field should summon RustyBoard.
# If it doesn't, the redeploy script (`scripts/redeploy.sh`)
# will kick KWin harder and dump the log for diagnosis.
#
# Idempotent — safe to re-run.
# ============================================================
set -euo pipefail

BIN_PATH="${BIN_PATH:-/usr/local/bin/rusty-board}"
DESKTOP_NAME="io.rustyboard.Keyboard.desktop"
DESKTOP_DIR="/usr/share/applications"
DESKTOP_FILE="$DESKTOP_DIR/$DESKTOP_NAME"

# Determine which kwinrc to write. Plasma Mobile sessions read
# from ~/.config/plasma-mobile/kwinrc *first*, so we write
# there if it exists; otherwise we write ~/.config/kwinrc.
if [[ -d "$HOME/.config/plasma-mobile" ]]; then
    KWINRC="$HOME/.config/plasma-mobile/kwinrc"
    echo "==> Detected Plasma Mobile profile (using $KWINRC)"
else
    KWINRC="$HOME/.config/kwinrc"
    echo "==> Plasma desktop profile (using $KWINRC)"
fi

# ---- 1. Sanity: binary exists --------------------------------
if [[ ! -x "$BIN_PATH" ]]; then
    echo "ERROR: $BIN_PATH does not exist or is not executable." >&2
    echo "       Build and install first:" >&2
    echo "           cargo b --release && sudo install -m755 target/release/rusty-board /usr/local/bin/" >&2
    exit 1
fi

# ---- 2. Write the virtual-keyboard .desktop file ------------
echo "==> Writing $DESKTOP_FILE"

# Detect any user-level .desktop file that would shadow ours.
# XDG search order: $HOME/.local/share/applications/* takes
# precedence over /usr/share/applications/*. A stale user-level
# .desktop file without `X-KDE-Wayland-VirtualKeyboard=true` will
# silently hide rusty-board from SystemSettings even after we
# write the correct system-level file. Detect and offer to remove.
USER_DESKTOP="$HOME/.local/share/applications/$DESKTOP_NAME"
if [[ -f "$USER_DESKTOP" ]]; then
    if ! grep -q "^X-KDE-Wayland-VirtualKeyboard=true" "$USER_DESKTOP"; then
        echo
        echo "WARNING: $USER_DESKTOP exists and lacks the X-KDE-Wayland-VirtualKeyboard line."
        echo "         This file will shadow $DESKTOP_FILE in KWin's lookup,"
        echo "         keeping RustyBoard out of SystemSettings → Virtual Keyboard."
        echo
        read -r -p "Remove the shadowing user-level file? [Y/n] " yn
        case "$yn" in
            [Nn]*) echo "Leaving $USER_DESKTOP in place; SystemSettings may not show RustyBoard." ;;
            *)     rm -f "$USER_DESKTOP" && echo "Removed $USER_DESKTOP" ;;
        esac
    fi
fi

sudo tee "$DESKTOP_FILE" >/dev/null <<EOF
[Desktop Entry]
Type=Application
Name=RustyBoard
GenericName=On-screen Keyboard
Comment=Modern Rust-based on-screen keyboard with swipe and voice input
Exec=$BIN_PATH
Icon=input-keyboard-virtual
Terminal=false
NoDisplay=true
Categories=Utility;Accessibility;
X-KDE-Wayland-VirtualKeyboard=true
StartupWMClass=io.rustyboard.Keyboard
EOF

# Update the desktop database so KWin sees it (best effort).
if command -v update-desktop-database >/dev/null 2>&1; then
    sudo update-desktop-database "$DESKTOP_DIR" 2>/dev/null || true
fi

# ---- 3. Select RustyBoard as the active virtual keyboard ----
# The kwinrc [Wayland] InputMethod key points at the *path*
# to a .desktop file. KWin will exec the file's `Exec=` line
# on a privileged Wayland socket (with WAYLAND_SOCKET set).
echo "==> Writing $KWINRC ([Wayland] InputMethod=$DESKTOP_FILE)"
mkdir -p "$(dirname "$KWINRC")"
if command -v kwriteconfig6 >/dev/null 2>&1; then
    kwriteconfig6 --file "$KWINRC" --group Wayland --key InputMethod "$DESKTOP_FILE"
elif command -v kwriteconfig5 >/dev/null 2>&1; then
    kwriteconfig5 --file "$KWINRC" --group Wayland --key InputMethod "$DESKTOP_FILE"
else
    # Manual fallback if neither kwriteconfig is present.
    # Strips any prior InputMethod= line from the [Wayland]
    # group and rewrites it.
    python3 - "$KWINRC" "$DESKTOP_FILE" <<'PY'
import sys, configparser, os
path, value = sys.argv[1], sys.argv[2]
cp = configparser.ConfigParser(strict=False)
cp.optionxform = str  # preserve case
if os.path.exists(path):
    cp.read(path)
if "Wayland" not in cp:
    cp["Wayland"] = {}
cp["Wayland"]["InputMethod"] = value
with open(path, "w") as f:
    cp.write(f, space_around_delimiters=False)
PY
fi

# ---- 4. Poke KWin to reload config --------------------------
# Telling KWin to re-read kwinrc without a session restart.
# We send TWO signals because they're not equivalent:
#
#   1. `qdbus org.kde.KWin /KWin reconfigure` — tells KWin to
#      re-read its config in general. Works for window-manager
#      settings, but EMPIRICALLY does NOT reliably pick up the
#      Wayland/InputMethod change. See
#      https://unnamed.website/posts/switch-virtual-keyboard/
#      for the writeup that led us to add #2.
#
#   2. `busctl emit ConfigChanged` — tells KWin specifically that
#      the Wayland/InputMethod key changed. This is what Plasma's
#      System Settings emits internally when you pick a different
#      virtual keyboard in the GUI; it's what actually gets KWin
#      to switch input methods. The weird ASCII byte array spells
#      "InputMethod" — that's just how the dbus type signature
#      "a{saay}" wants it.
#
# Both calls are best-effort — if KWin isn't running or dbus isn't
# reachable, we just continue (the changes still got written to
# the config file and will take effect on next session start).
# ---- 3b. KWin window rule for the egui (v2) keyboard ---------
# The GTK build docks itself with gtk4-layer-shell. The egui build runs
# on eframe/winit, which can only make an ordinary toplevel — so the
# three things a keyboard window needs (never take focus, stay above,
# sit at the bottom edge) come from a KWin rule matched on its app id
# instead. Without the rule, clicking a key focuses the keyboard window
# and the virtual-keyboard protocol types into the keyboard itself.
#
# Written with kwriteconfig6 into ~/.config/kwinrulesrc, keyed by a
# stable rule id so re-running replaces rather than duplicates. The
# position/size are forced from the primary output's geometry when
# kscreen-doctor can report it (logical pixels, which is what rules
# use); otherwise the rule still applies focus/above/border and the
# window lands wherever KWin puts it.
RULE_ID="rusty-board-egui-keyboard"
APP_ID="org.rustyboard.RustyBoard"    # must equal demo_app::APP_ID
KB_HEIGHT="${RUSTY_BOARD_HEIGHT:-340}"
KWINRULES="${XDG_CONFIG_HOME:-$HOME/.config}/kwinrulesrc"
write_kwin_rule() {
    local kw
    if command -v kwriteconfig6 >/dev/null 2>&1; then kw=kwriteconfig6
    elif command -v kwriteconfig5 >/dev/null 2>&1; then kw=kwriteconfig5
    else
        echo "NOTE: no kwriteconfig; skipping the KWin window rule."
        echo "      Add one by hand in SystemSettings → Window Management → Window Rules:"
        echo "      match window class '$APP_ID' exactly; force: accept focus = no,"
        echo "      keep above = yes, no titlebar = yes, skip taskbar/switcher/pager = yes."
        return 0
    fi
    echo "==> Writing KWin window rule '$RULE_ID' for $APP_ID ($KWINRULES)"
    local g="--file $KWINRULES --group $RULE_ID --key"
    $kw $g Description "RustyBoard on-screen keyboard (egui): no focus, on top, bottom edge"
    $kw $g wmclass "$APP_ID"
    $kw $g wmclassmatch 1          # exact
    $kw $g wmclasscomplete false
    $kw $g acceptfocus false;      $kw $g acceptfocusrule 2       # 2 = force
    $kw $g above true;             $kw $g aboverule 2
    $kw $g noborder true;          $kw $g noborderrule 2
    $kw $g skiptaskbar true;       $kw $g skiptaskbarrule 2
    $kw $g skipswitcher true;      $kw $g skipswitcherrule 2
    $kw $g skippager true;         $kw $g skippagerrule 2
    $kw $g types 1                 # normal windows only
    # Geometry from the primary output, if we can read it. kscreen-doctor
    # prints e.g. "Geometry: 0,0 1080x2340" per output.
    local geo
    geo=$(kscreen-doctor -o 2>/dev/null | grep -m1 -oE 'Geometry: [0-9]+,[0-9]+ [0-9]+x[0-9]+' || true)
    if [[ -n "$geo" ]]; then
        local ox oy ow oh
        read -r ox oy ow oh < <(echo "$geo" | sed -E 's/Geometry: ([0-9]+),([0-9]+) ([0-9]+)x([0-9]+)/\1 \2 \3 \4/')
        $kw $g position "$ox,$(( oy + oh - KB_HEIGHT ))"; $kw $g positionrule 2
        $kw $g size "${ow}x${KB_HEIGHT}";                 $kw $g sizerule 2
        echo "    forced geometry ${ow}x${KB_HEIGHT} at $ox,$(( oy + oh - KB_HEIGHT )) (output ${ow}x${oh})"
    else
        echo "    (kscreen-doctor unavailable — rule written without a forced position/size)"
    fi
    # Register the rule id in [General] rules= (a comma-separated list) if absent.
    local kr rules
    kr=${kw/kwrite/kread}
    rules=$($kr --file "$KWINRULES" --group General --key rules 2>/dev/null || true)
    case ",$rules," in
        *",$RULE_ID,"*) ;;
        *)  if [[ -n "$rules" ]]; then rules="$rules,$RULE_ID"; else rules="$RULE_ID"; fi
            $kw --file "$KWINRULES" --group General --key rules "$rules"
            $kw --file "$KWINRULES" --group General --key count "$(echo "$rules" | tr ',' '\n' | grep -c .)"
            ;;
    esac
}
write_kwin_rule

echo "==> Telling KWin to reconfigure"
if command -v qdbus6 >/dev/null 2>&1; then
    qdbus6 org.kde.KWin /KWin reconfigure 2>/dev/null || true
elif command -v qdbus >/dev/null 2>&1; then
    qdbus org.kde.KWin /KWin reconfigure 2>/dev/null || true
fi

if command -v busctl >/dev/null 2>&1; then
    # Emit ConfigChanged on /kwinrc telling KWin the
    # Wayland/InputMethod key changed. Bytes 73 110 112 117 116 77
    # 101 116 104 111 100 spell "InputMethod" in ASCII.
    busctl --user emit /kwinrc org.kde.kconfig.notify ConfigChanged \
        "a{saay}" 1 Wayland 1 11 73 110 112 117 116 77 101 116 104 111 100 \
        2>/dev/null || true
fi

# ---- 5. Done. -----------------------------------------------
cat <<EOF

==> Plasma setup complete.

RustyBoard is now registered as a virtual keyboard. To verify:

  - On Plasma desktop:
      SystemSettings → Keyboard → Virtual Keyboard
      (RustyBoard should be listed and selected)

  - On Plasma Mobile:
      SystemSettings → Virtual Keyboard

  - From the command line:
      grep -A1 '\\[Wayland\\]' "$KWINRC"

  - Smoke test: open any app with a text field. Tapping it
    should summon RustyBoard. If nothing happens, check:
      cat /run/user/\$(id -u)/rusty-board.log | head -40

If you need to switch back to the system default (e.g. Maliit):
  kwriteconfig6 --file "$KWINRC" --group Wayland --key InputMethod \\
    /usr/share/applications/com.github.maliit.keyboard.desktop
  qdbus6 org.kde.KWin /KWin reconfigure
EOF
