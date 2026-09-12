#!/usr/bin/env bash
# make_tiny_clips.sh — derive small fixture clips from full-size sources.
#
#   make_tiny_clips.sh <out-dir> <src.mkv> [start] [seconds] [width]
#
# Fixtures for iterating on the planner without waiting on 1080p. They are for
# exercising code paths and comparing a change against itself; they are NOT a
# substitute for the full-size clips when a number is going in a document.
# Two reasons, both of which bite:
#
#   * Downscaling changes the block-to-scene ratio. A 30 px block at 1080p
#     covers the same content as a 10 px block at 640x360, and a 40 px/frame
#     pan becomes 13 px/frame — well inside a diamond search that could not
#     reach it before. Scale --block-size with the width or the reuse
#     fraction is measuring a different question.
#   * Re-encoding replaces the source's codec artefacts with the ones this
#     script's encoder makes. Anything about artefacts, denoising or
#     restoration has to use the original file.
#
# The light blur (--blur, default on) is a size lever: it removes the
# high-frequency detail the encoder spends most of its bits on, so the fixture
# lands in a few MB. It also removes the detail a super-resolution network is
# supposed to reconstruct, so it makes clips useless for quality scoring and
# fine for planner counts. Pass `noblur` as the sixth argument to keep detail.
#
# To keep a low/high bitrate PAIR aligned, run the script twice with the same
# start and seconds on the two source files: the seek and the frame count
# match, so `tools/pair_dataset.py` still verifies alignment on the output.
#
# See docs/REUSE_FINDINGS.md for what each source clip is good for.
set -euo pipefail

out=${1:?usage: make_tiny_clips.sh <out-dir> <src.mkv> [start] [seconds] [width] [noblur]}
src=${2:?}
start=${3:-0}
secs=${4:-6}
width=${5:-640}
blur=${6:-blur}

mkdir -p "$out"
name=$(basename "${src%.*}" | tr ' [](){}' '_' | tr -s '_')
# The start time is part of the name: two shots out of the same film are two
# different fixtures and must not overwrite each other.
name="${name}_t${start%.*}s${secs}"
vf="scale=${width}:-2:flags=lanczos"
if [ "$blur" != "noblur" ]; then
  vf="$vf,gblur=sigma=0.6"
fi

# CRF 34 with a slow-ish preset: small, and the artefacts are at least
# consistent across every fixture made by this script.
ffmpeg -v error -y -ss "$start" -i "$src" -t "$secs" \
  -an -sn -dn -map_metadata -1 \
  -vf "$vf" -c:v libsvtav1 -crf 34 -preset 6 -g 240 \
  "$out/${name}_tiny.mkv" 2>/dev/null

ls -l "$out/${name}_tiny.mkv" | awk '{printf "%-52s %6.2f MB\n", $NF, $5/1048576}'
