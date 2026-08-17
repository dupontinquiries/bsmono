#!/bin/sh
# egui-check.sh — check the GUI crate and report a VERDICT, not a log to interpret.
#
# c4-839 shipped a broken build because I read "0 errors" from a background cargo check that was
# still running. An incomplete log has no errors in it yet, and that is indistinguishable from a
# clean one if you only grep. Three real errors were sitting in the code that bundle added.
#
# This crate takes 10-15 minutes to check, so it cannot be run in the foreground inside a tool call.
# Backgrounding it is correct; reading its log as a result before the compiler exits is not. So the
# script owns the waiting and prints exactly one of PASS / FAIL / TIMEOUT — there is nothing to
# misread.
#
#   sh tools/egui-check.sh          start it, wait up to 25 minutes
#   sh tools/egui-check.sh --status poll a run started earlier
set -e
cd "$(cd "$(dirname "$0")/.." && pwd)"
LOG=/tmp/egui-check.log
PIDF=/tmp/egui-check.pid

verdict() {
    # Only meaningful once the compiler is gone: a running build's log is a partial transcript.
    if [ -f "$PIDF" ] && kill -0 "$(cat $PIDF)" 2>/dev/null; then
        echo "RUNNING — no verdict yet (this is the state c4-839 misread as PASS)"; return 2
    fi
    if [ ! -f "$LOG" ]; then echo "NO RUN"; return 2; fi
    n=$(grep -cE '^error' "$LOG" 2>/dev/null || true)
    if [ "${n:-0}" -gt 0 ]; then
        echo "FAIL — $n error(s):"; grep -E '^error' -A4 "$LOG" | head -20; return 1
    fi
    # A clean run ends with cargo's own summary line; its absence means the log was truncated.
    if ! grep -qE 'Finished|Checking|warning|^$' "$LOG"; then
        echo "TIMEOUT or truncated — log has no compiler output at all"; return 2
    fi
    echo "PASS — 0 errors, compiler exited"; return 0
}

if [ "$1" = "--status" ]; then verdict; exit $?; fi

rm -f "$LOG" "$PIDF"
( cargo check -q -p logscope-egui --features wgpu-engine > "$LOG" 2>&1 ; rm -f "$PIDF" ) &
echo $! > "$PIDF"
# Wait for the compiler to actually exit, up to 25 minutes.
i=0
while [ -f "$PIDF" ] && [ $i -lt 300 ]; do sleep 5; i=$((i+1)); done
verdict
