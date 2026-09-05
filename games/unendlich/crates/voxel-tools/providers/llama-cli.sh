#!/usr/bin/env sh
# Provider backed by llama.cpp's CLI, run per request on $VOXEL_MODEL
# (set by /model). No server, no port, nothing left running.
#
# stdin is  <system>\0<user>  and stdout must be the plan, nothing else.
#
# Override the binary with $LLAMA_CLI if it is not on PATH or is named
# `main` (older builds) rather than `llama-cli`.
set -eu

: "${VOXEL_MODEL:?no model selected — run /model <name> in game first}"
BIN="${LLAMA_CLI:-}"
if [ -z "$BIN" ]; then
  for c in llama-cli llama main; do
    if command -v "$c" >/dev/null 2>&1; then BIN="$c"; break; fi
  done
fi
[ -n "$BIN" ] || { echo "llama-cli not found on PATH (set \$LLAMA_CLI)" >&2; exit 1; }
[ -f "$VOXEL_MODEL" ] || { echo "model file missing: $VOXEL_MODEL" >&2; exit 1; }

payload=$(cat)
system=$(printf '%s' "$payload" | awk 'BEGIN{RS="\0"} NR==1')
user=$(printf '%s' "$payload"   | awk 'BEGIN{RS="\0"} NR==2')
prompt=$(printf '%s\n\n%s' "$system" "$user")

# Flag names have churned across llama.cpp releases: --log-disable became
# -lv/--verbosity, and --no-display-prompt is absent from older builds.
# Probe once rather than guessing, so this works on whatever is installed.
QUIET=""
if "$BIN" --help 2>&1 | grep -q -- "--log-disable"; then
  QUIET="--log-disable"
elif "$BIN" --help 2>&1 | grep -q -- "--verbosity"; then
  QUIET="--verbosity 0"
fi
NODISP=""
if "$BIN" --help 2>&1 | grep -q -- "--no-display-prompt"; then
  NODISP="--no-display-prompt"
fi

# -no-cnv: single-shot completion, not an interactive chat turn. Without
# it some builds wait for input and the game hangs until you kill it.
CONV=""
if "$BIN" --help 2>&1 | grep -q -- "-no-cnv"; then
  CONV="-no-cnv"
fi

# stderr to /dev/null: llama.cpp writes load progress there, and the game
# shows a non-zero exit's stderr as the error message.
exec "$BIN" -m "$VOXEL_MODEL" \
  $QUIET $NODISP $CONV \
  -c "${VOXEL_CTX:-8192}" -n "${VOXEL_PREDICT:-512}" \
  --temp 0.3 --top-p 0.9 \
  -p "$prompt" 2>/dev/null
