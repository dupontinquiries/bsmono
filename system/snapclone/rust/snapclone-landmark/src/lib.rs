//! snapclone-landmark — typed landmark records, scope policy, wire format.
//!
//! V32 introduces "landmarks": user-tagged geographic points with optional
//! photo fingerprints. Use cases:
//!
//!   - "I saw this view from this spot" — friends can navigate to your view
//!   - "This is where the concert was" — turn an ephemeral location into a
//!     persistent reference you can share
//!   - "This restaurant is now closed" — community-curated catalog of
//!     defunct, demolished, or otherwise interesting places
//!
//! Three scope levels control visibility:
//!
//!   - `Scope::Private`: stored locally only, never sent anywhere.
//!   - `Scope::Friends`: published on the DHT under a key derived from the
//!     creator's identity, encrypted so only contacts can decrypt.
//!   - `Scope::Public`: published as plaintext, anyone can discover.
//!
//! Photo fingerprint is OPAQUE on the wire: 8 bytes of perceptual hash
//! (blur-aHash from snapclone-saliency) + 4 bytes of edge fingerprint.
//! Recipients can't reconstruct the photo from the fingerprint — they can
//! only verify "the camera is looking at the same thing this landmark was
//! tagged with." Matching is fuzzy (Hamming distance threshold).
//!
//! This crate exposes:
//!   - Core types (`Landmark`, `Scope`, `PhotoFingerprint`)
//!   - Wire format encoder/decoder (`encode_wire`, `decode_wire`)
//!   - Signature verification entry point (stub — real signing plumbed
//!     to snapclone-identity in a future turn)
//!   - Error type
//!
//! What it does NOT do (intentionally — these belong to other layers):
//!   - DHT publish/lookup (snapclone-dht)
//!   - Photo fingerprint generation (snapclone-saliency)
//!   - Storage / persistence (snapclone-storage)
//!   - UI presentation (C++ side)

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod types;
pub mod wire;
pub mod verify;

pub use types::{Landmark, LandmarkId, PhotoFingerprint, Scope};
pub use wire::{decode_wire, encode_wire, WIRE_FORMAT_VERSION};
pub use verify::{verify_signature, VerifyError};

/// Re-export of the unified error type so callers don't need to think
/// about which module owns which error.
pub use error::LandmarkError;

mod error {
    //! Single error enum for the crate. Variants are kept thin —
    //! callers pattern-match on the variant, not the message.
    use thiserror::Error;

    /// All errors this crate produces.
    #[derive(Debug, Error)]
    pub enum LandmarkError {
        /// Wire payload was too short to contain even the header.
        #[error("wire payload truncated: have {have} bytes, need at least {need}")]
        Truncated { have: usize, need: usize },

        /// Wire format version isn't supported.
        #[error("unsupported wire format version: {0}")]
        UnsupportedVersion(u8),

        /// A length-prefixed field declared a length larger than the
        /// remaining payload. Indicates corruption or malicious input.
        #[error("length-prefixed field overruns payload: declared {declared}, remaining {remaining}")]
        LengthOverrun { declared: usize, remaining: usize },

        /// Coordinates fell outside the valid range. Latitudes must be
        /// [-90, 90], longitudes [-180, 180]. Out-of-range coords on the
        /// wire are treated as malformed rather than auto-clamped.
        #[error("coordinate out of range: lat={lat} lon={lon}")]
        BadCoordinate { lat: f64, lon: f64 },

        /// Title / description exceeded the per-field length cap.
        /// Caps are conservative on purpose — landmarks are pointers,
        /// not blog posts.
        #[error("text field too long: field={field}, len={len}, max={max}")]
        TextTooLong { field: &'static str, len: usize, max: usize },

        /// Signature verification failed. Generic on purpose — never
        /// leak signature-specific detail (key vs payload mismatch
        /// vs malformed bytes) since that could help forge attacks.
        #[error("signature verification failed")]
        BadSignature,

        /// Internal: an unexpected condition that should never occur
        /// in normal operation. Indicates a bug in this crate.
        #[error("internal error: {0}")]
        Internal(&'static str),
    }
}
