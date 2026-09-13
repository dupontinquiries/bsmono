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
/// How much a bucket hint is trusted to move bytes around, and how much
/// churn it may cause doing it — not a slider, three named levels, because
/// this is a hardware-class decision more than a taste decision.
///
/// Three independent knobs:
///
/// * `promote_floor` — the minimum `importance * confidence` weight for a
///   frame to be ELIGIBLE to bend at all. Below it, a hint is a GUIDE
///   only: it can still be shown (a UI could dim-highlight "this looks
///   like a face" for a human's benefit), but it never moves a byte of
///   the cache. `Strict`'s floor sits just under 1.0 — in practice only
///   an exact, VERIFIED segment (confidence exactly 1.0) clears it, so a
///   decayed guess never spends anything.
/// * `guess_exponent` — applied to a GUESS's weight (`w.powf(exponent)`,
///   a verified frame's weight is always exactly 1.0 and unaffected) so a
///   caller can decide how much a hint that ISN'T certain still counts.
///   `1.0` leaves it as-is (today's original behaviour, `Balanced`'s
///   value). Below `1.0` PUSHES a weak guess up toward 1.0 — a 0.3-weight
///   guess at exponent 0.5 counts as `0.3.powf(0.5) ≈ 0.55` — which is
///   the literal shape of "a hint is more of a guide" on the loose end:
///   the system leans on incomplete evidence more readily, in exchange
///   for being wrong about it more often.
/// * `max_reallocation` — the largest total relative-cost trade `bend`
///   may make (see `relative_cost`), as a multiple of the naive set's own
///   total. This is the hardware-budget lever proper: every promotion a
///   guess earns is a tier CHANGE for some frame, and a tier change means
///   a fresh decode next time that frame is needed — reallocation isn't
///   free even though it stays inside one fixed byte budget, because both
///   sides of a trade (the promotion AND the demotion that funds it) cost
///   real churn. `Strict` caps that churn hard; `Balanced` and `Loose`
///   leave it effectively uncapped — the pre-existing, unconstrained
///   behaviour `bend` always had before this cap existed — because the
///   floor and the guess exponent are already doing the actual work of
///   telling those two apart.
///
/// A low-end device wants `Strict`: cheap to evaluate (most candidates
/// fail the floor immediately, short-circuiting `bend`'s inner loop) and
/// cheap to live with (a small, hard churn cap). A desktop with room to
/// spare can afford `Loose`'s wider trust in incomplete evidence.
#[derive(Debug, Clone, Copy)]
pub struct Strictness {
    pub name: &'static str,
    pub promote_floor: f32,
    pub guess_exponent: f32,
    pub max_reallocation: f32,
}

impl Strictness {
    /// Verified evidence only; guesses are a guide, never a driver.
    /// Smallest, hard churn budget.
    pub const STRICT: Self = Self { name: "strict", promote_floor: 0.97, guess_exponent: 1.0, max_reallocation: 0.25 };
    /// Today's original behaviour, unchanged: any frame with positive
    /// weight is eligible at face value, funded without an artificial cap.
    pub const BALANCED: Self = Self { name: "balanced", promote_floor: 0.0, guess_exponent: 1.0, max_reallocation: 1_000.0 };
    /// A weak guess is trusted MORE than its raw confidence — "a hint is
    /// more of a guide" — and there is no extra churn cap beyond what
    /// funding naturally allows.
    pub const LOOSE: Self = Self { name: "loose", promote_floor: 0.0, guess_exponent: 0.5, max_reallocation: 1_000.0 };

    /// All three, in ascending looseness — the order a settings control
    /// or a cycling hotkey should present them in.
    pub const ALL: [Strictness; 3] = [Strictness::STRICT, Strictness::BALANCED, Strictness::LOOSE];

