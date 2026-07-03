//! Reference selection and negotiation for Phase 2 delta compression.
//!
//! Phase 2 of the face-bandwidth project: when sending a snap that
//! contains a face, the sender looks in its local library for past
//! snaps of the same person and picks one as a delta-encoding
//! reference. The recipient applies the delta to their local copy of
//! the same reference to reconstruct the new snap.
//!
//! This module provides the building blocks:
//!
//!   - `ReferenceCandidate` — a library crop being considered as a
//!     reference, with a similarity score
//!   - `pick_best_reference` — given the detected face and a set of
//!     candidate crops, score each and return the best
//!   - `compute_perceptual_hash` — small perceptual hash used for
//!     fast "are these crops similar" checks before more expensive
//!     pixel diffing
//!
//! What this module deliberately does NOT do (yet):
//!
//!   - Actual delta encoding / decoding — that's a separate concern
//!     and will live in a media-codec module once we decide on the
//!     codec (WebP-ref, AVIF-ref, or something custom)
//!   - Network negotiation — the sender↔recipient "do you have
//!     reference X?" handshake is a transport concern; this module
//!     only computes the reference_hash that the transport carries.
//!
//! This is a SKELETON. The function signatures and types are
//! intended to be stable; the implementations are minimal/stub.
//! Concrete work will land once the codec choice is settled.

use crate::library::{Crop, Label, Library};

/// One candidate reference crop with a similarity score against the
/// probe.
///
/// Score semantics: 0.0 = no similarity, 1.0 = identical. Score
/// combines perceptual-hash distance with quality and recency
/// weights — the goal is "best candidate to actually save bandwidth
/// when used as a delta reference," not pure visual similarity.
#[derive(Debug, Clone)]
pub struct ReferenceCandidate {
    /// The crop being considered.
    pub crop: Crop,
    /// Similarity score in [0.0, 1.0].
    pub score: f32,
}

/// Compute an 8-byte perceptual hash of a small RGB image.
///
/// Average-hash (aHash) variant: downsample to 8×8 grayscale, take
/// the mean, output 64 bits where each bit is "is this pixel above
/// the mean."
///
/// Robust to small lighting, blur, and scale changes; sensitive to
/// large content changes. Hamming distance between two hashes is a
/// fast proxy for "are these crops of the same scene."
///
/// V32.32: stub returns all zeros — the real implementation lands
/// once we wire `library::Crop`'s bytes through to a decoded RGB
/// buffer (currently the library stores opaque encoded bytes, so
/// hashing here would need a decode step we haven't yet committed
/// to a single image crate for).
pub fn compute_perceptual_hash(_rgb: &[u8], _width: u32, _height: u32) -> [u8; 8] {
    // The real implementation:
    //
    //   1. Resize rgb to 8x8 using a fast box filter
    //   2. Convert to grayscale: luma = 0.299R + 0.587G + 0.114B
    //   3. Compute mean of the 64 luma values
    //   4. Build 64-bit hash: bit i = (pixel_i > mean)
    //   5. Pack into [u8; 8]
    //
    // ~50 lines of code; held until library bytes have a canonical
    // decode path.
    [0u8; 8]
}

/// Hamming distance between two perceptual hashes (count of
/// differing bits). 0 = identical hashes, 64 = maximally different.
///
/// Empirical thresholds for face crops at 8×8 aHash:
///   - 0-8   = very likely same scene
///   - 9-16  = probably same scene under variation
///   - 17-24 = could be same person, different shot
///   - 25+   = probably different content
pub fn hash_distance(a: &[u8; 8], b: &[u8; 8]) -> u32 {
    let mut d = 0u32;
    for i in 0..8 {
        d += (a[i] ^ b[i]).count_ones();
    }
    d
}

/// Pick the best reference candidate from the library for a given
/// probe face crop.
///
/// `probe_rgb` is the decoded RGB bytes of the face being sent.
/// `probe_label` is which library label to search — typically
/// `Label::SelfFace` when the user is sending their own selfie, or
/// `Label::Contact(sender_id)` when forwarding someone else's face
/// (not yet a use case, but the API supports it).
///
/// `library` is the source of candidate crops; we'll request up to
/// `max_candidates` recent crops to score.
///
/// Returns `None` when:
///   - No crops with that label exist in the library
///   - Best candidate's score is below the minimum useful threshold
///     (the calling code should then send the snap as a normal full
///     image instead of attempting delta compression — the cold-
///     start / poor-match safety net)
///
/// V32.32: stub. Returns None always. Wire-up pending real
/// `compute_perceptual_hash` + a codec-aware scoring function.
pub fn pick_best_reference(
    _probe_rgb: &[u8],
    _probe_width: u32,
    _probe_height: u32,
    probe_label: &Label,
    library: &dyn Library,
    max_candidates: usize,
) -> Option<ReferenceCandidate> {
    // The real implementation:
    //
    //   1. Compute perceptual hash of probe
    //   2. Pull up to max_candidates recent crops with probe_label
    //   3. For each candidate, compute its hash, get hash distance
    //   4. Score = combine(1 - hash_distance/64, quality, recency)
    //   5. Pick highest score; return Some(...) if above threshold,
    //      None otherwise
    //
    // Held until perceptual_hash has its real implementation.
    let _ = (probe_label, library, max_candidates);
    None
}

/// Minimum useful similarity score. Below this, delta compression
/// is unlikely to save bandwidth vs. just sending the full image,
/// so callers should fall back. Conservative default — Phase 2
/// can tune based on real measurements.
pub const MIN_USEFUL_REFERENCE_SCORE: f32 = 0.7;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_distance_matches_xor_popcount() {
        let a = [0xFF; 8];
        let b = [0x00; 8];
        assert_eq!(hash_distance(&a, &b), 64);
        assert_eq!(hash_distance(&a, &a), 0);
        let c = [0x0F; 8];
        // each byte: 0xFF ^ 0x0F = 0xF0 = 4 ones, 8 bytes total = 32
        assert_eq!(hash_distance(&a, &c), 32);
    }

    #[test]
    fn pick_best_reference_returns_none_with_empty_library() {
        let lib = crate::library::InMemoryLibrary::new();
        let probe = vec![0u8; 112 * 112 * 3];
        assert!(pick_best_reference(
            &probe, 112, 112, &Label::SelfFace, &lib, 10,
        ).is_none());
    }
}
