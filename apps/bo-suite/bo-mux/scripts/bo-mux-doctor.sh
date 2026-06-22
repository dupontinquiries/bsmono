#!/usr/bin/env bash
# bo-mux-doctor.sh — diagnose a bo-mux install. GUI-launchable.
#
# Runs a series of checks and reports each as:
#   CHECK <name> PASS  <detail>
#   CHECK <name> WARN  <detail>
#   CHECK <name> FAIL  <detail>
# Ends with:
#   SUMMARY <pass> pass, <warn> warn, <fail> fail
#
# With --json it instead emits a single JSON object the GUI can parse:
#   {"checks":[{"name":..,"status":"pass|warn|fail","detail":..}],"summary":{...}}
#
# Exit code: 0 if no FAILs, 1 if any FAIL.

set -uo pipefail

JSON=0
[ "${1:-}" = "--json" ] && JSON=1

BIN="$HOME/.local/bin/bo-muxd"
UNIT_DIR="$HOME/.config/systemd/user"
DATA_DIR="$HOME/.local/share/bo-mux"
STAMP="$DATA_DIR/version"
RUNTIME="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
SOCKET="$RUNTIME/bo-mux/control.sock"

pass=0; warn=0; fail=0
declare -a ROWS

record() { # name status detail
  local name="$1" status="$2" detail="$3"
  case "$status" in
    pass) pass=$((pass+1)) ;;
    warn) warn=$((warn+1)) ;;
    fail) fail=$((fail+1)) ;;
  esac
  ROWS+=("$name|$status|$detail")
  [ "$JSON" -eq 0 ] && printf "CHECK %-16s %-4s  %s\n" "$name" "$(echo "$status" | tr a-z A-Z)" "$detail"
}

# 1. binary present + executable
if [ -x "$BIN" ]; then record "binary" pass "$BIN"
elif [ -f "$BIN" ]; then record "binary" warn "present but not executable: $BIN"
else record "binary" fail "not installed ($BIN)"; fi

# 2. version stamp
if [ -f "$STAMP" ]; then record "version" pass "$(cat "$STAMP")"
else record "version" warn "no version stamp"; fi

# 3. unit files
if [ -f "$UNIT_DIR/bo-mux.socket" ] && [ -f "$UNIT_DIR/bo-mux.service" ]; then
  record "units" pass "socket + service present"
else
  record "units" fail "missing unit files in $UNIT_DIR"
fi

# 4. systemd --user reachable
if systemctl --user show-environment >/dev/null 2>&1; then
  record "systemd" pass "user instance responding"
else
  record "systemd" warn "user instance not responding (try: loginctl enable-linger $(id -un))"
fi

# 5. socket unit enabled
if systemctl --user is-enabled --quiet bo-mux.socket 2>/dev/null; then
  record "socket-unit" pass "enabled"
else
  record "socket-unit" warn "not enabled (run install or: systemctl --user enable --now bo-mux.socket)"
fi

# 6. control socket present
if [ -S "$SOCKET" ]; then record "socket" pass "$SOCKET"
else record "socket" warn "not present yet (created on first connection)"; fi

# 7. liveness ping (if socat/nc available)
PINGED="skipped (no socat/nc)"
if [ -S "$SOCKET" ]; then
  if command -v socat >/dev/null 2>&1; then
    REPLY=$(printf '{"id":1,"cmd":"ping"}\n' | timeout 2 socat - "UNIX-CONNECT:$SOCKET" 2>/dev/null | head -1)
    [ -n "$REPLY" ] && PINGED="ok: $REPLY" || PINGED="no reply"
  elif command -v nc >/dev/null 2>&1; then
    REPLY=$(printf '{"id":1,"cmd":"ping"}\n' | timeout 2 nc -U "$SOCKET" 2>/dev/null | head -1)
    [ -n "$REPLY" ] && PINGED="ok: $REPLY" || PINGED="no reply"
  fi
fi
case "$PINGED" in
  ok:*) record "daemon-ping" pass "${PINGED#ok: }" ;;
  skipped*) record "daemon-ping" warn "$PINGED" ;;
  *) record "daemon-ping" warn "$PINGED" ;;
esac

# 8. music dir
MUSIC="${BO_MUSIC_DIR:-$HOME/Music}"
if [ -d "${MUSIC%%:*}" ]; then record "music-dir" pass "${MUSIC}"
else record "music-dir" warn "not found: ${MUSIC} (set BO_MUSIC_DIR)"; fi

# 9. system audio player (for actual sound output)
PLAYER=""
for p in paplay ffplay aplay; do
  if command -v "$p" >/dev/null 2>&1; then PLAYER="$p"; break; fi
done
if [ -n "$PLAYER" ]; then record "audio-player" pass "$PLAYER"
else record "audio-player" warn "none found — install pipewire-utils/pulseaudio-utils or ffmpeg for sound"; fi

if [ "$JSON" -eq 1 ]; then
  printf '{"checks":['
  first=1
  for row in "${ROWS[@]}"; do
    IFS='|' read -r n s d <<< "$row"
    [ $first -eq 0 ] && printf ','
    # escape double-quotes in detail
    d=${d//\"/\\\"}
    printf '{"name":"%s","status":"%s","detail":"%s"}' "$n" "$s" "$d"
    first=0
  done
  printf '],"summary":{"pass":%d,"warn":%d,"fail":%d}}\n' "$pass" "$warn" "$fail"
else
  echo "SUMMARY $pass pass, $warn warn, $fail fail"
fi

[ "$fail" -eq 0 ]
