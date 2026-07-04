//! voice-kit — shared whisper-voice post-processing + model resolution.
//!
//! The pure logic that rusty-board (voice keyboard) and voiceterm/web2qml (voice
//! terminal) both apply *around* whisper transcription: filtering whisper's
//! hallucinations (silence sign-off phrases, `[BLANK_AUDIO]` artifacts,
//! repeated-shingle loops) and resolving/identifying ggml models. Both apps grew
//! their own copies with complementary coverage — rusty-board's `hallucination.rs`
//! even credits voiceterm's `filler_words.rs` — so voice-kit is the union:
//! canonical-list matching + trailing-artifact stripping (rusty-board) plus the
//! hallucination-loop trimmer and the "looks like real speech" gate (voiceterm).
//!
//! Layering: voice-kit is the whisper-specific TEXT + MODEL layer. The
//! signal-level math (mic RMS, VAD level) lives in `audio-kit`; the whisper-rs
//! binding and mic capture stay in each app (the IO seam). Everything here is a
//! pure function or plain data — zero-dependency, fully testable.
//!
//! Consumers: rusty-board `src/voice/`, voiceterm/web2qml `src/voice.rs`. A future
//! VAD gate would bridge to audio-kit's [`level::rms`](https://docs.rs) — the one
//! place voice-kit would gain an audio-kit dependency.

pub mod hallucination;
pub mod model;
pub mod pipeline;

pub use hallucination::{
    is_hallucination, looks_like_speech, strip_trailing_artifacts, trim_hallucination_loop,
};
pub use model::{find_model_in, variant_of, ModelSize, Variant, MODEL_CANDIDATES};
