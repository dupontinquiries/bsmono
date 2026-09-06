#!/bin/bash
# Headless scene capture in two halves, so neither exceeds a sandbox tool call's time limit:
#   tools/scene-shot.sh start "Cat Cave" "0.4,0.25,26"          # X + WM + scene-view (one call)
#   tools/scene-shot.sh grab  /tmp/cave.png                     # capture (next call, after ~12 s)
# Camera is LOGSCOPE_CAM: "yaw,pitch,dist" orbits the graph centroid; "yaw,pitch,dist,tx,ty,tz" orbits a
# fixed point — the only way to frame a prop outside the graph's footprint (the physical UI panel).
# Defaults pin what a headless render needs pinned: HOUR=14 (the sandbox clock is whatever it is — every
# unpinned render is a night shot at 04:00), POWER=full (llvmpipe otherwise trips the weak-GPU trim),
# WORLD_ONLY + HIDE_BAR (the graph and dock cover the world), DEBUG_LABELS (a per-mesh dump at
# /tmp/scene_labels_<theme>.txt, which is how to find a prop's coordinates for the six-value camera).
set -u
here="$(cd "$(dirname "$0")/.." && pwd)"
export DISPLAY=:99 LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe
case "${1:-}" in
  start)
    THEME="$2"; CAM="${3:-}"
    pgrep -x Xvfb >/dev/null || { nohup setsid Xvfb :99 -screen 0 1280x800x24 -ac +extension GLX +render -noreset >/tmp/xvfb.log 2>&1 </dev/null & sleep 2; }
    pgrep -x openbox >/dev/null || { nohup setsid openbox >/tmp/ob.log 2>&1 </dev/null & sleep 1; }
    pkill -x scene-view 2>/dev/null; sleep 0.5
    [ -f /tmp/tiny.logscope ] || printf 'user: hi\n' > /tmp/tiny.logscope
    cd "$here"
    LD_LIBRARY_PATH="$(rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/lib" \
    LOGSCOPE_THEME="$THEME" LOGSCOPE_CONVO="${LOGSCOPE_CONVO:-/tmp/tiny.logscope}" LOGSCOPE_CAM="$CAM" \
    LOGSCOPE_WORLD_ONLY=1 LOGSCOPE_HIDE_BAR=1 LOGSCOPE_DEBUG_LABELS=1 \
    LOGSCOPE_HOUR="${HOUR:-14}" LOGSCOPE_POWER="${POWER:-full}" \
    nohup setsid ./target/debug/scene-view >/tmp/scene.log 2>&1 </dev/null &
    sleep 4
    W=$(xdotool search --name Logscope 2>/dev/null | head -1); [ -n "$W" ] && xdotool windowmap "$W" 2>/dev/null
    echo "started theme=$THEME cam=$CAM win=$W pid=$(pgrep -x scene-view)" ;;
  grab)
    W=$(xdotool search --name Logscope 2>/dev/null | head -1); [ -n "$W" ] && xdotool windowmap "$W" 2>/dev/null
    import -window root "$2" 2>/dev/null && echo "captured $2 ($(pgrep -x scene-view | wc -l) app procs)"; tail -2 /tmp/scene.log ;;
  *) echo "usage: $0 start THEME [CAM] | grab OUT.png"; exit 2 ;;
esac
