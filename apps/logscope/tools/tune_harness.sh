#!/bin/sh
# tune_harness.sh <Scene> [cand_file]
# Renders each candidate env-set for <Scene> into /home/claude/candidates/<scene>/, scores it against
# SCENE_RULES.md, ranks, and montages the top-K with scores. NEVER edits the committed tree — candidates are
# pure env overrides in a scratch dir. I (the agent) review the top renders and hand-merge the winner.
#
# Candidate file lines:  label|ENV_KEY=val ENV_KEY=val ...
#   (ENV may include LOGSCOPE_SEED, LOGSCOPE_HOUR, LOGSCOPE_SEASON, POS_ANGLE_INIT, LOGSCOPE_EXP, LOGSCOPE_SAT,
#    LOGSCOPE_TUNE_SCALE, LOGSCOPE_TUNE_HEIGHT, LOGSCOPE_TUNE_DENSITY)
set -e
. /home/claude/env.sh
SUITE=/home/claude/logscope-suite
SCENE="$1"
KEY=$(echo "$SCENE" | tr 'A-Z @' 'a-z__')
CAND="${2:-$SUITE/tools/cand_${KEY}.txt}"
OUT=/home/claude/candidates/$KEY
mkdir -p "$OUT"
REF="$SUITE/tools/refs/${KEY}.png"
REFARG=""
if [ -f "$REF" ]; then REFARG="--ref $REF"; echo "using reference: $REF"; fi
: > "$OUT/scores.csv"
echo "score,label,note,env" >> "$OUT/scores.csv"
cd "$SUITE"
pkill -9 Xvfb 2>/dev/null || true; pkill -9 scene-view 2>/dev/null || true; sleep 1

while IFS='|' read -r label env; do
  [ -z "$label" ] && continue
  case "$label" in \#*) continue;; esac
  img="$OUT/$label.png"
  Xvfb :99 -screen 0 560x440x24 -ac +extension GLX +render -noreset >/dev/null 2>&1 & sleep 2
  # shellcheck disable=SC2086
  env DISPLAY=:99 LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe LOGSCOPE_THEME="$SCENE" $env \
      target/debug/scene-view >/tmp/cand.log 2>&1 & P=$!
  sleep 6
  import -display :99 -window root "$img" 2>/dev/null || true
  kill $P 2>/dev/null || true; pkill -9 scene-view 2>/dev/null || true; pkill -9 Xvfb 2>/dev/null || true; sleep 1
  line=$(python3 "$SUITE/tools/score_render.py" "$img" $REFARG --json)
  sc=$(echo "$line" | python3 -c "import sys,json;print(f\"{json.load(sys.stdin)['score']:.4f}\")")
  note=$(echo "$line" | python3 -c "import sys,json;print(json.load(sys.stdin)['note'] or '-')")
  echo "$sc,$label,$note,$env" >> "$OUT/scores.csv"
  echo "  $sc  $label  [$note]"
done < "$CAND"

echo "=== ranked (top scores) ==="
sort -t, -k1 -rn "$OUT/scores.csv" | grep -v '^score,' | head -12
# montage top-6 with score labels
TOP=$(sort -t, -k1 -rn "$OUT/scores.csv" | grep -v '^score,' | head -6 | awk -F, '{print $2"|"$1}')
files=""
for entry in $TOP; do
  lbl=$(echo "$entry" | cut -d'|' -f1); sc=$(echo "$entry" | cut -d'|' -f2)
  src="$OUT/$lbl.png"
  [ -f "$src" ] || continue
  convert "$src" -gravity North -background '#000000aa' -splice 0x16 -fill '#ffe055' -pointsize 13 \
    -annotate +0+1 "$sc  $lbl" "$OUT/_top_$lbl.png" 2>/dev/null
  files="$files $OUT/_top_$lbl.png"
done
montage $files -tile 3x2 -geometry +3+3 -background '#0a0f12' "$OUT/top.png" 2>/dev/null || true
echo "montage: $OUT/top.png"
