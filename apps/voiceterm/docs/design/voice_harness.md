# Voice-command test harness (v0.1.696)

User: "build out a test harness for voice commands using the tts and the files
I have uploaded in the past." (No pre-recorded command WAVs exist in uploads —
only images/docs/zips — so audio is SYNTHESIZED.)

## Two layers (test pyramid)
1. PARSE correctness — SANDBOX-runnable, no audio.
   - Fixture: tests/voice_corpus.txt — "<phrase>\t<expected_kind>" lines.
   - Test: voice_intent::corpus_regression::corpus_phrases_route_as_expected
     reads the fixture (include_str!) and asserts
     parse(phrase).to_qml_string()'s kind field == expected, for all phrases.
   - EVERY pair was validated against the LIVE parser. Building it found 6 of
     19 initial guesses WRONG (e.g. "open the editor" → edit_current_note not
     open_in_editor; "clear the terminal" → freeform; "search google for X" →
     search). The corpus now encodes verified ground truth (23 pairs), so a
     future parser change that silently re-routes a command fails the test.
   - Validated in-sandbox via a staged crate (voice_intent + editor_open +
     confusion_table real; token_stats/system_info/speakers stubbed — none
     are exercised by the corpus phrases). The full-binary `cargo test` target
     has unrelated pre-existing breakage in other modules; the staged run is
     the authoritative check until that's cleaned up.
2. RECOGNITION drift — LOCAL, needs audio: scripts/voice_tts_harness.sh.
   - Speaks each corpus phrase (piper neural TTS preferred, espeak-ng
     fallback) → 16kHz mono WAV → whisper-cli (same binary as voice.rs) →
     transcript. Checks the phrase's key words survive (overlap ≥ threshold).
   - Catches homophone/accent/codec drift that text-parse tests can't.
   - Usage: scripts/voice_tts_harness.sh [--tts piper|espeak] [--model PATH]

## Extending
Add a phrase+kind to tests/voice_corpus.txt, then run the regression test; if
it fails, the parser disagrees — fix the expectation (parser is truth) or the
parser. Then run the TTS harness locally to confirm it's recognizable aloud.

## Layer 3 — recorded-voice corpus WER (v0.1.704)

The test pyramid now has three layers, weakest→strongest signal:

1. **Parse regression** (tests/voice_corpus.txt + corpus_regression) — phrase
   text → expected intent KIND. Catches routing regressions. No audio.
2. **TTS harness** (scripts/voice_tts_harness.sh) — synthesized speech →
   whisper → key-word overlap. Catches gross STT drift with synthetic audio.
3. **Recorded-corpus WER** (scripts/corpus_wer_harness.sh) — the user's REAL
   recordings (corpus.zip: paired .opus + reference .txt, by speaker) →
   ffmpeg decode to 16kHz mono → whisper-cli → word error rate vs reference.
   Strongest signal: real human audio through the real STT path.

### The corpus
corpus/ has speaker folders (spk_*, unknown/, and a named onboarding dir with
spoken_name.wav @ 44.1kHz). 51 .opus+.txt pairs across 6 dirs. Transcripts are
dictation/reading captures — web UI text, CAD specs, shell commands, short
phrases. The .opus are Ogg Opus, 48kHz mono.

### Scoring (src/wer.rs, 8 tests)
normalize_words (lowercase, strip punctuation/symbols incl. the corpus's
⁄·↓emoji separators, collapse whitespace) → word-level Levenshtein → WER =
edits / ref_words. corpus_wer pools (total edits / total ref words), the
standard report (not the mean of per-utterance WERs, which over-weights short
clips). Exposed as the wer_score qt_method for in-app checking against the live
whisper path.

### Running it (local — needs whisper)
    scripts/corpus_wer_harness.sh --corpus path/to/corpus \
        --model ~/.local/share/whisper/ggml-base.en.bin \
        [--speaker spk_XXX] [--max N] [--threshold 0.35] [--save baseline.tsv]
Decodes each clip, transcribes, prints per-clip WER, then a pooled WER gated
against --threshold (exit non-zero on regression). The scorer is a standalone
rustc binary (scripts/wer_score.rs, mirrors src/wer.rs) so the harness needs
only rustc + ffmpeg + whisper, not the whole crate.

Verified in-sandbox: ffmpeg opus→16kHz-mono-WAV decode works; the full
plumbing (find pairs → decode → score → pool → gate) runs end-to-end with a
stub whisper. Only the real transcription needs the user's local whisper.
