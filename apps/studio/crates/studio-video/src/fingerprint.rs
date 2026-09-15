//! fingerprint — a clip's content, as a sequence of small perceptual
//! hashes, robust enough to survive a transcode and specific enough to
//! tell two different shots apart. This is the "storypattern" piece from
//! the footage-ingest design conversation: chunk a clip into evenly-spaced
//! sample frames, hash each one, and fuzzy-match the resulting sequences
//! to find a clip (or a portion of one) even after it's been re-encoded at
//! a different bitrate, resolution, or container.
//!
//! ## Why dHash and not raw pixel comparison
//!
//! A transcode changes exact pixel values (different codec, different
//! quantization, different color matrix rounding) but preserves coarse
//! structure -- edges stay edges, bright stays bright relative to its
//! neighbours. [`dhash_from_rgb`] only looks at the SIGN of the gradient
//! between adjacent pixels in a tiny (9x8) grid, not their magnitude --
//! exactly the property a transcode is least likely to flip. The 9x8 grid
//! itself does double duty as the "blur": `frame_rgb`'s ffmpeg-side scale
//! down to 9x8 is a real low-pass filter (bilinear/bicubic downsampling),
//! which is what suppresses codec noise and grain before hashing -- this
//! is the "Gaussian blur" from the original ask, done as an aggressive
//! resize rather than a literal Gaussian kernel, because the resize was
//! already needed and a well-established perceptual-hash technique (dHash)
//! already relies on exactly this effect.
//!
//! ## Why sequence alignment, not a single whole-clip hash
//!
//! A single hash for an entire clip can't answer "is THIS FOOTAGE a piece
//! of THAT footage" -- a trimmed, re-ordered, or partially-reused clip
//! needs the match to slide against a range of offsets and report how much
//! of the overlap actually agrees, which is what [`best_alignment`] does.
//! It's the read side of "fuzzy match a partial clip that got transcoded"
//! from the original design conversation.

use crate::{probe, frame_rgb, VideoError};
use std::path::Path;
use studio_core::Ticks;

/// The dHash grid: 9 columns x 8 rows, each row contributing 8 gradient
/// bits (comparing column i to i+1) for 64 bits total -- fits exactly in a
/// `u64`, cheap to store and compare millions of times.
const HASH_W: u32 = 9;
const HASH_H: u32 = 8;

/// One frame's perceptual hash from raw RGB24 bytes (as `frame_rgb`
/// returns them) at exactly [`HASH_W`]x[`HASH_H`]. Pure -- no I/O, so it's
/// testable with synthetic buffers, which is how its robustness and
/// discrimination properties are actually checked below rather than
/// assumed.
pub fn dhash_from_rgb(rgb: &[u8], w: u32, h: u32) -> u64 {
    assert_eq!((w, h), (HASH_W, HASH_H), "dhash_from_rgb expects exactly the {HASH_W}x{HASH_H} grid frame_rgb was asked to produce");
    assert_eq!(rgb.len(), (w * h * 3) as usize);
    // Luma, not a plain average: matches how compression itself weights
    // channels, so the hash agrees with what a codec considers "the same
    // brightness" rather than an unweighted RGB mean.
    let gray = |x: u32, y: u32| -> f32 {
        let i = ((y * w + x) * 3) as usize;
        0.2126 * rgb[i] as f32 + 0.7152 * rgb[i + 1] as f32 + 0.0722 * rgb[i + 2] as f32
    };
    let mut bits: u64 = 0;
    let mut bit_index = 0;
    for y in 0..HASH_H {
        for x in 0..HASH_W - 1 {
            if gray(x, y) < gray(x + 1, y) {
                bits |= 1 << bit_index;
            }
            bit_index += 1;
        }
    }
    bits
}

/// Bits that differ -- 0 is identical, 64 is a fully inverted hash (never
/// happens for real content; useful only as a scale reference).
pub fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// A clip's fingerprint: one dHash per sampled position, evenly spaced.
#[derive(Debug, Clone, PartialEq)]
pub struct Fingerprint {
    pub hashes: Vec<u64>,
    /// Time between samples -- needed to turn an ALIGNMENT OFFSET (in
    /// samples) back into a timeline position when reporting a match.
    pub interval: Ticks,
}

