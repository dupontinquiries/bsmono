//! dial_watermark — `DialWatermark.qml`, ported.
//!
//! A large product silhouette peeking from behind the volume dial: half-cut,
//! soft, with a dashed outline that flows.
//!
//! ## Three properties the QML names, and one it justifies
//!
//! * **CUT** — clipped along a 45° diagonal through its midpoint, upper-left
//!   triangle only.
//! * **SOFT** — ~50% opacity with a 4–16px blur, and the blur *degrades*: if
//!   the effects module is missing the watermark still shows un-blurred,
//!   *"so nothing breaks"*.
//! * **LIVING STROKE** — a dashed outline whose offset advances continuously.
//!
//! The fourth is the pacing, and the comment explains it: *"~60s loop when
//! quiet, ~25s when fully active. The dash pattern modulo (18px) means each
//! visible dash step takes ~1s at rest and never quite reaches frantic."*
//!
//! That last clause is a design constraint, not a description. A watermark that
//! sped up without a ceiling would pull the eye away from the dial it sits
//! behind — so the port keeps the exact 60s→25s range rather than treating it
//! as a tunable, and a test pins the resting dash step at about a second.
//!
//! ## Activity drives three things at once
//!
//! Speed, brightness and sharpness all rise together, which is what makes the
//! effect read as one response rather than three animations. They are computed
//! from a single `activity` here for the same reason.

/// Dash pattern period in pixels — one visible dash step.
pub const DASH_MODULO: f32 = 18.0;
/// Loop duration at rest and at full activity.
pub const LOOP_QUIET_MS: f32 = 60_000.0;
pub const LOOP_ACTIVE_MS: f32 = 25_000.0;
/// The phase animation's range, from the QML's `to: 1000`.
pub const PHASE_SPAN: f32 = 1000.0;

pub const MIN_BLUR: f32 = 4.0;
pub const MAX_BLUR: f32 = 16.0;
pub const MAX_OPACITY: f32 = 0.5;

#[derive(Clone, Debug)]
pub struct DialWatermark {
    /// 0..=1: drives speed, brightness and sharpness together.
    activity: f32,
    /// 0..PHASE_SPAN, advancing continuously.
    phase: f32,
    /// False when the platform has no blur effect available.
    blur_available: bool,
}

impl Default for DialWatermark {
    fn default() -> Self {
        DialWatermark { activity: 0.0, phase: 0.0, blur_available: true }
    }
}

impl DialWatermark {
    pub fn set_activity(&mut self, a: f32) { self.activity = a.clamp(0.0, 1.0); }
    pub fn activity(&self) -> f32 { self.activity }
    pub fn phase(&self) -> f32 { self.phase }

    /// Tell the watermark whether a blur is available.
    ///
    /// The QML loads `GraphicalEffects` optionally and shows the un-blurred
    /// canvas when it is missing. The same choice matters more on this stack,
    /// not less: a phone GPU without the shader should show a crisp watermark
    /// rather than nothing.
    pub fn set_blur_available(&mut self, ok: bool) { self.blur_available = ok; }

    /// Current loop duration.
    pub fn loop_ms(&self) -> f32 {
        LOOP_QUIET_MS - self.activity * (LOOP_QUIET_MS - LOOP_ACTIVE_MS)
    }

    /// Advance the dash offset.
    pub fn tick(&mut self, dt_ms: f32) {
        if dt_ms <= 0.0 { return; }
        // A resumed suspend advances by one long frame rather than teleporting
        // the dash pattern, which would read as a visible jolt behind the dial.
        let dt = dt_ms.min(LOOP_ACTIVE_MS / 10.0);
        let d = self.loop_ms();
        if d <= 0.0 { return; }
        // Wrapped, not accumulated: the phase feeds a dash offset, and letting
        // it grow for hours costs float precision until the dashes stop moving.
        self.phase = (self.phase + PHASE_SPAN * dt / d) % PHASE_SPAN;
    }

    /// Seconds for one visible dash step at the current activity.
    ///
    /// The QML's pacing claim in checkable form: about one second at rest.
    pub fn dash_step_secs(&self) -> f32 {
        (DASH_MODULO / PHASE_SPAN) * (self.loop_ms() / 1000.0)
    }

    /// Dash offset in pixels, which is what a painter actually needs.
    pub fn dash_offset(&self) -> f32 { self.phase % DASH_MODULO }

    /// Overall opacity.
    ///
    /// The QML's expression exactly: a lower resting value that lifts with
    /// activity, scaled so `max_opacity` remains the ceiling it names.
    pub fn opacity(&self) -> f32 {
        (0.30 + self.activity * 0.20) * (MAX_OPACITY / 0.5)
    }

    /// Blur radius, or `None` when the platform cannot blur.
    ///
    /// Sharper as activity rises — the watermark comes into focus rather than
    /// merely brightening, which is the "sharpness" the QML lists.
    pub fn blur(&self) -> Option<f32> {
        if !self.blur_available { return None; }
        Some(MAX_BLUR - self.activity * (MAX_BLUR - MIN_BLUR))
    }

