# Voice base (speech-to-text)

The upstream half of the voice pipeline is now in: **mic → whisper → `voice_ingest::Pass`**, feeding the
downstream that already existed (`voice_ingest::Transcriber` → `voicefan` → `reaction`).

## Pieces

- `crates/logscope-egui/src/whisper_pool.rs` — ported from web2qml's voiceterm. A pool of 1–4 whisper
  worker threads, each holding an `Arc<WhisperContext>` + its own `WhisperState`. `submit(Job)` /
  `drain_results() -> Vec<InferenceResult>`. This is the efficient-batching primitive for STT.
- `crates/logscope-egui/src/voice.rs` — the bridge:
  - `init(model)` — load a whisper GGML model, start the pool (once).
  - `VoiceCapture::start(speaker)` — open the default mic (cpal), stream mono audio, energy-VAD into
    utterances, resample to 16 kHz, submit to the pool. The **app owns** the `VoiceCapture` (a cpal
    `Stream` is `!Send`, so it lives on the UI thread) and calls `poll()` each frame.
  - `poll_passes() -> Vec<Pass>` — drain finished transcriptions as `voice_ingest::Pass`.
- Everything is behind the `voice` feature; without it, stubs compile so the default build is unchanged.

## Build (on the M2)

```
# system deps: libasound2-dev (or pipewire-alsa) for cpal; a C/C++ toolchain + cmake for whisper.cpp
cargo build --release --features "gui,gpu-scene,voice-vulkan"   # GPU whisper via Vulkan (Asahi)
```

Env: `LOGSCOPE_WHISPER_MODEL` (path to `ggml-*.bin`; default `$XDG_DATA_HOME/logscope/whisper/ggml-base.en.bin`),
`LOGSCOPE_STT_WORKERS` (default 1), `LOGSCOPE_STT_LANG` (default `auto`), `LOGSCOPE_STT_VAD` (energy
threshold, default 0.012).

## Not build-verified in-sandbox

The sandbox lacks ALSA headers and a whisper.cpp toolchain, so the `voice` feature itself can't be
compiled here (like the llama.cpp backend). What IS verified: the default build is green with the base
staged, the `voice` Cargo graph resolves, and the downstream `voice_ingest`/`voicefan` tests pass (11).
The `whisper_pool` is a verbatim port of working code; `voice.rs`'s cpal capture is written against the
cpal 0.15 / whisper-rs 0.16 APIs and wants a first build + mic test on the M2 (VAD threshold may need a nudge).

## API layer (done) — `voice_session.rs`

`VoiceSession` is the single surface the app holds and pumps. `poll() -> VoiceTick` returns finalized
`utterances`, the live `partial`, and a barge-in `reaction`. The Chat app owns `Option<VoiceSession>`,
pumps it every frame in `ui()` (`pump_voice`), and routes utterances through `on_voice_utterance` — the
ONE seam future voice features extend. A 🎙 Mic button in the composer toggles it; finalized speech
dictates into the composer today; while the model streams, a barge-in cancels generation. All of this
compiles and links in every build (stubs when `voice` is off); the mic button reports a clear error if
voice isn't compiled in or no model/mic is found.

## Voice-graph intents (done) — `voice_intent.rs`

`voice_intent::parse(text) -> VoiceIntent` (pure + unit-tested) maps speech to intents; `on_voice_utterance`
dispatches. Most are modal `Command`s reused via `run_command`:
  - "go to 12" / "focus on three" / "node seven" / "jump to twenty one" → Goto (digits + number words)
  - "branch here" · "prune" · "unprune" · "overview" · "show the palace"/spatial · "back to chat"/linear
    · "best" · "stop"/"cancel"
Handled directly: "start tour"/"stop tour" → fly-through, "free cam"/"exit freecam" → freecam,
"send it"/"submit" → send the composer. Anything else dictates. Adding a command is one arm in
`voice_intent::parse` (+ rarely a dispatch arm). 3 parser tests + the app wiring are green.

## Next (natural graph interaction)

More intents + readback:


Grow `on_voice_utterance`: parse `u.text` for graph intents before the dictation fallback —


Wire it into the chat/scene UI: a mic toggle that holds a `VoiceCapture`, pumps `poll()` + `poll_passes()`
into a `Transcriber`, and turns finalized `Utterance`s into graph turns — then voice-driven graph
interaction. That's the natural-interaction layer we do next.


## voiceterm ports (this session) — logging, fast-path, confusion table, preroll, multi-speaker, multi-language

- **clog!** (log.rs): category-routed logging. `LOGSCOPE_LOG=voice,whisper` routes those categories to
  `~/.config/logscope/logs/<category>.log` for isolated debugging; unset = stderr only (unchanged).
- **Fast-path** (fastpath.rs): races Tiny/Base/HighAccuracy whisper tiers per utterance (adaptive grace
  window: 300ms for short commands up to 1000ms for dictation). Additive — drop a `ggml-tiny*`/`ggml-base*`
  file in `~/models` (or set LOGSCOPE_WHISPER_TINY/_BASE) to enable; with only one model it's the plain
  single-tier path, unchanged. Cached WhisperState per tier avoids ~200MB alloc churn per call.
- **Confusion table** (voice_confusion.rs): self-learning misheard→intended map, seeded with command-vocab
  mishears ("brunch"→"branch", etc.), applied as a pre-pass before intent parsing. Persists to
  `~/.config/logscope/confusions.bin`. `record(misheard, intended)` is the hook for future explicit
  corrections; needs 2 confirmations before a learned (non-seeded) correction applies.
- **Preroll** (voice.rs): a ~300ms rolling ring buffer seeds each utterance on speech onset, so the syllable
  that trips the (necessarily lagging) RMS threshold isn't clipped.
- **Multi-speaker** (voice.rs + voice_session.rs): `LOGSCOPE_SPEAKERS="Alice:USB Headset,Bob:Built-in Mic"`
  opens one mic capture per named speaker; all feed the shared whisper pipeline, so `voicefan`'s existing
  per-speaker branching sees a proper mix. Single-mic default is unchanged.
- **Multi-language** (voice_lang.rs + voice_intent_{es,fr,de}.rs): LOGSCOPE_STT_LANG picks both whisper's
  transcription language and the command-intent parser. Spanish/French/German have real translated command
  vocabularies (incl. number words 0-30) and fall back to the English parser for anything unmatched — the
  same honest scope voiceterm itself has (a few real languages, not a stub per ISO code).

All of the above is unit-tested at the parser/logic level (whisper-independent); the audio-path pieces
(fast-path race, preroll, multi-mic capture) build and run only on the M2, like the rest of `voice`.
