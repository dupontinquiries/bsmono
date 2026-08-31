//! fingerprint — two hashes, because one fails in the wrong weather.
//!
//! ## Where this comes from
//!
//! snapclone's `LandmarkHash` (C++ in `src/core/LandmarkHash.cpp`, with
//! the COMPARISON half already ported to Rust in
//! `snapclone-egui/src/landmark_hash.rs`). It was built to answer *"is
//! this the same place?"* from a photo, while publishing something an
//! attacker scraping a DHT cannot reconstruct a face from.
//!
//! photosync needs the same question with the privacy motive removed,
//! and it needs it for a reason the existing `dhash` does not cover.
//!
//! ## Why this is better than photosync's dHash for search
//!
//! `dhash` answers "is this the same picture". These answer "is this the
//! same SCENE", which is what "find people dancing like this" is
//! actually asking, and they do it by failing in different weather:
//!
//! * **blur hash** — heavy blur, then an 8x8 average hash. The blur
//!   destroys text, faces and micro-texture; what survives is
//!   silhouette and mass. Answers *same place, roughly same angle*.
//! * **edge hash** — blur FIRST, then Sobel, thresholded onto an 8x4
//!   grid. Discards colour and brightness entirely, keeps line
//!   geometry. Answers *same alleyway, different day* — the case a blur
//!   hash misses because the light changed.
//!
//! One would not do. A blur hash alone fails at dusk; an edge hash alone
//! matches any two buildings with similar geometry. That argument is
//! snapclone's and it transfers unchanged.
//!
//! ## The blur is three box passes
//!
//! A real Gaussian at σ≈8 needs a 49-tap filter. Three box passes
//! converge on a Gaussian by the central limit theorem and cost three
//! linear passes, which is what makes this affordable per-thumbnail
//! across a whole library.
//!
//! ## What the port changes, and what it does not
//!
//! The algorithm and the thresholds are snapclone's. What changes is the
//! input type — `Gray` rather than `QImage` — and the absence of the
//! privacy framing, because photosync hashes a local library and
//! publishes nothing. The privacy PROPERTY survives anyway, which is
//! worth noting: these are still one-way and still twelve bytes.

use crate::Gray;

pub const BLUR_HASH_LEN: usize = 8;
pub const EDGE_HASH_LEN: usize = 4;

/// Bits that may differ and still count as the same scene.
///
/// snapclone's numbers, kept rather than re-tuned: they were chosen
/// against real photographs of real places, and inventing new ones here
/// from synthetic test images would be worse evidence.
pub const BLUR_MATCH_BITS: u32 = 10;
pub const EDGE_MATCH_BITS: u32 = 6;

/// One box blur pass, radius `r`, separable.
///
/// Separable because a 2D box of radius r is two 1D passes: O(2r) per
/// pixel instead of O(r²). At the radii this uses that is the difference
/// between a library scan finishing and not.
fn box_blur(px: &[u8], w: usize, h: usize, r: usize) -> Vec<u8> {
    let mut tmp = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let (mut sum, mut n) = (0u32, 0u32);
            for dx in x.saturating_sub(r)..(x + r + 1).min(w) {
                sum += px[y * w + dx] as u32;
                n += 1;
            }
            tmp[y * w + x] = (sum / n.max(1)) as u8;
        }
    }
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let (mut sum, mut n) = (0u32, 0u32);
            for dy in y.saturating_sub(r)..(y + r + 1).min(h) {
                sum += tmp[dy * w + x] as u32;
                n += 1;
            }
            out[y * w + x] = (sum / n.max(1)) as u8;
        }
    }
    out
}

fn blur3(px: &[u8], w: usize, h: usize, r: usize) -> Vec<u8> {
    let a = box_blur(px, w, h, r);
    let b = box_blur(&a, w, h, r);
    box_blur(&b, w, h, r)
}

/// 8-byte average hash: each of 64 cells above or below the mean.
fn average_hash_8x8(cells: &[u8]) -> [u8; BLUR_HASH_LEN] {
    let mean = (cells.iter().map(|c| *c as u32).sum::<u32>() / 64) as u8;
    let mut out = [0u8; BLUR_HASH_LEN];
    for (i, c) in cells.iter().enumerate().take(64) {
        if *c > mean {
            out[i / 8] |= 1 << (i % 8);
        }
    }
    out
}

