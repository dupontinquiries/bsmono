//! buckets — grouping a timeline by what is actually happening in it.
//!
//! Adobe draws a green bar over what is rendered. It says WHETHER a
//! region is cached and nothing about what the region IS, so the
//! decision about what to render next is the editor's, made by
//! scrubbing and waiting.
//!
//! The same per-frame motion analysis the global motion estimator
//! already does answers a more useful question: what KIND of footage is
//! this stretch? A cut is a discontinuity. A pan is a large, coherent
//! global motion. Handheld is small, incoherent motion. A locked-off
//! shot is nothing at all. Those are different things to preview and
//! they want different bitrates.
//!
//! ## Why this saves real time
//!
//! Preview render is a fixed budget spent badly. A locked-off dialogue
//! shot needs almost no bitrate to judge -- nothing moves, and what the
//! editor is checking is performance and timing. A whip pan needs a lot
//! or it turns to mush and they cannot see the thing they are judging.
//! Spending the same rate on both wastes it on the first and starves
//! the second.
//!
//! ## And it comes free with the encoder work
//!
//! `estimate` already computes, per frame pair: a global motion model,
//! how much of the frame it explains, and the residual after applying
//! it. That is exactly the signal needed here. Nothing extra is
//! decoded, and the two features share a measurement rather than each
//! having their own.

/// What a stretch of timeline is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Character {
    /// Nothing moves. Cheap to preview and cheap to encode.
    Locked,
    /// A coherent camera move: the global model explains most of it.
    Pan,
    /// Motion the global model does NOT explain: a subject moving, or
    /// handheld shake. Expensive, and the part an editor is usually
    /// judging.
    Subject,
    /// Fast enough that detail is lost to blur anyway.
    Whip,
    /// A discontinuity: one frame does not follow from the last.
    Cut,
}

impl Character {
    /// Preview bitrate multiplier, relative to the sequence default.
    ///
    /// These are the judgement, and they are judgements rather than
    /// measurements: how much rate a KIND of shot needs to be
    /// judgeable is about what the editor is looking for, not about
    /// PSNR. Stated as numbers so they can be argued with.
    pub fn rate_scale(self) -> f32 {
        match self {
            // nothing moves; the editor is judging performance and
            // timing, and a low rate shows both perfectly
            Character::Locked => 0.45,
            // a pan needs enough not to smear, but global motion means
            // the encoder predicts it well
            Character::Pan => 0.9,
            // the expensive case AND the one being judged
            Character::Subject => 1.4,
            // already blurred by the camera; spending rate on detail
            // that the lens destroyed is waste
            Character::Whip => 0.6,
            Character::Cut => 1.0,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Character::Locked => "locked off", Character::Pan => "camera move",
            Character::Subject => "subject motion", Character::Whip => "whip",
            Character::Cut => "cut",
        }
    }
}

/// One frame's worth of measurement, from the global motion estimator.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameMotion {
    pub index: u64,
    /// Magnitude of the global translation, in pixels.
    pub global_px: f64,
    /// How much of the frame the global model explained, 0 to 1.
    pub explained: f64,
    /// Mean absolute error remaining after the global model. High means
    /// something is moving that the camera is not.
    pub residual: f64,
}

impl FrameMotion {
    /// Build from an `Estimate`, which is where this data already
    /// exists.
    pub fn from_estimate(index: u64, e: &crate::Estimate) -> FrameMotion {
        FrameMotion {
            index,
            global_px: (e.warp.tx * e.warp.tx + e.warp.ty * e.warp.ty).sqrt(),
            explained: if e.sad_before > 0.0 {
                ((e.sad_before - e.sad_after) / e.sad_before).clamp(0.0, 1.0)
            } else { 0.0 },
            residual: e.sad_after,
        }
    }
}

/// Thresholds, in one place so they can be tuned per project rather
/// than hidden in the classifier.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    /// Below this much global motion, nothing is moving.
    pub still_px: f64,
    /// Above this, the camera is moving fast enough to blur.
    pub whip_px: f64,
    /// Residual above this means something is moving independently.
    pub busy_residual: f64,
    /// A frame the previous one does not predict at all.
    pub cut_residual: f64,
}

impl Default for Bounds {
    fn default() -> Self {
        Bounds { still_px: 0.7, whip_px: 24.0, busy_residual: 9.0, cut_residual: 28.0 }
    }
}

pub fn classify(m: &FrameMotion, b: &Bounds) -> Character {
    // A CUT FIRST. Everything else assumes the frames are related, and
    // a cut makes the global motion number meaningless rather than
    // large -- the estimator will happily report a confident warp
    // between two unrelated images.
    if m.residual >= b.cut_residual && m.explained < 0.25 { return Character::Cut; }
    if m.global_px >= b.whip_px { return Character::Whip; }
    if m.residual >= b.busy_residual { return Character::Subject; }
    if m.global_px <= b.still_px { return Character::Locked; }
    Character::Pan
}

/// A run of frames sharing a character.
#[derive(Debug, Clone, PartialEq)]
pub struct Bucket {
    pub character: Character,
    pub from: u64,
    /// Inclusive.
    pub to: u64,
}

