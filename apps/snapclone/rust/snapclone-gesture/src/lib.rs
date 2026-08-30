//! snapclone-gesture — the model moved to the kit; snapclone keeps the FFI.
//!
//! The two-tier gesture model now lives in `shared/gesture-kit`, extracted
//! at its second call site (studio's panel navigation). This crate is the
//! same public surface via re-export — nothing that linked or `use`d it
//! changes — plus the C FFI seam (`ffi`), which stays here because it
//! exists for THIS app's Qt frontend, not for the model.

#![forbid(unsafe_op_in_unsafe_fn)]

pub mod ffi;

pub use gesture_kit::*;
