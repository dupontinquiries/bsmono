//! scrollfeel — the flick and rebound tuning, ported from `Bo.qml` and
//! `BoRebound.qml`.
//!
//! Every scrollable page in the QML applies the same four values, and the source
//! states the goal: "scrolling that doesn't fight the user. Desktop browsers
//! glide a long way on a fling, respond instantly to a drag (no press delay),
//! and don't rubber-band hard at the ends."
//!
//! These are the numbers, and each has a reason attached in the original:
//!
//! * `flickDecel` **500** — low, so a fling coasts far. The default 1500 "feels
//!   sticky".
//! * `flickMaxVel` **8000** — high, so a fast swipe actually moves fast.
//! * `pressDelay` **0** — content tracks the finger immediately, with no
//!   "press, then catch up" lag.
//! * `reboundDur` **380 ms**, eased **OutCubic**.
//!
//! ## Why OutCubic and not OutBack
//!
//! `BoRebound.qml` is explicit: "the content is already returning FROM an
//! overshoot, so a back-easing would add a second wobble. A smooth decelerating
//! curve gives one clean settle." This is the detail a port gets wrong by
//! reaching for the springier-looking easing, and the result is a list that
//! bounces twice at every edge.
//!
//! ## Why drag-over-bounds but not flick-over-bounds
//!
//! You can *drag* past the edge, but a *flick* stops cleanly at the bound
//! rather than flinging into a big elastic overshoot. That is the desktop feel;
//! elastic-on-flick is the phone one, and mixing them gives a list that behaves
//! differently depending on how fast you moved.

/// Deceleration, px/s². Low so a fling coasts.
pub const FLICK_DECEL: f32 = 500.0;
/// Ceiling on fling speed, px/s.
pub const FLICK_MAX_VEL: f32 = 8000.0;
/// Rebound duration, seconds (380 ms in the QML).
pub const REBOUND_SECS: f32 = 0.380;

/// How far past the edge a *drag* may go, as a fraction of the viewport. A
/// drag is allowed over the bound; a flick is not.
pub const DRAG_OVERSHOOT: f32 = 0.18;

/// The rebound easing.
///
/// Decelerating with no overshoot: an OutBack here would undershoot past the
/// edge and come back, which is a second wobble on content already returning
/// from one.
///
/// This used to be a private copy — and it was the ONLY easing in the tree that
/// clamped its input. The shared one did not, so `uikit-term`'s block slide
/// could overshoot on a slow frame. The clamp now lives in the kit and this is
/// a re-export, so the guard applies everywhere instead of here alone.
pub use uikit_egui::motion::ease::out_cubic;


/// Where a fling ends up, given a release velocity.
///
/// Distance under constant deceleration is `v² / 2a`. The velocity is capped
/// first, because an uncapped flick on a trackpad can produce a number that
/// scrolls a list to its end from anywhere — which is not "fast", it is
/// "unusable".
pub fn fling_distance(velocity: f32) -> f32 {
    let v = velocity.abs().min(FLICK_MAX_VEL);
    (v * v) / (2.0 * FLICK_DECEL)
}

/// A flick STOPS at the bound; only a drag may pass it.
///
/// `offset` is where the content wants to go, `limit` the edge. Mixing the two
/// behaviours gives a list that acts differently depending on how fast you
/// moved, which is the thing that reads as a bug rather than a style.
pub fn clamp_flick(offset: f32, limit: f32) -> f32 { offset.clamp(0.0, limit.max(0.0)) }

/// A drag may pass the bound, by a bounded amount.
pub fn clamp_drag(offset: f32, limit: f32, viewport: f32) -> f32 {
    let slack = viewport * DRAG_OVERSHOOT;
    offset.clamp(-slack, limit.max(0.0) + slack)
}

/// The rebound: where the content is `elapsed` seconds into settling back from
/// `from` to `to`.
pub fn rebound_at(from: f32, to: f32, elapsed: f32) -> f32 {
    if REBOUND_SECS <= 0.0 { return to; }
    let t = (elapsed / REBOUND_SECS).clamp(0.0, 1.0);
    from + (to - from) * out_cubic(t)
}

/// Is the rebound finished?
pub fn rebound_done(elapsed: f32) -> bool { elapsed >= REBOUND_SECS }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_constants_are_the_qmls() {
        // A port that retunes these makes every list in the app feel different
        // from the one people know, for no stated reason.
        assert_eq!(FLICK_DECEL, 500.0);
        assert_eq!(FLICK_MAX_VEL, 8000.0);
        assert!((REBOUND_SECS - 0.380).abs() < 1e-6);
    }

    #[test]
    fn the_rebound_never_overshoots_its_target() {
        // The whole reason for OutCubic over OutBack: the content is already
        // returning FROM an overshoot, and a back-easing adds a second wobble.
        for i in 0..=100 {
            let t = i as f32 / 100.0;
            let v = out_cubic(t);
            assert!((0.0..=1.0).contains(&v), "out_cubic({t}) = {v}");
        }
        // and it settles rather than passing the edge
        for i in 0..=40 {
            let e = i as f32 * 0.01;
            let p = rebound_at(-30.0, 0.0, e);
            assert!(p <= 0.0001, "overshot to {p} at {e}s");
        }
    }

    #[test]
    fn the_rebound_decelerates_rather_than_moving_linearly() {
        // Most of the distance is covered early; a linear return reads as a
        // slide rather than a settle.
        let half = rebound_at(0.0, 100.0, REBOUND_SECS * 0.5);
        assert!(half > 60.0, "only got to {half} at halfway");
    }

    #[test]
    fn a_flick_stops_at_the_bound_and_a_drag_does_not() {
        // Mixing the two gives a list that behaves differently depending on how
        // fast you moved, which reads as a bug rather than a style.
        assert_eq!(clamp_flick(-50.0, 900.0), 0.0, "a flick does not pass the top");
        assert_eq!(clamp_flick(1200.0, 900.0), 900.0);
        let dragged = clamp_drag(-50.0, 900.0, 600.0);
        assert!(dragged < 0.0, "a drag may go past: {dragged}");
    }

    #[test]
    fn a_drag_cannot_be_pulled_arbitrarily_far() {
        // Unbounded drag-over means a list you can pull entirely off screen.
        let far = clamp_drag(-100_000.0, 900.0, 600.0);
        assert!(far >= -600.0 * DRAG_OVERSHOOT - 0.001, "pulled to {far}");
    }

    #[test]
    fn fling_velocity_is_capped_before_it_is_used() {
        // An uncapped trackpad flick produces a distance that scrolls any list
        // to its end from anywhere, which is not "fast" but "unusable".
        let capped = fling_distance(FLICK_MAX_VEL);
        assert_eq!(fling_distance(FLICK_MAX_VEL * 10.0), capped);
        // and a faster-but-legal flick still goes further
        assert!(fling_distance(4000.0) > fling_distance(1000.0));
    }

    #[test]
    fn a_low_deceleration_actually_coasts() {
        // The stated goal — "Chromium coasts a lot; default 1500 feels sticky".
        // At 2000 px/s a fling should carry several screens, not half of one.
        assert!(fling_distance(2000.0) > 3000.0, "{}", fling_distance(2000.0));
    }

    #[test]
    fn a_released_flick_at_rest_goes_nowhere() {
        assert_eq!(fling_distance(0.0), 0.0);
        assert_eq!(rebound_at(0.0, 0.0, 0.5), 0.0);
    }
}
