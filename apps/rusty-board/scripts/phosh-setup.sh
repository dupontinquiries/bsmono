#!/usr/bin/env bash
# ============================================================
# RustyBoard — Phosh / postmarketOS registration script.
#
# The counterpart to `plasma-setup.sh`. Plasma picks an input
# method by NAME in kwinrc; Phosh does not pick one at all —
# whichever client grabs the seat's `zwp_input_method_v2` first
# owns it, and there is exactly one per seat.
#
# That single fact is the whole reason this script exists, and
# why it is mostly about stopping something rather than starting
# it:
#
#   1. Disable squeekboard, which ships enabled on essentially
#      every Phosh image and takes the input method at login.
#      While it holds the seat, RustyBoard's `get_input_method`
#      returns an object that is immediately `unavailable` — it
#      never receives an Activate, so it never knows a text field
#      was focused, and (before the focus_policy work) it stayed
#      invisible with nothing in the UI to say why.
#
#      This is the "installs fine, never pops up on some
#      postmarketOS builds" report. It is not a RustyBoard bug and
#      no amount of reinstalling fixes it; the other keyboard has
#      to let go of the seat.
#
#   2. Install a `.desktop` file so RustyBoard is launchable and
#      identifiable, and (optionally) a systemd user unit so it
#      starts with the session the way squeekboard did.
#
#   3. Report what actually holds the input method afterwards, so
#      a failure is diagnosable on the device without a debugger.
#
# NOT a Plasma script: it does not write kwinrc, and it does not
# set X-KDE-Wayland-VirtualKeyboard, because phoc reads neither.
# Running the wrong one of the two is a no-op rather than damage,
# but it will not help.
#
# Idempotent — safe to re-run. Use `--undo` to give the seat back
# to squeekboard.
# ============================================================
set -euo pipefail

BIN_PATH="${BIN_PATH:-/usr/local/bin/rusty-board}"
DESKTOP_NAME="io.rustyboard.Keyboard.desktop"
DESKTOP_DIR="${DESKTOP_DIR:-/usr/share/applications}"
UNIT_DIR="${UNIT_DIR:-$HOME/.config/systemd/user}"
UNIT_NAME="rusty-board.service"

say()  { printf '  %s\n' "$*"; }
ok()   { printf '  \033[32m✓\033[0m %s\n' "$*"; }
warn() { printf '  \033[33m!\033[0m %s\n' "$*"; }
bad()  { printf '  \033[31m✗\033[0m %s\n' "$*"; }

# ------------------------------------------------------------
# Is this actually a Phosh session?
#
# Checked rather than assumed, because the failure mode of
# running this on Plasma is subtle: it would disable a
# squeekboard that is not running, install a unit nothing
# starts, and report success while the real problem (kwinrc)
# went untouched.
# ------------------------------------------------------------
detect_session() {
  local d="${XDG_CURRENT_DESKTOP:-}${DESKTOP_SESSION:-}"
  if printf '%s' "$d" | grep -qi 'phosh'; then
    echo phosh; return
  fi
  if pgrep -x phoc >/dev/null 2>&1; then
    echo phosh; return
  fi
  if printf '%s' "$d" | grep -qi 'plasma\|kde'; then
    echo plasma; return
  fi
  echo unknown
}

# ------------------------------------------------------------
# Who currently owns the seat's input method?
#
# There is no protocol call to ask this, so the answer is
# inferred from what is running. squeekboard is the near-universal
# case; the check is a list rather than a single name so a device
# running something else still gets a useful report instead of a
# confident wrong one.
# ------------------------------------------------------------
KNOWN_OSKS="squeekboard wvkbd-mobintl wvkbd svkbd maliit-keyboard"

running_osks() {
  local found=""
  for o in $KNOWN_OSKS; do
    if pgrep -x "$o" >/dev/null 2>&1; then
      found="$found $o"
    fi
  done
  printf '%s' "${found# }"
}

undo() {
  echo "RustyBoard — handing the input method back to squeekboard"
  systemctl --user disable --now "$UNIT_NAME" 2>/dev/null || true
  systemctl --user unmask squeekboard.service 2>/dev/null || true
  systemctl --user enable --now squeekboard.service 2>/dev/null \
    && ok "squeekboard re-enabled" \
    || warn "could not re-enable squeekboard via systemd — start it manually"
  gsettings set sm.puri.phosh osk-enabled true 2>/dev/null || true
  say "Log out and back in for the seat to be reclaimed."
  exit 0
}

[ "${1:-}" = "--undo" ] && undo