impl Bucket {
    pub fn frames(&self) -> u64 { self.to - self.from + 1 }
}

/// Group a sequence into runs.
///
/// `min_run` merges away single-frame flickers. Without it a handheld
/// shot alternates between Pan and Subject frame by frame and produces
/// two hundred buckets, which is not a segmentation, it is the raw
/// signal with extra steps.
pub fn bucket(motions: &[FrameMotion], b: &Bounds, min_run: u64) -> Vec<Bucket> {
    if motions.is_empty() { return Vec::new(); }
    let mut runs: Vec<Bucket> = Vec::new();
    for m in motions {
        let c = classify(m, b);
        match runs.last_mut() {
            Some(last) if last.character == c && m.index == last.to + 1 => last.to = m.index,
            _ => runs.push(Bucket { character: c, from: m.index, to: m.index }),
        }
    }
    if min_run <= 1 { return runs; }
    // absorb short runs into whichever neighbour is longer -- a cut is
    // never absorbed, because a one-frame cut is exactly what a cut is
    let mut out: Vec<Bucket> = Vec::new();
    for r in runs {
        if r.frames() < min_run && r.character != Character::Cut && !out.is_empty() {
            let last = out.last_mut().unwrap();
            last.to = r.to;
            continue;
        }
        out.push(r);
    }
    out
}

/// What to render, and at what rate.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderPlan {
    pub bucket: Bucket,
    /// Bitrate for this stretch, in the same units the caller passed.
    pub rate: f32,
    /// Why it got that, for the operator.
    pub because: String,
}

/// Spend a preview budget where it will be seen.
///
/// The naive alternative is a flat rate everywhere, which is what most
/// tools do. This gives the same total to the stretches that need it.
pub fn plan(buckets: &[Bucket], base_rate: f32) -> Vec<RenderPlan> {
    buckets.iter().map(|b| {
        let scale = b.character.rate_scale();
        RenderPlan {
            bucket: b.clone(),
            rate: base_rate * scale,
            because: format!("{} over {} frame(s): {:.0}% of base",
                             b.character.name(), b.frames(), scale * 100.0),
        }
    }).collect()
}

/// What the flat-rate alternative would have cost, against this.
///
/// Reported because a saving nobody measures is a claim.
pub fn savings(plans: &[RenderPlan], base_rate: f32) -> f64 {
    let flat: f64 = plans.iter().map(|p| base_rate as f64 * p.bucket.frames() as f64).sum();
    let ours: f64 = plans.iter().map(|p| p.rate as f64 * p.bucket.frames() as f64).sum();
    if flat <= 0.0 { return 0.0; }
    (flat - ours) / flat
}

/// The regions an editor is most likely to want rendered first.
///
/// Cuts and subject motion, because those are what a cut is judged on.
/// A locked-off stretch is watchable at almost any rate, so it can wait.
pub fn priority_order(buckets: &[Bucket]) -> Vec<&Bucket> {
    let mut v: Vec<&Bucket> = buckets.iter().collect();
    v.sort_by_key(|b| match b.character {
        Character::Cut => 0,
        Character::Subject => 1,
        Character::Whip => 2,
        Character::Pan => 3,
        Character::Locked => 4,
    });
    v
}

