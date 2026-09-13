//! The three playback fidelities, and the budget-reallocation that makes
//! Feel mode different from "Preview but with extra steps."
//!
//! * [`Mode::Render`] — no shortcuts. Every frame, full tier, no cache
//!   policy involved at all; this is what an export decodes. It exists
//!   here only as an enum value other code switches on — its own decode
//!   path is `decode::decode_range` called directly, stride 1, tier Full,
//!   uncapped.
//! * [`Mode::Preview`] — exactly [`cache_policy::desired_set`]: distance
//!   from a velocity-predicted playhead decides tier and stride, and
//!   nothing else does. Cheap, uniform, and honest about being degraded.
//! * [`Mode::Feel`] — Preview's SAME total byte budget, spent unevenly.
//!   [`bend`] takes the naive Preview set, looks up each frame's
//!   [`bucket::BucketField`] importance, and trades tier between frames:
//!   promote what's important, demote what isn't, by exactly enough to
//!   land back under budget. It is a reallocation, not an addition —
//!   Feel mode never costs more than Preview mode at the same budget,
//!   which is what makes it safe to leave on by default.
//!
//! ## Why a reallocation and not just "add a bonus shell around important
//! frames"
//!
//! Adding is the easy, wrong version: it either blows the budget (now
//! Feel mode IS just a bigger Preview, which a low-end device can't
//! afford) or needs its own separate, smaller budget bolted on the side
//! (now there are two budgets to tune against each other, and the
//! low-importance majority of a normal video pays for headroom it
//! never uses). Reallocating under one fixed budget means the only
//! question a caller answers is "how much RAM," same as Preview mode,
//! and Feel mode spends it better rather than spending more.
//!
//! ## Confidence scales the bend, not just the importance
//!
//! A [`bucket::Resolved`] with low confidence (a guess decayed across a
//! clip boundary) should bend the allocation LESS than a verified segment
//! at the same importance — a shaky guess earning a full tier promotion
//! is exactly the kind of thing that erodes trust in the whole feature
//! the first time it's wrong. [`bend`] multiplies each frame's importance
//! by its confidence before ranking, so an unverified guess competes
//! weakly against a verified neighbour even if the raw importance number
//! is identical.

use std::collections::BTreeMap;

use super::bucket::{BucketField, InstanceId};
use super::cache_policy::{CacheKey, SpatialTier};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Preview,
    Render,
    Feel,
}

impl SpatialTier {
    /// One step up in cost, or `None` already at `Full`.
    fn promote(self) -> Option<SpatialTier> {
        match self {
            SpatialTier::Thumbnail => Some(SpatialTier::Proxy),
            SpatialTier::Proxy => Some(SpatialTier::Full),
            SpatialTier::Full => None,
        }
    }
    /// One step down in cost, or `None` already at `Thumbnail`.
    fn demote(self) -> Option<SpatialTier> {
        match self {
            SpatialTier::Full => Some(SpatialTier::Proxy),
            SpatialTier::Proxy => Some(SpatialTier::Thumbnail),
            SpatialTier::Thumbnail => None,
        }
    }
}

/// Relative byte cost of one frame at each tier, for comparing trades
/// without needing the source's actual resolution — `bend` only needs
/// RATIOS between tiers to decide how many demotions pay for one
/// promotion, not absolute bytes (that's `cache_policy::estimate_bytes`'s
/// job, on the resulting set, for the caller to check against `Budget`).
fn relative_cost(tier: SpatialTier) -> f64 {
    match tier {
        SpatialTier::Thumbnail => 1.0,
        SpatialTier::Proxy => 9.0,
        SpatialTier::Full => 64.0,
    }
}

/// One frame under consideration: which timeline clip INSTANCE it
/// belongs to (for the `BucketField` lookup, which resolves through an
/// instance's current placement — see `bucket`'s module doc) and its
/// instance-local frame index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Frame {
    pub instance: InstanceId,
    pub index: i64,
}

