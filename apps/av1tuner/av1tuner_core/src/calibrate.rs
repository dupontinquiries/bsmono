//! calibrate — find out what this encoder's CRF actually does, here.
//!
//! Two problems this exists for, both observed rather than imagined.
//!
//! ## The CRF number is not a quality
//!
//! `x264 -crf 23` and `svtav1 -crf 23` are unrelated scales that happen
//! to share a number range. Measured on one clip: x264 at 23 gave
//! 1,872,988 bytes and SVT-AV1 at 23 gave 2,964,082 -- 58% BIGGER for
//! the same number. The equivalent was near 30, and 35 was 45% smaller
//! at comparable quality.
//!
//! Anyone moving a library from H.264 to AV1 hits this, concludes AV1
//! is worse, and stops. The offset is roughly +10 to +13, but "roughly"
//! is doing a lot of work: it varies with content, preset, and encoder
//! version. It should be MEASURED, on the actual footage, which takes
//! about a minute on a short sample and settles the question.
//!
//! ## The scale itself can be wrong
//!
//! SVT-AV1 4.x introduced quarter-step CRF, and 4.2 added a separate
//! `--cqp`. A build or wrapper written for integer CRF against a
//! quarter-step scale produces quality that jumps around in ways that
//! look like the encoder misbehaving. So a sweep here does not just
//! report sizes -- it CHECKS THE SCALE IS SANE: monotonic, and moving
//! by a plausible amount per step. A scale that fails those is a
//! toolchain problem, and saying so beats letting someone tune against
//! a broken axis for an afternoon.
//!
//! ## And where the sample comes from matters
//!
//! `pick_sample_range` takes a span of the clip. If that span happens
//! to be a locked-off shot of a wall, every CRF looks fine and the
//! chosen setting falls apart on the first handheld take. Sharp,
//! detailed, moving frames are the ones that cost bits, so the sample
//! should be drawn from those -- measured by the variance of a
//! Laplacian, which is the standard cheap focus estimator and happens
//! to also rank detail and motion blur the way we want.

use std::collections::BTreeMap;

/// One rung of a sweep.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rung {
    pub crf: f32,
    pub bytes: u64,
    /// Luma PSNR against the source, if it was measured.
    pub psnr_y: Option<f64>,
}

/// What a sweep found, and whether to trust it.
#[derive(Debug, Clone, PartialEq)]
pub struct Sweep {
    pub rungs: Vec<Rung>,
    /// Problems with the SCALE, not with the footage.
    pub scale_faults: Vec<String>,
}

impl Sweep {
    pub fn new(mut rungs: Vec<Rung>) -> Sweep {
        rungs.sort_by(|a, b| a.crf.partial_cmp(&b.crf).unwrap_or(std::cmp::Ordering::Equal));
        let faults = check_scale(&rungs);
        Sweep { rungs, scale_faults: faults }
    }

    /// Is the scale usable at all?
    pub fn trustworthy(&self) -> bool { self.scale_faults.is_empty() }

    /// The CRF whose output is closest to a target size, interpolated.
    pub fn crf_for_bytes(&self, target: u64) -> Option<f32> {
        if self.rungs.len() < 2 { return None; }
        for w in self.rungs.windows(2) {
            let (hi, lo) = (w[0].bytes, w[1].bytes);   // bytes fall as crf rises
            if target <= hi && target >= lo {
                let span = (hi - lo) as f64;
                if span < 1.0 { return Some(w[0].crf); }
                let t = (hi - target) as f64 / span;
                return Some(w[0].crf + (w[1].crf - w[0].crf) * t as f32);
            }
        }
        // outside the swept range: say which end rather than
        // extrapolating, because the curve is not linear out there
        None
    }

