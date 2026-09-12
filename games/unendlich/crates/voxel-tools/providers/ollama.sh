#!/usr/bin/env sh
# Provider backed by a local ollama. Requires curl and jq.
#   OLLAMA_MODEL  defaults to qwen2.5-coder:7b
#   OLLAMA_HOST   defaults to 127.0.0.1:11434
set -eu
MODEL="${OLLAMA_MODEL:-qwen2.5-coder:7b}"
HOST="${OLLAMA_HOST:-127.0.0.1:11434}"

# stdin is <system>\0<user>
payload=$(cat)
system=$(printf '%s' "$payload" | awk 'BEGIN{RS="\0"} NR==1')
user=$(printf '%s' "$payload"   | awk 'BEGIN{RS="\0"} NR==2')

req=$(jq -n --arg m "$MODEL" --arg s "$system" --arg u "$user" '{
  model: $m, stream: false,
  system: $s, prompt: $u,
  options: { temperature: 0.3, top_p: 0.9, num_predict: 512 }
}')

resp=$(curl -sS --max-time 120 "http://$HOST/api/generate" -d "$req") || {
  echo "ollama unreachable at $HOST" >&2; exit 1; }
printf '%s' "$resp" | jq -r '.response // empty'
