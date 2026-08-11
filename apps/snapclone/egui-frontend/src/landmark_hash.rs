//! landmark_hash — privacy-preserving landmark fingerprints, from
//! `LandmarkHash`.
//!
//! ## The guarantee is the design, not a side effect
//!
//! *"The original photo a user took to mark a landmark NEVER leaves the
//! publisher's device."* What is stored, and eventually published to the DHT, is
//! a pair of **lossy one-way transforms** — and the header is explicit about
//! why: *"an attacker who scrapes the DHT cannot recover faces or text from
//! what they collected."*
//!
//! Eight bytes and four bytes. There is not enough information in twelve bytes
//! to reconstruct anything, which is a much stronger property than "we
//! encrypted it" — there is no key to leak.
//!
//! ## Two hashes because they fail in different weather
//!
//! * **blurHash** — 8-byte average hash of a heavily blurred 64×64 downscale.
//!   *"Heavy gaussian (σ≈8) destroys text, faces, license plates,
//!   micro-textures. What survives: building silhouettes, skyline contour,
//!   dominant colour blocks."* Answers *is this the same place from roughly the
//!   same angle?*
//! * **edgeHash** — 4-byte fingerprint of a thresholded Sobel map on an 8×4
//!   grid. *"Discards colour and brightness entirely; keeps line geometry."*
//!   Answers the *"same alleyway different day / different season"* case, where
//!   a blur hash misses because the light changed.
//!
//! One would not do. A blur hash alone fails at dusk; an edge hash alone
//! matches any two buildings with similar geometry.
//!
//! ## The blur is three box passes, and the reason is cost
//!
//! *"We use a 3-pass box blur because the actual Gaussian kernel at σ=8 needs a
//! 49-tap filter and we want to keep this cheap."* Three box passes converge on
//! a Gaussian by the central limit theorem, and this runs on a phone at
//! landmark-creation time.

/// Blur hash length in bytes — a 64-bit average hash.
pub const BLUR_HASH_LEN: usize = 8;
/// Edge hash length in bytes — an 8×4 cell grid, one bit per cell.
pub const EDGE_HASH_LEN: usize = 4;

/// Working size the blur hash is computed at.
pub const BLUR_SIDE: u32 = 64;
/// Gaussian sigma the box passes approximate.
pub const BLUR_SIGMA: f32 = 8.0;
/// Box passes. Three converges on a Gaussian and costs far less than a 49-tap
/// kernel.
pub const BLUR_PASSES: u32 = 3;

/// Confident blur match: at most this many differing bits of 64.
pub const BLUR_MATCH_BITS: u32 = 10;
/// Confident edge match: at most this many differing bits of 32.
pub const EDGE_MATCH_BITS: u32 = 6;

/// Hamming distance across all bits of two byte strings.
///
/// Returns `None` on a length mismatch rather than comparing a prefix: two
/// hashes of different lengths are different KINDS of hash, and silently
/// comparing the overlap would report a blur hash and an edge hash as similar.
pub fn hamming(a: &[u8], b: &[u8]) -> Option<u32> {
    if a.len() != b.len() { return None; }
    Some(a.iter().zip(b).map(|(x, y)| (x ^ y).count_ones()).sum())
}

/// Which fingerprint a comparison is about.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HashKind { Blur, Edge }

impl HashKind {
    pub fn len(self) -> usize {
        match self { HashKind::Blur => BLUR_HASH_LEN, HashKind::Edge => EDGE_HASH_LEN }
    }
    pub fn bits(self) -> u32 { self.len() as u32 * 8 }
    pub fn threshold(self) -> u32 {
        match self { HashKind::Blur => BLUR_MATCH_BITS, HashKind::Edge => EDGE_MATCH_BITS }
    }
}

/// How two landmarks compare.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Match {
    /// Both hashes agree — the strongest evidence available.
    Both,
    /// Only the blur hash agrees: same place, similar light, geometry differs.
    BlurOnly,
    /// Only the edge hash agrees: same geometry, different light or season.
    EdgeOnly,
    Neither,
    /// One side is missing a fingerprint, so no claim can be made.
    Unknown,
}

impl Match {
    /// Should this be surfaced as the same landmark?
    ///
    /// Either hash alone is enough, because they exist to cover each other's
    /// failure: requiring both would reject exactly the dusk and season cases
    /// the second hash was added for.
    pub fn is_match(self) -> bool {
        matches!(self, Match::Both | Match::BlurOnly | Match::EdgeOnly)
    }

