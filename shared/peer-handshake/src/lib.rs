//! peer-handshake — establishing trust between two devices over SOUND.
//!
//! Hoisted out of `apps/voiceterm/src/` because the egui frontend needed the
//! state machine and could not have it: voiceterm's crate has no `[lib]` and
//! pulls `qmetaobject` unconditionally, so depending on it to reach a pure
//! `(state, event) -> state` function would have dragged Qt into an egui app.
//!
//! Both modules were already dependency-free — `audio_dialup` imports nothing
//! at all and `peer_handshake` imports only `audio_dialup`. The only thing
//! keeping them in an application crate was where they happened to be written.
//!
//! The alternative was reimplementing the protocol in the frontend, which is
//! the duplication that has cost this fleet a week: a second copy of a rule is
//! the copy that drifts, and a handshake that drifts is one that trusts the
//! wrong peer.

pub mod audio_dialup;
/// The voice-print maths. Pulled in with the other two because handshake.rs
/// reaches it through `crate::speaker_embed` — a path, not a `use`, which is
/// why a grep for imports said the pair was self-contained and it was three.
pub mod speaker_embed;
pub mod handshake;

pub use handshake::{
    embed_mfcc, embedding_from_wire, embedding_to_wire, is_terminal, step, voice_matches,
    HandshakePayload, HsEvent, HsState,
};
