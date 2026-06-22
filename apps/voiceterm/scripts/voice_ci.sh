#!/usr/bin/env bash
# v0.1.705 (#2) — one-command voice test pyramid. Runs all three layers and
# fails fast. Layers that need audio tooling (whisper) are skipped with a clear
# note if it's absent, so this is safe to run anywhere; the pure layers always
# run.
#
#   scripts/voice_ci.sh [--corpus path/to/corpus] [--threshold 0.35] \
#                       [--baseline per_speaker_baseline.tsv]
#
# Exit non-zero if any runnable layer fails.

set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
CORPUS=""; THRESHOLD="${THRESHOLD:-0.35}"; BASELINE=""
while [ $# -gt 0 ]; do
  case "$1" in
    --corpus) CORPUS="$2"; shift 2;;
    --threshold) THRESHOLD="$2"; shift 2;;
    --baseline) BASELINE="$2"; shift 2;;
    *) echo "unknown arg: $1"; exit 2;;
  esac
done

fail=0
echo "════════ voice CI ════════"

# ── Layer 1: parse regression + pure WER unit tests ──
echo
echo "── Layer 1: parse regression + WER unit tests (always run) ──"
if bash scripts/cargo_test_pure.sh 2>&1 | grep -E "wer::|corpus_regression|^test result"; then
  echo "  layer 1 OK"
else
  echo "  layer 1 FAIL"; fail=1
fi

# ── Layer 2: TTS harness (needs whisper + espeak-ng) ──
echo
echo "── Layer 2: TTS harness ──"
if command -v whisper-cli >/dev/null 2>&1 && command -v espeak-ng >/dev/null 2>&1; then
  bash scripts/voice_tts_harness.sh || { echo "  layer 2 FAIL"; fail=1; }
else
  echo "  SKIP — needs whisper-cli + espeak-ng"
fi

# ── Layer 3: recorded-corpus WER (needs whisper + a corpus) ──
echo
echo "── Layer 3: recorded-corpus WER ──"
if [ -n "$CORPUS" ] && command -v whisper-cli >/dev/null 2>&1; then
  args=(--corpus "$CORPUS" --threshold "$THRESHOLD" --mine-confusions)
  [ -n "$BASELINE" ] && args+=(--baseline "$BASELINE")
  bash scripts/corpus_wer_harness.sh "${args[@]}" || { echo "  layer 3 FAIL"; fail=1; }
else
  echo "  SKIP — needs --corpus + whisper-cli"
fi

echo
[ "$fail" -eq 0 ] && echo "════════ voice CI PASS ════════" || echo "════════ voice CI FAIL ════════"
exit "$fail"