    /// Confidence, for ordering results.
    pub fn rank(self) -> u8 {
        match self {
            Match::Both => 3, Match::EdgeOnly => 2, Match::BlurOnly => 1,
            Match::Neither | Match::Unknown => 0,
        }
    }
}

/// A landmark's stored fingerprints.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Fingerprint {
    pub blur: Option<Vec<u8>>,
    pub edge: Option<Vec<u8>>,
}

impl Fingerprint {
    /// Is this fingerprint structurally valid?
    ///
    /// Checked because a wrong-length hash reaching the matcher is a hash that
    /// silently never matches anything, which looks like "the landmark is not
    /// there" rather than "the data is corrupt".
    pub fn is_valid(&self) -> bool {
        self.blur.as_ref().map(|b| b.len() == BLUR_HASH_LEN).unwrap_or(true)
            && self.edge.as_ref().map(|e| e.len() == EDGE_HASH_LEN).unwrap_or(true)
            && (self.blur.is_some() || self.edge.is_some())
    }

    /// Total bytes published per landmark.
    pub fn published_bytes(&self) -> usize {
        self.blur.as_ref().map(|b| b.len()).unwrap_or(0)
            + self.edge.as_ref().map(|e| e.len()).unwrap_or(0)
    }
}

/// Compare two landmarks.
pub fn compare(a: &Fingerprint, b: &Fingerprint) -> Match {
    let blur = match (&a.blur, &b.blur) {
        (Some(x), Some(y)) => hamming(x, y).map(|d| d <= BLUR_MATCH_BITS),
        _ => None,
    };
    let edge = match (&a.edge, &b.edge) {
        (Some(x), Some(y)) => hamming(x, y).map(|d| d <= EDGE_MATCH_BITS),
        _ => None,
    };
    match (blur, edge) {
        (Some(true), Some(true)) => Match::Both,
        (Some(true), _) => Match::BlurOnly,
        (_, Some(true)) => Match::EdgeOnly,
        (Some(false), Some(false)) => Match::Neither,
        (Some(false), None) | (None, Some(false)) => Match::Neither,
        (None, None) => Match::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blur(b: u8) -> Vec<u8> { vec![b; BLUR_HASH_LEN] }
    fn edge(b: u8) -> Vec<u8> { vec![b; EDGE_HASH_LEN] }

    fn fp(bl: u8, ed: u8) -> Fingerprint {
        Fingerprint { blur: Some(blur(bl)), edge: Some(edge(ed)) }
    }

    #[test]
    fn a_landmark_publishes_twelve_bytes_and_no_more() {
        // The privacy guarantee is arithmetic, not policy: there is not enough
        // information in twelve bytes to reconstruct a face or a licence plate.
        // Stronger than "we encrypted it" — there is no key to leak.
        assert_eq!(fp(0, 0).published_bytes(), 12);
        assert_eq!(BLUR_HASH_LEN + EDGE_HASH_LEN, 12);
    }

    #[test]
    fn identical_photos_match_on_both_hashes() {
        assert_eq!(compare(&fp(0xAA, 0x55), &fp(0xAA, 0x55)), Match::Both);
        assert!(compare(&fp(1, 2), &fp(1, 2)).is_match());
    }

    #[test]
    fn the_thresholds_are_the_documented_ones() {
        // "Distance <= 10 bits (out of 64) is a confident blur match;
        // distance <= 6 bits (out of 32) is a confident edge match."
        assert_eq!(HashKind::Blur.bits(), 64);
        assert_eq!(HashKind::Edge.bits(), 32);
        assert_eq!(HashKind::Blur.threshold(), 10);
        assert_eq!(HashKind::Edge.threshold(), 6);
    }

    #[test]
    fn the_blur_threshold_is_inclusive_at_its_boundary() {
        // Ten differing bits is a match; eleven is not. An off-by-one here
        // shifts every borderline landmark at once.
        let mut near = blur(0);
        near[0] = 0b1111_1111;  // 8 bits
        near[1] = 0b0000_0011;  // 2 more = 10
        assert_eq!(hamming(&blur(0), &near), Some(10));
        let a = Fingerprint { blur: Some(blur(0)), edge: None };
        let b = Fingerprint { blur: Some(near.clone()), edge: None };
        assert_eq!(compare(&a, &b), Match::BlurOnly);

        near[1] = 0b0000_0111; // 11 bits
        let c = Fingerprint { blur: Some(near), edge: None };
        assert_eq!(compare(&a, &c), Match::Neither);
    }

    #[test]
    fn either_hash_alone_is_enough_to_match() {
        // They exist to cover each other's failure: requiring BOTH would reject
        // exactly the dusk and season cases the second hash was added for.
        let dusk = compare(&fp(0x00, 0x55), &fp(0xFF, 0x55));
        assert_eq!(dusk, Match::EdgeOnly, "the season case was rejected");
        assert!(dusk.is_match());
        let angle = compare(&fp(0xAA, 0x00), &fp(0xAA, 0xFF));
        assert_eq!(angle, Match::BlurOnly);
        assert!(angle.is_match());
    }

    #[test]
    fn two_different_places_match_on_neither() {
        assert_eq!(compare(&fp(0x00, 0x00), &fp(0xFF, 0xFF)), Match::Neither);
        assert!(!compare(&fp(0x00, 0x00), &fp(0xFF, 0xFF)).is_match());
    }

    #[test]
    fn agreement_on_both_outranks_agreement_on_one() {
        // Ordering matters when several landmarks are near: the strongest
        // evidence should be offered first.
        assert!(Match::Both.rank() > Match::EdgeOnly.rank());
        assert!(Match::EdgeOnly.rank() > Match::BlurOnly.rank());
        assert_eq!(Match::Neither.rank(), 0);
    }

    #[test]
    fn an_edge_match_outranks_a_blur_match() {
        // Geometry surviving a lighting change is stronger evidence than colour
        // blocks agreeing, since colour blocks agree for any two grey buildings
        // at the same time of day.
        assert!(Match::EdgeOnly.rank() > Match::BlurOnly.rank());
    }

    #[test]
    fn a_missing_fingerprint_is_unknown_rather_than_a_mismatch() {
        // An old landmark stored before hashing existed must not be reported as
        // "definitely somewhere else".
        let none = Fingerprint::default();
        assert_eq!(compare(&none, &fp(1, 2)), Match::Unknown);
        assert!(!compare(&none, &fp(1, 2)).is_match());
    }

    #[test]
    fn hashes_of_different_lengths_never_compare() {
        // Two hashes of different lengths are different KINDS, and silently
        // comparing the overlap would report a blur hash and an edge hash as
        // similar.
        assert_eq!(hamming(&blur(0), &edge(0)), None);
        assert_eq!(hamming(&[], &[0]), None);
    }

    #[test]
    fn a_wrong_length_fingerprint_is_caught_rather_than_never_matching() {
        // A corrupt hash reaching the matcher silently never matches, which
        // looks like "the landmark is not there" rather than "the data is bad".
        let bad = Fingerprint { blur: Some(vec![0; 7]), edge: None };
        assert!(!bad.is_valid());
        assert!(fp(0, 0).is_valid());
        assert!(!Fingerprint::default().is_valid(), "a fingerprint with nothing in it");
    }

    #[test]
    fn a_fingerprint_with_only_one_hash_is_still_valid() {
        // The edge hash was added later, so landmarks exist with only a blur.
        let only_blur = Fingerprint { blur: Some(blur(0)), edge: None };
        assert!(only_blur.is_valid());
        assert_eq!(only_blur.published_bytes(), 8);
    }

    #[test]
    fn hamming_counts_every_bit_not_every_byte() {
        // Counting differing BYTES would make the 10-of-64 threshold mean
        // something completely different.
        assert_eq!(hamming(&[0b0000_0000], &[0b0000_0001]), Some(1));
        assert_eq!(hamming(&[0b0000_0000], &[0b1111_1111]), Some(8));
        assert_eq!(hamming(&blur(0), &blur(0xFF)), Some(64));
    }

    #[test]
    fn the_blur_uses_three_box_passes_for_the_stated_reason() {
        // "The actual Gaussian kernel at sigma=8 needs a 49-tap filter and we
        // want to keep this cheap." Three box passes converge on a Gaussian and
        // this runs on a phone at landmark-creation time.
        assert_eq!(BLUR_PASSES, 3);
        assert_eq!(BLUR_SIGMA, 8.0);
        assert_eq!(BLUR_SIDE, 64);
    }
}
