#!/usr/bin/env bash
# Headless render of the scene-view fork on llvmpipe → PNG. Fast-iteration loop:
#   edit scene3d.rs / gpuscene.rs / glow_scene.rs → ./render-scene.sh Atlantis out.png
set -eu
THEME="${1:-Atlantis}"; OUT="${2:-/tmp/scene.png}"
CONVO="${LOGSCOPE_CONVO:-examples/ladder/99-fullspine-marathon.logscope}"
export PATH=/usr/local/bin:$PATH
export LD_LIBRARY_PATH=/usr/local/lib/rustlib/x86_64-unknown-linux-gnu/lib
cargo build --features gpu-scene --bin scene-view 2>&1 | tail -1
pkill -9 Xvfb 2>/dev/null || true; sleep 1
Xvfb :99 -screen 0 1280x820x24 -ac +extension GLX +render -noreset >/tmp/xvfb.log 2>&1 &
sleep 2
DISPLAY=:99 LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe \
  LOGSCOPE_THEME="$THEME" LOGSCOPE_CONVO="$CONVO" \
  setsid target/debug/scene-view >/tmp/scene.log 2>&1 < /dev/null &
APP=$!; sleep 14
import -display :99 -window root "$OUT" 2>/dev/null && echo "rendered: $OUT"
kill $APP 2>/dev/null || true; pkill -9 Xvfb 2>/dev/null || true
