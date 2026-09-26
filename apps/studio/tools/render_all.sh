#!/usr/bin/env bash
# render_all.sh LIB_DIR WORK_DIR OUT_DIR [BIN_DIR]
#
# Import every FL package (*.zip) in LIB_DIR with flp-batch into WORK_DIR,
# render each imported song in full with examples/render_project, and write
# OUT_DIR/<song>.opus (160 kbps) plus OUT_DIR/levels.txt.
#
# Level is kept AS RENDERED so one turn's renders can be compared with the
# next by ear: a render is only turned DOWN, and only if its peak is above
# -1 dBFS (to avoid clipping the encode). Never normalized up.
#
# BIN_DIR holds flp-batch and examples/render_project (a release build):
#   CARGO_TARGET_DIR=... cargo build --release -p studio-app \
#       --bin flp-batch --example render_project
set -euo pipefail
lib=$1; work=$2; out=$3; bin=${4:-${CARGO_TARGET_DIR:-target}/release}
mkdir -p "$work" "$out"
"$bin/flp-batch" "$lib" "$work" --store "$work/.store" > "$work/batch.log" 2>&1 || true
tail -n +1 "$work/BATCH.md" | sed -n '1,40p'
: > "$out/levels.txt"
for proj in "$work"/*/*.studio; do
  name=$(basename "$proj" .studio)
  raw=$(mktemp --suffix=.f32)
  ( cd "$(dirname "$proj")" && "$bin/examples/render_project" "$(basename "$proj")" "$raw" ) 2>&1 | grep '^timing' || true
  peak=$(ffmpeg -nostdin -f f32le -ar 48000 -ac 2 -i "$raw" -af astats -f null - 2>&1 | grep 'Peak level dB' | tail -1 | awk '{print $NF}')
  rms=$(ffmpeg -nostdin -f f32le -ar 48000 -ac 2 -i "$raw" -af astats -f null - 2>&1 | grep 'RMS level dB' | tail -1 | awk '{print $NF}')
  gain=$(python3 -c "print(round(min(0.0, -1.0 - float('$peak')), 2))")
  ffmpeg -nostdin -v error -y -f f32le -ar 48000 -ac 2 -i "$raw" -af "volume=${gain}dB" -c:a libopus -b:a 160k "$out/$name.opus"
  rm -f "$raw"
  printf '%-40s peak %7s dBFS  rms %7s dBFS  encode gain %s dB\n' "$name" "$peak" "$rms" "$gain" | tee -a "$out/levels.txt"
done
