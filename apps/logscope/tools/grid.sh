#!/bin/bash
# grid.sh CONVO [HOUR] [OUT] [WAIT] — 9 interior→overview views montaged into a 3x3 collage
set -u; . /home/claude/env.sh; cd /home/claude/logscope-suite
CONVO="$1"; HOUR="${2:-12}"; OUT="${3:-/home/claude/grid.png}"; WAIT="${4:-11}"
views=( "0.5,0.04,13" "2.2,0.0,12" "3.9,0.05,14" "5.2,0.08,16" )
pkill -9 Xvfb 2>/dev/null; sleep 1
Xvfb :99 -screen 0 880x560x24 -ac +extension GLX +render -noreset >/dev/null 2>&1 & sleep 2
i=0
for vw in "${views[@]}"; do
  DISPLAY=:99 LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe LOGSCOPE_THEME=Atlantis \
    LOGSCOPE_CONVO="$CONVO" LOGSCOPE_HOUR="$HOUR" POS_ANGLE_INIT="$vw" \
    target/debug/scene-view >/home/claude/scene-view.log 2>&1 & SV=$!
  sleep "$WAIT"
  import -display :99 -window root "/tmp/grid_$i.png" 2>/dev/null
  # label each tile with its yaw,pitch,dist
  convert "/tmp/grid_$i.png" -gravity SouthWest -pointsize 18 -fill '#9fe8ff' \
    -annotate +8+6 "$vw" "/tmp/grid_$i.png" 2>/dev/null
  kill $SV 2>/dev/null; sleep 1
  i=$((i+1))
done
pkill -9 Xvfb 2>/dev/null
montage /tmp/grid_0.png /tmp/grid_1.png /tmp/grid_2.png /tmp/grid_3.png -tile 2x2 -geometry +3+3 -background black "$OUT"
        /tmp/grid_3.png /tmp/grid_4.png /tmp/grid_5.png \
        /tmp/grid_6.png /tmp/grid_7.png /tmp/grid_8.png \
        -tile 3x3 -geometry +3+3 -background black "$OUT"
echo "wrote $OUT ($(identify -format '%wx%h' "$OUT" 2>/dev/null))"
