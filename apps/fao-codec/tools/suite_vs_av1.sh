#!/usr/bin/env bash
# suite_vs_av1.sh — the codec against REAL AV1 encodes of the same
# footage, per shot and in total, scored on the same cropped source
# with luma MS-SSIM and chroma PSNR.
#
#   tools/suite_vs_av1.sh <source.mkv> <shots.csv> <outdir> [anchor.mkv ...]
#
# shots.csv is the suite's shot table (shot,start,frames,...), the
# crop is the suite's 1920:800:0:140, and the anchors are AV1 files
# encoded from the same source (their bars are cropped away too;
# their bytes are summed from the packets whose pts fall inside the
# shot, which is per temporal unit so hidden frames are charged to
# the frame they support). Nothing here is interpolated: every row is
# a measurement of a file that exists.
#
# Env: BENCH, MSSSIM (binaries), FAO_* passes through to the encoder.
set -euo pipefail
SRC="$1"; SHOTS="$2"; OUT="$3"; shift 3
ANCHORS=("$@")
BENCH="${BENCH:-/tmp/fao/target/release/bench}"
MSSSIM="${MSSSIM:-/tmp/fao/target/release/msssim}"
CROP="crop=1920:800:0:140"
FPS="${FPS:-23.976}"
mkdir -p "$OUT"
RES="$OUT/results.csv"
echo "shot,start,frames,codec_bytes,codec_msssim,codec_u,codec_v$(for a in "${ANCHORS[@]}"; do n=$(basename "$a" .mkv | tail -c 24 | tr -c 'A-Za-z0-9\n' '_'); printf ',%s_bytes,%s_msssim,%s_u,%s_v' "$n" "$n" "$n" "$n"; done)" > "$RES"

# per-anchor packet tables once: pts_time,size
declare -a PKT
for i in "${!ANCHORS[@]}"; do
  PKT[$i]="$OUT/pkt_$i.csv"
  ffprobe -v error -select_streams v -show_entries packet=pts_time,size -of csv=p=0 "${ANCHORS[$i]}" > "${PKT[$i]}"
done

tail -n +2 "$SHOTS" | while IFS=, read -r shot start frames rest; do
  [ -z "$shot" ] && continue
  y4m="$OUT/shot_$shot.y4m"
  ffmpeg -v error -y -ss "$start" -i "$SRC" -frames:v "$frames" -vf "$CROP" -pix_fmt yuv420p "$y4m"
  # codec
  line=$("$BENCH" --csv "$OUT/bench_$shot.csv" "$y4m" 2>/dev/null | grep "^shot_$shot.y4m" | head -1 || true)
  cb=$(echo "$line" | awk '{print $4}'); cm=$(echo "$line" | awk '{print $5}'); cu=$(echo "$line" | awk '{print $6}'); cv=$(echo "$line" | awk '{print $7}')
  row="$shot,$start,$frames,$cb,$cm,$cu,$cv"
  end=$(python3 -c "print($start + $frames / $FPS)")
  for i in "${!ANCHORS[@]}"; do
    a="${ANCHORS[$i]}"
    dec="$OUT/anchor_${i}_shot_$shot.y4m"
    ffmpeg -v error -y -ss "$start" -i "$a" -frames:v "$frames" -vf "$CROP" -pix_fmt yuv420p "$dec"
    ab=$(awk -F, -v s="$start" -v e="$end" '$1>=s && $1<e {b+=$2} END{print b+0}' "${PKT[$i]}")
    am=$("$MSSSIM" "$y4m" "$dec" 2>/dev/null | tail -1 | awk '{print $2}')
    # near-black shots score "inf"; keep the loop alive and record 99
    uv=$(ffmpeg -v info -i "$y4m" -i "$dec" -lavfi psnr -f null - 2>&1 | grep -o "u:[0-9.inf]* v:[0-9.inf]*" | tail -1 || true)
    au=$(echo "$uv" | sed 's/u:\([0-9.inf]*\) v:.*/\1/; s/inf/99/'); av=$(echo "$uv" | sed 's/.*v:\([0-9.inf]*\)/\1/; s/inf/99/')
    row="$row,$ab,$am,$au,$av"
    rm -f "$dec"
  done
  echo "$row" >> "$RES"
  echo "shot $shot done: $row" >&2
  rm -f "$y4m"
done
echo "wrote $RES" >&2
