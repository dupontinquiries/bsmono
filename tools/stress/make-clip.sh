#!/bin/sh
# Generate the local test clip (no network, ~2 MB): 720p30 H.264 motion pattern, 20 s, silent.
# Usage: make-clip.sh [out.mp4]
set -eu
OUT=${1:-clip.mp4}
SRC="testsrc2=size=1280x720:rate=30,format=yuv420p"
if ffmpeg -hide_banner -encoders 2>/dev/null | grep -q libx264; then
  ENC="-c:v libx264 -preset veryfast -crf 28 -g 30"
else
  ENC="-c:v libopenh264 -b:v 1500k -g 30"   # Fedora's ffmpeg has no x264
fi
# shellcheck disable=SC2086
ffmpeg -y -loglevel error -f lavfi -i "$SRC" -t 20 -an $ENC -movflags +faststart "$OUT"
ls -la "$OUT"
