#!/bin/bash
# flygrid.sh CONVO [OUT] [WAIT] — 9 frames of the FLY-THROUGH (LOGSCOPE_FLY 0→1) montaged 3x3
set -u; . /home/claude/env.sh; cd /home/claude/logscope-suite
CONVO="$1"; OUT="${2:-/home/claude/flygrid.png}"; WAIT="${3:-12}"
ss=( 0.05 0.35 0.7 0.95 )
pkill -9 Xvfb 2>/dev/null; sleep 1
Xvfb :99 -screen 0 880x560x24 -ac +extension GLX +render -noreset >/dev/null 2>&1 & sleep 2
i=0
for s in "${ss[@]}"; do
  DISPLAY=:99 LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe LOGSCOPE_THEME=Atlantis \
    LOGSCOPE_CONVO="$CONVO" LOGSCOPE_HOUR=12 LOGSCOPE_FLY="$s" \
    target/debug/scene-view >/home/claude/scene-view.log 2>&1 & SV=$!
  sleep "$WAIT"
  import -display :99 -window root "/tmp/fly_$i.png" 2>/dev/null
  convert "/tmp/fly_$i.png" -gravity SouthWest -pointsize 18 -fill '#9fe8ff' -annotate +8+6 "fly s=$s" "/tmp/fly_$i.png" 2>/dev/null
  kill $SV 2>/dev/null; sleep 1
  i=$((i+1))
done
pkill -9 Xvfb 2>/dev/null
montage /tmp/fly_0.png /tmp/fly_1.png /tmp/fly_2.png /tmp/fly_3.png -tile 2x2 -geometry +3+3 -background black "$OUT"
        /tmp/fly_5.png /tmp/fly_6.png /tmp/fly_7.png /tmp/fly_8.png \
        -tile 3x3 -geometry +3+3 -background black "$OUT"
echo "wrote $OUT ($(identify -format '%wx%h' "$OUT" 2>/dev/null))"