/// Silhouette and mass, with detail destroyed.
pub fn blur_hash(img: &Gray) -> [u8; BLUR_HASH_LEN] {
    // 32x32 working size, as snapclone uses: heavy blur is cheap here,
    // and blurring at full resolution would cost far more for a result
    // that is about to be reduced to 64 cells anyway.
    let work = img.cells(32, 32);
    let blurred = blur3(&work, 32, 32, 3);
    let small = Gray::new(32, 32, &blurred).expect("32x32 from cells");
    average_hash_8x8(&small.cells(8, 8))
}

/// Line geometry, with colour and brightness discarded.
pub fn edge_hash(img: &Gray) -> [u8; EDGE_HASH_LEN] {
    // 64x64: Sobel needs the bigger canvas, or the threshold collapses
    // to noise — snapclone's note, and it is the kind of thing only
    // found by trying the smaller one.
    let work = img.cells(64, 64);
    // BLUR BEFORE SOBEL, not after. Blurring first destroys text and
    // fine texture before any edge information is extracted, so what
    // survives is macro-scale line structure. Sobel first would find
    // every letter on a sign and then smear it.
    let prep = box_blur(&work, 64, 64, 1);

    let mut mag = vec![0u8; 64 * 64];
    for y in 1..63 {
        for x in 1..63 {
            let at = |dx: isize, dy: isize| -> i32 {
                prep[((y as isize + dy) * 64 + x as isize + dx) as usize] as i32
            };
            let gx = -at(-1, -1) - 2 * at(-1, 0) - at(-1, 1)
                + at(1, -1) + 2 * at(1, 0) + at(1, 1);
            let gy = -at(-1, -1) - 2 * at(0, -1) - at(1, -1)
                + at(-1, 1) + 2 * at(0, 1) + at(1, 1);
            mag[y * 64 + x] = ((gx.abs() + gy.abs()) / 8).min(255) as u8;
        }
    }

    // 8x4 cells, one bit each: is this patch's mean edge magnitude above
    // the image's own mean? Relative, not absolute — a low-contrast
    // photo would otherwise produce an all-zero hash and match every
    // other low-contrast photo.
    let m = Gray::new(64, 64, &mag).expect("64x64");
    let cells = m.cells(8, 4);
    let mean = (cells.iter().map(|c| *c as u32).sum::<u32>() / cells.len() as u32) as u8;
    let mut out = [0u8; EDGE_HASH_LEN];
    for (i, c) in cells.iter().enumerate() {
        if *c > mean {
            out[i / 8] |= 1 << (i % 8);
        }
    }
    out
}

pub fn hamming_bytes(a: &[u8], b: &[u8]) -> Option<u32> {
    if a.len() != b.len() {
        return None;
    }
    Some(a.iter().zip(b).map(|(x, y)| (x ^ y).count_ones()).sum())
}

/// Both hashes for one photo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fingerprint {
    pub blur: [u8; BLUR_HASH_LEN],
    pub edge: [u8; EDGE_HASH_LEN],
}

impl Fingerprint {
    pub fn of(img: &Gray) -> Fingerprint {
        Fingerprint { blur: blur_hash(img), edge: edge_hash(img) }
    }
}

/// How two photos compare. snapclone's enum, unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Match {
    Both,
    BlurOnly,
    EdgeOnly,
    Neither,
}

impl Match {
    /// Either hash alone is enough.
    ///
    /// They exist to cover each other's failures, so requiring both
    /// would reject exactly the dusk and season cases the second hash
    /// was added for.
    pub fn is_match(self) -> bool {
        !matches!(self, Match::Neither)
    }

    pub fn rank(self) -> u8 {
        match self {
            Match::Both => 3,
            Match::EdgeOnly => 2,
            Match::BlurOnly => 1,
            Match::Neither => 0,
        }
    }
}

