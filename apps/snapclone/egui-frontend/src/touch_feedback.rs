//! touch_feedback — `TouchFeedback.cpp`, ported.
//!
//! A ripple from the point of contact, and a brief press response on the
//! control itself. On a touch screen these are not decoration: without them a
//! tap that takes 200ms to do anything is indistinguishable from a tap that
//! missed, and people tap again.
//!
//! ## One workaround does not port, and that is the interesting part
//!
//! The C++ press response fakes a scale by dipping **opacity** 1.0 → 0.7 → 1.0,
//! with a comment saying why: *"A true scale would require QGraphicsView; this
//! is the widget-native approach."* That is a Qt constraint, not a design
//! choice — egui draws to a transform every frame and can scale directly.
//!
//! So this ports the **intent** (a brief, symmetric acknowledgement peaking
//! around a third of the way through) as a real scale, and keeps the timing the
//! C++ tuned. Porting the opacity dip instead would carry a workaround for a
//! problem this toolkit does not have.
//!
//! ## The ripple's two curves are different on purpose
//!
//! Expansion is `OutCubic` — fast then settling, so it appears to leave the
//! finger. The fade is `InCubic` and **holds at 0.9 until 60% through**, so the
//! ripple stays visible while it grows and disappears only at the end. Fading
//! linearly from the start makes it vanish before it has travelled, which reads
//! as a flicker rather than a response.

/// Ripple lifetime.
pub const RIPPLE_MS: f32 = 380.0;
/// Press-response lifetime.
pub const PRESS_MS: f32 = 220.0;
/// How far into the ripple the fade begins in earnest.
pub const FADE_HOLD: f32 = 0.6;
/// Opacity still remaining at that point.
pub const FADE_HOLD_ALPHA: f32 = 0.9;
/// Deepest point of the press response, and when it happens.
pub const PRESS_DEPTH: f32 = 0.72;
pub const PRESS_PEAK_AT: f32 = 0.35;
/// Padding beyond the corner so the ripple clears the widget entirely.
pub const RIPPLE_OVERSHOOT: f32 = 20.0;

fn out_cubic(t: f32) -> f32 { let t = t.clamp(0.0, 1.0); 1.0 - (1.0 - t).powi(3) }
fn in_cubic(t: f32) -> f32 { let t = t.clamp(0.0, 1.0); t * t * t }

/// An expanding circle from the point of contact.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ripple {
    pub x: f32,
    pub y: f32,
    /// Milliseconds since it started.
    age_ms: f32,
    /// Widget size at spawn, which fixes the target radius.
    max_radius: f32,
}

impl Ripple {
    /// Start a ripple at a point inside a widget of `w` × `h`.
    ///
    /// The target radius is the diagonal plus a margin, so the circle clears
    /// even the far corner. Computed from the widget rather than a constant,
    /// because a fixed radius is a full-screen wash on a phone and a dot on a
    /// desktop.
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Ripple {
        Ripple { x, y, age_ms: 0.0, max_radius: (w * w + h * h).sqrt() + RIPPLE_OVERSHOOT }
    }

    pub fn progress(&self) -> f32 { (self.age_ms / RIPPLE_MS).clamp(0.0, 1.0) }
    pub fn is_done(&self) -> bool { self.age_ms >= RIPPLE_MS }

    pub fn radius(&self) -> f32 { out_cubic(self.progress()) * self.max_radius }

    /// Opacity, holding then falling.
    pub fn alpha(&self) -> f32 {
        let t = self.progress();
        if t <= FADE_HOLD {
            // Barely moves: the ripple should be visible for the whole
            // expansion, not fading while it travels.
            1.0 - (1.0 - FADE_HOLD_ALPHA) * (t / FADE_HOLD)
        } else {
            let rest = (t - FADE_HOLD) / (1.0 - FADE_HOLD);
            FADE_HOLD_ALPHA * (1.0 - in_cubic(rest))
        }
    }

    /// Advance. Returns true once finished, so a caller can drop it.
    pub fn tick(&mut self, dt_ms: f32) -> bool {
        if dt_ms > 0.0 {
            // A resumed suspend must not leave a ripple frozen mid-expansion;
            // clamping to its own lifetime completes it instead.
            self.age_ms += dt_ms.min(RIPPLE_MS);
        }
        self.is_done()
    }
}

