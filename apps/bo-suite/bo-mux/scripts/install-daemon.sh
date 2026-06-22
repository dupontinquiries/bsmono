#!/usr/bin/env bash
# scripts/install-daemon.sh
#
# Manual first-run installer for the bo-mux user daemon. The GUI normally
# does this via src/deploy.rs, but this script is the equivalent for manual
# setup / debugging / headless boxes.
#
# Installs USER-level (no root): binary → ~/.local/bin, units →
# ~/.config/systemd/user, then enables the socket.
#
# Usage:
#   ./scripts/install-daemon.sh /path/to/bo-muxd
#
set -euo pipefail

BIN_SRC="${1:?usage: install-daemon.sh /path/to/bo-muxd}"
HERE="$(cd "$(dirname "$0")/.." && pwd)"

BIN_DST="$HOME/.local/bin/bo-muxd"
UNIT_DIR="$HOME/.config/systemd/user"
DATA_DIR="$HOME/.local/share/bo-mux"

mkdir -p "$HOME/.local/bin" "$UNIT_DIR" "$DATA_DIR/scenes" "$DATA_DIR/clips"

echo "Installing binary → $BIN_DST"
install -m 0755 "$BIN_SRC" "$BIN_DST"

# Install the soundscape clip pool (pre-recorded spatial clips) if it
# ships beside the daemon. The daemon also synthesizes missing ambient
# beds via ffmpeg on first run, but the recorded brook/leaves/wind
# clips give a far richer result, so prefer them when present.
for CLIPSRC in "$HERE/../docs/scenes/clips_spatial" "$HERE/clips" "$HERE/../clips"; do
    if [ -d "$CLIPSRC" ]; then
        echo "Installing clip pool → $DATA_DIR/clips"
        cp -n "$CLIPSRC"/*.opus "$DATA_DIR/clips/" 2>/dev/null || true
        break
    fi
done

# Install bundled scene presets (the daemon also bootstraps these, but
# shipping them keeps a fresh install consistent with the binary).
if [ -d "$HERE/examples" ]; then
    echo "Installing scene presets → $DATA_DIR/scenes"
    cp -n "$HERE/examples"/*.soundscape.json "$DATA_DIR/scenes/" 2>/dev/null || true
fi

echo "Installing systemd user units → $UNIT_DIR"
install -m 0644 "$HERE/systemd/bo-mux.socket"  "$UNIT_DIR/bo-mux.socket"
install -m 0644 "$HERE/systemd/bo-mux.service" "$UNIT_DIR/bo-mux.service"

echo "Reloading systemd user daemon"
systemctl --user daemon-reload

echo "Enabling + starting the socket (daemon starts on first connection)"
systemctl --user enable --now bo-mux.socket

echo
echo "Done. Check status with:"
echo "  systemctl --user status bo-mux.socket"
echo "  echo '{\"id\":1,\"cmd\":\"ping\"}' | socat - UNIX-CONNECT:\${XDG_RUNTIME_DIR}/bo-mux/control.sock"
