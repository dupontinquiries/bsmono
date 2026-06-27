#!/usr/bin/env bash
# ============================================================
# RustyBoard — redeploy.sh
#
# The build-install-restart cycle bundled into one command.
# Use this every time you change source code on the device.
#
# Steps:
#   1. cargo build --release
#   2. sudo install -m755 target/release/rusty-board /usr/local/bin/
#   3. pkill -f rusty-board   (stop the running instance)
#   4. qdbus6 org.kde.KWin /KWin reconfigure
#      (kwin re-execs the configured virtual keyboard binary,
#       which now points at our freshly installed one)
#   5. tail the log so you can see startup / errors
#
# If you only changed config or resources (no Rust source),
# you can skip the build step with --skip-build.
# ============================================================
set -euo pipefail

SKIP_BUILD=0
WAIT_SECS=3
LOG_LINES=40

usage() {
    cat <<EOF
Usage: $0 [options]

Options:
  --skip-build       Skip cargo build; reinstall existing binary
  --wait N           Seconds to wait for KWin to re-spawn (default: 3)
  --log-lines N      How many lines of the log to show (default: 40)
  -h, --help         Show this help
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --skip-build)   SKIP_BUILD=1; shift ;;
        --wait)         WAIT_SECS="$2"; shift 2 ;;
        --log-lines)    LOG_LINES="$2"; shift 2 ;;
        -h|--help)      usage; exit 0 ;;
        *)              echo "Unknown option: $1" >&2; usage; exit 2 ;;
    esac
done

# ---- 1. Build -----------------------------------------------
if [[ "$SKIP_BUILD" -eq 0 ]]; then
    echo "==> cargo build --release"
    cargo build --release
fi

BIN_SRC="target/release/rusty-board"
if [[ ! -x "$BIN_SRC" ]]; then
    echo "ERROR: $BIN_SRC not found. Did the build fail?" >&2
    exit 1
fi

# ---- 2. Install --------------------------------------------
# Determine where to install based on the current .desktop file's
# Exec= line. install.sh may have put the binary at $HOME/.local/bin
# (user-install) or /usr/local/bin (system-install). Reusing the
# same path avoids divergence — if we installed to a different path,
# KWin would still launch the stale binary at the path the .desktop
# Exec= line points to.
DESKTOP_FILE="/usr/share/applications/io.rustyboard.Keyboard.desktop"
USER_DESKTOP_FILE="$HOME/.local/share/applications/io.rustyboard.Keyboard.desktop"

INSTALL_PATH="/usr/local/bin/rusty-board"
DETECT_FROM=""
for candidate in "$USER_DESKTOP_FILE" "$DESKTOP_FILE"; do
    if [[ -f "$candidate" ]]; then
        EXEC_LINE=$(grep '^Exec=' "$candidate" | head -1 | sed 's/^Exec=//')
        if [[ -n "$EXEC_LINE" ]]; then
            INSTALL_PATH="$EXEC_LINE"
            DETECT_FROM="$candidate"
            break
        fi
    fi
done

if [[ -n "$DETECT_FROM" ]]; then
    echo "==> Installing to $INSTALL_PATH (detected from $DETECT_FROM)"
else
    echo "==> Installing to $INSTALL_PATH (default — no .desktop file found)"
fi

# Use sudo only when writing to a system path.
if [[ "$INSTALL_PATH" == /usr/* ]] || [[ "$INSTALL_PATH" == /opt/* ]]; then
    sudo install -m755 "$BIN_SRC" "$INSTALL_PATH"
else
    install -m755 "$BIN_SRC" "$INSTALL_PATH"
fi

# ---- 3. Stop running instance ------------------------------
# Soft kill first; if the process is gone or already exiting,
# pkill returns non-zero — that's fine, ignore.
echo "==> Stopping running instance"
pkill -f "$INSTALL_PATH" 2>/dev/null || true
sleep 1

# ---- 4. Tell KWin to reconfigure ----------------------------
# KWin will re-launch the configured input method (us) the next
# time a text field gets focus, picking up the new binary.
echo "==> Asking KWin to reconfigure"
if command -v qdbus6 >/dev/null 2>&1; then
    qdbus6 org.kde.KWin /KWin reconfigure
elif command -v qdbus >/dev/null 2>&1; then
    qdbus org.kde.KWin /KWin reconfigure
else
    echo "WARNING: no qdbus binary found; KWin may not pick up the new binary"
    echo "         until you focus a text field."
fi

echo "==> Waiting ${WAIT_SECS}s for KWin to re-spawn rusty-board..."
sleep "$WAIT_SECS"

# ---- 5. Show log --------------------------------------------
LOG_FILE="/run/user/$(id -u)/rusty-board.log"
if [[ -f "$LOG_FILE" ]]; then
    echo "==> Last ${LOG_LINES} lines of $LOG_FILE:"
    echo "----------------------------------------"
    tail -n "$LOG_LINES" "$LOG_FILE"
    echo "----------------------------------------"
else
    echo "NOTE: $LOG_FILE doesn't exist yet."
    echo "      RustyBoard is launched on-demand by KWin when a text"
    echo "      field gains focus. Open a text input to spawn it,"
    echo "      then re-run: tail -f $LOG_FILE"
fi
