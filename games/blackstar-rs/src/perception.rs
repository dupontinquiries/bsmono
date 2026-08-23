//! PERCEPTION — being noticed, as a rate rather than a flag.
//!
//! Nothing in this project modelled sight. `skirmish.rs` says so in its own
//! doc: a one-dimensional resolver, "the shape of a fight without the
//! geometry of one", which is the right call for resolving a firefight and
//! useless for avoiding one. The C engine's `enemy.c` has no line-of-sight
//! either. So this is not a system being ported across from the raid side;
//! it is the one both sides turned out to be missing.
//!
//! It exists now because of a specific debt. `tradecraft::WayIn` carries a
//! hard-coded `seen_cost` — 300 for the front door, 40 for the service
//! entrance, 0 for the roof — and `best_way_in` takes a `watched` list as an
//! argument, because at the time there was nothing that could work either
//! out. Those are three magic numbers and a caller-supplied fact standing in
//! for a model. This is the model.
//!
//! ## Detection is a rate, not a die roll
//!
//! A watcher does not "spot you" with some probability per second. Attention
//! accumulates: distance, how much of you is visible, whether you are
//! moving, and how hard the watcher is actually looking all set the RATE at
//! which it builds, and you are noticed when it reaches one. That makes the
//! useful question answerable in closed form — *how many seconds can I be
//! here* — which is exactly what `approach::Opening` needs and exactly what
//! a probability per tick cannot give you.
//!
//! ## What is physical and what is a dial
//!
//! The falloff is physical, and it is the one part worth getting right: a
//! target's angular size goes as `1/distance`, and the eye's ability to
//! resolve it goes with that, so detection rate falls as the INVERSE SQUARE
//! of distance. That is why the difference between forty metres and eighty
//! is not a factor of two, and why "just stay a bit further back" is such
//! bad advice at close range and such good advice at long range.
//!
//! Everything else — the base rate, what concealment is worth, how much
//! motion costs — is a dial with a defensible starting value and no
//! measurement behind it. Marked as such, and in `--tune`.
//!
//! ## The two callers
//!
//! Both were already asking this question and answering it with constants:
//! `tradecraft` (which door, and is it watched) and the hunted-by-something
//! preset for the Jhiaxus horror, which is the same arithmetic with a
//! watcher that never stops looking.

/// Something that might notice you.
#[derive(Clone, Copy, Debug)]
pub struct Watcher {
    /// How far it can see at all, metres. Past this, nothing.
    pub range_m: f32,
    /// How hard it is looking, 0..1. A bored guard is not a searchlight.
    pub attention: f32,
    /// Does it keep looking when it has found nothing? A patrol gives up and
    /// a hunter does not, and that is the whole difference between avoiding
    /// a guard and being hunted.
    pub relentless: bool,
}

impl Watcher {
    /// A guard on a round: sees a fair way, is not really looking, gives up.
    pub fn guard() -> Watcher {
        Watcher { range_m: 60.0, attention: 0.35, relentless: false }
    }
    /// Somebody who has been told to watch this door.
    pub fn sentry() -> Watcher {
        Watcher { range_m: 80.0, attention: 0.8, relentless: false }
    }
    /// The thing in the dark. Shorter range than a floodlit sentry and it
    /// never, ever stops.
    pub fn hunter() -> Watcher {
        Watcher { range_m: 35.0, attention: 1.0, relentless: true }
    }
}

/// How hidden you are where you are standing, 0 = in the open, 1 = invisible.
#[derive(Clone, Copy, Debug)]
pub struct Cover(pub f32);

impl Cover {
    pub const OPEN: Cover = Cover(0.0);
    pub const PARTIAL: Cover = Cover(0.5);
    pub const GOOD: Cover = Cover(0.85);
    pub fn clamped(self) -> f32 {
        self.0.clamp(0.0, 1.0)
    }
}

/// Attention accrued per second at one metre, in the open, standing still,
/// by a watcher looking as hard as it can. A DIAL: it sets the whole scale.
pub const BASE_RATE: f32 = 12.0;

/// Multiplier on the rate while moving. A DIAL, and the one most likely to be
/// wrong — the eye is far better at motion than at detail, so this is large
/// on purpose.
pub const MOTION_FACTOR: f32 = 3.0;

/// Distance at which the inverse-square falloff is normalised, metres.
/// Inside this the rate is capped rather than exploding: at half a metre you
/// have been seen regardless of the arithmetic.
pub const NEAR_M: f32 = 4.0;

/// Attention gained per second. Zero means you will not be noticed here.
pub fn rate(w: &Watcher, distance_m: f32, cover: Cover, moving: bool) -> f32 {
    if distance_m > w.range_m {
        return 0.0;
    }
    // angular size goes as 1/d, and resolving it goes with that: inverse
    // square. Clamped near the observer so the model does not divide by a
    // distance nobody survives anyway.
    let d = distance_m.max(NEAR_M);
    let falloff = (NEAR_M / d).powi(2);
    let motion = if moving { MOTION_FACTOR } else { 1.0 };
    BASE_RATE * w.attention.clamp(0.0, 1.0) * falloff * motion * (1.0 - cover.clamped())
}

/// How long you can stay before you are noticed. `None` means indefinitely.
pub fn time_to_notice_s(w: &Watcher, distance_m: f32, cover: Cover, moving: bool) -> Option<f32> {
    let r = rate(w, distance_m, cover, moving);
    if r <= 0.0 {
        return None;
    }
    Some(1.0 / r)
}

