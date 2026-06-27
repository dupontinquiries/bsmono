#!/usr/bin/env bash
# ============================================================
# RustyBoard — fix-registration.sh
#
# One-shot recovery for "rusty-board disappeared from
# SystemSettings → Virtual Keyboard" or "KWin won't launch it
# anymore." Removes the most common shadowing files and re-runs
# plasma-setup.sh from scratch.
#
# Equivalent to manually running:
#   rm ~/.local/share/applications/io.rustyboard.Keyboard.desktop
#   ./scripts/plasma-setup.sh
#
# Run after sanity-check.sh reports FAILs.
# ============================================================
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DESKTOP_NAME="io.rustyboard.Keyboard.desktop"
USER_DESKTOP="$HOME/.local/share/applications/$DESKTOP_NAME"

echo "=== RustyBoard registration recovery ==="
echo

# 1. Remove user-level .desktop file. install.sh from older builds
#    wrote one without the magic line, which then shadowed the
#    correct system-level file. Newer install.sh writes the magic
#    line, but if you ran an older install.sh recently, the file is
#    still wrong on disk. Just remove it; plasma-setup.sh writes
#    the system-level one which is what we actually want.
if [[ -f "$USER_DESKTOP" ]]; then
    echo "==> Removing user-level $USER_DESKTOP"
    rm -f "$USER_DESKTOP"
else
    echo "==> No user-level .desktop file to remove (good)."
fi

# 2. Re-run plasma-setup.sh. Writes the system-level .desktop with
#    the magic line, updates kwinrc, and sends both KWin reconfigure
#    signals (qdbus reconfigure + busctl ConfigChanged emit).
echo "==> Re-running plasma-setup.sh"
bash "$SCRIPT_DIR/plasma-setup.sh"

echo
echo "==> Done. Run ./scripts/sanity-check.sh to verify."
echo "    If kwinrc still points at the wrong keyboard, you may need"
echo "    to open SystemSettings → Virtual Keyboard and select"
echo "    RustyBoard manually — Plasma sometimes resets this on"
echo "    package updates or reboots."