echo "RustyBoard — Phosh / postmarketOS setup"
echo

SESSION="$(detect_session)"
case "$SESSION" in
  phosh)
    ok "Phosh session detected"
    ;;
  plasma)
    bad "This looks like a Plasma session, not Phosh."
    say "Run scripts/plasma-setup.sh instead — it writes kwinrc, which is"
    say "what Plasma reads. This script would do nothing useful here."
    exit 1
    ;;
  unknown)
    warn "Could not identify the session (XDG_CURRENT_DESKTOP is"
    warn "'${XDG_CURRENT_DESKTOP:-unset}' and phoc is not running)."
    warn "Continuing anyway — the steps below are harmless if wrong."
    ;;
esac

# ---- 1. the seat ------------------------------------------------------
echo
echo "[1/3] Input method seat"
HELD="$(running_osks)"
if [ -n "$HELD" ]; then
  warn "another on-screen keyboard is running:$HELD"
  say  "zwp_input_method_v2 allows ONE client per seat. While that one"
  say  "holds it, RustyBoard is handed a dead object and never learns a"
  say  "text field was focused."
  for o in $HELD; do
    if systemctl --user disable --now "$o.service" 2>/dev/null; then
      ok "disabled $o.service"
    elif pkill -x "$o" 2>/dev/null; then
      ok "stopped $o (no systemd unit found — it may come back at login)"
    else
      bad "could not stop $o — stop it by hand, then re-run"
    fi
  done
  # Phosh's own switch, independent of the systemd unit: with this true
  # the shell will restart squeekboard even after it is stopped.
  if command -v gsettings >/dev/null 2>&1; then
    gsettings set sm.puri.phosh osk-enabled false 2>/dev/null \
      && ok "sm.puri.phosh osk-enabled = false" \
      || warn "could not set sm.puri.phosh osk-enabled (schema not installed?)"
  fi
else
  ok "no other on-screen keyboard is running"
fi

# ---- 2. desktop entry -------------------------------------------------
echo
echo "[2/3] Desktop entry"
if [ ! -x "$BIN_PATH" ]; then
  bad "no executable at $BIN_PATH"
  say "Install first (scripts/install.sh), or set BIN_PATH=... and re-run."
  exit 1
fi
if [ -w "$DESKTOP_DIR" ] || [ "$(id -u)" = 0 ]; then
  cat > "$DESKTOP_DIR/$DESKTOP_NAME" <<DESKTOP
[Desktop Entry]
Type=Application
Name=RustyBoard
Comment=On-screen keyboard
Exec=$BIN_PATH
Icon=input-keyboard
Terminal=false
NoDisplay=true
Categories=Utility;
DESKTOP
  ok "wrote $DESKTOP_DIR/$DESKTOP_NAME"
else
  warn "$DESKTOP_DIR is not writable — re-run with sudo for the desktop entry"
fi

# ---- 3. autostart -----------------------------------------------------
echo
echo "[3/3] Session autostart"
mkdir -p "$UNIT_DIR"
cat > "$UNIT_DIR/$UNIT_NAME" <<UNIT
[Unit]
Description=RustyBoard on-screen keyboard
# The compositor must own the seat before we ask it for the input
# method; starting earlier means binding against nothing.
PartOf=graphical-session.target
After=graphical-session.target

[Service]
Type=simple
ExecStart=$BIN_PATH
Restart=on-failure
RestartSec=2

[Install]
WantedBy=graphical-session.target
UNIT
systemctl --user daemon-reload 2>/dev/null || true
if systemctl --user enable "$UNIT_NAME" 2>/dev/null; then
  ok "enabled $UNIT_NAME (starts with the session)"
else
  warn "could not enable the unit — start RustyBoard by hand for now"
fi

# ---- report -----------------------------------------------------------
echo
echo "Result"
STILL="$(running_osks)"
if [ -n "$STILL" ]; then
  bad "still running:$STILL"
  say "Log out and back in — some are respawned by the shell rather than"
  say "by systemd, and will keep the seat until the session restarts."
else
  ok "the input method seat is free"
fi
echo
say "Log out and back in, then focus a text field."
say "If RustyBoard still does not appear, it will now SAY why rather than"
say "staying blank: the status line reports 'another on-screen keyboard"
say "owns the input method' when it was handed a dead object."
say ""
say "Escape hatch, if the compositor never signals focus at all:"
say "  set behavior.auto_hide_on_no_focus = false in the config, and"
say "  RustyBoard stays on screen regardless."
say ""
say "To undo all of this and give the seat back:  $0 --undo"
