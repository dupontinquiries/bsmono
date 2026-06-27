//! Whisper hallucination filter — now a thin shim over `voice-kit`.
//!
//! These filters (canonical-list matching + trailing-artifact stripping) were
//! lifted into `voice_kit::hallucination`, alongside voiceterm's complementary
//! loop-trimmer and speech gate. Re-exported here so the call sites in
//! `ui/window.rs` stay unchanged.

pub use voice_kit::hallucination::{is_hallucination, strip_trailing_artifacts};
