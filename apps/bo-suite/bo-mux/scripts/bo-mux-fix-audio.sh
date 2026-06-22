#!/usr/bin/env bash
# bo-mux-fix-audio.sh
#
# Reverts bo-mux's audio state when something's gone sideways and the
# user just wants their normal output back. Non-destructive: this
# script stops bo-mux from spawning new clips, reaps lingering audio
# subprocesses, and (optionally) restarts the user-level audio
# services. It does NOT touch system services, kernel modules, or
# anything that requires sudo.
#
# Output protocol: every line is prefixed with one of:
#   STEP <msg>   — about to do something; UI shows as in-progress
#   OK   <msg>   — step succeeded
#   WARN <msg>   — non-fatal issue; continuing
#   ERR  <msg>   — fatal; script exits non-zero
#   DONE         — final line on success
# The GUI's run_script_streaming pipeline tags lines by prefix to
# colorize them in the log area.
#
# Args:
#   --restart-audio   Also restart pipewire / pipewire-pulse user
#                     services. Off by default because restarting
#                     audio is invasive (every playing app gets a
#                     brief interruption).
#
# Exit codes:
#   0  bo-muxd not running, no orphans, audio path returned to system
#   1  could not stop bo-muxd cleanly
#   2  could not reap orphans

set -uo pipefail

RESTART_AUDIO=0
for arg in "$@"; do
    case "$arg" in
        --restart-audio) RESTART_AUDIO=1 ;;
        *) echo "WARN unknown argument: $arg" ;;
    esac
done

step() { echo "STEP $*"; }
ok()   { echo "OK   $*"; }
warn() { echo "WARN $*"; }
err()  { echo "ERR  $*"; }

# ── 1. Ask the daemon to stop cleanly via its socket ────────────────
step "Asking bo-muxd to stop playback via control socket"
SOCK="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/bo-mux.sock"
if [[ -S "$SOCK" ]]; then
    if command -v nc >/dev/null 2>&1; then
        # Two requests back-to-back: stop transport, then shutdown
        # the daemon. The daemon serializes these on its read loop;
        # the socket close after our write doesn't drop responses
        # because we don't actually read them — we don't care.
        printf '{"id":1,"cmd":"stop"}\n{"id":2,"cmd":"shutdown"}\n' \
            | timeout 2 nc -U "$SOCK" -w 1 >/dev/null 2>&1 || true
        ok "sent stop+shutdown commands"
    else
        warn "nc not installed — falling through to SIGTERM"
    fi
else
    ok "socket not present, daemon likely not running"
fi

# Give the daemon a moment to exit cleanly after the shutdown command.
sleep 1

# ── 2. SIGTERM the daemon process, then SIGKILL if it ignores us ────
step "Ensuring bo-muxd process is stopped"
if pgrep -x bo-muxd >/dev/null 2>&1; then
    pkill -TERM -x bo-muxd 2>/dev/null
    # 2-second window for graceful exit.
    for i in 1 2 3 4; do
        sleep 0.5
        if ! pgrep -x bo-muxd >/dev/null 2>&1; then break; fi
    done
    if pgrep -x bo-muxd >/dev/null 2>&1; then
        warn "graceful stop ignored after 2s; sending SIGKILL"
        pkill -KILL -x bo-muxd 2>/dev/null
        sleep 0.5
        if pgrep -x bo-muxd >/dev/null 2>&1; then
            err "bo-muxd refusing to die — check process state manually"
            exit 1
        fi
    fi
    ok "bo-muxd stopped"
else
    ok "bo-muxd was not running"
fi

# ── 3. Reap orphaned audio subprocesses ─────────────────────────────
#
# When bo-muxd dies, its spawned paplay/ffplay/aplay children are
# inherited by PID 1 (systemd / init). They keep playing their
# 16-second clip until it ends naturally. Usually that's fine, but
# if the user is in a "fix it now" mood they want silence
# immediately. Identify orphans (ppid=1) whose cmdline references
# a clips/ path — bo-mux's clip directory convention — and
# terminate them gracefully.
step "Reaping orphan paplay/ffplay/aplay subprocesses"
killed=0
checked=0
# pgrep -P 1 lists processes whose parent is PID 1 (orphans). We
# intersect that with the known player names and the clips/ path
# pattern so we don't accidentally kill the user's music app.
for player in paplay ffplay aplay; do
    while IFS= read -r pid; do
        [[ -z "$pid" ]] && continue
        checked=$((checked + 1))
        if [[ -r "/proc/$pid/cmdline" ]] \
            && tr '\0' ' ' < "/proc/$pid/cmdline" | grep -q "clips/"; then
            if kill -TERM "$pid" 2>/dev/null; then
                killed=$((killed + 1))
            fi
        fi
    done < <(pgrep -P 1 -x "$player" 2>/dev/null || true)
done
if [[ "$killed" -gt 0 ]]; then
    ok "reaped $killed orphan audio subprocess(es) (checked $checked candidates)"
else
    ok "no orphan audio subprocesses found (checked $checked candidates)"
fi

# Wait briefly for the SIGTERMed children to actually exit. If they
# don't, the user-level pipewire sessions tied to them will linger.
sleep 0.5

# ── 4. Optionally restart pipewire ──────────────────────────────────
if [[ "$RESTART_AUDIO" -eq 1 ]]; then
    step "Restarting user-level pipewire services (--restart-audio)"
    if command -v systemctl >/dev/null 2>&1; then
        # Use --user; we never touch system services. Failure here
        # is non-fatal — the user can still play audio, they just
        # might have a stale stream router state until next reboot.
        if systemctl --user restart pipewire 2>&1 \
            | sed 's/^/      /'; then
            ok "pipewire restarted"
        else
            warn "pipewire restart returned non-zero (may not be running under systemd)"
        fi
        if systemctl --user restart pipewire-pulse 2>&1 \
            | sed 's/^/      /'; then
            ok "pipewire-pulse restarted"
        else
            warn "pipewire-pulse restart returned non-zero (not always installed)"
        fi
    else
        warn "systemctl not available; cannot restart audio services"
    fi
fi

# ── 5. Final state check ────────────────────────────────────────────
step "Verifying clean state"
if pgrep -x bo-muxd >/dev/null 2>&1; then
    err "bo-muxd is still running after cleanup — manual intervention needed"
    exit 1
fi
# Check for default sink, which proves the audio stack is responsive.
if command -v pactl >/dev/null 2>&1; then
    if default=$(pactl get-default-sink 2>/dev/null); then
        ok "default sink: $default"
    else
        warn "pactl could not query default sink (pulse/pipewire may not be running)"
    fi
fi
ok "audio path returned to system control"
echo "DONE"
