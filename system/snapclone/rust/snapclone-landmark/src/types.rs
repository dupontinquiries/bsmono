//! Core landmark types.

use serde::{Deserialize, Serialize};

/// 16-byte stable identifier for a landmark.
///
/// Derived from `hash(creator_pubkey || created_at_ms || latitude_e7 || longitude_e7)`
/// so the same physical landmark tagged twice by the same user gets the
/// same id (idempotency for accidental retags), but different users
/// tagging the same point produce different ids (privacy — your "I was
/// here" doesn't collide with someone else's "I was here").
///
/// The actual hash function is plumbed in a future turn — for now this
/// is just a typed wrapper around the bytes so callers don't have to
/// think about byte order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LandmarkId(pub [u8; 16]);

impl LandmarkId {
    /// Convenience: 16 zero bytes. Useful as a placeholder during
    /// construction before the id is computed.
    pub const ZERO: Self = LandmarkId([0u8; 16]);

    /// Render as 32-char lowercase hex. Stable, suitable for use as a
    /// DHT key suffix or a debug log identifier.
    pub fn to_hex(&self) -> String {
        let mut out = String::with_capacity(32);
        for b in self.0 {
            use std::fmt::Write;
            // unwrap: writing to a String never fails
            write!(&mut out, "{b:02x}").unwrap();
        }
        out
    }
}

/// Who can see this landmark.
///
/// Wire encoding: 1 byte. Values picked so plain bit-comparison gives
/// sensible orderings if needed (Private < Friends < Public by exposure
/// breadth).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum Scope {
    /// Stored locally only. Never published.
    Private = 0,
    /// Published to DHT, encrypted to the creator's contact set.
    /// Contacts can decrypt; strangers see only the encrypted blob.
    Friends = 1,
    /// Published in plaintext on the DHT. Anyone can discover.
    /// Reserved for community-curated places (historical sites, etc.).
    Public  = 2,
}

impl Scope {
    /// Round-trip from a wire byte. Unknown values are conservative
    /// — they map to Private so a corrupted/malicious wire payload
    /// can't accidentally publish.
    pub fn from_wire(byte: u8) -> Self {
        match byte {
            1 => Scope::Friends,
            2 => Scope::Public,
            _ => Scope::Private,  // 0 OR any unknown value
        }
    }

    /// Single byte for wire encoding.
    pub fn to_wire(self) -> u8 { self as u8 }
}

/// 12-byte opaque photo fingerprint.
///
/// Layout:
///   - Bytes 0..8:  blur-aHash (perceptual hash of a 8×8 downsampled
///                  blur of the photo; matches against scenes that
///                  look similar after motion blur or compression)
///   - Bytes 8..12: edge fingerprint (Sobel-based edge density signature
///                  at 4 quadrants of the frame; distinguishes "rocks
///                  in front of trees" from "rocks in front of sky")
///
/// Total 12 bytes is intentionally small — landmarks are pointers to
/// places, not photos. The publisher loses information (irreversibly)
/// so even a determined hop in the DHT can't reconstruct the original.
/// Recipients can compare a freshly-captured frame's fingerprint
/// against this and decide "yes, you're looking at the right thing"
/// via Hamming distance threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhotoFingerprint(pub [u8; 12]);

impl PhotoFingerprint {
    /// 12 zero bytes. Used by landmarks tagged without a photo.
    pub const NONE: Self = PhotoFingerprint([0u8; 12]);

    /// True if all 12 bytes are zero — i.e. no fingerprint was captured.
    pub fn is_none(&self) -> bool { self.0 == [0u8; 12] }

    /// Hamming distance to another fingerprint. Returns the total count
    /// of differing bits across all 12 bytes (range 0..96). Lower is
    /// more similar. A threshold of ~16-24 typically corresponds to
    /// "the same scene under different conditions" for blur-aHash;
    /// callers should tune for their precision/recall tradeoff.
    pub fn distance(&self, other: &Self) -> u32 {
        let mut d = 0u32;
        for i in 0..12 {
            d += (self.0[i] ^ other.0[i]).count_ones();
        }
        d
    }
}

/// A user-tagged geographic point.
///
/// `latitude` and `longitude` are stored as f64 in memory and as
/// `i32 = lat * 1e7` on the wire (E7 fixed-point — ~1cm precision,
/// 4 bytes each instead of 8). Conversion happens in the wire module.
///
/// NOTE: serialized EXCLUSIVELY via the manual byte-level codec in wire.rs
/// (encode_wire/decode_wire), never via serde — so it deliberately does NOT
/// derive Serialize/Deserialize. It also couldn't: the [u8; 64] signature field
/// exceeds serde's built-in array impls (which top out at [T; 32]); a stray
/// serde derive here was a latent compile break wherever it got instantiated.
#[derive(Debug, Clone)]
pub struct Landmark {
    /// Stable identifier — see LandmarkId docs for derivation.
    pub id: LandmarkId,

    /// 32-byte ed25519 public key of the creator. Required for signature
    /// verification on the receive side; identifies who claimed this
    /// point in case of disputes / moderation.
    pub creator_pubkey: [u8; 32],

