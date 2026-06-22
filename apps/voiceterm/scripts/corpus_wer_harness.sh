#!/usr/bin/env bash
# v0.1.704 — recorded-voice corpus WER harness (LOCAL; needs whisper).
#
# The user's corpus (corpus.zip) pairs each .opus recording with a reference
# .txt transcript, organized by speaker. This harness measures REAL recognition
# quality (not synthesized): decode each .opus → 16kHz mono WAV (ffmpeg) →
# transcribe (whisper-cli, same binary as voice.rs) → word error rate vs the
# reference. Complements voice_tts_harness.sh (which uses synthesized speech)
# and the parse regression (which checks intent routing on text).
#
# This is the strongest signal of the three: real human audio through the real
# STT path. A regression here means whisper config / model / preprocessing
# actually got worse for this speaker set.
#
# Usage:
#   scripts/corpus_wer_harness.sh --corpus path/to/corpus [--model PATH] \
#       [--speaker spk_XXX] [--max N] [--threshold 0.30] [--save baseline.tsv]
#
# Exit non-zero if pooled WER exceeds --threshold.

set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CORPUS=""
WMODEL="${WHISPER_MODEL:-$HOME/.local/share/whisper/ggml-base.en.bin}"
SPEAKER=""
MAX=0
THRESHOLD="${THRESHOLD:-0.35}"
SAVE=""
BASELINE=""
TOLERANCE="0.05"
MINE_CONFUSIONS=0

while [ $# -gt 0 ]; do
  case "$1" in
    --corpus) CORPUS="$2"; shift 2;;
    --model) WMODEL="$2"; shift 2;;
    --speaker) SPEAKER="$2"; shift 2;;
    --max) MAX="$2"; shift 2;;
    --threshold) THRESHOLD="$2"; shift 2;;
    --save) SAVE="$2"; shift 2;;
    --baseline) BASELINE="$2"; shift 2;;        # v0.1.705 (#1) per-speaker diff
    --tolerance) TOLERANCE="$2"; shift 2;;
    --mine-confusions) MINE_CONFUSIONS=1; shift;; # v0.1.705 (#4)
    *) echo "unknown arg: $1"; exit 2;;
  esac
done

[ -z "$CORPUS" ] && { echo "!! --corpus <dir> required (extracted corpus/)"; exit 2; }
[ -d "$CORPUS" ] || { echo "!! corpus dir not found: $CORPUS"; exit 2; }

WHISPER_BIN="$(command -v whisper-cli || command -v whisper-cpp || command -v main || true)"
[ -z "$WHISPER_BIN" ] && { echo "!! no whisper-cli/whisper-cpp on PATH" >&2; exit 3; }
command -v ffmpeg >/dev/null || { echo "!! ffmpeg required to decode .opus" >&2; exit 3; }

# the WER scorer: a tiny rust binary built from src/wer.rs (see
# scripts/wer_score.rs). Build once.
WER_BIN="$ROOT/target/wer_score"
if [ ! -x "$WER_BIN" ]; then
  echo "building wer_score…"
  rustc --edition 2021 -O "$ROOT/scripts/wer_score.rs" -o "$WER_BIN" 2>/dev/null \
    || { echo "!! couldn't build wer_score (need rustc)"; exit 3; }
fi

TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
echo "corpus WER harness: model=$(basename "$WMODEL") threshold=$THRESHOLD"
echo "  corpus=$CORPUS ${SPEAKER:+speaker=$SPEAKER}"
echo

transcribe() {  # $1 wav -> stdout transcript
  "$WHISPER_BIN" -m "$WMODEL" -f "$1" --no-prints 2>/dev/null | tr '\n' ' ' | xargs
}

