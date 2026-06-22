# Voice testing

Three layers, weakest → strongest signal. One entry point: `scripts/voice_ci.sh`.

```
scripts/voice_ci.sh --corpus path/to/corpus --threshold 0.35 \
    --baseline docs/voice/per_speaker_baseline.tsv
```

Layers that need audio tooling skip cleanly (with a note) when `whisper-cli` /
`espeak-ng` aren't present, so this runs anywhere; the pure layers always run.

## Layer 1 — parse regression + WER unit tests (no audio)
- `tests/voice_corpus.txt` + the `corpus_regression` test: phrase text →
  expected intent KIND. Catches routing regressions.
- `src/wer.rs` unit tests: the WER scorer, alignment, confusion mining.
- Run alone: `bash scripts/cargo_test_pure.sh`.

## Layer 2 — TTS harness (needs whisper + espeak-ng)
`scripts/voice_tts_harness.sh` — synthesized speech → whisper → key-word
overlap. Gross STT-drift check with synthetic audio.

## Layer 3 — recorded-corpus WER (needs whisper + a corpus)
`scripts/corpus_wer_harness.sh` — the REAL recordings (paired `.opus` +
reference `.txt`, by speaker) → ffmpeg decode → whisper → word error rate.

```
scripts/corpus_wer_harness.sh --corpus path/to/corpus \
    --model ~/.local/share/whisper/ggml-base.en.bin \
    [--speaker spk_X] [--max N] [--threshold 0.35] \
    [--baseline per_speaker_baseline.tsv] [--tolerance 0.05] \
    [--mine-confusions] [--save per_speaker_baseline.tsv]
```

- **Per-speaker breakdown** — pooled WER per speaker, so you see *who*
  regressed, not just the aggregate.
- **`--baseline`** — diff each speaker against a saved baseline; exit non-zero
  if any speaker regressed beyond `--tolerance`.
- **`--save`** — write the current per-speaker WER as a new baseline
  (`speaker<TAB>wer`).
- **`--mine-confusions`** — print the top word substitutions (ref → hyp ×
  count); systematic mishears are candidates to add to `confusion_table`.

### Establishing the first baseline
Run once to see real numbers, then save them and tune the threshold ~0.05–0.10
above the observed pooled WER:
```
scripts/corpus_wer_harness.sh --corpus CORPUS --save docs/voice/per_speaker_baseline.tsv
```

## Finding TTS-unreliable phrases (Layer 2 hygiene)
`scripts/voice_tts_compare.sh` — synthesize each reference, compare synth WER
to the real-recording WER; phrases where TTS is much worse (`--margin`) are
written to a skiplist. These are "words not to test TTS with if they sound
off" — exclude them from the TTS harness so synthesis artifacts don't masquerade
as recognizer regressions.

```
scripts/voice_tts_compare.sh --corpus CORPUS \
    --real-baseline real_pairs.tsv --margin 0.25 --skiplist tts_skiplist.txt
```

## What's verified in-sandbox vs. needs a machine with whisper
The WER scorer and all alignment/confusion/baseline logic are unit-tested
(`src/wer.rs`, also mirrored in `scripts/wer_score.rs`). The ffmpeg opus→16kHz
decode and espeak-ng synthesis are verified to run. Only the whisper
transcription step needs a local model — the harness scripts are otherwise
proven end-to-end.