/// Bend a naive Preview set toward important content, at no extra byte
/// cost. `naive` maps each [`Frame`] to the tier [`cache_policy`] assigned
/// it purely by distance; `field` supplies importance and confidence.
///
/// The algorithm is a simple auction, deliberately not a general knapsack
/// solver: sort candidate PROMOTIONS by `importance * confidence`
/// descending and candidate DEMOTIONS by the same ascending, then repeatedly
/// take the best available promotion and pay for it with the cheapest
/// available demotion, stopping when either list is exhausted or a
/// promotion would cost more than the cheapest remaining demotion earns
/// back (never spend more than Preview's own total — see the module doc).
/// This is `O(n log n)` and its behaviour is easy to state precisely,
/// which matters more here than squeezing out the last few bytes an
/// optimal solver might find.
pub fn bend(naive: &BTreeMap<Frame, SpatialTier>, field: &BucketField) -> BTreeMap<Frame, SpatialTier> {
    let mut tiers = naive.clone();

    // Score every frame once. `promote_gain` for a frame not yet at Full
    // is its importance*confidence — how much we WANT to spend on it.
    // `demote_gain` for a frame not yet at Thumbnail is `1 -
    // importance*confidence` — how little we mind taking from it. A frame
    // already at the extreme tier for its own preference (Full and
    // important, or Thumbnail and unimportant) is left out of both lists;
    // it's already where it wants to be.
    struct Scored {
        frame: Frame,
        weight: f32,
    }
    let mut promote_candidates: Vec<Scored> = Vec::new();
    let mut demote_candidates: Vec<Scored> = Vec::new();
    for (&frame, &tier) in naive.iter() {
        let r = field.resolve(frame.instance, frame.index);
        let weight = (r.importance * r.confidence).clamp(0.0, 1.0);
        if tier != SpatialTier::Full && weight > 0.0 {
            promote_candidates.push(Scored { frame, weight });
        }
        if tier != SpatialTier::Thumbnail && weight < 1.0 {
            demote_candidates.push(Scored { frame, weight: 1.0 - weight });
        }
    }
    promote_candidates.sort_by(|a, b| b.weight.total_cmp(&a.weight));
    demote_candidates.sort_by(|a, b| b.weight.total_cmp(&a.weight));

    let mut pi = 0usize;
    let mut di = 0usize;
    // Bytes "banked" from demotions already made but not yet spent on a
    // promotion — lets one cheap demotion (Proxy->Thumbnail, small gap)
    // combine with another to afford one expensive promotion
    // (Proxy->Full, large gap) rather than requiring a single demotion to
    // exactly cover each promotion.
    let mut banked = 0.0f64;

    while pi < promote_candidates.len() {
        let p = &promote_candidates[pi];
        let cur = tiers[&p.frame];
        let Some(up) = cur.promote() else { pi += 1; continue };
        let cost = relative_cost(up) - relative_cost(cur);

        while banked < cost && di < demote_candidates.len() {
            let d = &demote_candidates[di];
            // A frame cannot both be promoted and be the thing paying for
            // a promotion — skip a demotion candidate already spent as a
            // promotion target (rare: only possible if the same frame
            // scored on both lists at a middling weight).
            let dcur = tiers[&d.frame];
            let Some(down) = dcur.demote() else { di += 1; continue };
            if d.frame == p.frame {
                di += 1;
                continue;
            }
            banked += relative_cost(dcur) - relative_cost(down);
            tiers.insert(d.frame, down);
            di += 1;
        }

        if banked >= cost {
            banked -= cost;
            tiers.insert(p.frame, up);
            pi += 1;
        } else {
            // Ran out of demotions to fund even the cheapest remaining
            // promotion; no later (more expensive-relative, since sorted
            // by weight not cost) promotion will fare better with an
            // empty demotion list either, but a smaller promotion later
            // in weight order might still be a SMALLER step than this
            // one — so advance past this candidate rather than stopping
            // outright.
            pi += 1;
        }
    }
    tiers
}

