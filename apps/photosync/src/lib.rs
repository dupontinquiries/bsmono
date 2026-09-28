//! photosync-core — what a photo IS, and how to find one like it.
//!
//! Headless by construction: no egui, no wgpu, no image decoder. It
//! takes greyscale pixels and produces hashes, tags, ascii and match
//! scores. That is what lets the search rules be tested without a
//! display, a GPU, or a photo library — the same split `snapshot-cam`
//! draws between `lib.rs` and its shell.
//!
//! ## On fotema
//!
//! Fotema is the reference for the FEATURE SET — thumbnails, face
//! extraction, metadata scan versioning, people. It is GPL-3.0 and no
//! code is taken from it; what is borrowed is the shape of the problem,
//! and one specific idea worth crediting: a **scan version** stored
//! beside each photo, so a metadata bug fix can re-scan exactly the
//! photos scanned by the buggy version instead of the whole library.
//!
//! ## The search idea, and what each piece is for
//!
//! The operator picks a photo and says "find people dancing like this".
//! Three signals, deliberately cheap and deliberately different:
//!
//! * **dHash** — a 64-bit perceptual hash. Near-duplicates and crops.
//!   Fast, exact-ish, no semantics.
//! * **ASCII** — the photo downsampled to a grid of characters. This is
//!   the one an LLM can read: it is TEXT, so a model can look at it,
//!   describe the pose, and be asked to adjust a query. A histogram
//!   cannot be shown to a language model; a picture drawn in characters
//!   can.
//! * **Tags** — whatever a CV pass or a person attached. Names for
//!   things the pixels cannot spell.
//!
//! None of them is a good matcher alone. dHash finds the same photo,
//! not the same pose. ASCII finds a similar composition, including a
//! completely different subject in the same arrangement. Tags find
//! whatever somebody bothered to label. `similarity` combines them, and
//! the weights are the interesting decision.
//!
//! ## Why ascii, honestly
//!
//! It is a coarse edge-and-mass sketch that survives resizing and
//! recolouring, and it costs nothing. It is NOT a pose estimator, and a
//! query built only from ascii will match a lamp shaped like a person.
//! The tags exist to stop that.

/// Cheap, explainable measurements that produce tags from pixels.
pub mod cv;
/// Two-hash scene fingerprints, ported from snapclone's LandmarkHash.
pub mod fingerprint;

/// A greyscale image: row-major, `w * h` bytes.
pub struct Gray<'a> {
    pub w: usize,
    pub h: usize,
    pub px: &'a [u8],
}