pairs_tsv="$TMP/pairs.tsv"     # ref<TAB>hyp per line, for pooled WER
triples_tsv="$TMP/triples.tsv" # speaker<TAB>ref<TAB>hyp, for per-speaker
: > "$pairs_tsv"; : > "$triples_tsv"
n=0
while IFS= read -r opus; do
  ref_txt="${opus%.opus}.txt"
  [ -s "$ref_txt" ] || continue
  if [ -n "$SPEAKER" ] && [[ "$opus" != *"$SPEAKER"* ]]; then continue; fi
  n=$((n+1))
  [ "$MAX" -gt 0 ] && [ "$n" -gt "$MAX" ] && { n=$((n-1)); break; }
  wav="$TMP/clip.wav"
  ffmpeg -y -i "$opus" -ar 16000 -ac 1 "$wav" >/dev/null 2>&1
  hyp="$(transcribe "$wav")"
  ref="$(tr '\n\t' '  ' < "$ref_txt")"
  # speaker = the immediate parent dir name (spk_*, unknown, …)
  spk="$(basename "$(dirname "$opus")")"
  printf '%s\t%s\n' "$ref" "$hyp" >> "$pairs_tsv"
  printf '%s\t%s\t%s\n' "$spk" "$ref" "$hyp" >> "$triples_tsv"
  printf '  %-44s wer=%s\n' "$(basename "$opus")" "$("$WER_BIN" --one "$ref" "$hyp")"
done < <(find "$CORPUS" -name '*.opus' | sort)

echo
echo "scored $n clips"

# ── per-speaker breakdown (#1) ──
echo
echo "per-speaker WER (pooled within speaker):"
"$WER_BIN" --pooled-speaker "$triples_tsv" | while IFS=$'\t' read -r spk wer rw; do
  printf '  %-28s %s  (%s ref words)\n' "$spk" "$wer" "$rw"
done

# ── baseline diff (#1) ──
if [ -n "$BASELINE" ] && [ -f "$BASELINE" ]; then
  echo
  echo "vs baseline ($BASELINE, tolerance $TOLERANCE):"
  cur_spk="$TMP/cur_spk.tsv"
  "$WER_BIN" --pooled-speaker "$triples_tsv" > "$cur_spk"
  regressed=0
  while IFS=$'\t' read -r spk wer rw; do
    base="$(awk -F'\t' -v s="$spk" '$1==s{print $2}' "$BASELINE")"
    if [ -z "$base" ]; then
      printf '  %-28s %s  (new — no baseline)\n' "$spk" "$wer"
    else
      if awk -v w="$wer" -v b="$base" -v t="$TOLERANCE" 'BEGIN{exit (w+0 > b+0+t+0)?0:1}'; then
        printf '  %-28s %s  ▲ REGRESSED from %s\n' "$spk" "$wer" "$base"; regressed=1
      else
        printf '  %-28s %s  ok (baseline %s)\n' "$spk" "$wer" "$base"
      fi
    fi
  done < "$cur_spk"
  [ "$regressed" -eq 1 ] && { echo "FAIL — a speaker regressed"; exit 1; }
fi

# ── confusion mining (#4) ──
if [ "$MINE_CONFUSIONS" -eq 1 ]; then
  echo
  echo "top word confusions (ref → hyp × count) — candidates for confusion_table:"
  "$WER_BIN" --subs "$pairs_tsv" | head -20 | while IFS=$'\t' read -r rw hw c; do
    printf '  %-20s → %-20s ×%s\n' "$rw" "$hw" "$c"
  done
fi

# ── save per-speaker baseline (#1) ──
if [ -n "$SAVE" ]; then
  "$WER_BIN" --pooled-speaker "$triples_tsv" | cut -f1,2 > "$SAVE"
  echo
  echo "saved per-speaker baseline → $SAVE"
fi
POOLED="$("$WER_BIN" --pooled "$pairs_tsv")"
echo "pooled WER: $POOLED  (threshold $THRESHOLD)"
awk -v w="$POOLED" -v t="$THRESHOLD" 'BEGIN{ exit (w+0 > t+0) ? 1 : 0 }' \
  && echo "PASS" || { echo "FAIL — recognition regressed"; exit 1; }
