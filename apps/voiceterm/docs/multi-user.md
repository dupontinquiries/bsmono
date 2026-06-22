# Multi-user collaboration

The system architecture that lets multiple people share one screen,
one voice channel, and one terminal session — with the system knowing
who said what.

## Why this exists

The user described it: "interleaving partner code sessions and
situations where a more knowledgeable expert would want to just show
and not tell for a bit then go back to letting newer users learn in
the terminal."

The goal is fluid handoff. Expert drives ("show, don't tell") by
voicing commands directly. Learner explores by typing or speaking
their own. System distinguishes them visually so it's never ambiguous
who's currently in control.

## Status (v0.1.120)

**Working:**
- Speaker registry: register a person's voice profile under a name +
  color. Stored in `~/.config/web2qml/speakers.toml`.
- Per-utterance speaker identification: after foreground voice
  transcribes, `speakers::identify()` matches against the registry.
- Per-CHUNK speaker identification (v0.1.119): preview thread
  classifies ~500ms windows via Separator with 3-chunk hysteresis.
- Per-speaker audio routing (v0.1.120): chunks accumulate into
  per-speaker buffers; on per-speaker silence, audio submits to
  whisper_pool for finalized transcription tagged with speaker name.
- Hard whitelist: muted speakers' utterances are discarded (no intent
  fires); visual preview during recording still shows them
  (acceptable trade-off).
- Speaker circles overlay in voicePopup: 20px colored circles, one per
  registered speaker. Pulse animation when active. Tap to mute/unmute.
- Multicolor live transcript view: whisper-streamed words colored
  per speaker, with fade-in animation. Per-speaker finalized
  transcripts REPLACE the cumulative-streamed words for that
  speaker when they arrive.
- Register-self UI: appears in voicePopup after a recognized utterance
  when no speakers are registered yet.

**Foundations built but not yet fully exploited:**
- `whisper_pool`: 1-4 long-lived workers. With 1 worker (default),
  per-speaker submissions queue. Set `WEB2QML_WHISPER_WORKERS=2`
  for parallel processing.

**Not yet implemented:**
- True acoustic source separation (overlapping speech still
  attributed to the dominant speaker)
- Time-aligned word seating across speakers (per-speaker words
  append in arrival order, not strict chronological)