impl<'a> Gray<'a> {
    pub fn new(w: usize, h: usize, px: &'a [u8]) -> Option<Gray<'a>> {
        // Rejected rather than clamped: a wrong stride produces a
        // plausible-looking hash from misaligned rows, and a silently
        // wrong perceptual hash is worse than a refusal because every
        // comparison it takes part in is quietly meaningless.
        if w == 0 || h == 0 || px.len() != w * h {
            return None;
        }
        Some(Gray { w, h, px })
    }

    /// Average of the pixels in a source-space box. Used by both the
    /// hash and the ascii pass, so they agree about what a cell IS.
    fn box_avg(&self, x0: usize, y0: usize, x1: usize, y1: usize) -> u32 {
        let (x1, y1) = (x1.min(self.w), y1.min(self.h));
        if x0 >= x1 || y0 >= y1 {
            return 0;
        }
        let mut sum = 0u32;
        let mut n = 0u32;
        for y in y0..y1 {
            for x in x0..x1 {
                sum += self.px[y * self.w + x] as u32;
                n += 1;
            }
        }
        sum / n.max(1)
    }

    /// Downsample to `cw * ch` cells by box averaging.
    ///
    /// Box average rather than nearest-neighbour: nearest sampling of a
    /// detailed photo picks one pixel per cell, so two photos of the
    /// same scene at different resolutions produce unrelated grids —
    /// which defeats the entire point of a perceptual hash.
    pub fn cells(&self, cw: usize, ch: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(cw * ch);
        for cy in 0..ch {
            for cx in 0..cw {
                let x0 = cx * self.w / cw;
                let x1 = ((cx + 1) * self.w).div_ceil(cw).max(x0 + 1);
                let y0 = cy * self.h / ch;
                let y1 = ((cy + 1) * self.h).div_ceil(ch).max(y0 + 1);
                out.push(self.box_avg(x0, y0, x1, y1) as u8);
            }
        }
        out
    }
}

/// A 64-bit difference hash.
///
/// dHash rather than average-hash: it compares each cell to its right
/// neighbour, so it encodes GRADIENTS. An average hash of a photo and
/// the same photo brightened are different; a dHash of both is the
/// same, because every pixel moved together and the comparisons did
/// not.
pub fn dhash(img: &Gray) -> u64 {
    // 9x8 so that eight comparisons fit in each of eight rows.
    let c = img.cells(9, 8);
    let mut h = 0u64;
    for y in 0..8 {
        for x in 0..8 {
            let left = c[y * 9 + x];
            let right = c[y * 9 + x + 1];
            h = (h << 1) | u64::from(left > right);
        }
    }
    h
}

/// How many bits differ. 0 is identical; 64 is the inverse.
pub fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// Below this, two photos are the same picture.
///
/// Five bits of 64. Chosen to survive re-encoding and a light crop
/// while refusing two different photos of the same room — set higher
/// and a dedup pass starts deleting distinct pictures, which is
/// unrecoverable in a way that a missed duplicate is not.
pub const DUPLICATE_BITS: u32 = 5;

/// The ramp, darkest to lightest.
///
/// Ten levels, not the sixty-odd of a classic ramp. An LLM reading this
/// needs to distinguish shapes, not reproduce a photograph, and a long
/// ramp spends its resolution on tonal steps a model will not use while
/// making the text harder to read.
pub const RAMP: &[u8] = b"@%#*+=-:. ";

/// Render to ascii, one character per cell.
///
/// Rows are newline-separated so it can go straight into a prompt.
pub fn ascii(img: &Gray, cw: usize, ch: usize) -> String {
    let cells = img.cells(cw, ch);
    let mut s = String::with_capacity((cw + 1) * ch);
    for y in 0..ch {
        for x in 0..cw {
            let v = cells[y * cw + x] as usize;
            // 255 maps to the last index, not past it.
            let i = v * (RAMP.len() - 1) / 255;
            s.push(RAMP[i] as char);
        }
        if y + 1 < ch {
            s.push('\n');
        }
    }
    s
}

/// The default ascii grid.
///
/// 32x16 is about 500 characters — small enough to put several in one
/// prompt beside the question, large enough that a standing figure and
/// a seated one look different. Wider than tall because cells are
/// square and characters are not; a 32x32 grid renders as a photo
/// stretched to double height.
pub const ASCII_W: usize = 32;
pub const ASCII_H: usize = 16;

/// Everything known about one photo.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub path: String,
    pub dhash: u64,
    pub ascii: String,
    /// Lowercase, deduplicated, sorted. Sorted so two entries with the
    /// same tags compare equal regardless of insertion order.
    pub tags: Vec<String>,
    /// Blur + edge hashes: "is this the same SCENE", where `dhash`
    /// answers "is this the same PICTURE". Ported from snapclone's
    /// LandmarkHash — see `fingerprint` for why one hash would not do.
    pub fingerprint: crate::fingerprint::Fingerprint,
    /// Which version of the scanner produced this.
    ///
    /// Fotema's idea, and the reason it is worth copying: when a
    /// metadata bug is fixed, this says exactly which photos need
    /// re-scanning. Without it the choice is re-scan everything or
    /// trust data known to be wrong.
    pub scan_version: u32,
}

/// Bump when anything above changes meaning.
///
/// A hash produced by a different cell layout is not comparable to one
/// produced by this layout, and comparing them silently is how a dedup
/// pass starts reporting nonsense.
pub const SCAN_VERSION: u32 = 1;

impl Entry {
    pub fn scan(path: &str, img: &Gray, tags: &[&str]) -> Entry {
        let mut t: Vec<String> = tags.iter().map(|s| s.trim().to_lowercase()).collect();
        // DERIVED TAGS, alongside whatever was written.
        //
        // `cv::auto_tags` produces `auto:`-prefixed names describing the
        // IMAGE — dark, busy, centred, portrait — never its subject.
        // Without them the tag signal, which `similarity` weights
        // highest, only works for photos somebody labelled by hand: in a
        // real library, approximately none of them.
        //
        // Prefixed rather than merged so `tag_similarity` can tell
        // "somebody said this is a portrait of Ada" from "the frame is
        // taller than it is wide". Same word, different evidence.
        t.extend(cv::auto_tags_prefixed(&cv::features(img)));
        t.retain(|s| !s.is_empty());
        t.sort();
        t.dedup();
        Entry {
            path: path.to_string(),
            dhash: dhash(img),
            fingerprint: crate::fingerprint::Fingerprint::of(img),
            ascii: ascii(img, ASCII_W, ASCII_H),
            tags: t,
            scan_version: SCAN_VERSION,
        }
    }

