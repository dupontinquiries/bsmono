# Voice pipeline

End-to-end flow from microphone to executed action. This is the
central nervous system of the application.

## High level

```
mic → ring buffer → VAD → whisper × N tiers → intent parse → confirm → execute
                  ↓
                  └─ preview (parallel): phonetic + whisper streaming
```

## Stage 1: capture

`src/voice.rs` owns `RingBuf` and `BgRingBuf`, two ring buffers fed
from the cpal audio callback.

- **RingBuf** (foreground) — gated by `capturing` flag, used for the
  current recording. 30 seconds capacity.
- **BgRingBuf** (background) — always-on, 3 seconds capacity, never
  gated. Used by the wake-word detector and the background fingerprint
  enrichment loop.

Both receive every sample. Foreground is drained at utterance end.

Source format: 44100Hz, 2-channel (varies by device). Resampled to
16000Hz mono before any downstream processing.

## Stage 2: VAD (voice activity detection)

The VAD runs in a watcher thread that tails the RingBuf and detects
silence to know when an utterance ends.

- **Speech threshold**: dynamically calibrated. Initial estimate from
  `audio_profile`'s noise floor; refined live as more speech RMS is
  observed (median of speech-frame RMS).
- **Silence duration**: 825ms of below-threshold RMS triggers
  finalization.
- **Min utterance**: ~400ms to avoid spurious finalizations.

When VAD fires, the foreground ring drains, audio resamples to 16kHz,
and we proceed to recognition.

## Stage 3: recognition

`transcribe_with_dual_model()` in `voice.rs`:

- Runs Tiny, Base, and (if loaded) HighAccuracy whisper models
  concurrently
- All share the same audio input
- A grace window (1000ms) lets higher-accuracy models overtake the
  Tiny result if they arrive in time
- Returns the highest-priority result available

State management is critical: each tier has its own `WhisperState`
cached in a `OnceLock<Mutex<Option<WhisperState>>>` so we reuse the
state across calls (saves the ~210MB allocation). One foreground
inference at a time per tier; multiple tiers run in parallel.

## Stage 4: parallel preview (during recording)

A dedicated preview worker thread runs while `capturing` is true.
Two tiers:

**Tier 1: phonetic match every 250ms.**
- Cheap (~10ms): snapshot ring buffer, resample, MFCC compute, DB
  search via `phonetic::top_matches_with_hint`
- Threshold 0.55 — show low-confidence matches as `"≈ <label>"`
- Catches known utterances (anything the user has voiced + confirmed
  before)

**Tier 2: real whisper streaming every 1500ms.**
- Uses dedicated `PREVIEW_STATE` (separate from foreground states to
  avoid lock contention)
- Runs Tiny on the growing audio buffer
- Only fires when buffer grew ≥ 8000 samples since last call
- Single-segment mode for lower latency
- Output replaces `PARTIAL_PREVIEW` (and updates
  `PARTIAL_PREVIEW_WORDS` for multicolor view)

Both surfaces live in `voice.rs` as `static Mutex<...>`. QML polls.

## Stage 5: phonetic cross-check

After whisper finalizes, we also run the phonetic matcher on the same
audio. If phonetic strongly matches a different label than whisper
heard, we surface both as candidates so the user can pick.

