#!/usr/bin/env sh
# Provider backed by llama.cpp's CLI, run per request on $VOXEL_MODEL
# (set by /model). No server, no port, nothing to leave running.
set -eu
: "${VOXEL_MODEL:?/model must be set first}"
payload=$(cat)
system=$(printf '%s' "$payload" | awk 'BEGIN{RS="\0"} NR==1')
user=$(printf '%s' "$payload"   | awk 'BEGIN{RS="\0"} NR==2')
exec llama-cli -m "$VOXEL_MODEL" \
  --no-display-prompt --log-disable \
  -c 16384 -n 512 --temp 0.3 --top-p 0.9 \
  -p "$(printf '%s\n\n%s' "$system" "$user")" 2>/dev/null
