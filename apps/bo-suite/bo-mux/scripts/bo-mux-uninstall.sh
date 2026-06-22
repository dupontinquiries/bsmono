#!/usr/bin/env bash
# bo-mux-uninstall.sh — remove the bo-mux daemon + systemd units.
# GUI-launchable; same STEP/OK/WARN/ERR/DONE tagging as the installer.
#
#   --keep-data   leave ~/.local/share/bo-mux (scenes, version) in place
#
set -uo pipefail

KEEP_DATA=0
[ "${1:-}" = "--keep-data" ] && KEEP_DATA=1

BIN="$HOME/.local/bin/bo-muxd"
UNIT_DIR="$HOME/.config/systemd/user"
DATA_DIR="$HOME/.local/share/bo-mux"

echo "STEP  stopping + disabling units"
systemctl --user stop bo-mux.service 2>/dev/null || true
systemctl --user stop bo-mux.socket 2>/dev/null || true
systemctl --user disable bo-mux.socket 2>/dev/null || true
echo "OK    units stopped + disabled"

echo "STEP  removing unit files"
rm -f "$UNIT_DIR/bo-mux.socket" "$UNIT_DIR/bo-mux.service"
systemctl --user daemon-reload 2>/dev/null || true
echo "OK    unit files removed"

echo "STEP  removing binary"
rm -f "$BIN"
echo "OK    binary removed"

if [ "$KEEP_DATA" -eq 0 ]; then
  echo "STEP  removing data dir"
  rm -rf "$DATA_DIR"
  echo "OK    data removed"
else
  echo "OK    keeping data dir ($DATA_DIR)"
fi

echo "DONE  bo-mux uninstalled"