/// What being exposed here for `seconds` is worth, in seconds-equivalent, for
/// `approach::Opening::seen_cost`.
///
/// The shape matters more than the scale: it is zero when you cannot be seen,
/// and it rises steeply once the exposure outlasts the time it takes to be
/// noticed. Being seen is not a cost you pay in proportion — you either got
/// away with it or you did not.
pub fn seen_cost(w: &Watcher, distance_m: f32, cover: Cover, seconds: f32) -> f32 {
    let Some(t) = time_to_notice_s(w, distance_m, cover, true) else { return 0.0 };
    if seconds <= 0.0 {
        return 0.0;
    }
    // ratio of exposure to the window you had; squared, so half as long is a
    // quarter as bad and twice as long is four times
    let ratio = seconds / t;
    100.0 * ratio * ratio
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falloff_is_inverse_square_not_linear() {
        // the one physical claim in the module: doubling the distance
        // quarters the rate, because angular size goes as 1/d
        let w = Watcher::sentry();
        let near = rate(&w, 20.0, Cover::OPEN, false);
        let far = rate(&w, 40.0, Cover::OPEN, false);
        assert!(near > 0.0 && far > 0.0);
        let ratio = near / far;
        assert!((ratio - 4.0).abs() < 0.05, "expected 4x, got {ratio}");
    }

    #[test]
    fn past_the_range_you_are_simply_not_seen() {
        let w = Watcher::guard();
        assert_eq!(rate(&w, w.range_m + 1.0, Cover::OPEN, true), 0.0);
        assert_eq!(time_to_notice_s(&w, w.range_m + 1.0, Cover::OPEN, true), None);
        assert_eq!(seen_cost(&w, w.range_m + 1.0, Cover::OPEN, 600.0), 0.0);
    }

    #[test]
    fn cover_and_stillness_both_buy_time_and_cover_buys_more() {
        let w = Watcher::sentry();
        let open_moving = time_to_notice_s(&w, 30.0, Cover::OPEN, true).unwrap();
        let open_still = time_to_notice_s(&w, 30.0, Cover::OPEN, false).unwrap();
        let hidden_moving = time_to_notice_s(&w, 30.0, Cover::GOOD, true).unwrap();
        assert!(open_still > open_moving, "standing still should help");
        assert!(hidden_moving > open_moving, "cover should help");
        // total concealment is not seen at all
        assert_eq!(time_to_notice_s(&w, 30.0, Cover(1.0), true), None);
    }

    #[test]
    fn a_bored_guard_is_not_a_sentry() {
        let d = 30.0;
        let g = time_to_notice_s(&Watcher::guard(), d, Cover::PARTIAL, true).unwrap();
        let s = time_to_notice_s(&Watcher::sentry(), d, Cover::PARTIAL, true).unwrap();
        assert!(g > s, "the guard should give you longer: {g} vs {s}");
    }

    #[test]
    fn the_hunter_is_close_range_and_never_gives_up() {
        let h = Watcher::hunter();
        assert!(h.relentless);
        assert!(!Watcher::guard().relentless && !Watcher::sentry().relentless);
        // it cannot see as far as a floodlit sentry ...
        assert!(h.range_m < Watcher::sentry().range_m);
        // ... and inside its range it is the worst thing in the module
        let d = 20.0;
        let hunted = time_to_notice_s(&h, d, Cover::PARTIAL, true).unwrap();
        let watched_by = time_to_notice_s(&Watcher::sentry(), d, Cover::PARTIAL, true).unwrap();
        assert!(hunted < watched_by, "the hunter should notice first");
        // and the answer to a hunter is distance, not cover: outside its
        // range it has nothing, which is the whole shape of that encounter
        assert_eq!(time_to_notice_s(&h, h.range_m + 1.0, Cover::OPEN, true), None);
    }

    #[test]
    fn seen_cost_is_nothing_until_it_is_everything() {
        let w = Watcher::sentry();
        let t = time_to_notice_s(&w, 30.0, Cover::OPEN, true).unwrap();
        let brief = seen_cost(&w, 30.0, Cover::OPEN, t * 0.5);
        let over = seen_cost(&w, 30.0, Cover::OPEN, t * 2.0);
        assert!(brief > 0.0);
        // squared: twice as long is four times as bad, not twice
        assert!((over / brief - 16.0).abs() < 0.1, "{over} / {brief}");
        assert_eq!(seen_cost(&w, 30.0, Cover::OPEN, 0.0), 0.0);
    }

    #[test]
    fn there_is_no_ruling_a_route_out() {
        // A `watched()` predicate lived here and is gone. It answered "is
        // this door unusable", which double-counted with `seen_cost` — the
        // same fact charged twice — and ruled out the front door BY
        // DEFINITION, since being seen is what the front door is. Being
        // noticed is a cost; whether it is worth paying belongs to
        // `approach::choose`, which already knows how to decide.
        //
        // Left as a test so the predicate does not come back.
        let w = Watcher::sentry();
        assert!(seen_cost(&w, 8.0, Cover::OPEN, 20.0) > 0.0, "a bad route still costs");
        assert!(seen_cost(&w, 500.0, Cover::OPEN, 20.0) == 0.0, "an unseen one does not");
    }
}
