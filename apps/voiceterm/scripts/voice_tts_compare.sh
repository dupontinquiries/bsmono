#!/usr/bin/env bash
# v0.1.705 (#3) — TTS-vs-real corpus comparison (LOCAL; needs whisper + espeak-ng).
#
# For each corpus reference transcript, synthesize it with TTS (espeak-ng) and
# transcribe THAT, giving a "synth WER". Compare against the real-recording WER
# (from a baseline produced by corpus_wer_harness.sh --save). Where synth WER is
# much worse than real WER, the phrase is UNRELIABLE to test TTS with — TTS
# mispronounces it, so a failure is a synthesis artifact, not a real recognizer
# regression. Emits a tts_skiplist.txt of such phrases.
#
# This is the user's "words to not test TTS with if they sound off", measured.
#
# Usage:
#   scripts/voice_tts_compare.sh --corpus path/to/corpus \
#       --real-baseline real_pairs.tsv [--model PATH] [--margin 0.25] \
#       [--max N] [--skiplist tts_skiplist.txt]
#
# --real-baseline is a ref<TAB>real_wer TSV (one per phrase). If absent, real
# WER is assumed 0 (treats every phrase as "should transcribe perfectly").

set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CORPUS=""; WMODEL="${WHISPER_MODEL:-$HOME/.local/share/whisper/ggml-base.en.bin}"
REAL_BASELINE=""; MARGIN="0.25"; MAX=0; SKIPLIST="tts_skiplist.txt"

while [ $# -gt 0 ]; do
  case "$1" in
    --corpus) CORPUS="$2"; shift 2;;
    --model) WMODEL="$2"; shift 2;;
    --real-baseline) REAL_BASELINE="$2"; shift 2;;
    --margin) MARGIN="$2"; shift 2;;
    --max) MAX="$2"; shift 2;;
    --skiplist) SKIPLIST="$2"; shift 2;;
    *) echo "unknown arg: $1"; exit 2;;
  esac
done
[ -d "$CORPUS" ] || { echo "!! --corpus <dir> required"; exit 2; }

WHISPER_BIN="$(command -v whisper-cli || command -v whisper-cpp || true)"
[ -z "$WHISPER_BIN" ] && { echo "!! no whisper on PATH"; exit 3; }
command -v espeak-ng >/dev/null || { echo "!! espeak-ng required for synthesis"; exit 3; }
command -v ffmpeg >/dev/null || { echo "!! ffmpeg required"; exit 3; }

WER_BIN="$ROOT/target/wer_score"
[ -x "$WER_BIN" ] || rustc --edition 2021 -O "$ROOT/scripts/wer_score.rs" -o "$WER_BIN" \
  || { echo "!! couldn't build wer_score"; exit 3; }

TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
: > "$SKIPLIST"
echo "TTS-vs-real comparison: margin=$MARGIN"
echo

real_wer_for() {  # $1 = ref text → real WER from baseline, or 0
  [ -z "$REAL_BASELINE" ] && { echo "0.000"; return; }
  awk -F'\t' -v r="$1" '$1==r{print $2; found=1} END{if(!found)print "0.000"}' "$REAL_BASELINE"
}

n=0; flagged=0
while IFS= read -r ref_txt; do
  [ -s "$ref_txt" ] || continue
  n=$((n+1)); [ "$MAX" -gt 0 ] && [ "$n" -gt "$MAX" ] && { n=$((n-1)); break; }
  ref="$(tr '\n\t' '  ' < "$ref_txt")"
  # synthesize → wav → whisper
  espeak-ng -v en-us -s 165 -w "$TMP/synth.wav" "$ref" 2>/dev/null
  ffmpeg -y -i "$TMP/synth.wav" -ar 16000 -ac 1 "$TMP/synth16.wav" >/dev/null 2>&1
  synth_hyp="$("$WHISPER_BIN" -m "$WMODEL" -f "$TMP/synth16.wav" --no-prints 2>/dev/null | tr '\n' ' ' | xargs)"
  synth_wer="$("$WER_BIN" --one "$ref" "$synth_hyp")"
  real_wer="$(real_wer_for "$ref")"
  # flag if synth is much worse than real
  if awk -v s="$synth_wer" -v r="$real_wer" -v m="$MARGIN" 'BEGIN{exit (s+0 - r+0 > m+0)?0:1}'; then
    printf '  TTS-unreliable  synth=%s real=%s  %s\n' "$synth_wer" "$real_wer" "$(echo "$ref" | head -c 50)"
    echo "$ref" >> "$SKIPLIST"
    flagged=$((flagged+1))
  fi
done < <(find "$CORPUS" -name '*.txt' ! -name '*.meta.json' | sort)

echo
echo "checked $n phrases; flagged $flagged as TTS-unreliable → $SKIPLIST"
echo "feed $SKIPLIST to voice_tts_harness.sh as an exclusion list."
