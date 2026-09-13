//! The cheap content analyzer, and the checker that keeps its output from
//! going stale unnoticed.
//!
//! ## What's cheap here and what isn't
//!
//! Every function in this module operates on pixel buffers the caller
//! already has — it does no decoding. That's what makes it cheap: motion
//! and cut detection reuse the SAME thumbnail-tier frames the far shell
//! of [`cache_policy`] is already decoding for preview, so profiling a
//! whole source costs close to nothing extra once a scrub has passed over
//! it once.
//!
//! [`Bucket::Face`]'s signal is a skin-tone color-range fraction —
//! **not** a face detector. Said plainly here and at every call site
//! because the name invites the opposite assumption: this is a crude,
//! fast proxy ("a warm-toned region large enough to plausibly be skin"),
//! wrong on plenty of real footage (a wooden desk, a sunset, anyone
//! backlit), and nothing downstream should treat a `Face` bucket as
//! "there is definitely a person here."
//!
//! ## Freezing and going stale
//!
//! A profile is only useful if it can be TRUSTED without re-running the
//! analyzer every time — that's the whole point of freezing it into a
//! sidecar. But two things can make a frozen profile wrong without
//! touching it: the SOURCE can change (re-encoded, replaced, trimmed) or
//! the ANALYZER can change (a bug fix, a retuned threshold). Telling
//! these apart matters because the fix is different — a changed source
//! means re-profile that one file; a changed algorithm means every frozen
//! profile everywhere is suspect.
//!
//! [`fingerprint`] is computed from the SAME cheap scores the analyzer
//! produces, quantized coarsely first so float noise (a re-encode at a
//! different bitrate producing near-identical but not bit-identical
//! motion scores) doesn't register as a change. [`ProfileFreeze::verify`]
//! recomputes it from a fresh (equally cheap) re-scan and compares:
//! version mismatch means the algorithm moved (always stale, unconditionally);
//! same version but different fingerprint means the source moved.
//!
//! [`golden_fingerprint_is_pinned`] is the actual regression checker: a
//! fixed synthetic scenario's fingerprint is asserted against a literal
//! constant. Change the scoring math — the motion formula, the skin
//! range, the cut threshold, the segment merge rule — and that test
//! fails immediately, which is the point: it forces a conscious choice
//! between "that was a bug, revert it" and "that's an intentional
//! retune, bump [`CURRENT_VERSION`] and update the golden constant,"
//! rather than letting frozen profiles silently start meaning something
//! different from what they said when they were written.

use super::bucket::{Bucket, ClipId, Segment};

/// Bumped whenever the SCORING ALGORITHM changes — the motion formula,
/// the skin heuristic, the cut threshold, or the segment merge rule.
/// [`ProfileFreeze::verify`] treats any version other than this as stale
/// unconditionally, and [`golden_fingerprint_is_pinned`] is the test that
/// forces this to be a conscious edit rather than a silent drift.
pub const CURRENT_VERSION: u32 = 1;

// ---------------------------------------------------------------- scoring

/// Mean absolute difference between two equally-sized 8-bit luma buffers,
/// normalised to `[0, 1]`. The motion signal: cheap (one pass, integer
/// buffers), and — unlike a pixel-perfect diff — insensitive to the kind
/// of dither/banding noise a lossy codec introduces between two frames
/// that are visually identical.
pub fn luma_diff(prev: &[u8], cur: &[u8]) -> f32 {
    if prev.is_empty() || prev.len() != cur.len() {
        return 0.0;
    }
    let sum: u64 = prev.iter().zip(cur).map(|(&a, &b)| (a as i16 - b as i16).unsigned_abs() as u64).sum();
    sum as f32 / (prev.len() as f32 * 255.0)
}

