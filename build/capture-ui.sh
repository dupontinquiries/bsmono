#!/usr/bin/env bash
# Render a built GUI binary headless (Xvfb) and screenshot it. Qt apps build against Qt6 (QMAKE=qmake6);
# egui apps need software GL (LIBGL_ALWAYS_SOFTWARE=1). Usage: capture-ui.sh <binary> <out.png> [secs]
set -u
BIN="$1"; OUT="${2:-/tmp/ui.png}"; WAIT="${3:-12}"
SYSROOT="$(rustc --print sysroot)"
export LD_LIBRARY_PATH="$SYSROOT/lib/rustlib/x86_64-unknown-linux-gnu/lib:/home/claude/.local/lib:${LD_LIBRARY_PATH:-}"
export DISPLAY=:99 XDG_RUNTIME_DIR=/tmp/runtime-root QT_QPA_PLATFORM=xcb LIBGL_ALWAYS_SOFTWARE=1
mkdir -p /tmp/runtime-root; chmod 700 /tmp/runtime-root
Xvfb :99 -screen 0 1280x800x24 >/tmp/xvfb.log 2>&1 & XVFB=$!
sleep 2
"$BIN" >/tmp/ui-run.log 2>&1 & APP=$!
sleep "$WAIT"
import -window root "$OUT" 2>/dev/null
kill "$APP" "$XVFB" 2>/dev/null
identify "$OUT" 2>/dev/null