    pub fn is_stale(&self) -> bool {
        self.scan_version != SCAN_VERSION
    }
}

/// How much each signal counts, 0..1 each.
///
/// Defaults are the argument, not the numbers: **tags dominate, ascii
/// breaks ties, dhash is nearly a veto for identity.**
///
/// Tags are the only signal that knows what a thing IS. Ascii knows
/// arrangement, which is what "dancing like this" is mostly asking, so
/// it is second. dHash is weighted low for SEARCH because a high dhash
/// score means "the same photo", and someone searching for photos like
/// this one rarely wants this one.
#[derive(Debug, Clone, Copy)]
pub struct Weights {
    pub tags: f32,
    pub ascii: f32,
    pub dhash: f32,
    /// Scene agreement from the two-hash fingerprint.
    pub scene: f32,
}

impl Default for Weights {
    fn default() -> Self {
        // `scene` sits between tags and ascii: it knows more than
        // composition and less than a label. `dhash` drops to 0.10
        // because the fingerprint now covers "same picture" better than
        // it did — dhash's remaining job is exact-duplicate detection,
        // which `duplicate_groups` does separately anyway.
        Weights { tags: 0.45, ascii: 0.20, dhash: 0.10, scene: 0.25 }
    }
}

/// Ascii similarity: the fraction of cells within one ramp step.
///
/// "Within one step" rather than equal, because a cell on a boundary
/// flips between two characters for a brightness change no one can see,
/// and exact matching would score two renders of the same photo well
/// below 1.0.
pub fn ascii_similarity(a: &str, b: &str) -> f32 {
    let ab: Vec<u8> = a.bytes().filter(|c| *c != b'\n').collect();
    let bb: Vec<u8> = b.bytes().filter(|c| *c != b'\n').collect();
    if ab.is_empty() || ab.len() != bb.len() {
        // Different grid sizes are not comparable. Zero rather than a
        // partial compare: a partial one would rank a 16x8 render above
        // a real match for no reason a reader could see.
        return 0.0;
    }
    let idx = |c: u8| RAMP.iter().position(|r| *r == c).unwrap_or(0) as i32;
    let close = ab
        .iter()
        .zip(&bb)
        .filter(|(x, y)| (idx(**x) - idx(**y)).abs() <= 1)
        .count();
    close as f32 / ab.len() as f32
}

/// Weighted Jaccard over tags: shared / total, each tag counting its
/// weight.
///
/// A written tag counts 1.0 and a machine-derived `auto:` tag counts
/// [`cv::AUTO_TAG_WEIGHT`]. `cv`'s own doc promised this — "derived
/// tags are worth less than written ones and the caller has to be able
/// to say so" — and then nothing applied the constant: two photos that
/// were both `auto:dark` and `auto:centred` scored exactly as similar as
/// two both labelled `ada` and `wedding`, which is composition matching
/// presented as semantic matching, the failure the module names.
pub fn tag_similarity(a: &[String], b: &[String]) -> f32 {
    if a.is_empty() && b.is_empty() {
        // Two untagged photos are not "perfectly similar" — they are
        // uninformative. Returning 1.0 here would make every untagged
        // pair top the results in a library that is mostly untagged.
        return 0.0;
    }
    let weight = |t: &String| if cv::is_auto(t) { cv::AUTO_TAG_WEIGHT } else { 1.0 };
    let shared: f32 = a.iter().filter(|t| b.contains(t)).map(weight).sum();
    let total: f32 = a.iter().map(weight).sum::<f32>() + b.iter().map(weight).sum::<f32>() - shared;
    if total <= 0.0 { 0.0 } else { shared / total }
}

