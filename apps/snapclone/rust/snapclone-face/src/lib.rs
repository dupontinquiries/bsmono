//! snapclone-face — face detection and face-aware bandwidth optimization.
//!
//! This crate underpins SnapClone's plan to reduce mobile data usage
//! for face-containing photos by storing a local library of faces the
//! user has already sent or received, and (in Phase 2+) using those
//! as reference frames for delta-compressed snap transmission.
//!
//! See `docs/FACE_BANDWIDTH.md` for the full design rationale. The
//! short version:
//!
//!   - **Opportunistic compression.** When the system can save
//!     bandwidth by sending a "delta against past snap" instead of
//!     a full image, it does. When it can't, it falls back to a
//!     full image. Recipients never see degraded reconstruction —
//!     either a clean delta-reconstructed image or a clean full
//!     image, never a half-broken neural artefact.
//!
//!   - **Personal data only.** The local library stores crops from
//!     photos the user took (selfies) and photos contacts chose to
//!     send the user. Never strangers, never images outside the
//!     SnapClone communication graph.
//!
//!   - **Cold start = current behavior.** Day-one users have an empty
//!     library; every snap sends as full image. The library grows
//!     over weeks of use; compression kicks in opportunistically.
//!     No "wait three weeks before SnapClone works" experience.
//!
//! # Module map
//!
//!   - `detector` — face detection (YOLO-class ONNX). Real, ready
//!     to wire up. Determines "is there a face here?" and where it is.
//!   - `library` — Phase 1 active. Per-label face crop storage with
//!     LRU eviction, byte budgets, and consent gating. Foundation
//!     for everything else.
//!   - `reference` — Phase 2 skeleton. Reference selection helpers
//!     for delta compression. Implementations land once the codec
//!     choice is settled.
//!   - `consent` — opt-in state and trait for persistence.
//!   - `embedding` & `identity` — **DEFERRED**. Scaffolded in V32.32
//!     for a prior generic-face-recognition direction that's been
//!     superseded by the personal-bandwidth approach. Kept in the
//!     crate (rather than removed) because if Phase 3 ever ships
//!     and decides to use embeddings, the API is already there.
//!     Not on the critical path; do not consume from new code.
//!
//! # Phase status (V32.32)
//!
//!   - Phase 1 (data collection): IMPLEMENTED in this crate. C++
//!     wiring to capture/inbox paths is a separate turn.
//!   - Phase 2 (delta compression): SKELETON. Real impl pending
//!     codec decision (WebP-Ref, AVIF-Ref, or custom).
//!   - Phase 3 (on-device neural model): DEFERRED. See TODO.md
//!     for prerequisites.

// Crate-wide lint posture:
// - `deny(unsafe_code)` (was `forbid`) so the Rust-only modules
//   (library, detector, reference, consent) reject any unsafe by
//   default. They genuinely don't need raw pointers.
// - We downgrade from `forbid` to `deny` so the FFI module (`ffi.rs`)
//   can opt out with `#![allow(unsafe_code)]`. C FFI by definition
//   requires `unsafe extern "C"` and `#[no_mangle]`, both of which
//   the unsafe-code lint flags. `forbid` is final and can't be
//   overridden per-module; `deny` is the right level here.
#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod detector;
pub mod library;
pub mod reference;
pub mod consent;

// FFI surface for the C++ side. Lives behind a module rather than at
// the crate root so the symbols are organized; the `#[no_mangle]
// extern "C"` declarations inside still get linked into the staticlib.
pub mod ffi;

// Deferred — kept for forward compatibility with Phase 3, do not
// import from new code:
pub mod embedding;
pub mod identity;

// ── Active re-exports (Phase 1 + 2) ─────────────────────────────────
pub use detector::{Detection, Detector, DetectorConfig};
pub use library::{
    Crop, Label, Library, InMemoryLibrary,
    MAX_CROPS_PER_LABEL, DEFAULT_LIBRARY_BUDGET_BYTES,
};
pub use reference::{
    ReferenceCandidate, pick_best_reference, compute_perceptual_hash,
    hash_distance, MIN_USEFUL_REFERENCE_SCORE,
};
pub use consent::{ConsentState, ConsentStore, InMemoryConsentStore};

// ── Deferred re-exports (Phase 3 if ever) ───────────────────────────
// Re-exporting these because some downstream callers may already
// reference them from V32.32 scaffolding. New code should not.
pub use embedding::{Embedder, FaceEmbedding, EMBEDDING_DIM};
pub use identity::{ContactId, FaceProfile, IdentityMatcher, MatchResult};

/// Unified error type. Variants are kept thin — pattern-match on
/// variant, not on the embedded message.
#[derive(Debug, thiserror::Error)]
pub enum FaceError {
    /// Model file missing or corrupt at the given path.
    #[error("model load failed: {0}")]
    ModelLoad(String),

    /// Inference failed at runtime.
    #[error("inference error: {0}")]
    Inference(String),

    /// An input image was empty, too small, or otherwise unusable.
    #[error("invalid input: {0}")]
    InvalidInput(&'static str),

    /// The requested operation is disabled by consent. Callers should
    /// treat this as a soft "feature off" rather than an error to
    /// display.
    #[error("face recognition is disabled by user consent")]
    ConsentDisabled,
}