    /// The knee: where spending more bits stops buying much.
    ///
    /// Defined as the last rung where dropping one step of CRF still
    /// costs less than `worth` extra bytes per dB gained. Without PSNR
    /// there is no knee to find, and returning a guess would be worse
    /// than returning nothing.
    pub fn knee(&self, worth_bytes_per_db: f64) -> Option<Rung> {
        let with: Vec<&Rung> = self.rungs.iter().filter(|r| r.psnr_y.is_some()).collect();
        if with.len() < 2 { return None; }
        let mut best: Option<&Rung> = None;
        for w in with.windows(2) {
            let (sharper, softer) = (w[0], w[1]);
            let d_db = sharper.psnr_y.unwrap() - softer.psnr_y.unwrap();
            if d_db <= 0.0 { continue; }
            let d_bytes = sharper.bytes.saturating_sub(softer.bytes) as f64;
            if d_bytes / d_db <= worth_bytes_per_db { best = Some(sharper); }
        }
        best.copied().or_else(|| with.last().map(|r| **r))
    }
}

/// Is this a sane quality scale?
///
/// Checks the things that go wrong when a tool and an encoder disagree
/// about what CRF means, rather than anything about the picture.
pub fn check_scale(rungs: &[Rung]) -> Vec<String> {
    let mut out = Vec::new();
    if rungs.len() < 3 { return out; }
    // MONOTONIC: a higher CRF must not produce a bigger file. When it
    // does, the number is not reaching the encoder as intended -- a
    // quarter-step scale read as integers, or a wrapper clamping.
    for w in rungs.windows(2) {
        if w[1].bytes > w[0].bytes {
            out.push(format!(
                "crf {} produced MORE bytes than crf {} ({} vs {}). A higher crf must never \
                 cost more; the value is probably not reaching the encoder as written -- \
                 SVT-AV1 4.x has quarter-step crf and a separate --cqp.",
                w[1].crf, w[0].crf, w[1].bytes, w[0].bytes));
        }
    }
    // MOVING: if several steps produce near-identical sizes, the knob
    // is being rounded away
    let flat = rungs.windows(2).filter(|w| {
        let (a, b) = (w[0].bytes as f64, w[1].bytes as f64);
        (a - b).abs() / a.max(1.0) < 0.02
    }).count();
    if flat >= 2 && flat * 2 >= rungs.len() {
        out.push(format!(
            "{flat} of {} steps changed the size by under 2%. The crf value is being rounded \
             or ignored rather than applied.", rungs.len() - 1));
    }
    // PLAUSIBLE RANGE: across a wide sweep the size should move a lot
    let (first, last) = (rungs[0].bytes as f64, rungs[rungs.len() - 1].bytes as f64);
    let span = rungs[rungs.len() - 1].crf - rungs[0].crf;
    if span >= 12.0 && first / last.max(1.0) < 1.5 {
        out.push(format!(
            "crf {} to {} changed the size by only {:.0}%. A span that wide should change it \
             several-fold.", rungs[0].crf, rungs[rungs.len() - 1].crf,
            100.0 * (first / last.max(1.0) - 1.0)));
    }
    out
}

/// Map a CRF from one encoder onto another, by size.
///
/// The honest way to answer "what SVT-AV1 crf matches my x264 23": not
/// a remembered offset, but a sweep of each on the SAME footage,
/// matched where they produce the same bytes.
pub fn equivalent_crf(from: &Sweep, from_crf: f32, to: &Sweep) -> Option<f32> {
    let target = interpolate_bytes(from, from_crf)?;
    to.crf_for_bytes(target)
}

fn interpolate_bytes(s: &Sweep, crf: f32) -> Option<u64> {
    if s.rungs.is_empty() { return None; }
    if let Some(exact) = s.rungs.iter().find(|r| (r.crf - crf).abs() < 0.01) {
        return Some(exact.bytes);
    }
    for w in s.rungs.windows(2) {
        if crf >= w[0].crf && crf <= w[1].crf {
            let t = ((crf - w[0].crf) / (w[1].crf - w[0].crf)) as f64;
            let (a, b) = (w[0].bytes as f64, w[1].bytes as f64);
            return Some((a + (b - a) * t) as u64);
        }
    }
    None
}

// ── choosing what to measure ─────────────────────────────────────────

/// How hard one frame is to encode, cheaply.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameCost {
    pub index: u64,
    /// Variance of the Laplacian: high means sharp and detailed, which
    /// is also what costs bits. Doubles as the focus measure.
    pub detail: f64,
    /// Mean absolute difference from the previous sampled frame.
    pub motion: f64,
}

