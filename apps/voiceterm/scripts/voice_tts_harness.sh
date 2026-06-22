#!/usr/bin/env bash
# v0.1.696 — voice-command TTS harness (LOCAL; needs audio + whisper).
#
# Two layers of the voice test pyramid:
#   1. PARSE correctness — covered in-repo by the sandbox-runnable regression
#      test (cargo test corpus_phrases_route_as_expected). That asserts every
#      phrase in tests/voice_corpus.txt routes to its expected intent kind.
#      Run it anywhere; no audio needed.
#   2. RECOGNITION drift — THIS script. It SPEAKS each corpus phrase with a
#      TTS engine, transcribes the audio back with whisper, and checks the
#      transcript still contains the phrase's key words. This catches cases
#      where a command is parseable in text but mis-heard in practice
#      (homophones, dropped articles, accent/codec drift).
#
# Why generated audio: there are no pre-recorded command WAVs in the repo, so
# we synthesize. piper (neural) is preferred for realism; espeak-ng is the
# zero-setup fallback. Whisper is the same binary the app uses (voice.rs).
#
# Usage:
#   scripts/voice_tts_harness.sh [--tts piper|espeak] [--model PATH] \
#       [--corpus tests/voice_corpus.txt] [--keep]
#
# Exit non-zero if any phrase's transcript loses too many key words.

set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CORPUS="$ROOT/tests/voice_corpus.txt"
TTS="auto"
WMODEL="${WHISPER_MODEL:-$HOME/.local/share/whisper/ggml-base.en.bin}"
PIPER_VOICE="${PIPER_VOICE:-$HOME/.local/share/piper/en_US-amy-medium.onnx}"
KEEP=0
THRESHOLD="${THRESHOLD:-0.6}"   # min fraction of key words that must survive

while [ $# -gt 0 ]; do
  case "$1" in
    --tts) TTS="$2"; shift 2;;
    --model) WMODEL="$2"; shift 2;;
    --corpus) CORPUS="$2"; shift 2;;
    --keep) KEEP=1; shift;;
    *) echo "unknown arg: $1"; exit 2;;
  esac
done

# locate tools
WHISPER_BIN="$(command -v whisper-cli || command -v whisper-cpp || command -v main || true)"
if [ -z "$WHISPER_BIN" ]; then
  echo "!! no whisper-cli/whisper-cpp on PATH — install whisper.cpp first." >&2
  echo "   (the app uses the same binary; see voice.rs)" >&2
  exit 3
fi
if [ "$TTS" = "auto" ]; then
  if command -v piper >/dev/null && [ -f "$PIPER_VOICE" ]; then TTS="piper";
  elif command -v espeak-ng >/dev/null || command -v espeak >/dev/null; then TTS="espeak";
  else echo "!! no TTS engine (piper or espeak-ng) found." >&2; exit 3; fi
fi
ESPEAK_BIN="$(command -v espeak-ng || command -v espeak || true)"

TMP="$(mktemp -d)"
trap '[ "$KEEP" = "1" ] || rm -rf "$TMP"' EXIT
echo "harness: tts=$TTS whisper=$(basename "$WHISPER_BIN") model=$(basename "$WMODEL")"
echo "         corpus=$CORPUS threshold=$THRESHOLD  tmp=$TMP"
echo

speak() {  # $1 phrase, $2 out.wav — synthesize 16kHz mono PCM (whisper-friendly)
  local phrase="$1" out="$2"
  if [ "$TTS" = "piper" ]; then
    echo "$phrase" | piper --model "$PIPER_VOICE" --output_file "$out.raw.wav" >/dev/null 2>&1
    # resample to 16k mono if sox/ffmpeg available, else hope piper's rate is ok
    if command -v ffmpeg >/dev/null; then
      ffmpeg -y -i "$out.raw.wav" -ar 16000 -ac 1 "$out" >/dev/null 2>&1
    else mv "$out.raw.wav" "$out"; fi
  else
    "$ESPEAK_BIN" -s 165 -w "$out.raw.wav" "$phrase" >/dev/null 2>&1
    if command -v ffmpeg >/dev/null; then
      ffmpeg -y -i "$out.raw.wav" -ar 16000 -ac 1 "$out" >/dev/null 2>&1
    else mv "$out.raw.wav" "$out"; fi
  fi
}

transcribe() {  # $1 wav -> stdout transcript (lowercased, punctuation stripped)
  "$WHISPER_BIN" -m "$WMODEL" -f "$1" --no-prints 2>/dev/null \
    | tail -1 | tr '[:upper:]' '[:lower:]' | sed 's/[[:punct:]]//g' | xargs
}

# fraction of phrase key words present in the transcript
overlap() {  # $1 phrase, $2 transcript -> prints fraction 0..1
  python3 - "$1" "$2" << 'PY'
import sys
phrase = sys.argv[1].lower().split()
trans = set(sys.argv[2].lower().split())
stop = {"the","a","an","to","for","of","my","this","that","on","in","it"}
keys = [w for w in phrase if w not in stop] or phrase
hit = sum(1 for w in keys if w in trans)
print(f"{hit/len(keys):.2f}")
PY
}

total=0; passed=0; fails=()
while IFS=$'\t' read -r phrase kind; do
  case "$phrase" in ''|\#*) continue;; esac
  [ -z "${kind:-}" ] && continue
  total=$((total+1))
  wav="$TMP/$(printf '%03d' "$total").wav"
  speak "$phrase" "$wav"
  trans="$(transcribe "$wav")"
  frac="$(overlap "$phrase" "$trans")"
  ok=$(python3 -c "print(1 if float('$frac')>=float('$THRESHOLD') else 0)")
  if [ "$ok" = "1" ]; then
    passed=$((passed+1)); status="ok  "
  else
    status="DRIFT"; fails+=("\"$phrase\" -> heard \"$trans\" (overlap $frac)")
  fi
  printf "  [%s] %-34s heard: %s\n" "$status" "$phrase" "$trans"
done < "$CORPUS"

echo
echo "recognition: $passed/$total phrases survived TTS->whisper (threshold $THRESHOLD)"
if [ "${#fails[@]}" -gt 0 ]; then
  echo "drift:"; for f in "${fails[@]}"; do echo "  - $f"; done
  exit 1
fi
echo "all clear."