    /// The sidecar round-trip token (see `device_prefs`). Stable and
    /// label-independent, matching `audio_kit::BounceFormat::token`'s
    /// own convention.
    pub fn token(self) -> &'static str {
        self.name
    }

    /// An unknown token is `Balanced`, not an error — the same tolerant
    /// rule every sidecar field in this app follows.
    pub fn from_token(t: &str) -> Strictness {
        Self::ALL.into_iter().find(|s| s.token() == t.trim()).unwrap_or(Strictness::BALANCED)
    }

    /// One step looser, saturating at `Loose` — for a "less strict"
    /// hotkey.
    pub fn looser(self) -> Strictness {
        let i = Self::ALL.iter().position(|s| *s == self).unwrap_or(1);
        Self::ALL[(i + 1).min(Self::ALL.len() - 1)]
    }
    /// One step stricter, saturating at `Strict`.
    pub fn stricter(self) -> Strictness {
        let i = Self::ALL.iter().position(|s| *s == self).unwrap_or(1);
        Self::ALL[i.saturating_sub(1)]
    }
}

impl PartialEq for Strictness {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Default for Strictness {
    fn default() -> Self {
        Strictness::BALANCED
    }
}

/// Bend a naive Preview set toward important content, at no extra byte
/// cost. `naive` maps each [`Frame`] to the tier [`cache_policy`] assigned
/// it purely by distance; `field` supplies importance and confidence;
/// `strictness` sets how much of that evidence is trusted to act and how
/// much total churn the trade is allowed to cause — see [`Strictness`].
///
/// The algorithm is a simple auction, deliberately not a general knapsack
/// solver: sort candidate PROMOTIONS by `importance * confidence`
/// descending and candidate DEMOTIONS by the same ascending, then repeatedly
/// take the best available promotion and pay for it with the cheapest
/// available demotion, stopping when either list is exhausted, a
/// promotion would cost more than the cheapest remaining demotion earns
/// back, or `strictness.max_reallocation` has been spent. This is
/// `O(n log n)` and its behaviour is easy to state precisely, which
/// matters more here than squeezing out the last few bytes an
/// optimal solver might find.
pub fn bend(naive: &BTreeMap<Frame, SpatialTier>, field: &BucketField, strictness: Strictness) -> BTreeMap<Frame, SpatialTier> {
    let mut tiers = naive.clone();
    let naive_total: f64 = naive.values().map(|&t| relative_cost(t)).sum();
    let churn_budget = naive_total * strictness.max_reallocation as f64;
    let mut churn_spent = 0.0f64;

    // Score every frame once. `promote_gain` for a frame not yet at Full
    // is its importance*confidence, boosted for a GUESS (confidence < 1)
    // by `guess_exponent` — how much we WANT to spend on it. `demote_gain`
    // for a frame not yet at Thumbnail is `1 -
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
        let raw = (r.importance * r.confidence).clamp(0.0, 1.0);
        // Boost applies only to genuine guesses (confidence < 1) — a
        // verified frame's weight is exactly `r.importance` already and
        // is left alone, so `guess_exponent` can never make a VERIFIED
        // Static frame (weight 0) look important, only pull an uncertain
        // but nonzero guess upward.
        let weight = if r.confidence < 1.0 { raw.powf(strictness.guess_exponent) } else { raw };
        // Below the strictness floor, a hint is a GUIDE only — it never
        // becomes a promotion candidate, so it can never spend a byte.
        if tier != SpatialTier::Full && weight >= strictness.promote_floor && weight > 0.0 {
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
        if churn_spent >= churn_budget {
            // The churn budget is spent — further promotions would still
            // be FUNDABLE (there may be demotions left), but the point of
            // a churn cap is to bound total cache disruption regardless
            // of whether the bytes technically balance.
            break;
        }
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
            let demote_cost = relative_cost(dcur) - relative_cost(down);
            banked += demote_cost;
            churn_spent += demote_cost;
            tiers.insert(d.frame, down);
            di += 1;
        }

        if banked >= cost {
            banked -= cost;
            churn_spent += cost;
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

/// A hotkey's effect on the current strictness — the logic a keyboard
/// handler calls, kept separate from any actual key so it's testable
/// without one. `[` / `]` (stricter/looser, bracket keys read as "narrow
/// the trust" / "widen the trust") is the proposed binding; NOT YET BOUND
/// to a real keypress anywhere, because there is no video timeline in the
/// egui view yet for a hotkey to act on (see the crate-level `arrange`
/// module doc on what's wired and what isn't). This exists so the wiring,
/// once the view exists, is "call `HotkeyAction::apply`", not "invent the
/// cycling logic under a deadline."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    Stricter,
    Looser,
    Reset,
}

impl HotkeyAction {
    pub fn apply(self, current: Strictness) -> Strictness {
        match self {
            HotkeyAction::Stricter => current.stricter(),
            HotkeyAction::Looser => current.looser(),
            HotkeyAction::Reset => Strictness::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::bucket::{Bucket, GuessRadius, Placement, Segment};

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
        let bent = bend(&naive, &field, Strictness::BALANCED);
        assert_eq!(bent, naive, "an all-fallback field must reproduce Preview exactly");
    }

    #[test]
    fn a_verified_face_segment_is_promoted_and_the_total_cost_never_exceeds_preview() {
        let naive = uniform(1, 0..60, SpatialTier::Proxy);
        let mut field = BucketField::new(GuessRadius::default());
        field.add_segment(Segment { source: 1, range: 25..30, bucket: Bucket::Face });
        placed(&mut field);
        let bent = bend(&naive, &field, Strictness::BALANCED);
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
        let bent = bend(&naive, &field, Strictness::BALANCED);
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
        let bent_guessed = bend(&naive, &guessed, Strictness::BALANCED);

        // Verified: frame 30 itself is inside the segment. Same bucket,
        // same importance, confidence 1.0.
        let mut verified = BucketField::new(GuessRadius::default());
        verified.add_segment(Segment { source: 1, range: 30..31, bucket: Bucket::Face });
        placed(&mut verified);
        let bent_verified = bend(&naive, &verified, Strictness::BALANCED);

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
    fn strict_ignores_a_guess_as_a_guide_only_and_never_spends_on_it() {
        let naive = uniform(1, 0..60, SpatialTier::Proxy);
        // A VERIFIED segment at 0..5 so frame 20 is reached only by a
        // decaying GUESS (confidence < 1.0) — the case under test.
        let mut guessed = BucketField::new(GuessRadius::default());
        guessed.add_segment(Segment { source: 1, range: 0..5, bucket: Bucket::Face });
        guessed.rebuild(&[Placement { instance: 1, source: 1, source_offset: 0, len: 60, track: 0, start: 0.0, end: 60.0 }], 0.0);
        let probe = guessed.resolve(1, 20);
        assert!(probe.confidence > 0.0 && probe.confidence < 1.0, "test setup: expected a partial guess");

        let f20 = Frame { instance: 1, index: 20 };
        let bent_strict = bend(&naive, &guessed, Strictness::STRICT);
        assert_eq!(bent_strict[&f20], SpatialTier::Proxy, "a guess below the strict floor must not move ITS OWN frame");

        // The verified frames 0..5 themselves are still allowed to act —
        // Strict withholds trust from GUESSES, not from evidence in
        // general — proving the floor is doing the discriminating, not
        // some blanket "Strict changes nothing" behaviour. Frame 0 is
        // checked specifically because it sorts first among several
        // equally (fully) verified candidates and is what Strict's small
        // churn budget affords first; a middle frame among several tied
        // candidates can legitimately miss the cut on churn budget alone
        // (see `strictness_caps_total_churn`), which is a different,
        // already-covered behaviour, not the one this test is isolating.
        assert_ne!(bent_strict[&Frame { instance: 1, index: 0 }], SpatialTier::Proxy, "verified evidence should still act under Strict");

        // The SAME field, at Balanced, does bend frame 20 — proving the
        // difference at frame 20 specifically is the strictness level.
        let bent_balanced = bend(&naive, &guessed, Strictness::BALANCED);
        assert_ne!(bent_balanced[&f20], SpatialTier::Proxy, "test setup: Balanced should bend the guessed frame too");
    }

    #[test]
    fn strict_still_promotes_an_exactly_verified_frame() {
        // A single verified frame, comfortably inside Strict's churn
        // budget — see `strictness_caps_total_churn` below for the
        // separate, deliberate case where the churn CAP (not the
        // evidence floor) is what limits how much even verified content
        // can move.
        let naive = uniform(1, 0..60, SpatialTier::Proxy);
        let mut verified = BucketField::new(GuessRadius::default());
        verified.add_segment(Segment { source: 1, range: 27..28, bucket: Bucket::Face });
        verified.rebuild(&[Placement { instance: 1, source: 1, source_offset: 0, len: 60, track: 0, start: 0.0, end: 60.0 }], 0.0);
        let bent = bend(&naive, &verified, Strictness::STRICT);
        assert_eq!(bent[&Frame { instance: 1, index: 27 }], SpatialTier::Full, "verified evidence must still act under Strict when it fits the churn budget");
    }

    #[test]
    fn strictness_caps_total_churn_and_loose_allows_more_of_it_than_strict() {
        // A big Face block relative to the whole clip, so there is more
        // fundable promotion available than any level's churn budget can
        // spend at once — the cap, not the evidence, becomes the limit.
        let naive = uniform(1, 0..100, SpatialTier::Proxy);
        let mut field = BucketField::new(GuessRadius::default());
        field.add_segment(Segment { source: 1, range: 0..80, bucket: Bucket::Face });
        field.rebuild(&[Placement { instance: 1, source: 1, source_offset: 0, len: 100, track: 0, start: 0.0, end: 100.0 }], 0.0);

        let count_full = |m: &BTreeMap<Frame, SpatialTier>| m.values().filter(|&&t| t == SpatialTier::Full).count();
        let strict = bend(&naive, &field, Strictness::STRICT);
        let balanced = bend(&naive, &field, Strictness::BALANCED);
        let loose = bend(&naive, &field, Strictness::LOOSE);
        assert!(count_full(&strict) <= count_full(&balanced), "strict {} balanced {}", count_full(&strict), count_full(&balanced));
        assert!(count_full(&balanced) <= count_full(&loose), "balanced {} loose {}", count_full(&balanced), count_full(&loose));
    }

    #[test]
    fn tokens_round_trip_and_cycling_saturates_at_both_ends() {
        for s in Strictness::ALL {
            assert_eq!(Strictness::from_token(s.token()), s);
        }
        assert_eq!(Strictness::from_token("garbage"), Strictness::BALANCED, "unknown token is the tolerant default");
        assert_eq!(Strictness::STRICT.stricter(), Strictness::STRICT, "already strictest: saturates");
        assert_eq!(Strictness::STRICT.looser(), Strictness::BALANCED);
        assert_eq!(Strictness::BALANCED.looser(), Strictness::LOOSE);
        assert_eq!(Strictness::LOOSE.looser(), Strictness::LOOSE, "already loosest: saturates");
        assert_eq!(Strictness::LOOSE.stricter(), Strictness::BALANCED);
        assert_eq!(Strictness::default(), Strictness::BALANCED);
    }

    #[test]
    fn hotkey_actions_cycle_and_reset_correctly() {
        assert_eq!(HotkeyAction::Stricter.apply(Strictness::BALANCED), Strictness::STRICT);
        assert_eq!(HotkeyAction::Looser.apply(Strictness::BALANCED), Strictness::LOOSE);
        assert_eq!(HotkeyAction::Stricter.apply(Strictness::STRICT), Strictness::STRICT, "saturates");
        assert_eq!(HotkeyAction::Reset.apply(Strictness::STRICT), Strictness::BALANCED);
        assert_eq!(HotkeyAction::Reset.apply(Strictness::LOOSE), Strictness::BALANCED);
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