/// The brief response on the control itself.
///
/// A real scale rather than the C++ opacity dip — see the module docs.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct PressResponse {
    age_ms: f32,
    active: bool,
}

impl PressResponse {
    pub fn start() -> PressResponse { PressResponse { age_ms: 0.0, active: true } }
    pub fn is_active(&self) -> bool { self.active }

    /// Scale factor, 1 → PRESS_DEPTH → 1.
    ///
    /// Asymmetric in time: down quickly, back more slowly, which is what makes
    /// it feel like a button rather than a blink. Symmetric timing reads as a
    /// glitch.
    pub fn scale(&self) -> f32 {
        if !self.active { return 1.0; }
        let t = (self.age_ms / PRESS_MS).clamp(0.0, 1.0);
        if t <= PRESS_PEAK_AT {
            let k = out_cubic(t / PRESS_PEAK_AT);
            1.0 - (1.0 - PRESS_DEPTH) * k
        } else {
            let k = out_cubic((t - PRESS_PEAK_AT) / (1.0 - PRESS_PEAK_AT));
            PRESS_DEPTH + (1.0 - PRESS_DEPTH) * k
        }
    }

    pub fn tick(&mut self, dt_ms: f32) -> bool {
        if !self.active || dt_ms <= 0.0 { return !self.active; }
        self.age_ms += dt_ms.min(PRESS_MS);
        if self.age_ms >= PRESS_MS { self.active = false; }
        !self.active
    }
}

/// Live feedback for one surface.
#[derive(Clone, Debug, Default)]
pub struct TouchFeedback {
    ripples: Vec<Ripple>,
    press: PressResponse,
    enabled: bool,
}

/// More ripples than this at once and the screen is noise rather than feedback.
///
/// Reached by a stuck finger or a synthetic event storm, not by a person — but
/// unbounded growth is a leak, and the oldest is the one already fading.
pub const MAX_RIPPLES: usize = 8;

