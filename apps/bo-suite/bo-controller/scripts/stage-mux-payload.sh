#!/usr/bin/env bash
# scripts/stage-mux-payload.sh
#
# Bundle the bo-mux daemon payload so the GUI's "Install soundscape engine"
# button (MuxBridge::install) can find and deploy it. The bridge looks for a
# `bo-mux-payload/` dir containing `bo-muxd` + the systemd unit files,
# beside the GUI executable (and a couple of fallback locations).
#
# Usage, after building both crates:
#   ./scripts/stage-mux-payload.sh /path/to/bo-mux /path/to/controller/target/release
#
set -euo pipefail
BOMUX_DIR="${1:?usage: stage-mux-payload.sh <bo-mux-crate-dir> <controller-target-dir>}"
TARGET_DIR="${2:?usage: stage-mux-payload.sh <bo-mux-crate-dir> <controller-target-dir>}"

PAYLOAD="$TARGET_DIR/bo-mux-payload"
mkdir -p "$PAYLOAD"

# 1. The daemon binary (build it first: cargo build --release in bo-mux).
if [ -f "$BOMUX_DIR/target/release/bo-muxd" ]; then
  cp "$BOMUX_DIR/target/release/bo-muxd" "$PAYLOAD/bo-muxd"
  chmod +x "$PAYLOAD/bo-muxd"
  echo "staged bo-muxd"
else
  echo "WARNING: $BOMUX_DIR/target/release/bo-muxd not found — build bo-mux first:"
  echo "  (cd $BOMUX_DIR && cargo build --release)"
fi

# 2. The systemd user unit files.
cp "$BOMUX_DIR/systemd/bo-mux.socket"  "$PAYLOAD/bo-mux.socket"
cp "$BOMUX_DIR/systemd/bo-mux.service" "$PAYLOAD/bo-mux.service"
echo "staged unit files"

# 3. The GUI-launchable setup/diagnostics scripts.
mkdir -p "$PAYLOAD/scripts"
for s in bo-mux-install.sh bo-mux-uninstall.sh bo-mux-doctor.sh; do
  if [ -f "$BOMUX_DIR/scripts/$s" ]; then
    cp "$BOMUX_DIR/scripts/$s" "$PAYLOAD/scripts/$s"
    chmod +x "$PAYLOAD/scripts/$s"
  fi
done
echo "staged setup scripts"

echo "Payload ready at: $PAYLOAD"
echo "The GUI's Install button will deploy from here on first run."