pub fn summary(plans: &[RenderPlan], base_rate: f32) -> String {
    let mut s = format!("{} region(s), {:.0}% less preview data than a flat rate\n",
                        plans.len(), savings(plans, base_rate) * 100.0);
    for p in plans {
        s.push_str(&format!("  {:>5}-{:<5} {:<16} {:>6.0} kb/s\n",
                            p.bucket.from, p.bucket.to, p.bucket.character.name(), p.rate));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(index: u64, global_px: f64, explained: f64, residual: f64) -> FrameMotion {
        FrameMotion { index, global_px, explained, residual }
    }

    #[test]
    fn the_five_characters_are_told_apart() {
        let b = Bounds::default();
        assert_eq!(classify(&m(0, 0.1, 0.9, 2.0), &b), Character::Locked);
        assert_eq!(classify(&m(1, 6.0, 0.85, 3.0), &b), Character::Pan);
        assert_eq!(classify(&m(2, 3.0, 0.4, 14.0), &b), Character::Subject);
        assert_eq!(classify(&m(3, 40.0, 0.7, 5.0), &b), Character::Whip);
        assert_eq!(classify(&m(4, 2.0, 0.05, 45.0), &b), Character::Cut);
    }

    #[test]
    fn a_cut_is_checked_before_anything_else() {
        // between two unrelated frames the estimator reports a
        // confident warp, and the global motion number is meaningless
        // rather than large. Checking motion first would call a cut a
        // whip.
        let b = Bounds::default();
        let cut_that_looks_fast = m(0, 60.0, 0.05, 50.0);
        assert_eq!(classify(&cut_that_looks_fast, &b), Character::Cut,
                   "a cut is not a whip");
        // but a genuine whip -- large motion the model DOES explain --
        // is a whip
        assert_eq!(classify(&m(1, 60.0, 0.8, 6.0), &b), Character::Whip);
    }

    #[test]
    fn a_handheld_shot_does_not_become_two_hundred_buckets() {
        // alternating frame by frame is the raw signal with extra
        // steps, not a segmentation
        let b = Bounds::default();
        let jitter: Vec<FrameMotion> = (0..60).map(|i| {
            if i % 2 == 0 { m(i, 3.0, 0.5, 11.0) } else { m(i, 3.0, 0.8, 4.0) }
        }).collect();
        let raw = bucket(&jitter, &b, 1);
        assert!(raw.len() > 40, "unmerged, it flickers: {}", raw.len());
        let merged = bucket(&jitter, &b, 6);
        assert!(merged.len() <= 3, "merged into runs: {}", merged.len());
        // and the frames are all still covered
        assert_eq!(merged.last().unwrap().to, 59);
        assert_eq!(merged[0].from, 0);
    }

    #[test]
    fn a_single_frame_cut_is_never_merged_away() {
        // a one-frame run is exactly what a cut IS, and absorbing it
        // loses the boundary the editor cares most about
        let b = Bounds::default();
        let mut seq: Vec<FrameMotion> = (0..20).map(|i| m(i, 0.2, 0.9, 2.0)).collect();
        seq[10] = m(10, 2.0, 0.02, 50.0);
        let out = bucket(&seq, &b, 8);
        assert!(out.iter().any(|x| x.character == Character::Cut && x.from == 10),
                "{out:?}");
        // the cut splits the run in two
        assert!(out.len() >= 3, "{out:?}");
    }

    #[test]
    fn rate_follows_what_the_editor_is_judging() {
        // a locked-off dialogue shot is judged on performance and
        // timing, both visible at a low rate; subject motion is the
        // expensive case AND the one being judged
        assert!(Character::Subject.rate_scale() > Character::Pan.rate_scale());
        assert!(Character::Pan.rate_scale() > Character::Locked.rate_scale());
        // a whip is already blurred by the lens; spending rate on
        // detail the camera destroyed is waste
        assert!(Character::Whip.rate_scale() < Character::Pan.rate_scale());
        assert!(Character::Locked.rate_scale() < 0.6);
    }

    #[test]
    fn the_saving_against_a_flat_rate_is_measured_not_claimed() {
        let b = Bounds::default();
        // a realistic mix: mostly locked-off dialogue, some movement
        let mut seq: Vec<FrameMotion> = Vec::new();
        for i in 0..200 { seq.push(m(i, 0.2, 0.9, 2.0)); }
        for i in 200..260 { seq.push(m(i, 8.0, 0.85, 3.0)); }
        for i in 260..300 { seq.push(m(i, 3.0, 0.4, 14.0)); }
        let buckets = bucket(&seq, &b, 6);
        let plans = plan(&buckets, 4000.0);
        let s = savings(&plans, 4000.0);
        assert!(s > 0.2, "a dialogue-heavy timeline saves a lot: {:.1}%", s * 100.0);
        assert!(s < 0.6, "and not implausibly much: {:.1}%", s * 100.0);
        let txt = summary(&plans, 4000.0);
        assert!(txt.contains("less preview data than a flat rate"), "{txt}");
        assert!(txt.contains("locked off"), "{txt}");
    }

    #[test]
    fn what_gets_rendered_first_is_what_a_cut_is_judged_on() {
        let bs = vec![
            Bucket { character: Character::Locked, from: 0, to: 100 },
            Bucket { character: Character::Cut, from: 101, to: 101 },
            Bucket { character: Character::Subject, from: 102, to: 160 },
            Bucket { character: Character::Pan, from: 161, to: 200 },
        ];
        let order = priority_order(&bs);
        assert_eq!(order[0].character, Character::Cut);
        assert_eq!(order[1].character, Character::Subject);
        assert_eq!(order.last().unwrap().character, Character::Locked,
                   "a locked-off stretch is watchable at any rate and can wait");
    }

    #[test]
    fn it_is_built_from_the_estimate_the_encoder_already_made() {
        // nothing extra is decoded; the two features share a
        // measurement rather than each having their own
        let e = crate::Estimate {
            model: crate::Model::Translation,
            warp: crate::Warp::translation(-6.0, 8.0),
            sad_before: 20.0, sad_after: 4.0, inlier_ratio: 0.9,
        };
        let fm = FrameMotion::from_estimate(7, &e);
        assert_eq!(fm.index, 7);
        assert!((fm.global_px - 10.0).abs() < 0.01, "3-4-5 triangle: {}", fm.global_px);
        assert!((fm.explained - 0.8).abs() < 0.01, "{}", fm.explained);
        assert_eq!(fm.residual, 4.0);
        assert_eq!(classify(&fm, &Bounds::default()), Character::Pan);
    }

    #[test]
    fn an_empty_timeline_is_not_a_panic() {
        assert!(bucket(&[], &Bounds::default(), 4).is_empty());
        assert!(plan(&[], 4000.0).is_empty());
        assert_eq!(savings(&[], 4000.0), 0.0);
    }
}
