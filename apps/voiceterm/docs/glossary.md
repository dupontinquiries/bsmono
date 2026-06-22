# Glossary

Terms and acronyms used in web2qml.

## Voice / audio

- **ANSI escape** — terminal control sequence starting `\x1b[` for
  colors, cursor moves, etc. Parsed by `pty_term.rs`.
- **CPAL** — Cross-Platform Audio Library. Rust crate we use for
  microphone capture.
- **DTW** — Dynamic Time Warping. Used in `phonetic.rs` to align
  two MFCC sequences of different lengths.
- **Filler** — disfluency or politeness word stripped before intent
  parsing. "um", "uh", "please", etc. See `token_stats.rs`.
- **Fingerprint (phonetic)** — MFCC sequence + TonalProfile +
  metadata stored as a labeled exemplar in `voice_fingerprints.bin`.
- **MFCC** — Mel-Frequency Cepstral Coefficients. 13-dimensional
  feature vector per frame summarizing speech spectrum. Foundation
  of phonetic matching.
- **Phantom cell** — `ATTR_PHANTOM` bit-7 marker in `pty_term::Cell`.
  Right half of a wide character (CJK, emoji); rendered as nothing
  so the wide glyph's left half draws across both cell positions.
- **Pitch hint** — speaker's typical fundamental frequency, used to
  bias phonetic matching toward fingerprints from similar pitch
  range. See `audio_profile.rs`.
- **Style ID** — fingerprint classification (1=normal, 2=whispered,
  3=energetic, 4=noisy, 5=fragmented). See `phonetic.rs::classify_style`.
- **TonalProfile** — 8-float speaker descriptor (pitch_hz, rms,
  activity, spectral_centroid_hz, voicing_prob, energy_low/mid/high).
- **VAD** — Voice Activity Detection. Our VAD is RMS-threshold based:
  silence of 825ms below a calibrated threshold finalizes the
  utterance.
- **Wake word** — phrase recognized in always-listening background
  mode that activates foreground capture. Default "hey claude" or
  user-registered phrase.
- **Whisper** — OpenAI's speech recognition model. We use whisper.cpp
  via whisper-rs.
- **Whisper tier** — model size: Tiny (~39M params, fast), Base (~74M,
  default), HighAccuracy (large variants, slow but accurate).

## Multi-user

- **Active speaker** — speaker most recently identified during current
  recording. `speakers::current_active()`. Drives UI pulse animation.
- **Allowed / disallowed** — speaker whitelist flag. Disallowed
  speakers still appear in live transcript but don't drive UI signals
  or (future) command pipeline.
- **Floor** — concept from speaker handoff. Whoever has the floor is
  driving commands. Future: hand-touch gesture claims the floor.
- **Hysteresis (in Separator)** — requirement that N consecutive
  chunks must agree before changing active speaker. Prevents
  per-chunk noise from flipping classification.
- **Speaker circle** — small colored circle in voicePopup UI
  representing one registered speaker. Pulses when active.

## Terminal

- **Block (terminal block)** — one command + its output. Tracked by
  `term_blocks::BlockTracker`. Each block can have a summary, missing-
  package list, exit code, duration.
- **Cell** — one grid position in the terminal. Carries char, fg/bg
  color, attribute bits. `pty_term::Cell`.
- **PTY** — pseudo-terminal. Linux IPC mechanism for emulating a
  terminal. Our process owns the master; the child shell owns the
  slave.
- **Scroll region** — DEC private mode subset of rows that scroll
  together when text overflows. Used by some TUIs (less, vim).
- **Smart-cd** — when `cd` changes directory, we track the previous
  CWD and offer an undo toast for ~6s.

## Browser

- **alx.sh-style** — single-file static sites using CSS-only
  interactivity (radio buttons drive `:checked` state for tabs/menus).
  See `docs/browser.md` (TODO).
- **Compat UA** — fallback User-Agent string sent when a server
  rejects our default UA. Some sites block "ureq".
- **Layout cache** — qml_gen output cached by content hash for
  unchanged sections (NavBar, Generic, etc).

## Build / process

- **qt_method** — qmetaobject-rs macro for declaring a method
  callable from QML on the Backend object.
- **OnceLock\<Mutex\<...\>\>** — typical pattern for lazy-init
  thread-safe state in Rust modules.
- **Worker pool** — `src/whisper_pool.rs`. Long-lived threads
  pulling from a job channel. v0.1.116+.

## Data files

- `~/.config/web2qml/voice_fingerprints.bin` — phonetic DB, format FP04.
- `~/.config/web2qml/audio_profile_v2.bin` — speaker tonal profile.
- `~/.config/web2qml/tokens.bin` — token stats (filler classifier).
- `~/.config/web2qml/confusion_table.bin` — learned whisper corrections.
- `~/.config/web2qml/decision_cache.bin` — phrase → command cache.
- `~/.config/web2qml/llm_cache.bin` — LLM Q&A cache.
- `~/.config/web2qml/speakers.toml` — speaker registry (v0.1.116+).
- `~/.config/web2qml/workflows/<name>.toml` — workflow definitions.
- `~/.config/web2qml/models/ggml-*.bin` — whisper model files.

## Environment variables

- `WEB2QML_PERSIST_LEARNING=1` — enable persistence of voice
  fingerprints, confusion table, token stats. Default off.
- `WEB2QML_VOICE_LANG=<lang>` — set voice input language (en/es/fr/
  auto/...). Default en.
- `WEB2QML_WHISPER_WORKERS=N` — number of whisper pool workers.
  Default 1. Cap 4.
- `WEB2QML_DEBUG=1` — disable security hardening (allow coredumps,
  ptrace). For debugging only.