impl TouchFeedback {
    /// Enabled by default only on touch: a ripple under a mouse cursor is the
    /// tell of a phone UI wearing a desktop's clothes.
    pub fn new(touch: bool) -> TouchFeedback {
        TouchFeedback { ripples: Vec::new(), press: PressResponse::default(), enabled: touch }
    }

    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        if !on {
            self.ripples.clear();
            // The press response too, and this is the half I forgot: clearing
            // only the ripples leaves a control STUCK MID-SCALE at whatever
            // depth it had reached when the setting changed, with nothing left
            // running to restore it. A test caught it.
            self.press = PressResponse::default();
        }
    }
    pub fn is_enabled(&self) -> bool { self.enabled }

    pub fn tap(&mut self, x: f32, y: f32, w: f32, h: f32) {
        if !self.enabled { return; }
        if self.ripples.len() >= MAX_RIPPLES { self.ripples.remove(0); }
        self.ripples.push(Ripple::new(x, y, w, h));
        self.press = PressResponse::start();
    }

    pub fn ripples(&self) -> &[Ripple] { &self.ripples }
    pub fn scale(&self) -> f32 { self.press.scale() }

    /// Advance everything and drop what finished.
    pub fn tick(&mut self, dt_ms: f32) {
        self.ripples.retain_mut(|r| !r.tick(dt_ms));
        self.press.tick(dt_ms);
    }

    /// Is anything animating? Drives whether a repaint is requested.
    pub fn is_animating(&self) -> bool {
        !self.ripples.is_empty() || self.press.is_active()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ripple() -> Ripple { Ripple::new(50.0, 50.0, 200.0, 100.0) }

    #[test]
    fn the_ripple_clears_the_far_corner() {
        // A fixed radius is a full-screen wash on a phone and a dot on a
        // desktop, so it comes from the widget's diagonal.
        let mut r = ripple();
        r.tick(RIPPLE_MS);
        let diagonal = (200.0f32 * 200.0 + 100.0 * 100.0).sqrt();
        assert!(r.radius() >= diagonal, "radius {} vs diagonal {diagonal}", r.radius());
    }

    #[test]
    fn the_ripple_stays_visible_while_it_travels() {
        // Fading linearly from the start makes it vanish before it has moved,
        // which reads as a flicker rather than a response.
        let mut r = ripple();
        r.tick(RIPPLE_MS * FADE_HOLD);
        assert!(r.alpha() >= FADE_HOLD_ALPHA - 0.01, "alpha at the hold: {}", r.alpha());
        assert!(r.radius() > 0.5 * r.max_radius, "and it has already travelled");
    }

    #[test]
    fn the_ripple_is_gone_by_the_end() {
        let mut r = ripple();
        r.tick(RIPPLE_MS);
        assert!(r.is_done());
        assert!(r.alpha() < 0.01, "alpha {}", r.alpha());
    }

    #[test]
    fn expansion_decelerates_so_it_appears_to_leave_the_finger() {
        // OutCubic: most of the distance early.
        let mut r = ripple();
        r.tick(RIPPLE_MS * 0.5);
        assert!(r.radius() > 0.5 * r.max_radius, "half the time, more than half the way");
    }

    #[test]
    fn the_press_response_is_a_real_scale_not_an_opacity_dip() {
        // The C++ faked a scale through opacity because "a true scale would
        // require QGraphicsView" — a Qt constraint, not a design choice. egui
        // draws to a transform every frame, so the intent ports and the
        // workaround does not.
        let mut p = PressResponse::start();
        assert_eq!(p.scale(), 1.0, "starts at rest");
        p.tick(PRESS_MS * PRESS_PEAK_AT);
        assert!((p.scale() - PRESS_DEPTH).abs() < 0.02, "dips to {}", p.scale());
    }

    #[test]
    fn the_press_returns_to_rest_and_stops() {
        let mut p = PressResponse::start();
        p.tick(PRESS_MS);
        assert!(!p.is_active());
        assert_eq!(p.scale(), 1.0);
    }

    #[test]
    fn the_press_goes_down_faster_than_it_comes_back() {
        // Symmetric timing reads as a glitch; asymmetric feels like a button.
        let mut down = PressResponse::start();
        down.tick(PRESS_MS * PRESS_PEAK_AT * 0.5);
        let travelled_down = 1.0 - down.scale();

        let mut up = PressResponse::start();
        up.tick(PRESS_MS * (PRESS_PEAK_AT + (1.0 - PRESS_PEAK_AT) * 0.5));
        let remaining_up = 1.0 - up.scale();
        assert!(travelled_down > remaining_up * 0.5,
                "down {travelled_down}, still-down {remaining_up}");
    }

    #[test]
    fn feedback_is_off_for_a_mouse() {
        // A ripple under a cursor is the tell of a phone UI wearing a desktop's
        // clothes.
        let mut f = TouchFeedback::new(false);
        f.tap(10.0, 10.0, 100.0, 100.0);
        assert!(f.ripples().is_empty());
        assert!(!f.is_animating());
    }

    #[test]
    fn disabling_clears_what_is_already_running() {
        // Otherwise a ripple hangs on screen at whatever radius it had reached
        // when the setting changed.
        let mut f = TouchFeedback::new(true);
        f.tap(10.0, 10.0, 100.0, 100.0);
        assert!(f.is_animating());
        f.set_enabled(false);
        assert!(!f.is_animating());
    }

    #[test]
    fn ripples_are_bounded_and_the_oldest_goes_first() {
        // Unbounded growth is a leak, and the oldest is the one already fading.
        let mut f = TouchFeedback::new(true);
        for i in 0..(MAX_RIPPLES + 5) {
            f.tap(i as f32, 0.0, 100.0, 100.0);
        }
        assert_eq!(f.ripples().len(), MAX_RIPPLES);
        assert!(f.ripples()[0].x > 0.0, "the first taps were dropped");
    }

    #[test]
    fn finished_ripples_are_dropped_so_nothing_accumulates() {
        let mut f = TouchFeedback::new(true);
        f.tap(10.0, 10.0, 100.0, 100.0);
        for _ in 0..40 { f.tick(16.0); }
        assert!(f.ripples().is_empty());
        assert!(!f.is_animating(), "and it stops asking for repaints");
    }

    #[test]
    fn a_resumed_suspend_completes_the_animation_rather_than_freezing_it() {
        let mut f = TouchFeedback::new(true);
        f.tap(10.0, 10.0, 100.0, 100.0);
        f.tick(4_000_000.0);
        assert!(f.ripples().is_empty());
        assert_eq!(f.scale(), 1.0);
    }

    #[test]
    fn a_negative_delta_does_not_run_the_animation_backwards() {
        let mut r = ripple();
        r.tick(100.0);
        let at = r.radius();
        r.tick(-500.0);
        assert_eq!(r.radius(), at);
    }
}