/// Fraction of pixels in an RGB8 (row-major, 3 bytes/pixel) buffer that
/// fall in a broad, deliberately permissive skin-tone range. THE `Face`
/// PROXY, NOT A FACE DETECTOR — see the module doc. The range is checked
/// in plain RGB ratios (no HSV conversion) to keep this a single cheap
/// pass: red the dominant channel, green between red and blue, and both
/// comfortably below red — the shape common to a very wide range of skin
/// tones under ordinary lighting, and also to plenty of things that
/// aren't skin at all.
pub fn skin_fraction(rgb: &[u8]) -> f32 {
    if rgb.len() < 3 {
        return 0.0;
    }
    let mut hits = 0usize;
    let mut n = 0usize;
    for px in rgb.chunks_exact(3) {
        let (r, g, b) = (px[0] as i32, px[1] as i32, px[2] as i32);
        n += 1;
        if r > 60 && r > b + 10 && g > b && r - g < r / 2 + 10 && r >= g && g >= b.saturating_sub(5) {
            hits += 1;
        }
    }
    if n == 0 { 0.0 } else { hits as f32 / n as f32 }
}

/// Per-frame scores for one source, at whatever cheap proxy resolution
/// the caller decoded. `motion[i]` is `luma_diff` between frame `i-1` and
/// `i` (0.0 for `i == 0`, no previous frame to compare); `skin[i]` is
/// `skin_fraction` of frame `i`. Equal length, one entry per frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Scores {
    pub motion: Vec<f32>,
    pub skin: Vec<f32>,
}

/// Frame indices flagged as hard cuts: a spike well above the LOCAL
/// median motion, not a fixed threshold — a fast-motion scene's ordinary
/// frame-to-frame difference can exceed a static scene's cut, so "well
/// above this frame's own neighbourhood" is what actually generalises.
///
/// `window` is how many frames on each side form the local baseline;
/// `ratio` is how many times the local median a frame must exceed;
/// `floor` is an absolute minimum motion score below which nothing is
/// ever called a cut, since a near-zero baseline makes the ratio test
/// alone fire on negligible noise.
pub fn detect_cuts(motion: &[f32], window: usize, ratio: f32, floor: f32) -> Vec<usize> {
    let n = motion.len();
    let mut cuts = Vec::new();
    for i in 0..n {
        if motion[i] < floor {
            continue;
        }
        let lo = i.saturating_sub(window);
        let hi = (i + window + 1).min(n);
        let mut neighbourhood: Vec<f32> = motion[lo..hi].to_vec();
        // Exclude the candidate frame itself from its own baseline — a
        // real cut IS the outlier and must not smooth itself into the
        // median it's being compared against.
        let self_index = i - lo;
        if self_index < neighbourhood.len() {
            neighbourhood.remove(self_index);
        }
        if neighbourhood.is_empty() {
            continue;
        }
        neighbourhood.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median = neighbourhood[neighbourhood.len() / 2];
        if motion[i] > median * ratio && motion[i] > floor {
            cuts.push(i);
        }
    }
    cuts
}

/// Classify one frame from its own scores plus whether it falls in a cut
/// window. Priority order (highest wins): `Face` > `CutBoundary` >
/// `Motion` > `Static` — a face during a cut is still a face, and matters
/// more; a cut in an otherwise static scene still matters more than the
/// stillness around it.
fn classify(motion: f32, skin: f32, near_cut: bool, skin_threshold: f32, motion_threshold: f32) -> Bucket {
    if skin >= skin_threshold {
        Bucket::Face
    } else if near_cut {
        Bucket::CutBoundary
    } else if motion >= motion_threshold {
        Bucket::Motion
    } else {
        Bucket::Static
    }
}

/// Tunables for [`profile`]. Defaults are named constants below so a
/// caller can start from them and override one at a time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    pub cut_window: usize,
    pub cut_ratio: f32,
    pub cut_floor: f32,
    /// Frames on each side of a detected cut that are marked
    /// `CutBoundary` (unless outscored by `Face`).
    pub cut_margin: usize,
    pub skin_threshold: f32,
    pub motion_threshold: f32,
}

impl Default for Params {
    fn default() -> Self {
        Self { cut_window: 8, cut_ratio: 4.0, cut_floor: 0.02, cut_margin: 12, skin_threshold: 0.12, motion_threshold: 0.04 }
    }
}