This catches whisper mistranscriptions of known commands (e.g.
"open hacker news" being heard as "open hocker news" gets corrected
because phonetic matches the user's prior fingerprint).

## Stage 6: intent parsing

`voice_lang::parse_for_lang(text)` dispatches by configured language:

- `"en"` → `voice_intent::parse(text)`
- `"es"` → `voice_intent_es::parse_es(text)`
- `"fr"` → `voice_intent_fr::parse_fr(text)`
- other → English fallback

Each parser:

1. Strip filler tokens via `token_stats::strip_fillers`
2. Apply learned confusion corrections (`confusion_table::apply_corrections`)
3. Normalize natural-language scaffolding (politeness prefixes,
   trailing softeners)
4. Match against intent patterns

The intent is an enum `Intent { Navigate, SearchWeb, GoBack, ..., TerminalCommand, Workflow*, Unknown, ... }`.

## Stage 7: speaker identity (v0.1.116+)

After transcription:

```rust
let profile = TonalProfile::from_audio(&audio);
match speakers::identify(&profile) {
    Some((name, sim)) if speakers::is_allowed(&name) => {
        speakers::note_active(&name);
    }
    _ => speakers::clear_active(),
}
```

`speaker_active_name` qt_method becomes the QML signal channel
for who's currently speaking.

## Stage 8: confirmation UI

`voicePopup` in QML shows the recognized text + intent description
+ candidate list. User confirms (yes/no/next via auto-listen, or
button tap).

The auto-listen loop continues listening for a confirmation phrase
after intent display, so users don't have to manually retrigger.

## Stage 9: execution

QML's `executeIntent()` switch dispatches by intent kind:

- `navigate` → `backend.load_page(url, depth)`
- `term_cmd` → `backend.pty_write(cmd + "\r")`
- `workflow_rec_start` → `backend.workflow_start_recording(name)`
- etc.

Some intents have UI side effects (toast, popup state changes).

## Stage 10: learning

After confirmed execution:

- `phonetic::add_fingerprint(label, audio)` stores the audio under
  the recognized command label
- `confusion_table::record_correction(whisper_text, intended_label)`
  if the user picked a non-whisper alternative
- `token_stats::record_utterance(text, kind, confirmed=true)` feeds
  the filler-detection statistics

All persist to disk (if `WEB2QML_PERSIST_LEARNING=1` — default is
session-only).

## Module map for voice flow

| Stage | Module(s) |
|-------|-----------|
| Capture | `voice.rs` (RingBuf, cpal callback) |
| VAD | `voice.rs` (VAD watcher) |
| Recognition | `voice.rs` (transcribe_with_dual_model) |
| Preview | `voice.rs` (ensure_preview_thread) |
| Phonetic match | `phonetic.rs` |
| Intent parse | `voice_intent.rs`, `voice_intent_es.rs`, `voice_intent_fr.rs`, `voice_lang.rs` |
| Speaker identify | `speakers.rs` |
| Confusion correction | `confusion_table.rs`, `phonetic_confusions.rs` |
| Filler strip | `token_stats.rs` |
| Confirmation UI | `qml/main.qml` voicePopup |
| Execution | `qml/main.qml` executeIntent, `main.rs` qt_methods |
| Learning | `phonetic.rs`, `confusion_table.rs`, `token_stats.rs` |

## Performance budget

Target latency from end-of-utterance to confirmation popup ready:

| Stage | Budget |
|-------|--------|
| VAD finalize | ~0ms (event-driven) |
| Resample 44.1k → 16k | ~30ms |
| Tiny whisper | ~200-400ms |
| Base whisper | ~700-1200ms |
| Phonetic match (cross-check) | ~50-150ms |
| Intent parse | <5ms |
| QML render | ~10ms |

Total typical: ~300-500ms to first useful display. Base finishes
~1s, which becomes the recognized text shown. Workable for
interactive use.

## Where to tune

- **Recognition speed**: model tier choice. Tiny is fast but
  inaccurate; HighAccuracy is the opposite. Default Base is
  the trade-off.
- **VAD sensitivity**: `speech_threshold` calibration in `voice.rs`.
- **Preview cadence**: 250ms phonetic, 1500ms whisper. Faster
  costs CPU; slower delays feedback.
- **Phonetic threshold**: 0.55 in preview (permissive), 0.70 in
  cross-check (strict).
- **Speaker threshold**: 0.65 in `speakers::identify`.
- **Filler aggressiveness**: SEED_FILLERS list in `token_stats.rs`
  + learned per-user.