    /// Is a point inside the 45° upper-left triangle?
    ///
    /// The cut runs through the midpoint, so the kept half is everything above
    /// the line `x/w + y/h = 1`. Expressed in fractions rather than pixels so a
    /// resize needs no recomputation.
    pub fn in_cut(&self, x: f32, y: f32, w: f32, h: f32) -> bool {
        if w <= 0.0 || h <= 0.0 { return false; }
        x >= 0.0 && y >= 0.0 && x <= w && y <= h && (x / w + y / h) <= 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dash_step_takes_about_a_second_at_rest() {
        // The QML's own pacing claim, in checkable form: "each visible dash
        // step takes ~1s at rest and never quite reaches frantic".
        let w = DialWatermark::default();
        let step = w.dash_step_secs();
        assert!((step - 1.08).abs() < 0.05, "resting dash step is {step}s");
    }

    #[test]
    fn full_activity_speeds_up_without_reaching_frantic() {
        // A watermark that accelerated without a ceiling would pull the eye
        // away from the dial it sits behind — which is the whole point of it
        // being a watermark.
        let mut w = DialWatermark::default();
        w.set_activity(1.0);
        assert_eq!(w.loop_ms(), LOOP_ACTIVE_MS);
        let step = w.dash_step_secs();
        assert!(step > 0.4, "still calm at full activity: {step}s");
    }

    #[test]
    fn activity_is_clamped_rather_than_trusted() {
        // The property is documented 0..1 and an audio level arriving hot is
        // ordinary; a loop duration going negative would run the dashes
        // backwards.
        let mut w = DialWatermark::default();
        w.set_activity(5.0);
        assert_eq!(w.activity(), 1.0);
        w.set_activity(-2.0);
        assert_eq!(w.activity(), 0.0);
        assert!(w.loop_ms() > 0.0);
    }

    #[test]
    fn the_phase_wraps_rather_than_accumulating() {
        // It feeds a dash offset, and letting it grow for hours costs float
        // precision until the dashes stop moving.
        let mut w = DialWatermark::default();
        for _ in 0..50_000 { w.tick(16.0); }
        assert!(w.phase() >= 0.0 && w.phase() < PHASE_SPAN, "phase {}", w.phase());
        let before = w.dash_offset();
        w.tick(16.0);
        assert_ne!(w.dash_offset(), before, "still moving after 13 minutes");
    }

    #[test]
    fn the_dash_offset_stays_inside_one_pattern_period() {
        let mut w = DialWatermark::default();
        w.set_activity(0.7);
        for _ in 0..2_000 {
            w.tick(16.0);
            assert!(w.dash_offset() >= 0.0 && w.dash_offset() < DASH_MODULO);
        }
    }

    #[test]
    fn a_resumed_suspend_does_not_jolt_the_pattern() {
        // A dash pattern teleporting behind the dial is a visible glitch at
        // exactly the moment a user looks at the screen.
        let mut w = DialWatermark::default();
        let before = w.dash_offset();
        w.tick(4_000_000.0);
        let jump = (w.dash_offset() - before).abs();
        assert!(jump < DASH_MODULO, "jumped {jump}px in one frame");
    }

    #[test]
    fn opacity_rises_with_activity_and_respects_the_ceiling() {
        let mut w = DialWatermark::default();
        assert!((w.opacity() - 0.30).abs() < 0.001, "resting: {}", w.opacity());
        w.set_activity(1.0);
        assert!((w.opacity() - MAX_OPACITY).abs() < 0.001, "peak: {}", w.opacity());
        assert!(w.opacity() <= MAX_OPACITY);
    }

    #[test]
    fn the_watermark_sharpens_as_activity_rises() {
        // Sharpness is one of three things activity drives, and dropping it
        // would make the effect read as mere brightening.
        let mut w = DialWatermark::default();
        let resting = w.blur().unwrap();
        w.set_activity(1.0);
        let active = w.blur().unwrap();
        assert_eq!(resting, MAX_BLUR);
        assert_eq!(active, MIN_BLUR);
        assert!(active < resting, "it comes into focus");
    }

    #[test]
    fn a_platform_without_blur_still_shows_the_watermark() {
        // The QML loads GraphicalEffects optionally "so nothing breaks", and
        // that matters more on this stack rather than less: a phone GPU without
        // the shader should show a crisp watermark rather than nothing.
        let mut w = DialWatermark::default();
        w.set_blur_available(false);
        assert_eq!(w.blur(), None);
        assert!(w.opacity() > 0.0, "it is still drawn");
    }

    #[test]
    fn the_cut_keeps_the_upper_left_triangle() {
        // 45° through the midpoint: everything above x/w + y/h = 1.
        let w = DialWatermark::default();
        assert!(w.in_cut(0.0, 0.0, 100.0, 100.0), "the corner is kept");
        assert!(w.in_cut(10.0, 10.0, 100.0, 100.0));
        assert!(!w.in_cut(90.0, 90.0, 100.0, 100.0), "the far corner is cut");
        assert!(w.in_cut(50.0, 50.0, 100.0, 100.0), "the midpoint is on the line");
        assert!(!w.in_cut(60.0, 60.0, 100.0, 100.0));
    }

    #[test]
    fn the_cut_holds_its_proportions_on_a_non_square_widget() {
        // Expressed in fractions rather than pixels, so a resize needs no
        // recomputation and a wide watermark is not cut at a different angle.
        let w = DialWatermark::default();
        assert!(w.in_cut(90.0, 10.0, 400.0, 100.0), "wide: still inside");
        assert!(!w.in_cut(390.0, 90.0, 400.0, 100.0));
    }

    #[test]
    fn a_zero_sized_widget_cuts_nothing_rather_than_dividing_by_zero() {
        // Reachable during layout before a size is assigned.
        let w = DialWatermark::default();
        assert!(!w.in_cut(0.0, 0.0, 0.0, 0.0));
    }

    #[test]
    fn a_negative_delta_does_not_run_the_dashes_backwards() {
        let mut w = DialWatermark::default();
        w.tick(500.0);
        let mid = w.phase();
        w.tick(-500.0);
        assert_eq!(w.phase(), mid);
    }
}