/// Run the whole analyzer over one source's scores and produce
/// contiguous [`Segment`]s — the verified, level-1 evidence
/// [`super::bucket::BucketField`] resolves against. Adjacent frames with
/// the same classification are merged into one segment; this is also
/// where a cut's margin gets expanded and can absorb neighbouring frames
/// of the same or lower priority.
pub fn profile(clip: ClipId, scores: &Scores, params: Params) -> Vec<Segment> {
    let n = scores.motion.len();
    if n == 0 || scores.skin.len() != n {
        return Vec::new();
    }
    let cuts = detect_cuts(&scores.motion, params.cut_window, params.cut_ratio, params.cut_floor);
    let mut near_cut = vec![false; n];
    for &c in &cuts {
        let lo = c.saturating_sub(params.cut_margin);
        let hi = (c + params.cut_margin + 1).min(n);
        for f in near_cut.iter_mut().take(hi).skip(lo) {
            *f = true;
        }
    }

    let buckets: Vec<Bucket> = (0..n)
        .map(|i| classify(scores.motion[i], scores.skin[i], near_cut[i], params.skin_threshold, params.motion_threshold))
        .collect();

    let mut segments = Vec::new();
    let mut start = 0usize;
    for i in 1..=n {
        if i == n || buckets[i] != buckets[start] {
            segments.push(Segment { clip, range: (start as i64)..(i as i64), bucket: buckets[start] });
            start = i;
        }
    }
    segments
}

// ------------------------------------------------------- freeze / verify

/// A stable hash, deliberately NOT `std`'s `DefaultHasher`: its exact
/// algorithm is not a documented cross-version guarantee, and a freeze
/// mechanism whose fingerprint could shift under a toolchain upgrade
/// would defeat the purpose. FNV-1a, spelled out in full so its behaviour
/// never depends on anything outside this file.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Quantize a score to a coarse step before hashing, so float noise
/// between two decodes of visually-identical content (a re-encode at a
/// different bitrate; different SIMD rounding on another machine) doesn't
/// register as a change. 1/32 steps: coarse enough to absorb that noise,
/// fine enough that a real difference in motion or skin fraction still
/// moves it.
fn quantize(x: f32) -> u8 {
    (x.clamp(0.0, 1.0) * 32.0).round() as u8
}

/// A fingerprint of a source's cheap scores — see the module doc for what
/// it's for. Order-sensitive (a fingerprint is meaningless if frames were
/// silently reordered) and quantized before hashing.
pub fn fingerprint(scores: &Scores) -> u64 {
    let mut bytes = Vec::with_capacity(scores.motion.len() * 2);
    for i in 0..scores.motion.len() {
        bytes.push(quantize(scores.motion[i]));
        bytes.push(quantize(scores.skin.get(i).copied().unwrap_or(0.0)));
    }
    fnv1a(&bytes)
}

/// A profile, frozen for storage in a project sidecar.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileFreeze {
    pub version: u32,
    pub fingerprint: u64,
    pub segments: Vec<Segment>,
}

impl ProfileFreeze {
    pub fn new(clip: ClipId, scores: &Scores, params: Params) -> Self {
        Self { version: CURRENT_VERSION, fingerprint: fingerprint(scores), segments: profile(clip, scores, params) }
    }
}

/// What a fresh re-scan found, compared to a frozen profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verify {
    /// Same algorithm version, same fingerprint: trust the frozen segments.
    Fresh,
    /// The frozen profile was written by a different algorithm version —
    /// always stale, regardless of fingerprint, because the fingerprint
    /// itself may be computed differently between versions.
    AlgorithmChanged { frozen_version: u32, current_version: u32 },
    /// Same algorithm version, different fingerprint: the SOURCE changed
    /// (re-encoded, replaced, trimmed) since this was profiled.
    SourceChanged,
}

impl Verify {
    pub fn is_fresh(self) -> bool {
        matches!(self, Verify::Fresh)
    }
}

