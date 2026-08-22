//! MOVED to `shared/peer-handshake`.
//!
//! This module and its two siblings were dependency-free and stuck inside an
//! application crate that has no `[lib]` and pulls qmetaobject, so the egui
//! frontend could not reach the state machine without dragging Qt in. Re-export
//! rather than copy: two copies of a handshake is the copy that drifts, and a
//! handshake that drifts trusts the wrong peer.
pub use peer_handshake::handshake::*;
