#!/usr/bin/env bash
# bo-mux-install.sh — install/upgrade the bo-mux daemon + systemd user units.
#
# GUI-launchable: prints structured, line-oriented progress so a GUI can
# stream and parse it. Every significant line is prefixed with a tag:
#   STEP  <text>   a step started
#   OK    <text>   a step succeeded
#   WARN  <text>   non-fatal problem
#   ERR   <text>   fatal problem (script exits non-zero)
#   DONE  <text>   final success line
#
# Usage:
#   bo-mux-install.sh --payload <dir-with-bo-muxd+units> [--version X.Y.Z]
#
# The payload dir must contain: bo-muxd, bo-mux.socket, bo-mux.service
#
# Exit code 0 = success, non-zero = failure (GUI should surface ERR lines).

set -uo pipefail

PAYLOAD=""
VERSION="unknown"
while [ $# -gt 0 ]; do
  case "$1" in
    --payload) PAYLOAD="${2:-}"; shift 2 ;;
    --version) VERSION="${2:-}"; shift 2 ;;
    *) echo "WARN  ignoring unknown arg: $1"; shift ;;
  esac
done

fail() { echo "ERR   $*"; exit 1; }

BIN_DST="$HOME/.local/bin/bo-muxd"
UNIT_DIR="$HOME/.config/systemd/user"
DATA_DIR="$HOME/.local/share/bo-mux"
STAMP="$DATA_DIR/version"

echo "STEP  validating payload"
[ -n "$PAYLOAD" ] || fail "no --payload given"
[ -d "$PAYLOAD" ] || fail "payload dir not found: $PAYLOAD"
for f in bo-muxd bo-mux.socket bo-mux.service; do
  [ -f "$PAYLOAD/$f" ] || fail "payload missing $f"
done
echo "OK    payload has bo-muxd + unit files"

echo "STEP  checking systemd --user availability"
if ! command -v systemctl >/dev/null 2>&1; then
  fail "systemctl not found — systemd is required"
fi
if ! systemctl --user show-environment >/dev/null 2>&1; then
  echo "WARN  systemd --user not responding (no session bus?). Files will be"
  echo "WARN  installed but you may need: loginctl enable-linger $(id -un)"
fi
echo "OK    systemd present"

echo "STEP  creating directories"
mkdir -p "$(dirname "$BIN_DST")" "$UNIT_DIR" "$DATA_DIR" || fail "mkdir failed"
echo "OK    directories ready"

# Stop running units before swapping the binary (ignore if not yet present).
if systemctl --user is-active --quiet bo-mux.service 2>/dev/null; then
  echo "STEP  stopping running daemon for upgrade"
  systemctl --user stop bo-mux.service 2>/dev/null || true
  systemctl --user stop bo-mux.socket 2>/dev/null || true
  echo "OK    stopped"
fi

echo "STEP  installing binary"
install -m 0755 "$PAYLOAD/bo-muxd" "$BIN_DST.tmp" || fail "copy binary failed"
mv -f "$BIN_DST.tmp" "$BIN_DST" || fail "rename binary failed"
echo "OK    installed bo-muxd → $BIN_DST"

echo "STEP  installing unit files"
install -m 0644 "$PAYLOAD/bo-mux.socket"  "$UNIT_DIR/bo-mux.socket"  || fail "copy socket unit failed"
install -m 0644 "$PAYLOAD/bo-mux.service" "$UNIT_DIR/bo-mux.service" || fail "copy service unit failed"
echo "OK    installed unit files → $UNIT_DIR"

echo "STEP  writing version stamp"
echo "$VERSION" > "$STAMP" || fail "write version stamp failed"
echo "OK    version $VERSION"

echo "STEP  reloading + enabling socket"
systemctl --user daemon-reload 2>/dev/null || echo "WARN  daemon-reload failed"
if systemctl --user enable --now bo-mux.socket 2>/dev/null; then
  echo "OK    bo-mux.socket enabled + started"
else
  echo "WARN  could not enable socket (systemd --user may be unavailable)"
fi

echo "DONE  bo-mux $VERSION installed"