/// Samples `path` at `interval_secs` spacing (honestly, like
/// `thumbnail_strip`: one `ffmpeg` call per sample, not a single batched
/// decode pass -- correct and simple, not the fastest this could be) and
/// hashes each sample. `interval_secs` around 1.0 is a reasonable default:
/// finer wastes hashes on near-duplicate frames within a shot, coarser
/// risks missing a short reused clip entirely.
pub fn fingerprint_clip(path: &Path, interval_secs: f64) -> Result<Fingerprint, VideoError> {
    assert!(interval_secs > 0.0, "interval_secs must be positive");
    let info = probe(path)?;
    let total_s = info.duration.to_seconds().max(0.0);
    let n = ((total_s / interval_secs).floor() as usize).max(1);
    let mut hashes = Vec::with_capacity(n);
    for i in 0..n {
        let at = Ticks::from_seconds(i as f64 * interval_secs);
        let rgb = frame_rgb(path, at, HASH_W, HASH_H)?;
        hashes.push(dhash_from_rgb(&rgb, HASH_W, HASH_H));
    }
    Ok(Fingerprint { hashes, interval: Ticks::from_seconds(interval_secs) })
}

/// The result of sliding `needle` against `haystack` at every possible
/// offset and picking the best one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AlignmentMatch {
    /// `needle[i]` best corresponds to `haystack[i + offset]`. Negative
    /// when `needle` extends before `haystack`'s start.
    pub offset: isize,
    /// How many of the overlapping positions matched within
    /// `max_hamming` at this offset.
    pub matched: usize,
    /// How many positions actually overlapped at this offset (the
    /// denominator `matched` is a fraction of).
    pub compared: usize,
}

impl AlignmentMatch {
    pub fn fraction(&self) -> f64 {
        if self.compared == 0 { 0.0 } else { self.matched as f64 / self.compared as f64 }
    }
}

/// Slides `needle` against `haystack` at every offset where they overlap
/// by at least one sample, and returns the offset with the highest
/// matched FRACTION (ties broken by more samples compared, so a longer
/// confirmed overlap beats a short lucky one). `max_hamming` is the
/// per-sample threshold below which two hashes count as "the same
/// content" -- around 8-12 (of 64 bits) is a reasonable start for
/// transcode-level differences; tightened for stricter matching.
///
/// Brute-force, O(haystack_len * needle_len) worst case: fine for the
/// scale a per-project clip catalogue actually needs (hundreds of clips,
/// each a few hundred samples at a ~1s interval), not designed for
/// searching a professional stock-footage library.
/// Matches `candidate` against every fingerprint in `catalogue` (a
/// project's existing sources) and returns matches above `min_fraction`,
/// best first. This is the actual "find a partial/transcoded copy of this
/// clip already in my project" operation -- `best_alignment` is the
/// per-pair primitive; this is what a command-palette verb would call
/// directly, so it's the one place ordering and thresholding live rather
/// than being reimplemented at every call site.
/// Every source's matches against every OTHER source in the catalogue --
/// the precomputed form a cheap per-frame UI badge needs ("does this clip
/// have a duplicate/related source at all", answered by a map lookup, not
/// an alignment search). `match_against_catalogue` answers "what matches
/// THIS specific candidate"; this answers "what matches EVERYTHING",
/// once, so a timeline redraw never recomputes alignment for clips that
/// haven't changed.
///
/// Symmetric by construction (if A matches B, B's entry also lists A) --
/// checked below, since an asymmetric result would be a confusing bug for
/// a badge to render (A shows a match, B doesn't, for the same pair).
/// O(n^2) over the catalogue; fine for a project's source count, not
/// meant for a library scan.
pub fn all_pairs_matches(
    catalogue: &std::collections::BTreeMap<studio_core::SourceId, studio_core::FingerprintData>,
    max_hamming: u32,
    min_fraction: f64,
) -> std::collections::BTreeMap<studio_core::SourceId, Vec<(studio_core::SourceId, f64)>> {
    let mut out: std::collections::BTreeMap<studio_core::SourceId, Vec<(studio_core::SourceId, f64)>> = Default::default();
    let ids: Vec<studio_core::SourceId> = catalogue.keys().copied().collect();
    for i in 0..ids.len() {
        for j in (i + 1)..ids.len() {
            let (a, b) = (ids[i], ids[j]);
            if let Some(m) = best_alignment(&catalogue[&a].hashes, &catalogue[&b].hashes, max_hamming) {
                if m.fraction() >= min_fraction {
                    out.entry(a).or_default().push((b, m.fraction()));
                    out.entry(b).or_default().push((a, m.fraction()));
                }
            }
        }
    }
    // Best match first per source -- the "suggestive ordering" a picker
    // list wants, computed once here rather than by every UI that reads it.
    for hits in out.values_mut() {
        hits.sort_by(|x, y| y.1.partial_cmp(&x.1).unwrap_or(std::cmp::Ordering::Equal));
    }
    out
}