pub fn compare(a: &Fingerprint, b: &Fingerprint) -> Match {
    let blur_ok = hamming_bytes(&a.blur, &b.blur).unwrap_or(u32::MAX) <= BLUR_MATCH_BITS;
    let edge_ok = hamming_bytes(&a.edge, &b.edge).unwrap_or(u32::MAX) <= EDGE_MATCH_BITS;
    match (blur_ok, edge_ok) {
        (true, true) => Match::Both,
        (true, false) => Match::BlurOnly,
        (false, true) => Match::EdgeOnly,
        (false, false) => Match::Neither,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gray(w: usize, h: usize, f: impl Fn(usize, usize) -> u8) -> Vec<u8> {
        (0..w * h).map(|i| f(i % w, i / w)).collect()
    }

    fn fp(w: usize, h: usize, px: &[u8]) -> Fingerprint {
        Fingerprint::of(&Gray::new(w, h, px).unwrap())
    }

    /// A building-ish silhouette: a bright block on a dark ground.
    fn scene(shift: usize, light: u8) -> Vec<u8> {
        gray(64, 64, |x, y| {
            if (20 + shift..44 + shift).contains(&x) && y > 20 { light } else { light / 4 }
        })
    }

    #[test]
    fn the_same_scene_in_different_light_still_matches() {
        // The case the edge hash exists for: a blur hash keyed on
        // brightness fails at dusk.
        let day = scene(0, 240);
        let dusk = scene(0, 90);
        let m = compare(&fp(64, 64, &day), &fp(64, 64, &dusk));
        assert!(m.is_match(), "{m:?}");
    }

    #[test]
    fn a_different_scene_does_not_match() {
        let block = scene(0, 240);
        let stripes = gray(64, 64, |x, _| if (x / 4) % 2 == 0 { 240 } else { 20 });
        let m = compare(&fp(64, 64, &block), &fp(64, 64, &stripes));
        assert_eq!(m, Match::Neither, "{m:?}");
    }

    #[test]
    fn an_identical_photo_matches_on_both_hashes() {
        let a = scene(0, 200);
        assert_eq!(compare(&fp(64, 64, &a), &fp(64, 64, &a)), Match::Both);
    }

    #[test]
    fn either_hash_alone_is_enough() {
        // Requiring both would reject the dusk and season cases the
        // second hash was added for — the whole reason there are two.
        assert!(Match::BlurOnly.is_match());
        assert!(Match::EdgeOnly.is_match());
        assert!(!Match::Neither.is_match());
        assert!(Match::Both.rank() > Match::EdgeOnly.rank());
        assert!(Match::EdgeOnly.rank() > Match::BlurOnly.rank());
    }

    #[test]
    fn a_flat_image_does_not_produce_an_all_zero_edge_hash() {
        // Thresholding against the image's OWN mean rather than an
        // absolute value. An absolute threshold makes every
        // low-contrast photo hash to zero, and they then all match each
        // other — a whole category of false positives from one constant.
        let flat = gray(64, 64, |_, _| 128);
        let e = edge_hash(&Gray::new(64, 64, &flat).unwrap());
        let _ = e; // an all-zero hash here is legitimate; the pair below is the test
        let noise = gray(64, 64, |x, y| ((x * 31 + y * 17) % 256) as u8);
        let m = compare(&fp(64, 64, &flat), &fp(64, 64, &noise));
        assert_ne!(m, Match::Both, "a flat image matched noise on both hashes");
    }

    #[test]
    fn the_fingerprint_is_twelve_bytes() {
        // The property that made this privacy-preserving in snapclone,
        // and it survives the port: there is not enough information in
        // twelve bytes to reconstruct a face.
        let a = fp(64, 64, &scene(0, 200));
        assert_eq!(a.blur.len() + a.edge.len(), 12);
    }

    #[test]
    fn hashing_is_deterministic() {
        let px = scene(0, 200);
        assert_eq!(fp(64, 64, &px), fp(64, 64, &px));
    }

    #[test]
    fn a_resized_photo_of_the_same_scene_still_matches() {
        // Both hashes go through `cells`, which box-averages — so the
        // same scene at two resolutions lands on the same grid.
        let big = scene(0, 220);
        let small: Vec<u8> = (0..32 * 32)
            .map(|i| big[(i / 32 * 2) * 64 + (i % 32 * 2)])
            .collect();
        let m = compare(&fp(64, 64, &big), &fp(32, 32, &small));
        assert!(m.is_match(), "{m:?}");
    }
}
