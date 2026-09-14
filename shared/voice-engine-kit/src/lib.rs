//! voice-engine-kit — model loading, capture, and dual-tier transcription,
//! lifted out of `apps/voiceterm/src/voice.rs`.
//!
//! ## What this crate is, and is not
//!
//! This is the part of voiceterm's voice engine that has no opinion about
//! WHO is using it: pick a whisper tier, load its model, open a mic stream,
//! run VAD, transcribe. It does not know about speakers, acoustic-index
//! fingerprint caching, corpus recording for later training, or app
//! backgrounding — those are real, valuable things voiceterm layers on top,
//! and they stay in voiceterm (or, for the fingerprint math specifically,
//! in `voiceprint-kit`, extracted separately — see
//! `maintenance/C18_TODO.md` section 18 for that crate's own unfinished
//! repoint).
//!
//! The boundary was not guessed. `maintenance/C18_TODO.md` sections 13, 18
//! and 19 record four rounds of finding a dependency this crate's core
//! functions actually needed, then finding it was optional after all
//! (opt-in behind an env var or an atomic-bool gate) and could be cut with
//! the consuming app re-adding the hook around the call rather than inside
//! it. Read those before assuming a function belongs here just because
//! `voice.rs` used to have it.
//!
//! ## What moved and why each piece is here
//!
//! - [`voice_lang`]: language code resolution (`current()`,
//!   `whisper_lang_code()`) only. Its intent-dispatch function
//!   (`parse_for_lang`, text -> app command) was NOT copied — it alone
//!   pulls in 6,690 lines across six voiceterm-specific parser modules for
//!   a concern (turning speech into an app command) this crate has no
//!   business owning.
//! - [`whisper_pool`]: the background worker pool for streaming/preview
//!   transcription jobs.
//! - [`corpus_recorder`]: opt-in recording of (audio, transcript) pairs for
//!   later training data. Self-contained and gated on a single
//!   atomic-bool, so it moves here rather than being left behind and
//!   re-duplicated — but nothing in THIS crate's own capture path calls
//!   it. A consuming app that wants corpus recording calls
//!   `corpus_recorder::push_samples` itself, from its own capture callback
//!   or a wrapper around this crate's.
//! - [`cpu_efficiency`]: the eco-mode CPU guard `corpus_recorder`'s write
//!   thread uses. Self-contained, moved for the same reason.
//!
//! ## Not yet here (this file is deliberately incomplete)
//!
//! The core functions themselves — model loading, `ensure_capture_running`,
//! `record_and_transcribe_inner` (VAD loop, acoustic-index section
//! stripped), `transcribe_with_dual_model`, `run_one` — are the next piece
//! to land, per `C18_TODO.md` section 19's finished scoping. Landing the
//! sibling modules first and confirming THEY compile standalone is the
//! same incremental-verification discipline this session used throughout:
//! catch a wrong assumption (like `corpus_recorder`'s reach back into
//! `voice.rs`, found and fixed while copying these four files) before it
//! is buried under another thousand lines on top of it.

#[macro_use]
mod log;

pub mod voice_lang;
pub mod whisper_pool;
pub mod corpus_recorder;
pub mod cpu_efficiency;
pub mod power_profile;