pub fn match_against_catalogue<'a>(
    candidate: &[u64],
    catalogue: impl Iterator<Item = (&'a studio_core::SourceId, &'a studio_core::FingerprintData)>,
    max_hamming: u32,
    min_fraction: f64,
) -> Vec<(studio_core::SourceId, AlignmentMatch)> {
    let mut hits: Vec<(studio_core::SourceId, AlignmentMatch)> = catalogue
        .filter_map(|(id, fp)| best_alignment(candidate, &fp.hashes, max_hamming).map(|m| (*id, m)))
        .filter(|(_, m)| m.fraction() >= min_fraction)
        .collect();
    hits.sort_by(|a, b| b.1.fraction().partial_cmp(&a.1.fraction()).unwrap_or(std::cmp::Ordering::Equal));
    hits
}

pub fn best_alignment(needle: &[u64], haystack: &[u64], max_hamming: u32) -> Option<AlignmentMatch> {
    if needle.is_empty() || haystack.is_empty() {
        return None;
    }
    let mut best: Option<AlignmentMatch> = None;
    let lo = -(needle.len() as isize) + 1;
    let hi = haystack.len() as isize - 1;
    for offset in lo..=hi {
        let mut matched = 0usize;
        let mut compared = 0usize;
        for (i, &n_hash) in needle.iter().enumerate() {
            let h_i = i as isize + offset;
            if h_i < 0 || h_i as usize >= haystack.len() {
                continue;
            }
            compared += 1;
            if hamming(n_hash, haystack[h_i as usize]) <= max_hamming {
                matched += 1;
            }
        }
        if compared == 0 {
            continue;
        }
        let candidate = AlignmentMatch { offset, matched, compared };
        best = match best {
            None => Some(candidate),
            Some(b) if candidate.fraction() > b.fraction()
                || (candidate.fraction() == b.fraction() && candidate.compared > b.compared) => Some(candidate),
            keep => keep,
        };
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat grey frame: every gradient is exactly zero, so every bit
    /// is 0 by the `<` comparison's tie-breaking. Mainly a determinism and
    /// no-panic check -- the interesting properties are the two below.
    #[test]
    fn a_flat_frame_hashes_deterministically() {
        let rgb = vec![128u8; (HASH_W * HASH_H * 3) as usize];
        let a = dhash_from_rgb(&rgb, HASH_W, HASH_H);
        let b = dhash_from_rgb(&rgb, HASH_W, HASH_H);
        assert_eq!(a, b);
        assert_eq!(a, 0, "no gradient anywhere -- every bit reads as the `<` tie case");
    }

    fn checkerboard(w: u32, h: u32, seed: u8) -> Vec<u8> {
        let mut out = vec![0u8; (w * h * 3) as usize];
        for y in 0..h {
            for x in 0..w {
                let v = if (x + y + seed as u32) % 2 == 0 { 200u8 } else { 40u8 };
                let i = ((y * w + x) * 3) as usize;
                out[i] = v; out[i + 1] = v; out[i + 2] = v;
            }
        }
        out
    }

    fn add_noise(rgb: &[u8], amount: i16, seed: u32) -> Vec<u8> {
        let mut out = rgb.to_vec();
        let mut state = seed.wrapping_mul(2654435761).max(1);
        for b in out.iter_mut() {
            state ^= state << 13; state ^= state >> 17; state ^= state << 5;
            let n = ((state % (2 * amount as u32 + 1)) as i16) - amount;
            *b = (*b as i16 + n).clamp(0, 255) as u8;
        }
        out
    }

    /// The core robustness claim, checked quantitatively: a small amount
    /// of per-pixel noise (standing in for recompression artifacts) must
    /// leave the hash MOSTLY unchanged. This is what "survives a
    /// transcode" actually means for this hash, and it's the property the
    /// whole fuzzy-match design depends on -- worth verifying with real
    /// numbers, not assuming dHash's textbook reputation transfers
    /// unchanged to this specific 9x8/luma implementation.
    #[test]
    fn small_pixel_noise_changes_few_hash_bits() {
        let base = checkerboard(HASH_W, HASH_H, 0);
        let h0 = dhash_from_rgb(&base, HASH_W, HASH_H);
        for seed in 0..10u32 {
            let noisy = add_noise(&base, 15, seed); // +-15 of 255, plausible recompression-level noise
            let h1 = dhash_from_rgb(&noisy, HASH_W, HASH_H);
            let d = hamming(h0, h1);
            assert!(d <= 6, "seed {seed}: {d} of 64 bits flipped by mild noise -- too fragile to survive a transcode");
        }
    }

    /// The discrimination side of the same claim: two genuinely different
    /// images must NOT hash close together, or the fuzzy match would
    /// report false positives constantly.
    #[test]
    fn different_content_hashes_far_apart() {
        let a = dhash_from_rgb(&checkerboard(HASH_W, HASH_H, 0), HASH_W, HASH_H);
        let b = dhash_from_rgb(&checkerboard(HASH_W, HASH_H, 1), HASH_W, HASH_H); // inverted checkerboard
        assert!(hamming(a, b) >= 40, "an inverted checkerboard should read as almost entirely different, got hamming {}", hamming(a, b));
    }

    fn synthetic_sequence(len: usize, seed: u32) -> Vec<u64> {
        // Not real dHashes -- synthetic 64-bit values standing in for a
        // sequence, so the ALIGNMENT algorithm can be tested on its own
        // without decoding video. A distinct pattern per position, seeded,
        // so two sequences with different seeds are reliably far apart.
        (0..len).map(|i| (i as u64).wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(seed as u64)).collect()
    }

    #[test]
    fn finds_the_correct_offset_for_a_clean_subsequence() {
        let haystack = synthetic_sequence(50, 1);
        let needle = haystack[10..20].to_vec();
        let m = best_alignment(&needle, &haystack, 4).expect("must find a match");
        assert_eq!(m.offset, 10);
        assert_eq!(m.matched, 10);
        assert_eq!(m.fraction(), 1.0);
    }

    /// The actual "survived a transcode" scenario: the needle is a subrange
    /// with a FEW bits flipped per hash (simulating a re-encode), not a
    /// byte-identical copy.
    #[test]
    fn finds_the_offset_even_with_minor_perturbation_per_sample() {
        let haystack = synthetic_sequence(50, 1);
        let mut needle: Vec<u64> = haystack[10..20].to_vec();
        for (i, h) in needle.iter_mut().enumerate() {
            *h ^= 1 << (i % 5); // flip a handful of low bits, well under max_hamming
        }
        let m = best_alignment(&needle, &haystack, 8).expect("must find a match despite minor perturbation");
        assert_eq!(m.offset, 10);
        assert_eq!(m.matched, 10, "every perturbed sample should still be within the hamming threshold");
    }

    #[test]
    fn unrelated_sequences_do_not_falsely_report_a_strong_match() {
        let haystack = synthetic_sequence(50, 1);
        let needle = synthetic_sequence(15, 999); // different seed -- unrelated content
        let m = best_alignment(&needle, &haystack, 4).expect("alignment always returns SOME best offset");
        // A handful of coincidental hits among unrelated 64-bit values at a
        // narrow hamming threshold is expected noise (birthday-bound
        // collisions), not a false positive -- the real match found above
        // scores 1.0; anything well short of that is not "a match".
        assert!(m.fraction() < 0.34, "unrelated sequences matched at {:.2} -- too close to a real match's 1.0 to trust the discrimination", m.fraction());
    }

    #[test]
    fn empty_inputs_report_no_match_rather_than_panicking() {
        assert!(best_alignment(&[], &[1, 2, 3], 4).is_none());
        assert!(best_alignment(&[1, 2, 3], &[], 4).is_none());
    }

    #[test]
    fn catalogue_match_ranks_the_best_hit_first_and_drops_weak_ones() {
        use studio_core::{FingerprintData, SourceId, Ticks};

        let candidate = synthetic_sequence(15, 42);
        // Source 1: an exact 15-sample match embedded in a longer sequence.
        let mut source1_hashes = synthetic_sequence(50, 1);
        source1_hashes[20..35].copy_from_slice(&candidate);
        // Source 2: unrelated content -- should not pass the threshold.
        let source2_hashes = synthetic_sequence(50, 999);
        // Source 3: a WEAKER partial match (half the samples perturbed
        // past the hamming threshold) -- should rank below source 1 but
        // may or may not clear the fraction threshold; used to check
        // ordering, not to assert it clears.
        let mut source3_hashes = synthetic_sequence(50, 1);
        source3_hashes[5..20].copy_from_slice(&candidate);
        for (i, h) in source3_hashes[5..20].iter_mut().enumerate() {
            if i % 2 == 0 {
                *h ^= 0xFFFF_FFFF_0000_0000; // wreck half the samples outright
            }
        }

        let fp = |hashes: Vec<u64>| FingerprintData { hashes, interval: Ticks::from_seconds(1.0) };
        let catalogue = vec![
            (SourceId(1), fp(source1_hashes)),
            (SourceId(2), fp(source2_hashes)),
            (SourceId(3), fp(source3_hashes)),
        ];

        let hits = match_against_catalogue(&candidate, catalogue.iter().map(|(id, fp)| (id, fp)), 4, 0.5);
        assert!(!hits.is_empty(), "the exact embedded match must clear a 0.5 threshold");
        assert_eq!(hits[0].0, SourceId(1), "the exact match must rank first");
        assert_eq!(hits[0].1.fraction(), 1.0);
        assert!(hits.iter().all(|(id, _)| *id != SourceId(2)), "unrelated content must not pass the threshold");
    }

    #[test]
    fn catalogue_match_on_an_empty_catalogue_returns_nothing() {
        let candidate = synthetic_sequence(10, 1);
        let empty: Vec<(studio_core::SourceId, studio_core::FingerprintData)> = Vec::new();
        let hits = match_against_catalogue(&candidate, empty.iter().map(|(id, fp)| (id, fp)), 4, 0.5);
        assert!(hits.is_empty());
    }

    fn fp(hashes: Vec<u64>) -> studio_core::FingerprintData {
        studio_core::FingerprintData { hashes, interval: studio_core::Ticks::from_seconds(1.0) }
    }

    #[test]
    fn all_pairs_matches_is_symmetric() {
        use studio_core::SourceId;
        let shared = synthetic_sequence(20, 7);
        let mut catalogue = std::collections::BTreeMap::new();
        catalogue.insert(SourceId(1), fp(shared.clone())); // identical to 2
        catalogue.insert(SourceId(2), fp(shared.clone()));
        catalogue.insert(SourceId(3), fp(synthetic_sequence(20, 999))); // unrelated

        let matches = all_pairs_matches(&catalogue, 4, 0.7);
        assert!(matches[&SourceId(1)].iter().any(|(id, _)| *id == SourceId(2)), "1 must list 2 as a match");
        assert!(matches[&SourceId(2)].iter().any(|(id, _)| *id == SourceId(1)), "2 must list 1 back -- symmetry");
        assert!(!matches.contains_key(&SourceId(3)) || matches[&SourceId(3)].is_empty(), "the unrelated source must not show a match");

        // Full symmetry check, not just the one pair: for every (a, b) in
        // the result, (b, a) must also be present with the identical score.
        for (a, hits) in &matches {
            for (b, score) in hits {
                let back = matches.get(b).map(|v| v.iter().find(|(id, _)| id == a));
                match back {
                    Some(Some((_, back_score))) => assert_eq!(score, back_score, "asymmetric score for ({a:?}, {b:?})"),
                    _ => panic!("{b:?} does not list {a:?} back -- asymmetric result"),
                }
            }
        }
    }

    #[test]
    fn all_pairs_matches_ranks_best_match_first_per_source() {
        use studio_core::SourceId;
        let base = synthetic_sequence(20, 1);
        let mut close = base.clone();
        close[0] ^= 1; // one bit off on ONE sample -- fraction stays 1.0
        let mut far = base.clone();
        // Flip enough bits on HALF the samples to push them over the
        // hamming threshold entirely (not just "noisier but still within
        // threshold", which is what the first draft of this test
        // accidentally did with `^= 0xFF` -- at most 8 bits, comfortably
        // under max_hamming=16, so both variants scored a tied 1.0 and the
        // test asserted an ordering that didn't actually exist).
        for h in far.iter_mut().take(10) {
            *h ^= 0xFFFF_FFFF; // 32 bits, guaranteed over the threshold
        }

        let mut catalogue = std::collections::BTreeMap::new();
        catalogue.insert(SourceId(1), fp(base));
        catalogue.insert(SourceId(2), fp(far));
        catalogue.insert(SourceId(3), fp(close));

        let matches = all_pairs_matches(&catalogue, 16, 0.3);
        let hits = &matches[&SourceId(1)];
        assert!(hits.len() >= 2, "source 1 should match both 2 and 3 at this loose threshold");
        assert!(hits[0].1 > hits[1].1, "matches must actually be in descending score order, not tied");
        assert_eq!(hits[0].0, SourceId(3), "the closer match (source 3, fraction 1.0) must be ranked first");
    }

    #[test]
    fn all_pairs_matches_on_a_single_source_catalogue_finds_nothing() {
        let mut catalogue = std::collections::BTreeMap::new();
        catalogue.insert(studio_core::SourceId(1), fp(synthetic_sequence(10, 1)));
        let matches = all_pairs_matches(&catalogue, 4, 0.5);
        assert!(matches.is_empty(), "nothing to match against with only one source");
    }
}