/// Check a frozen profile against a fresh (cheap) re-scan of the same
/// source. Never re-runs the full analyzer — that's the caller's job if
/// this returns anything but `Fresh`.
pub fn verify(freeze: &ProfileFreeze, live_scores: &Scores) -> Verify {
    if freeze.version != CURRENT_VERSION {
        return Verify::AlgorithmChanged { frozen_version: freeze.version, current_version: CURRENT_VERSION };
    }
    if freeze.fingerprint != fingerprint(live_scores) {
        return Verify::SourceChanged;
    }
    Verify::Fresh
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb_gray(w: usize, h: usize, v: u8) -> Vec<u8> {
        vec![v; w * h * 3]
    }
    fn rgb_skin(w: usize, h: usize) -> Vec<u8> {
        let mut v = Vec::with_capacity(w * h * 3);
        for _ in 0..w * h {
            v.extend_from_slice(&[210, 160, 130]); // a plausible mid skin tone
        }
        v
    }

    #[test]
    fn luma_diff_is_zero_for_identical_frames_and_scales_with_a_uniform_step() {
        let a = vec![100u8; 300];
        assert_eq!(luma_diff(&a, &a), 0.0);
        let b = vec![110u8; 300];
        assert!((luma_diff(&a, &b) - 10.0 / 255.0).abs() < 1e-6);
        assert_eq!(luma_diff(&a, &[]), 0.0, "mismatched lengths are refused quietly, not a panic");
    }

    #[test]
    fn skin_fraction_is_high_on_a_skin_toned_buffer_and_low_on_gray_or_blue() {
        assert!(skin_fraction(&rgb_skin(4, 4)) > 0.8);
        assert!(skin_fraction(&rgb_gray(4, 4, 128)) < 0.1);
        let blue = { let mut v = Vec::new(); for _ in 0..16 { v.extend_from_slice(&[30, 60, 200]); } v };
        assert!(skin_fraction(&blue) < 0.1);
    }

    #[test]
    fn detect_cuts_finds_a_spike_against_a_moving_baseline_not_a_fixed_threshold() {
        // A scene with ordinary motion around 0.10, and one frame that
        // spikes to 0.5 — a real cut against a BUSY scene, which a fixed
        // absolute threshold tuned for a static scene would miss or, tuned
        // for this scene, would false-positive on static footage instead.
        let mut motion = vec![0.10f32; 40];
        motion[20] = 0.5;
        let cuts = detect_cuts(&motion, 8, 4.0, 0.02);
        assert_eq!(cuts, vec![20]);
        // The same absolute spike value, but as the ONLY motion in an
        // otherwise still scene, is still found (ratio test against a
        // near-zero median, gated by the floor so it isn't noise).
        let mut still = vec![0.0f32; 40];
        still[20] = 0.5;
        assert_eq!(detect_cuts(&still, 8, 4.0, 0.02), vec![20]);
        // Uniform motion, no spike anywhere: no cuts.
        assert!(detect_cuts(&vec![0.1f32; 40], 8, 4.0, 0.02).is_empty());
    }

    #[test]
    fn classify_priority_is_face_over_cut_over_motion_over_static() {
        let p = Params::default();
        assert_eq!(classify(0.0, 0.9, true, p.skin_threshold, p.motion_threshold), Bucket::Face);
        assert_eq!(classify(0.5, 0.0, true, p.skin_threshold, p.motion_threshold), Bucket::CutBoundary);
        assert_eq!(classify(0.5, 0.0, false, p.skin_threshold, p.motion_threshold), Bucket::Motion);
        assert_eq!(classify(0.0, 0.0, false, p.skin_threshold, p.motion_threshold), Bucket::Static);
    }

    #[test]
    fn profile_merges_adjacent_same_bucket_frames_and_expands_a_cut_margin() {
        let n = 60;
        let mut motion = vec![0.0f32; n];
        motion[30] = 0.9; // an isolated hard cut in an otherwise static clip
        let skin = vec![0.0f32; n];
        let params = Params { cut_margin: 5, ..Default::default() };
        let segs = profile(7, &Scores { motion, skin }, params);
        // Expect: Static 0..25, CutBoundary 25..36, Static 36..60 (margin
        // 5 on each side of frame 30).
        assert_eq!(segs.len(), 3, "{segs:?}");
        assert_eq!(segs[0], Segment { clip: 7, range: 0..25, bucket: Bucket::Static });
        assert_eq!(segs[1], Segment { clip: 7, range: 25..36, bucket: Bucket::CutBoundary });
        assert_eq!(segs[2], Segment { clip: 7, range: 36..60, bucket: Bucket::Static });
    }

    #[test]
    fn a_sustained_face_run_produces_one_face_segment_even_through_a_cut_inside_it() {
        let n = 40;
        let mut motion = vec![0.02f32; n];
        motion[20] = 0.9; // a cut happens mid-shot (e.g. a lighting change)
        let skin = vec![0.5f32; n]; // skin throughout
        let segs = profile(1, &Scores { motion, skin }, Params::default());
        assert_eq!(segs, vec![Segment { clip: 1, range: 0..40, bucket: Bucket::Face }], "Face outranks CutBoundary throughout");
    }

    #[test]
    fn fingerprint_is_stable_under_tiny_float_noise_and_changes_on_a_real_difference() {
        let base = Scores { motion: vec![0.10, 0.11, 0.50, 0.12], skin: vec![0.0, 0.0, 0.0, 0.0] };
        let noisy = Scores { motion: base.motion.iter().map(|x| x + 0.001).collect(), skin: base.skin.clone() };
        assert_eq!(fingerprint(&base), fingerprint(&noisy), "quantization should absorb this much noise");
        let different = Scores { motion: vec![0.10, 0.11, 0.50, 0.90], skin: base.skin.clone() };
        assert_ne!(fingerprint(&base), fingerprint(&different));
    }

    #[test]
    fn verify_distinguishes_a_changed_source_from_a_changed_algorithm() {
        let scores = Scores { motion: vec![0.1, 0.1, 0.6, 0.1], skin: vec![0.0; 4] };
        let freeze = ProfileFreeze::new(1, &scores, Params::default());
        assert_eq!(verify(&freeze, &scores), Verify::Fresh);

        let changed_source = Scores { motion: vec![0.1, 0.1, 0.1, 0.1], skin: vec![0.0; 4] };
        assert_eq!(verify(&freeze, &changed_source), Verify::SourceChanged);

        let mut old_freeze = freeze.clone();
        old_freeze.version = CURRENT_VERSION - 1;
        assert_eq!(
            verify(&old_freeze, &scores),
            Verify::AlgorithmChanged { frozen_version: CURRENT_VERSION - 1, current_version: CURRENT_VERSION }
        );
        // Algorithm-changed wins even if the fingerprint still happens to
        // match — it isn't safe to trust a stale version's fingerprint
        // math regardless of its numeric value.
        assert!(!verify(&old_freeze, &scores).is_fresh());
    }

    /// THE CHECKER. A fixed synthetic scenario's fingerprint, pinned to a
    /// literal constant. If this fails, either a real bug was just fixed
    /// (good — but bump `CURRENT_VERSION` and update the constant below so
    /// every already-frozen profile in the wild is correctly treated as
    /// stale) or the scoring math drifted by accident (bad — revert it).
    /// Either way, this test's job is to make that an explicit decision
    /// instead of a silent one.
    #[test]
    fn golden_fingerprint_is_pinned() {
        assert_eq!(CURRENT_VERSION, 1, "bump this alongside the constant below when the scoring math intentionally changes");
        let n = 30;
        let motion: Vec<f32> = (0..n).map(|i| if i == 15 { 0.8 } else { 0.05 + 0.01 * (i % 3) as f32 }).collect();
        let skin: Vec<f32> = (0..n).map(|i| if i >= 20 { 0.4 } else { 0.0 }).collect();
        let scores = Scores { motion, skin };
        // Computed once by running this test and reading the value out of
        // its own failure message, then pasted here — this literal IS the
        // golden value, not a guess at one.
        assert_eq!(fingerprint(&scores), 0x2e5e_a7b6_d6bd_74d1, "fingerprint drifted — see this test's doc comment");
    }
}