impl FrameCost {
    /// One number for "expensive to encode". Detail and motion both
    /// cost bits and they are not the same thing -- a sharp still and a
    /// blurry pan are each cheap in one and dear in the other.
    pub fn hardness(&self) -> f64 { self.detail.sqrt() * (1.0 + self.motion / 8.0) }
}

/// Variance of the Laplacian over a single-channel image.
///
/// The standard cheap focus estimator. Used here for two things at
/// once: finding the sharpest frame (which is where a clapperboard is
/// legible) and finding the most detailed frame (which is where a
/// bitrate decision should be made).
pub fn detail_of(px: &[u8], w: usize, h: usize) -> f64 {
    if w < 3 || h < 3 || px.len() < w * h { return 0.0; }
    let at = |x: usize, y: usize| px[y * w + x] as f64;
    let mut vals = Vec::with_capacity((w - 2) * (h - 2));
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            vals.push(at(x + 1, y) + at(x - 1, y) + at(x, y + 1) + at(x, y - 1) - 4.0 * at(x, y));
        }
    }
    if vals.is_empty() { return 0.0; }
    let mean = vals.iter().sum::<f64>() / vals.len() as f64;
    vals.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / vals.len() as f64
}

/// Mean absolute difference between two frames of the same size.
pub fn motion_between(a: &[u8], b: &[u8]) -> f64 {
    if a.len() != b.len() || a.is_empty() { return 0.0; }
    a.iter().zip(b).map(|(x, y)| (*x as f64 - *y as f64).abs()).sum::<f64>() / a.len() as f64
}

/// Choose which frames a sweep should be run on.
///
/// THE HARDEST ONES, not a contiguous span. A sample taken from a
/// locked-off shot of a wall makes every crf look fine, and the setting
/// chosen from it falls apart on the first handheld take. Tuning on the
/// easy part of a clip is how a whole library gets encoded too low.
///
/// Spread across the clip too: the hardest ten frames are often
/// consecutive, and ten frames of one shot is one measurement.
pub fn hard_frames(costs: &[FrameCost], want: usize, spread_over: u64) -> Vec<u64> {
    if costs.is_empty() || want == 0 { return Vec::new(); }
    let want = want.min(costs.len());
    let buckets = want.max(1);
    let span = spread_over.max(1);
    let mut by_bucket: BTreeMap<u64, &FrameCost> = BTreeMap::new();
    for c in costs {
        let b = (c.index * buckets as u64 / span).min(buckets as u64 - 1);
        by_bucket.entry(b)
            .and_modify(|best| if c.hardness() > best.hardness() { *best = c })
            .or_insert(c);
    }
    let mut picked: Vec<&FrameCost> = by_bucket.into_values().collect();
    // if some buckets were empty, top up with the hardest of what is left
    if picked.len() < want {
        let mut rest: Vec<&FrameCost> = costs.iter()
            .filter(|c| !picked.iter().any(|p| p.index == c.index)).collect();
        rest.sort_by(|a, b| b.hardness().partial_cmp(&a.hardness()).unwrap_or(std::cmp::Ordering::Equal));
        picked.extend(rest.into_iter().take(want - picked.len()));
    }
    let mut out: Vec<u64> = picked.iter().map(|c| c.index).collect();
    out.sort_unstable();
    out.truncate(want);
    out
}

/// The sharpest frame in a window: where a clapperboard is legible.
///
/// Focus varies enormously across even a short handheld shot --
/// measured 19 to 73 across fifteen frames of one clip, nearly 4x. A
/// reader handed an arbitrary frame from that window is being set up to
/// fail for no reason.
pub fn sharpest(costs: &[FrameCost], from: u64, to: u64) -> Option<u64> {
    costs.iter().filter(|c| c.index >= from && c.index <= to)
        .max_by(|a, b| a.detail.partial_cmp(&b.detail).unwrap_or(std::cmp::Ordering::Equal))
        .map(|c| c.index)
}

