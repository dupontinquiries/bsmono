#!/usr/bin/env bash
# End-to-end video analysis: proxy -> motion plan -> dense capture of
# the cinematic sections only.
#
#   ./run_video.sh part1.mkv [palette.tsv]
#
# Produces, next to the video:
#   <name>.proxy.mkv     throwaway low-res copy (planning only)
#   <name>.plan.tsv      PAN / ACTIVE / STATIC / CUT sections
#   <name>.blocks.tsv    block-change events from the PAN sections
#
# Needs: ffmpeg, and ocrscan built (see below).
set -euo pipefail

VIDEO="${1:?usage: run_video.sh <video.mkv> [palette.tsv]}"
PALETTE="${2:-palette_example.tsv}"
BASE="${VIDEO%.*}"

# Find ocrscan: built binary, or on PATH.
OCR="$(dirname "$0")/../pipeline/ocrscan/target/release/ocrscan"
if [[ ! -x "$OCR" ]]; then
  OCR="$(command -v ocrscan || true)"
fi
if [[ -z "$OCR" || ! -x "$OCR" ]]; then
  echo "ocrscan not found. Build it first:" >&2
  echo "    cd $(dirname "$0")/../pipeline/ocrscan && cargo build --release" >&2
  exit 1
fi

# 1. Proxy. Mandatory for 4K AV1: ffmpeg must fully decode the source
#    even to emit a few probe frames per second, so planning directly
#    on a 4K file takes longer than the analysis it feeds.
if [[ ! -f "$BASE.proxy.mkv" ]]; then
  echo "== proxy (one slow pass; everything after this is fast)"
  ffmpeg -nostdin -hide_banner -loglevel error -y -i "$VIDEO" \
      -vf "fps=4,scale=-2:180" -c:v libx264 -crf 28 -an "$BASE.proxy.mkv"
fi

# 2. Motion plan. Timestamps refer to the ORIGINAL video.
echo "== plan"
"$OCR" plan --video "$BASE.proxy.mkv" --probe-hz 4 \
    --dump-raw "$BASE.motion.tsv" --out "$BASE.plan.tsv"

# 3. Dense capture of the cinematic sections only. Each section is cut
#    out first so the 4K file is decoded once per section rather than
#    once per frame interval.
echo "== blocks (PAN sections only)"
: > "$BASE.blocks.tsv"
n=0
while IFS=$'\t' read -r s e cls iv rest; do
  [[ "$cls" == "PAN" ]] || continue
  n=$((n+1))
  seg="$BASE.seg$n.mkv"
  ffmpeg -nostdin -hide_banner -loglevel error -y -ss "$s" -to "$e" -i "$VIDEO" \
      -an -c:v libx264 -crf 20 "$seg"
  "$OCR" blocks --video "$seg" --interval "$iv" --palette "$PALETTE" \
      --out "$seg.blocks.tsv" >/dev/null 2>&1 || true
  # shift event timestamps back into original-video time
  awk -F'\t' -v off="$s" 'NR>1 {$1=sprintf("%.1f",$1+off); print}' OFS='\t' \
      "$seg.blocks.tsv" >> "$BASE.blocks.tsv" 2>/dev/null || true
  rm -f "$seg" "$seg.blocks.tsv"
done < <(grep -v '^#' "$BASE.plan.tsv")

echo
echo "plan:   $BASE.plan.tsv"
echo "motion: $BASE.motion.tsv   (per-instant mag/unif, for calibration)"
echo "blocks: $BASE.blocks.tsv   ($(grep -c . "$BASE.blocks.tsv" 2>/dev/null || echo 0) events from $n PAN sections)"
if [[ "$n" -eq 0 ]]; then
  echo
  echo "No PAN sections found. Either this video has no camera moves, or"
  echo "the thresholds need calibrating for your footage. Check the"
  echo "distribution and lower --uniform-min / --pan-explained:"
  echo "    awk -F'\\t' 'NR>1{print \$3}' $BASE.motion.tsv | sort -n | tail -20"
  echo "See MOTION_PLANNING.md."
fi