/// Overall similarity, 0..1.
pub fn similarity(a: &Entry, b: &Entry, w: Weights) -> f32 {
    let d = 1.0 - (hamming(a.dhash, b.dhash) as f32 / 64.0);
    // Scene agreement, from the two-hash fingerprint. Contributed as a
    // RANK (0..3 over 3) rather than a bit distance, because the whole
    // point of the pair is that either hash alone is sufficient
    // evidence — averaging their distances would let a strong blur
    // match be dragged down by an edge hash that legitimately
    // disagrees, which is the dusk case the second hash exists for.
    let scene = crate::fingerprint::compare(&a.fingerprint, &b.fingerprint).rank() as f32 / 3.0;
    let s = w.tags * tag_similarity(&a.tags, &b.tags)
        + w.ascii * ascii_similarity(&a.ascii, &b.ascii)
        + w.dhash * d
        + w.scene * scene;
    let total = w.tags + w.ascii + w.dhash + w.scene;
    if total <= 0.0 { 0.0 } else { (s / total).clamp(0.0, 1.0) }
}

/// Best matches for `query`, most similar first.
///
/// The query itself is excluded by PATH, not by score: "find photos like
/// this one" never means "this one", and excluding by a similarity of
/// 1.0 would also drop a genuine duplicate the operator probably wants
/// to see.
pub fn best_matches<'a>(
    query: &Entry,
    library: &'a [Entry],
    w: Weights,
    limit: usize,
) -> Vec<(&'a Entry, f32)> {
    let mut scored: Vec<(&Entry, f32)> = library
        .iter()
        .filter(|e| e.path != query.path)
        .map(|e| (e, similarity(query, e, w)))
        .collect();
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            // Ties broken by path so the order is stable across runs.
            // An unstable result list makes "the third one" meaningless
            // between two identical searches.
            .then_with(|| a.0.path.cmp(&b.0.path))
    });
    scored.truncate(limit);
    scored
}