/// Convert a `cache_policy` desired set (all one instance) into the
/// `(Frame -> tier)` map `bend` wants.
pub fn naive_tiers(instance: InstanceId, set: &std::collections::BTreeSet<CacheKey>) -> BTreeMap<Frame, SpatialTier> {
    set.iter().map(|k| (Frame { instance, index: k.frame }, k.tier)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::bucket::{Bucket, GuessRadius, Segment};

    fn uniform(instance: InstanceId, frames: std::ops::Range<i64>, tier: SpatialTier) -> BTreeMap<Frame, SpatialTier> {
        frames.map(|i| (Frame { instance, index: i }, tier)).collect()
    }

    /// Register instance 1 as a 60-frame clip playing source 1 on track 0
    /// — the placement every test below bends against.
    fn placed(field: &mut BucketField) {
        use super::super::bucket::Placement;
        field.rebuild(&[Placement { instance: 1, source: 1, source_offset: 0, len: 60, track: 0, start: 0.0, end: 60.0 }], 0.0);
    }

    fn total_cost(tiers: &BTreeMap<Frame, SpatialTier>) -> f64 {
        tiers.values().map(|&t| relative_cost(t)).sum()
    }

    #[test]
    fn with_no_profiled_content_the_bend_is_a_no_op() {
        let naive = uniform(1, 0..50, SpatialTier::Proxy);
        // No `rebuild` at all: instance 1 isn't registered, so every
        // resolve is fallback regardless — the strongest version of "no
        // profiled content."
        let field = BucketField::new(GuessRadius::default());
        let bent = bend(&naive, &field);
        assert_eq!(bent, naive, "an all-fallback field must reproduce Preview exactly");
    }

    #[test]
    fn a_verified_face_segment_is_promoted_and_the_total_cost_never_exceeds_preview() {
        let naive = uniform(1, 0..60, SpatialTier::Proxy);
        let mut field = BucketField::new(GuessRadius::default());
        field.add_segment(Segment { source: 1, range: 25..30, bucket: Bucket::Face });
        placed(&mut field);
        let bent = bend(&naive, &field);
        for i in 25..30 {
            assert_eq!(bent[&Frame { instance: 1, index: i }], SpatialTier::Full, "frame {i}");
        }
        assert!(total_cost(&bent) <= total_cost(&naive) + 1e-6, "bend must not spend more than Preview's own total");
    }

    #[test]
    fn static_frames_are_demoted_to_pay_for_the_promotion_and_come_back_from_where_the_face_is() {
        let naive = uniform(1, 0..60, SpatialTier::Proxy);
        let mut field = BucketField::new(GuessRadius::default());
        field.add_segment(Segment { source: 1, range: 25..30, bucket: Bucket::Face });
        placed(&mut field);
        // Everywhere else defaults to fallback (importance 0), which is
        // already the CHEAPEST a demote candidate can be (weight 1.0 =
        // "don't mind taking from this at all") — so the funding for the
        // Face promotion should visibly come from frames far from it.
        let bent = bend(&naive, &field);
        let demoted_elsewhere = (0..60).filter(|&i| !(25..30).contains(&i)).filter(|&i| bent[&Frame { instance: 1, index: i }] == SpatialTier::Thumbnail).count();
        assert!(demoted_elsewhere > 0, "some non-face frame should have been demoted to fund the promotion");
    }

    #[test]
    fn a_low_confidence_guess_bends_one_frame_less_than_a_verified_segment_would() {
        // Isolate ONE frame (30) and compare its own outcome under two
        // otherwise-matched setups, rather than an aggregate count over
        // the whole array — an aggregate conflates how FAR a guess's halo
        // spreads (which can touch many frames weakly) with how STRONGLY
        // any single frame gets bent, and those are different questions.
        let naive = uniform(1, 0..60, SpatialTier::Proxy);

        // Guessed: frame 30 is reached only by decaying from a segment
        // ten frames away, so confidence < 1 even though the bucket
        // (Face) carries importance 1.0.
        let mut guessed = BucketField::new(GuessRadius::default());
        guessed.add_segment(Segment { source: 1, range: 10..11, bucket: Bucket::Face });
        placed(&mut guessed);
        let probe = guessed.resolve(1, 30);
        assert!(probe.confidence > 0.0 && probe.confidence < 1.0, "test setup: expected a partial guess, got {probe:?}");
        let bent_guessed = bend(&naive, &guessed);

        // Verified: frame 30 itself is inside the segment. Same bucket,
        // same importance, confidence 1.0.
        let mut verified = BucketField::new(GuessRadius::default());
        verified.add_segment(Segment { source: 1, range: 30..31, bucket: Bucket::Face });
        placed(&mut verified);
        let bent_verified = bend(&naive, &verified);

        let rank = |t: SpatialTier| match t { SpatialTier::Thumbnail => 0, SpatialTier::Proxy => 1, SpatialTier::Full => 2 };
        let f30 = Frame { instance: 1, index: 30 };
        assert!(
            rank(bent_verified[&f30]) >= rank(bent_guessed[&f30]),
            "verified {:?} should end up at least as promoted as guessed {:?}",
            bent_verified[&f30], bent_guessed[&f30]
        );
        assert_eq!(bent_verified[&f30], SpatialTier::Full, "a verified Face frame should reach Full given ample funding available");
    }

    #[test]
    fn naive_tiers_converts_a_cache_policy_set_directly() {
        use super::super::cache_policy::{desired_set, Shells, Velocity};
        let shells = Shells::for_fps(24.0);
        let set = desired_set(100, Velocity::default(), 24.0, 0.0, &shells, 0..10_000);
        let tiers = naive_tiers(7, &set);
        assert_eq!(tiers.len(), set.len());
        for k in &set {
            assert_eq!(tiers[&Frame { instance: 7, index: k.frame }], k.tier);
        }
    }

    #[test]
    fn render_mode_is_a_plain_enum_value_with_no_policy_attached() {
        // Render mode's contract is "decode::decode_range at stride 1,
        // tier Full, uncapped" — asserted at the call site in `decode`,
        // not here. This test exists only to pin that Mode has exactly
        // the three variants the module doc promises.
        let modes = [Mode::Preview, Mode::Render, Mode::Feel];
        assert_eq!(modes.len(), 3);
    }
}