/// What to tell the operator about a sweep.
pub fn report(s: &Sweep, fps: f64) -> String {
    let mut out = String::new();
    if !s.trustworthy() {
        out.push_str("THE QUALITY SCALE ITSELF LOOKS WRONG:\n");
        for f in &s.scale_faults { out.push_str(&format!("  {f}\n")); }
        out.push_str("Fix that before tuning against it.\n\n");
    }
    out.push_str("  crf     bytes        Mb/s    psnr\n");
    for r in &s.rungs {
        let mbps = r.bytes as f64 * 8.0 * fps / 1e6;
        out.push_str(&format!("  {:<6.1}  {:<11}  {:<6.1}  {}\n", r.crf, r.bytes, mbps,
            r.psnr_y.map(|p| format!("{p:.2}")).unwrap_or_else(|| "-".into())));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rung(crf: f32, bytes: u64, psnr: f64) -> Rung {
        Rung { crf, bytes, psnr_y: Some(psnr) }
    }

    /// The real numbers measured on one clip, so the tests are checking
    /// against something that happened.
    fn measured_svtav1() -> Sweep {
        Sweep::new(vec![rung(23.0, 2_964_082, 46.0), rung(30.0, 1_881_933, 44.2),
                        rung(35.0, 1_033_508, 42.6), rung(40.0, 401_921, 40.1)])
    }

    #[test]
    fn a_crf_from_one_encoder_maps_onto_another_by_measurement() {
        // "what svtav1 crf matches my x264 23" answered by sweeping
        // BOTH on the same footage, not by a remembered offset
        let x264 = Sweep::new(vec![rung(18.0, 3_100_000, 47.0), rung(23.0, 1_872_988, 44.1),
                                   rung(28.0, 1_050_000, 41.5)]);
        let av1 = measured_svtav1();
        let eq = equivalent_crf(&x264, 23.0, &av1).expect("a mapping");
        // x264 23 gave 1,872,988 bytes; svtav1 hits that near crf 30
        assert!((eq - 30.0).abs() < 1.5, "x264 23 maps to svtav1 {eq:.1}");
        // and that is the +7 to +13 offset people quote, MEASURED
        assert!(eq > 23.0 + 5.0, "the offset is real and large: {eq:.1}");
    }

    #[test]
    fn a_scale_that_goes_backwards_is_reported_as_a_toolchain_fault() {
        // a higher crf producing a BIGGER file means the number is not
        // reaching the encoder -- quarter-step crf read as integers, a
        // wrapper clamping, a version mismatch
        let broken = Sweep::new(vec![rung(20.0, 1_000_000, 44.0), rung(25.0, 1_400_000, 43.0),
                                     rung(30.0, 900_000, 42.0)]);
        assert!(!broken.trustworthy());
        let f = &broken.scale_faults[0];
        assert!(f.contains("MORE bytes"), "{f}");
        assert!(f.contains("quarter-step"), "it names the likely cause: {f}");
        // and a sane scale passes
        assert!(measured_svtav1().trustworthy(), "{:?}", measured_svtav1().scale_faults);
    }

    #[test]
    fn a_scale_that_does_not_move_is_also_a_fault() {
        // several steps producing the same size means the knob is being
        // rounded away, which looks like the encoder ignoring you
        let stuck = Sweep::new(vec![rung(20.0, 1_000_000, 44.0), rung(25.0, 995_000, 44.0),
                                    rung(30.0, 990_000, 43.9), rung(35.0, 988_000, 43.9)]);
        assert!(!stuck.trustworthy());
        assert!(stuck.scale_faults.iter().any(|f| f.contains("rounded or ignored")),
                "{:?}", stuck.scale_faults);
        // and the wide-span check catches a scale that barely moves
        let narrow = Sweep::new(vec![rung(20.0, 1_000_000, 44.0), rung(28.0, 920_000, 43.0),
                                     rung(36.0, 850_000, 42.0)]);
        assert!(narrow.scale_faults.iter().any(|f| f.contains("several-fold")),
                "{:?}", narrow.scale_faults);
    }

    #[test]
    fn a_target_size_becomes_a_crf() {
        let s = measured_svtav1();
        // between the crf 30 and crf 35 rungs
        let c = s.crf_for_bytes(1_500_000).expect("a crf");
        assert!(c > 30.0 && c < 35.0, "{c}");
        // outside the swept range returns nothing rather than
        // extrapolating a curve that is not linear out there
        assert!(s.crf_for_bytes(50).is_none(), "too small to have been measured");
        assert!(s.crf_for_bytes(9_000_000).is_none(), "too big");
    }

    #[test]
    fn the_sample_comes_from_the_hard_part_of_the_clip() {
        // tuning on a locked-off shot of a wall is how a whole library
        // gets encoded too low
        let costs: Vec<FrameCost> = (0..100u64).map(|i| FrameCost {
            index: i,
            // one busy stretch in the middle, still elsewhere
            detail: if (40..60).contains(&i) { 900.0 } else { 20.0 },
            motion: if (40..60).contains(&i) { 12.0 } else { 0.5 },
        }).collect();
        let picked = hard_frames(&costs, 4, 100);
        assert_eq!(picked.len(), 4);
        assert!(picked.iter().any(|i| (40..60).contains(i)), "it found the busy part: {picked:?}");
        // BUT SPREAD OUT: the hardest ten frames are often consecutive,
        // and ten frames of one shot is one measurement
        let span = picked.last().unwrap() - picked.first().unwrap();
        assert!(span > 40, "sampled across the clip, not one burst: {picked:?}");
    }

    #[test]
    fn the_sharpest_frame_in_a_window_is_findable() {
        // focus varied 19 to 73 across fifteen frames of a real clip --
        // nearly 4x. Handing a reader an arbitrary frame from that
        // window sets it up to fail for no reason.
        let costs: Vec<FrameCost> = vec![
            FrameCost { index: 0, detail: 19.1, motion: 1.0 },
            FrameCost { index: 7, detail: 73.4, motion: 1.0 },
            FrameCost { index: 14, detail: 26.3, motion: 1.0 },
        ];
        assert_eq!(sharpest(&costs, 0, 14), Some(7));
        assert_eq!(sharpest(&costs, 8, 14), Some(14), "restricted to the window");
        assert_eq!(sharpest(&costs, 20, 30), None, "nothing in range");
    }

    #[test]
    fn detail_and_motion_are_different_kinds_of_expensive() {
        // a sharp still and a blurry pan are each cheap in one and dear
        // in the other, and hardness must not let either hide
        let sharp_still = FrameCost { index: 0, detail: 900.0, motion: 0.2 };
        let blurry_pan = FrameCost { index: 1, detail: 25.0, motion: 30.0 };
        let easy = FrameCost { index: 2, detail: 20.0, motion: 0.3 };
        assert!(sharp_still.hardness() > easy.hardness() * 3.0);
        assert!(blurry_pan.hardness() > easy.hardness());
        // and a frame that is both is hardest of all
        let both = FrameCost { index: 3, detail: 900.0, motion: 30.0 };
        assert!(both.hardness() > sharp_still.hardness());
        assert!(both.hardness() > blurry_pan.hardness());
    }

    #[test]
    fn the_laplacian_ranks_a_sharp_image_above_a_flat_one() {
        let (w, h) = (32, 32);
        let flat = vec![128u8; w * h];
        let mut edges = vec![128u8; w * h];
        for y in 0..h { for x in 0..w { if (x / 2) % 2 == 0 { edges[y * w + x] = 240; } } }
        assert!(detail_of(&edges, w, h) > detail_of(&flat, w, h) * 50.0);
        assert_eq!(detail_of(&flat, w, h), 0.0, "a flat field has no detail");
        // and it does not panic on nonsense
        assert_eq!(detail_of(&[], 0, 0), 0.0);
        assert_eq!(detail_of(&[1, 2, 3], 32, 32), 0.0, "short buffer");
    }

    #[test]
    fn a_broken_scale_is_said_before_the_numbers_not_after() {
        let broken = Sweep::new(vec![rung(20.0, 1_000_000, 44.0), rung(25.0, 1_400_000, 43.0),
                                     rung(30.0, 900_000, 42.0)]);
        let r = report(&broken, 24.0);
        let warn = r.find("QUALITY SCALE ITSELF LOOKS WRONG").expect("warned");
        let table = r.find("crf     bytes").expect("table");
        assert!(warn < table, "the warning comes first, or it is read after the decision");
        assert!(r.contains("Fix that before tuning against it"));
        // a good sweep just shows the table
        assert!(!report(&measured_svtav1(), 24.0).contains("LOOKS WRONG"));
    }
}
