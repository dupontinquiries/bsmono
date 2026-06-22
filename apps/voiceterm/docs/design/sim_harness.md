# TTS-driven workflow simulation harness (v0.1.655)

## Why
The user asked: "you can synthesize voice data — make a test harness that
feeds workflows via simulation." Yes. espeak-ng synthesizes real speech;
we run it through the actual pipeline. This complements tests/voice_cases.rs
(which needs recorded WAVs + a whisper model) by GENERATING audio and by
testing the text→intent boundary with no model needed.

## Two layers (scripts/sim-workflow.sh)
1. **Audio plumbing** — espeak-ng synthesizes a scripted workflow of command
   phrases to WAV; the WAVs feed the real phonetic pipeline (compute_mfcc +
   acoustic_index::fingerprint). Proven: real speech → sensible MFCC frame
   counts (scale with phrase length) → 39-dim fingerprints.
2. **Intent routing** — stages voice_intent + its dep closure (security,
   util, async_jobs, llm_cache, sysmon, phonetic, confusion_table,
   editor_open, speakers, system_info, token_stats, voice_intent_es/fr) as
   a standalone crate and RUNS workflow→intent assertions. No model, no
   audio, CI-able.

## What it found (real bugs, first run)
- **recall regression**: "what did i do last time" was being swallowed by
  the audit-log arm ("what did X do" → ShowAuditLogForSpeaker) because that
  arm runs before the recall arm and its first-person guard only rejected a
  bare "i", not "i do last time". FIXED (guard now skips any "i "/"we "
  prefix). Permanent regression assertion added in voice_intent.rs tests +
  the sim.
- confirmed the Spanish parser routes correctly via parse_es (the top-level
  parse() is English; the dispatcher voice_lang::parse_for_lang selects es
  when voice_lang::current()=="es" — so "abre terminal" failing under an
  en config is correct, tying back to the v652 model finding).

## HONEST SCOPE (what it does NOT validate)
- **Real-human speaker-ID accuracy**: espeak voices are spectrally near-
  identical (one synthesis engine + pitch shift, not different vocal
  tracts) — their 39-dim fingerprints barely separate (~0.0001–0.0012
  cosine). That is NOT evidence the embedding fails on humans; espeak is
  the wrong tool for accuracy. Use real recorded voices
  (speaker_detection_plan.md).
- **Whisper transcription quality**: no model in the sandbox/CI, so the
  audio→text step runs on-device (tests/voice_cases.rs). Layer 2 tests
  text→intent directly, where clean robotic speech is fine.

## Standalone-staging recipe (reused from cargo_test_pure.sh)
A temp crate with a generated lib.rs of `#[path=...] pub mod X;` lines + a
clog! stub, then `cargo test`. Lets modules that aren't in the pure tier
(voice_intent has 5 crate deps) be exercised in isolation.

## v0.1.656 — audio augmentation for more variety (scripts/sim_augment.py)
User asked to repitch / add distortion / vary word spacing for variety, and
to try the surgeshaper-tanh crate. Tested all of it.

### surgeshaper-tanh: NOT usable (tested, not assumed)
Exists on crates.io but is alpha (0.2.12-alpha.0), pulls 136 packages (a
slice of the Surge synth framework), and transitively needs coreaudio-sys
whose build script panics on Linux (macOS-only). Dead end. A tanh
waveshaper is `tanh(drive*x)/tanh(drive)` — three lines, no dependency.

### Measured findings (variants run through the REAL 39-dim fingerprint)
Pairwise cosine distance of "run cargo test" variants:
  - PITCH (clean vs low/high): ~0.0000–0.0001. Pitch barely moves the
    fingerprint — MFCCs encode FORMANTS (vocal-tract shape), and espeak's
    pitch knob shifts F0 without changing formants. So repitching one
    engine ≈ identical fingerprint. Good for pitch-BIN robustness testing,
    NOT separation.
  - DISTORTION (tanh, clean vs dist2/dist5): 0.0011 → 0.0039, monotonic
    with drive — ~40x the pitch effect. Nonlinear waveshaping adds
    harmonics the MFCCs see. The right lever for spectral variety. BUT
    still tiny vs real human separation (~20x): one synthetic source
    perturbed is not different vocal tracts. Use for ROBUSTNESS, not
    speaker-accuracy.
  - WORD SPACING (most valuable): splice per-word audio with randomized
    inter-word gaps. An "erroneous" 900ms gap exceeds the 880ms VAD silence
    threshold (runtime logs) → reproduces a PREMATURE VAD CUTOFF mid-phrase
    ("run cargo" [stop] "test" lost). A real failure class the harness can
    now generate on demand.

### Bottom line
Augmentation extends SINGLE-USER simulation usefully for pipeline
ROBUSTNESS (pitch bins, distortion tolerance, VAD silence handling). It does
NOT manufacture real-speaker separation — that still needs recorded human
voices (speaker_detection_plan.md). espeak + augmentation is the right tool
for the former, the wrong tool for the latter, and the harness docs say so.