/// Near-duplicate groups, by dHash only.
///
/// Deliberately NOT using tags or ascii: dedup answers "is this the
/// same picture", and a tag agreement between two different photos of
/// the same wedding must never merge them. Deletion is unrecoverable,
/// so this signal is the strict one.
pub fn duplicate_groups(library: &[Entry], bits: u32) -> Vec<Vec<String>> {
    let mut seen = vec![false; library.len()];
    let mut out = Vec::new();
    for i in 0..library.len() {
        if seen[i] {
            continue;
        }
        let mut group = vec![library[i].path.clone()];
        for j in (i + 1)..library.len() {
            if !seen[j] && hamming(library[i].dhash, library[j].dhash) <= bits {
                seen[j] = true;
                group.push(library[j].path.clone());
            }
        }
        if group.len() > 1 {
            group.sort();
            out.push(group);
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A gradient, so cells differ predictably.
    fn ramp_img(w: usize, h: usize) -> Vec<u8> {
        (0..w * h).map(|i| ((i * 255) / (w * h).max(1)) as u8).collect()
    }

    fn entry(path: &str, px: &[u8], w: usize, h: usize, tags: &[&str]) -> Entry {
        Entry::scan(path, &Gray::new(w, h, px).unwrap(), tags)
    }

    #[test]
    fn a_mismatched_stride_is_refused_not_clamped() {
        // A wrong stride produces a plausible hash from misaligned rows,
        // and every comparison it takes part in is then quietly
        // meaningless.
        assert!(Gray::new(4, 4, &[0u8; 15]).is_none());
        assert!(Gray::new(0, 4, &[]).is_none());
        assert!(Gray::new(4, 4, &[0u8; 16]).is_some());
    }

    #[test]
    fn brightening_a_photo_does_not_change_its_dhash() {
        // The reason for dHash over average-hash: every pixel moved
        // together, so the neighbour comparisons did not.
        let w = 32;
        let h = 32;
        let base = ramp_img(w, h);
        let bright: Vec<u8> = base.iter().map(|p| p.saturating_add(20)).collect();
        let a = dhash(&Gray::new(w, h, &base).unwrap());
        let b = dhash(&Gray::new(w, h, &bright).unwrap());
        assert!(hamming(a, b) <= DUPLICATE_BITS, "{} bits apart", hamming(a, b));
    }

    #[test]
    fn the_same_scene_at_two_resolutions_hashes_close() {
        // Box averaging is what makes this hold; nearest-neighbour
        // sampling would pick one pixel per cell and produce unrelated
        // grids.
        let big = ramp_img(64, 64);
        let small = ramp_img(32, 32);
        let a = dhash(&Gray::new(64, 64, &big).unwrap());
        let b = dhash(&Gray::new(32, 32, &small).unwrap());
        assert!(hamming(a, b) <= DUPLICATE_BITS, "{} bits apart", hamming(a, b));
    }

    #[test]
    fn ascii_is_a_rectangle_of_the_requested_size() {
        // It goes into a prompt; a ragged grid is one a model reads as a
        // different shape than the photo was.
        let px = ramp_img(40, 40);
        let art = ascii(&Gray::new(40, 40, &px).unwrap(), ASCII_W, ASCII_H);
        let rows: Vec<&str> = art.lines().collect();
        assert_eq!(rows.len(), ASCII_H);
        assert!(rows.iter().all(|r| r.chars().count() == ASCII_W), "{rows:?}");
    }

    #[test]
    fn every_ascii_char_is_from_the_ramp() {
        // A character outside the ramp means an index escaped, and
        // `ascii_similarity` would score it as the darkest level.
        let px = ramp_img(40, 40);
        let art = ascii(&Gray::new(40, 40, &px).unwrap(), ASCII_W, ASCII_H);
        for c in art.bytes().filter(|c| *c != b'\n') {
            assert!(RAMP.contains(&c), "{:?} is not in the ramp", c as char);
        }
    }

    #[test]
    fn white_and_black_land_at_the_ends_of_the_ramp() {
        let black = vec![0u8; 64];
        let white = vec![255u8; 64];
        let a = ascii(&Gray::new(8, 8, &black).unwrap(), 4, 2);
        let b = ascii(&Gray::new(8, 8, &white).unwrap(), 4, 2);
        assert!(a.starts_with('@'), "{a:?}");
        assert!(b.starts_with(' '), "{b:?}");
    }

    #[test]
    fn two_untagged_photos_are_not_perfectly_similar() {
        // Returning 1.0 for two empty tag sets would make every untagged
        // pair top the results in a library that is mostly untagged —
        // which is every real library.
        assert_eq!(tag_similarity(&[], &[]), 0.0);
    }

    #[test]
    fn tags_are_normalised_so_case_and_order_do_not_matter() {
        let px = ramp_img(16, 16);
        let a = entry("a", &px, 16, 16, &["Dancing", "  party ", "dancing"]);
        // Written tags are normalised and deduplicated; derived ones are
        // added beside them and marked. Asserting the WRITTEN set
        // exactly, rather than the whole list, so this test keeps
        // testing normalisation when the cv thresholds change — a test
        // that breaks every time an unrelated threshold moves is one
        // people stop reading.
        let written: Vec<&String> = a.tags.iter().filter(|t| !cv::is_auto(t)).collect();
        assert_eq!(written, vec!["dancing", "party"]);
        assert!(a.tags.iter().any(|t| cv::is_auto(t)), "no derived tags: {:?}", a.tags);
    }

    #[test]
    fn a_written_tag_in_common_outweighs_a_derived_one() {
        // Both pairs share exactly one tag; the pair sharing a word a
        // person wrote must score higher than the pair sharing a word
        // the scanner derived from brightness.
        let written = tag_similarity(&["wedding".into()], &["wedding".into()]);
        let derived = tag_similarity(&["auto:dark".into()], &["auto:dark".into()]);
        assert_eq!(written, 1.0);
        assert_eq!(derived, 1.0, "two identical sets are still identical");
        // Mixed: one written match against one derived mismatch.
        let a = vec!["wedding".to_string(), "auto:dark".to_string()];
        let b = vec!["wedding".to_string(), "auto:bright".to_string()];
        let c = vec!["auto:dark".to_string(), "party".to_string()];
        let d = vec!["auto:dark".to_string(), "picnic".to_string()];
        assert!(tag_similarity(&a, &b) > tag_similarity(&c, &d),
                "a shared written tag ({}) must beat a shared derived one ({})",
                tag_similarity(&a, &b), tag_similarity(&c, &d));
    }

    #[test]
    fn different_ascii_sizes_score_zero_rather_than_partially() {
        // A partial compare would rank a small render above a real match
        // for no reason a reader could see.
        assert_eq!(ascii_similarity("abc", "abcdef"), 0.0);
    }

    #[test]
    fn an_unlabelled_library_is_still_searchable() {
        // The point of the cv pass. Before it, tag_similarity was 0.0
        // for every pair in a library nobody had labelled — which is
        // every real library — so the highest-weighted signal
        // contributed nothing and search fell back to ascii alone.
        let dark = vec![10u8; 32 * 32];
        let bright = vec![245u8; 32 * 32];
        let q = entry("q", &dark, 32, 32, &[]);
        let like = entry("like", &dark, 32, 32, &[]);
        let unlike = entry("unlike", &bright, 32, 32, &[]);
        assert!(tag_similarity(&q.tags, &like.tags) > 0.0,
                "no derived tags to match on: {:?}", q.tags);
        // Bound, because `best_matches` borrows the library and a
        // temporary array would not outlive the call.
        let lib = [like, unlike];
        let got = best_matches(&q, &lib, Weights::default(), 2);
        assert_eq!(got[0].0.path, "like", "{got:?}");
    }

    #[test]
    fn the_query_photo_is_excluded_from_its_own_results() {
        // "Find photos like this one" never means "this one".
        let px = ramp_img(16, 16);
        let q = entry("q.jpg", &px, 16, 16, &["dancing"]);
        let lib = vec![q.clone(), entry("b.jpg", &px, 16, 16, &["dancing"])];
        let got = best_matches(&q, &lib, Weights::default(), 10);
        assert!(got.iter().all(|(e, _)| e.path != "q.jpg"), "{got:?}");
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn results_are_stable_between_identical_searches() {
        // An unstable order makes "the third one" meaningless between
        // two runs of the same query.
        let px = ramp_img(16, 16);
        let q = entry("q", &px, 16, 16, &["x"]);
        let lib: Vec<Entry> = ["c", "a", "b"]
            .iter()
            .map(|n| entry(n, &px, 16, 16, &["x"]))
            .collect();
        let one: Vec<&str> = best_matches(&q, &lib, Weights::default(), 10)
            .iter().map(|(e, _)| e.path.as_str()).collect();
        let two: Vec<&str> = best_matches(&q, &lib, Weights::default(), 10)
            .iter().map(|(e, _)| e.path.as_str()).collect();
        assert_eq!(one, two);
        assert_eq!(one, vec!["a", "b", "c"], "ties should break by path");
    }

    #[test]
    fn dedup_uses_only_the_hash_not_the_tags() {
        // Two different photos of the same wedding share every tag.
        // Merging them would delete a distinct picture, which is
        // unrecoverable in a way a missed duplicate is not.
        let a_px = ramp_img(32, 32);
        let b_px: Vec<u8> = a_px.iter().rev().copied().collect();
        let lib = vec![
            entry("a", &a_px, 32, 32, &["wedding", "cake"]),
            entry("b", &b_px, 32, 32, &["wedding", "cake"]),
        ];
        assert!(duplicate_groups(&lib, DUPLICATE_BITS).is_empty(),
                "different pictures were grouped by their tags");
    }

    #[test]
    fn a_re_encoded_photo_is_found_as_a_duplicate() {
        let px = ramp_img(32, 32);
        let noisy: Vec<u8> = px.iter().enumerate()
            .map(|(i, p)| if i % 7 == 0 { p.saturating_add(2) } else { *p })
            .collect();
        let lib = vec![
            entry("orig.jpg", &px, 32, 32, &[]),
            entry("copy.jpg", &noisy, 32, 32, &[]),
        ];
        let groups = duplicate_groups(&lib, DUPLICATE_BITS);
        assert_eq!(groups.len(), 1, "{groups:?}");
        assert_eq!(groups[0], vec!["copy.jpg", "orig.jpg"]);
    }

    #[test]
    fn a_stale_scan_version_is_detectable() {
        // Fotema's idea: when a scanner bug is fixed, this says exactly
        // which photos need re-scanning. Without it the choice is
        // re-scan everything or trust data known to be wrong.
        let px = ramp_img(16, 16);
        let mut e = entry("a", &px, 16, 16, &[]);
        assert!(!e.is_stale());
        e.scan_version = SCAN_VERSION - 1;
        assert!(e.is_stale());
    }

    #[test]
    fn similarity_stays_within_range_for_every_weighting() {
        // A score above 1.0 would sort above a perfect match and read as
        // a bug in the ranking rather than in the weights.
        let px = ramp_img(16, 16);
        let a = entry("a", &px, 16, 16, &["x", "y"]);
        let b = entry("b", &px, 16, 16, &["y"]);
        for w in [
            Weights::default(),
            Weights { tags: 1.0, ascii: 0.0, dhash: 0.0, scene: 0.0 },
            Weights { tags: 0.0, ascii: 0.0, dhash: 1.0, scene: 0.0 },
            Weights { tags: 0.0, ascii: 0.0, dhash: 0.0, scene: 0.0 },
        ] {
            let s = similarity(&a, &b, w);
            assert!((0.0..=1.0).contains(&s), "{s} for {w:?}");
        }
    }
}
