#!/bin/sh
# Profile vidkit-player on one video under four presenter configurations.
#
#   tools/profile/run.sh URL|FILE [MINUTES] [OUT_DIR]
#   FORMAT=vp9 tools/profile/run.sh URL ...   # or FORMAT=av1: force the codec
#
# FORMAT matters on machines where one codec is hardware and the other is
# not (an M2 Max: VP9 on AVD through the VA layer, AV1 in dav1d); without
# it yt-dlp takes whichever YouTube ranks best.
#
# MINUTES (default 30) is split evenly across the configurations. The video
# is fetched ONCE (yt-dlp, <=1080p) so every configuration plays the same
# local file: streaming, the radio and the background keep-download would
# otherwise differ run to run. Output: one profile TSV per configuration,
# system.txt, and summary.md (tools/profile/summarize.py).
#
# Needs: a built vidkit-player (VIDKIT_PLAYER=path to override), ffmpeg,
# yt-dlp for URLs, python3 for the summary. Leave the window visible and
# the machine otherwise idle; plug in or not, but say which (system.txt
# records the power state).
set -eu
SRC=${1:?usage: run.sh URL|FILE [MINUTES] [OUT_DIR]}
MIN=${2:-30}
OUT=${3:-vidkit-profile-$(date +%Y%m%d-%H%M%S)}
HERE=$(cd "$(dirname "$0")" && pwd)
PLAYER=${VIDKIT_PLAYER:-$HERE/../../target/release/vidkit-player}
[ -x "$PLAYER" ] || PLAYER=$HERE/../../target/debug/vidkit-player
[ -x "$PLAYER" ] || { echo "build vidkit-player first (cargo build --release)"; exit 1; }
mkdir -p "$OUT"

# --- the video, once -----------------------------------------------------
case "$SRC" in
  http://*|https://*)
    echo "fetching $SRC ..."
    case "${FORMAT:-}" in
      vp9) SEL='bv*[vcodec^=vp09][height<=1080]+ba/bv*[vcodec^=vp9][height<=1080]+ba' ;;
      av1) SEL='bv*[vcodec^=av01][height<=1080]+ba' ;;
      *)   SEL='bv*[height<=1080]+ba/b[height<=1080]/b' ;;
    esac
    yt-dlp -q -f "$SEL" --merge-output-format mkv \
      -o "$OUT/video.%(ext)s" --print-to-file '%(title)s' "$OUT/title.txt" "$SRC"
    VIDEO=$(ls "$OUT"/video.* | head -1) ;;
  *) VIDEO=$SRC; echo "$SRC" > "$OUT/title.txt" ;;
esac

# --- what machine this is ------------------------------------------------
{
  echo "date: $(date -Is)"
  echo "player: $PLAYER"
  echo "video: $(cat "$OUT/title.txt")"
  ffprobe -v error -select_streams v:0 -show_entries stream=codec_name,width,height,r_frame_rate,pix_fmt,color_transfer -of default=noprint_wrappers=1 "$VIDEO" | sed 's/^/  /'
  echo "cpu: $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2- | sed 's/^ //') x $(nproc)"
  echo "gl: $(glxinfo -B 2>/dev/null | grep -E 'OpenGL renderer' | cut -d: -f2- || echo unknown)"
  echo "hwaccels: $(ffmpeg -hide_banner -hwaccels 2>/dev/null | tail -n +2 | tr '\n' ' ')"
  for s in /sys/class/power_supply/*; do [ -e "$s/type" ] && echo "power: $(basename "$s") $(cat "$s/type") $(cat "$s/online" 2>/dev/null || cat "$s/status" 2>/dev/null) $(cat "$s/capacity" 2>/dev/null)"; done
  echo "gpu selftest:"; timeout 60 "$PLAYER" --gpu-selftest 2>/dev/null | sed 's/^/  /' || echo "  (failed or no GL)"
} > "$OUT/system.txt" 2>&1
cat "$OUT/system.txt"

# --- the runs --------------------------------------------------------------
SECS=$((MIN * 60 / 4))
run() {
  name=$1; shift
  echo "== $name for ${SECS}s"
  env "$@" VIDKIT_PROFILE="$OUT/$name.tsv" timeout "$SECS" "$PLAYER" "$VIDEO" >"$OUT/$name.log" 2>&1 || true
}
run gpu
run cpu-whole     VIDKIT_GPU=0 VIDKIT_DAMAGE=0
run cpu-damage    VIDKIT_GPU=0
run cpu-quarter   VIDKIT_GPU=0 VIDKIT_SW_BUDGET=0.25

python3 "$HERE/summarize.py" "$OUT" | tee "$OUT/summary.md"
echo "done: $OUT"