    /// Unix timestamp in milliseconds when the landmark was created.
    pub created_at_ms: i64,

    /// Latitude in degrees, [-90, 90].
    pub latitude:  f64,
    /// Longitude in degrees, [-180, 180].
    pub longitude: f64,

    /// Optional accuracy estimate in meters. Higher = less precise.
    /// 0 means "unknown / not reported". Helps recipients judge
    /// whether the landmark refers to a specific spot or a general area.
    pub accuracy_m: u16,

    /// User-supplied title. Max 64 chars (utf-8 bytes). Truncated on
    /// wire encode if longer.
    pub title: String,

    /// User-supplied description. Max 256 chars. Empty if user gave
    /// just a title.
    pub description: String,

    /// Who can see this landmark.
    pub scope: Scope,

    /// Optional photo fingerprint (12B opaque). PhotoFingerprint::NONE
    /// if the user didn't capture a photo.
    pub photo_fp: PhotoFingerprint,

    /// 64-byte ed25519 signature over the wire-canonical form of all
    /// fields above. Computed by snapclone-identity when publishing;
    /// verified here on receive. Zero bytes when unsigned (private
    /// landmarks don't need a signature since they never leave the
    /// device).
    pub signature: [u8; 64],
}

impl Landmark {
    /// Maximum wire size in bytes — useful for buffer sizing and DHT
    /// payload budgeting. The 134 in the original V32.31 design is the
    /// FIXED portion (sizes below); title+description add up to 320B
    /// max so a fully-populated landmark fits in ~454B on the wire.
    pub const MAX_WIRE_SIZE: usize =
          1   // version
        + 16  // id
        + 32  // creator_pubkey
        + 8   // created_at_ms
        + 4   // latitude_e7
        + 4   // longitude_e7
        + 2   // accuracy_m
        + 1   // scope
        + 12  // photo_fp
        + 64  // signature
        + 2 + 64    // title length-prefix + max bytes
        + 2 + 256;  // description length-prefix + max bytes

    /// Maximum title length in UTF-8 bytes.
    pub const MAX_TITLE_LEN: usize = 64;
    /// Maximum description length in UTF-8 bytes.
    pub const MAX_DESCRIPTION_LEN: usize = 256;
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Scope wire mapping (security-relevant: unknown must NOT publish) ──────

    #[test]
    fn scope_roundtrips_known_values() {
        for s in [Scope::Private, Scope::Friends, Scope::Public] {
            assert_eq!(Scope::from_wire(s.to_wire()), s);
        }
    }

    #[test]
    fn scope_unknown_bytes_map_to_private_not_public() {
        // The whole point: a corrupted/malicious wire byte must default to the
        // most private scope so it can't accidentally publish a landmark.
        for byte in [3u8, 4, 7, 99, 128, 255] {
            assert_eq!(Scope::from_wire(byte), Scope::Private,
                       "unknown scope byte {} must map to Private", byte);
        }
    }

    #[test]
    fn scope_wire_values_are_stable() {
        // Pin the on-wire integers — changing these silently breaks every peer.
        assert_eq!(Scope::Private.to_wire(), 0);
        assert_eq!(Scope::Friends.to_wire(), 1);
        assert_eq!(Scope::Public.to_wire(), 2);
    }

    // ── PhotoFingerprint distance + NONE semantics ───────────────────────────

    #[test]
    fn distance_to_self_is_zero() {
        let fp = PhotoFingerprint([0xAB; 12]);
        assert_eq!(fp.distance(&fp), 0);
    }

    #[test]
    fn distance_is_symmetric_and_bounded() {
        let a = PhotoFingerprint([0x00; 12]);
        let b = PhotoFingerprint([0xFF; 12]);
        assert_eq!(a.distance(&b), 96);     // all 96 bits differ
        assert_eq!(a.distance(&b), b.distance(&a));
        // a single differing bit
        let mut c = a;
        c.0[0] = 0x01;
        assert_eq!(a.distance(&c), 1);
    }

    #[test]
    fn is_none_detects_empty_fingerprint() {
        assert!(PhotoFingerprint::NONE.is_none());
        assert!(PhotoFingerprint([0u8; 12]).is_none());
        let mut almost = [0u8; 12];
        almost[11] = 1;
        assert!(!PhotoFingerprint(almost).is_none());
    }

    #[test]
    fn distance_against_none_is_popcount_caller_must_guard() {
        // CHARACTERIZATION of a sharp edge: distance() does NOT special-case
        // NONE. Comparing a real fingerprint against NONE returns the popcount
        // of the real fp (its set bits), which for a sparse fp can be SMALL and
        // could spuriously pass a similarity threshold. Callers MUST check
        // is_none() before trusting a distance. Pinned so this stays documented.
        let mut bytes = [0u8; 12];
        bytes[0] = 0x01; // one set bit
        let sparse = PhotoFingerprint(bytes);
        // distance to NONE is just the popcount — here 1, which would look like
        // an excellent match under any reasonable threshold.
        assert_eq!(sparse.distance(&PhotoFingerprint::NONE), 1);
        // hence the guard: a NONE on either side means "no comparison possible".
        assert!(PhotoFingerprint::NONE.is_none());
    }
}