- Hand-touch handoff gesture (physical tap on screen claims floor)
- Observer mode (allowed but doesn't drive commands)
- Per-speaker command permissions (e.g. learner can read, expert can
  modify)
- Background-channel transcript for muted speakers
- "+ another" UI to register additional speakers after the first

## Pipeline (current and target)

```
                        ┌─────────────────┐
mic audio ─────────────▶│  ring buffer    │
                        └────────┬────────┘
                                 │ snapshot every 250ms
                                 ▼
                        ┌─────────────────┐
                        │ preview thread  │
                        └────────┬────────┘
                                 │
                ┌────────────────┴────────────────┐
                │                                 │
                ▼                                 ▼
       ┌─────────────────┐               ┌──────────────────┐
       │ phonetic match  │ (every 250ms) │  whisper stream  │ (every 1500ms)
       │   (dumb-DB)     │               │     (tiny)       │
       └────────┬────────┘               └────────┬─────────┘
                │                                 │
                ▼                                 ▼
                       PARTIAL_PREVIEW (string)
                       PARTIAL_PREVIEW_WORDS (per-word, tagged)
                                 │
                                 ▼
                       speakers::identify on full audio
                                 │
                                 ▼
                       speakers::note_active(name)
                                 │
                                 ▼
                          QML poll surface
                            (every 150-200ms)
                                 │
                                 ▼
                       speaker circles + multicolor view
```

The **TARGET** pipeline (per-chunk routing) splits before the
whisper call:

```
mic audio ──▶ ring buffer ──▶ chunk every 500ms
                                  │
                                  ▼
                    Separator::classify_chunk
                                  │
                ┌─────────────────┼─────────────────┐
                ▼                 ▼                 ▼
        speaker_A_buffer  speaker_B_buffer    rejected (noise)
                │                 │
                ▼ on VAD finalize ▼
        whisper_pool ◀────────────┘
            │
            ▼
        InferenceResult { text, speaker, ... }
            │
            ▼
        PARTIAL_PREVIEW_WORDS update with speaker tag
```

## Module responsibilities

### `src/speakers.rs`

- `Speaker { name, color, profile: TonalProfile, sample_count, allowed }`
- Persisted to `~/.config/web2qml/speakers.toml` (human-readable;
  hand-edit safe)
- `add_speaker(name, profile)` — register or update (weighted average
  blend; existing samples weighted by `sample_count`)
- `identify(profile) -> Option<(name, similarity)>` — best-match
  registered speaker if similarity >= 0.65
- `is_allowed(name)`, `set_allowed(name, allowed)` — whitelist gates
- `note_active(name)`, `current_active() -> String`, `clear_active()`
  — UI signal channel; QML polls `speaker_active_name` every 150ms

### `src/separation.rs`

- `Separator` — per-session classifier with hysteresis state
- `classify_chunk(audio_16k) -> Decision` — returns Speaker/Unknown/Silence
- Hysteresis: 3 consecutive matching decisions required to change
  active speaker. Prevents per-chunk pitch noise from flipping
  speaker on every chunk (~1.5s latency to switch)
- Not yet wired to the preview thread

### `src/whisper_pool.rs`

- 1-4 long-lived worker threads, each holding a `WhisperState`
- Shared `Arc<WhisperContext>` (the Tiny context by default)
- Job channel: `submit(Job { audio, speaker, generation })`
- Result deque: `drain_results() -> Vec<InferenceResult>`
- Configurable via `WEB2QML_WHISPER_WORKERS` env (default 1)
- Memory cost: ~210MB per worker after first job

## Speaker identity quality

TonalProfile-based matching is the foundation. Features in the profile:

- `pitch_hz` — fundamental frequency estimate
- `rms` — energy level
- `activity` — fraction of frames with voicing
- `spectral_centroid_hz` — "brightness"
- `voicing_prob` — fraction of frames classified as voiced
- `energy_low`, `energy_mid`, `energy_high` — three-band power

`TonalProfile::similarity()` weights pitch (0.7) and activity (0.3).
Threshold 0.65 for "this is them"; below = unknown.

**Honest limits:**
- People with similar pitch will be confused (parent/child often fine,
  two adult males of similar age/build maybe not).
- Same person varying voice style (whispered, energetic) will score
  lower against their own registered profile. Solution: register
  multiple times across styles, blending strengthens.
- TonalProfile per ~500ms chunk is significantly noisier than
  per-utterance (which integrates more frames). Hysteresis in
  Separator compensates.

**Future improvements:**
- MFCC-based speaker embeddings (more discriminative than TonalProfile;
  ~256 floats vs 8)
- Learn per-speaker thresholds (some voices fingerprint cleanly at
  0.75, others struggle at 0.55)
- Adversarial validation during registration ("speak again, but try
  to sound different" — ensures profile captures range)

## Whitelist semantics

Today (v0.1.117):

- `allowed=true` (default): identified speaker drives `note_active` →
  circle pulses → words colored
- `allowed=false`: identified speaker does NOT drive `note_active`
  (circle stays static, no color tag on words)
- Words still appear in the live view either way (just colored
  neutrally when speaker is muted)
- Intent pipeline runs regardless of `allowed` — commands still
  execute

Future: hard gating where disallowed speakers' audio is dropped
before whisper. Useful for filtering side-channel chatter during
focused sessions.

## Handoff gesture (TODO)

Vision: physical tap on screen claims the floor for whoever just
touched. Mechanism options:

1. **Speaker circle tap → "I'm next"** — already exists for
   toggling allowed; could be repurposed for floor claim. Tap your
   own circle to take over.
2. **General screen tap during recording** — claims floor for
   speaker associated with most recent touch. Requires correlating
   touch events with current voice activity.
3. **Physical button** — explicit handoff button on the UI. Less
   ambiguous, more buttons.

Current thinking: option 1 (tap own circle) for explicit "I'm
driving now" + option 3 for the "everyone shut up" case (mute all
others temporarily).

## Live view rendering details

`_wordsView` in QML:

- Flow layout (wraps naturally at popup width)
- Polls `voice_partial_preview_words()` every 200ms
- Each word: text + speaker name + color (hex) + arrival_ms
- Fade-in opacity = `min(1, max(0.3, age_ms / 280))`
- Behavior NumberAnimation smooths between poll steps so the visible
  result is not stepped

`_spkRow` in QML:

- Centered Row of 20px circles
- Pulses via SequentialAnimation on scale when active
- Tap toggles allowed

When zero speakers are registered, both `_wordsView` and `_spkRow`
collapse to height 0 (Behavior NumberAnimation 160ms). Solo users
fall back to the legacy `_previewBox` single-line preview.

## How to test

1. Build, run.
2. Voice an utterance.
3. Register self: from QML console or test button,
   `backend.speaker_register_current("Me")`.
4. Voice again — stderr shows `[speakers] identified "Me"`, circle
   appears and pulses.
5. For partner: have them voice, then call
   `backend.speaker_register_current("Partner")`.
6. Take turns. Active circle should track who's speaking (subject
   to per-utterance latency).
7. Tap a circle to toggle mute. Greyed appearance + opacity drop.

## Edge cases observed

- **Whispered speech reduces pitch confidence.** Speaker identification
  drops to "Unknown" even for registered speakers. Multi-style
  fingerprints (v0.1.109) help; user should register multiple times.
- **Background music or TV** can match a registered speaker if
  pitch/energy ranges overlap. Future: explicit noise classifier
  before identification.
- **Very brief utterances (<1s)** have unreliable profiles. We
  require minimum 4000 samples (250ms) before scoring; even so,
  short commands like "yes" are riskier than longer phrases.

## Privacy

Speaker registry stays local. `~/.config/web2qml/speakers.toml` is
plain text, owned by the user. The voice profile is acoustic features
(pitch, RMS, etc) not raw audio. Sharing the file is safe in the
sense that it doesn't contain recordings — but the profile IS unique
to a voice and could in principle be matched against other voice
recordings. Treat it like a low-grade biometric.

No speaker data is transmitted off-device. No cloud sync.
