#!/bin/sh
# Fetch rav1d at the pinned commit, apply the patches here, build, and
# report per-stage CPU time decoding FILE (IVF/WebM/MKV/MP4 AV1).
#   shared/third_party/rav1d-hybrid/measure.sh footage.mkv [threads]
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
SRC=${RAV1D_SRC:-${XDG_CACHE_HOME:-$HOME/.cache}/rav1d-hybrid}
REV=d3d1cd67059f47803919be8276650e5870c9fd02
IN=${1:?usage: measure.sh FILE [threads]}
T=${2:-4}
if [ ! -d "$SRC/.git" ]; then git clone -q https://github.com/memorysafety/rav1d "$SRC"; fi
cd "$SRC" && git fetch -q origin "$REV" 2>/dev/null || true
git checkout -q -f "$REV" && git clean -qfd
for p in "$HERE"/0*.patch; do git apply "$p"; done
cargo build -q --release
case "$IN" in *.ivf) SRCF=$IN ;; *) SRCF=$(mktemp --suffix=.ivf); ffmpeg -v error -y -i "$IN" -map 0:v:0 -c copy -f ivf "$SRCF" ;; esac
echo "bit-exact vs dav1d: $( [ "$(dav1d -q --muxer md5 -o - -i "$SRCF" 2>/dev/null)" = "$(target/release/dav1d -q --threads "$T" --framedelay "$T" --muxer md5 -o - -i "$SRCF" 2>/dev/null)" ] && echo yes || echo 'NO (or no dav1d CLI to compare)')"
RAV1D_STAGE_TIMES=1 target/release/dav1d -q --threads "$T" --framedelay "$T" --muxer null -o /dev/null -i "$SRCF"
