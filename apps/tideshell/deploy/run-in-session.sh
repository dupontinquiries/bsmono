#!/bin/sh
# Run tideshell inside the logged-in cosmic session from an ssh shell.
#   sh run-in-session.sh [--show-strips] [extra tideshell args...]
# Foreground; Ctrl-C (or `pkill -x tideshell`) stops it and its surfaces vanish.
# Log: ~/tideshell-run.log  (engine trace + Wayland/EGL errors).
# Detached (no terminal: nohup/setsid/ssh with redirects) it writes straight to
# the log file. Through `| tee` it died when the ssh channel closed: tee lost
# its output, and tideshell's next trace line hit a broken pipe (seen on the FP5).
set -u
BIN=${TIDESHELL_BIN:-$HOME/.local/bin/tideshell}
UID_N=$(id -u)
export XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-/run/user/$UID_N}

# Borrow the session's environment from a process that runs inside it, so
# WAYLAND_DISPLAY / DBUS_SESSION_BUS_ADDRESS match what cosmic apps see.
# Process names as the kernel reports them: truncated to 15 characters, so
# cosmic-settings-daemon is "cosmic-settings" (pgrep -x cannot match more).
for name in rusty-board cosmic-panel cosmic-osk cosmic-settings cosmic-session; do
    pid=$(pgrep -u "$UID_N" -x "$name" | head -1)
    [ -n "$pid" ] || continue
    for var in WAYLAND_DISPLAY DBUS_SESSION_BUS_ADDRESS XDG_SESSION_TYPE; do
        val=$(tr '\0' '\n' < /proc/$pid/environ 2>/dev/null | sed -n "s/^$var=//p" | head -1)
        [ -n "$val" ] && export "$var=$val"
    done
    echo "session env from $name (pid $pid)"
    break
done
: "${WAYLAND_DISPLAY:=$(ls "$XDG_RUNTIME_DIR" | grep -E '^wayland-[0-9]+$' | head -1)}"
: "${DBUS_SESSION_BUS_ADDRESS:=unix:path=$XDG_RUNTIME_DIR/bus}"
export WAYLAND_DISPLAY DBUS_SESSION_BUS_ADDRESS
echo "WAYLAND_DISPLAY=$WAYLAND_DISPLAY DBUS_SESSION_BUS_ADDRESS=$DBUS_SESSION_BUS_ADDRESS"

if [ "${1:-}" = "--show-strips" ]; then export TIDESHELL_SHOW_STRIPS=1; shift; fi
export TIDESHELL_TRACE=stderr
# The same per-phone settings file the systemd unit reads.
if [ -f "$HOME/.config/tideshell/env" ]; then set -a; . "$HOME/.config/tideshell/env"; set +a; fi
[ -x "$BIN" ] || { echo "no binary at $BIN (set TIDESHELL_BIN)"; exit 1; }
pgrep -x tideshell >/dev/null && { echo "tideshell already running (pid $(pgrep -x tideshell)); stop it first"; exit 1; }
LOG=$HOME/tideshell-run.log
echo "running $BIN $* ; log: $LOG"
if [ -t 1 ]; then
    "$BIN" "$@" 2>&1 | tee "$LOG"
else
    exec "$BIN" "$@" > "$LOG" 2>&1 < /dev/null
fi
