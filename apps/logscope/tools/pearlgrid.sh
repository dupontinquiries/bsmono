#!/bin/bash
# pearlgrid.sh OUT [WAIT] — 4 states of the pearl composer montaged 2x2
set -u; . /home/claude/env.sh; cd /home/claude/logscope-suite
OUT="${1:-/home/claude/pearlgrid.png}"; WAIT="${2:-7}"
texts=( "watch the inode; reopen on rotate" "Build the file watcher: track offset, reopen on inode change, reset when the log shrinks." "why does latency spike at 240ms?" "ship it — Build r178 ✦" )
pulses=( 0 0 0 1 )
pkill -9 Xvfb 2>/dev/null; sleep 1
Xvfb :99 -screen 0 900x300x24 -ac +extension GLX +render -noreset >/dev/null 2>&1 & sleep 2
for i in 0 1 2 3; do
  DISPLAY=:99 LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe \
    LOGSCOPE_PEARL_TEXT="${texts[$i]}" LOGSCOPE_PEARL_PULSE="${pulses[$i]}" \
    target/debug/pearl-input-view >/home/claude/pearl-view.log 2>&1 & PV=$!
  sleep "$WAIT"
  import -display :99 -window root "/tmp/pearl_$i.png" 2>/dev/null
  kill $PV 2>/dev/null; sleep 1
done
pkill -9 Xvfb 2>/dev/null
montage /tmp/pearl_0.png /tmp/pearl_1.png /tmp/pearl_2.png /tmp/pearl_3.png -tile 2x2 -geometry +3+3 -background black "$OUT"
echo "wrote $OUT ($(identify -format '%wx%h' "$OUT" 2>/dev/null))"
