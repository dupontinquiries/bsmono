//! motion — easing curves and eased animation helpers, so egui widgets get QML-quality motion by
//! default instead of egui's linear `animate_bool`.
//!
//! The QML views leaned heavily on tuned `Behavior`/`NumberAnimation` transitions — `OutCubic` fades,
//! `OutBack` spring pop-ins, 120–200ms `ColorAnimation` hover/countdown feedback. Ports that drop those
//! feel dead. This module brings the curves over as reusable primitives:
//!
//!   * [`ease`] — the easing functions (matching the `Easing.*` the QML used),
//!   * [`animate_eased`] — `animate_bool` with a curve,
//!   * [`animate_color`] — a smooth colour approach (`Behavior on color`),
//!   * [`interactive_fill`] — a button/cell fill that eases between rest/hover/press by default,
//!   * [`pop_scale`] — a spring pop-in scale for things that appear.

use egui::{Color32, Context, Id, Response};

/// Easing curves in `t ∈ [0,1]`. Names mirror QML `Easing.*`.
pub mod ease {
    /// No easing.
    pub fn linear(t: f32) -> f32 {
        t
    }
    /// Decelerating (fast → slow). QML `Easing.OutQuad`.
    pub fn out_quad(t: f32) -> f32 {
        1.0 - (1.0 - t) * (1.0 - t)
    }
    /// Accelerating (slow → fast). QML `Easing.InQuad`.
    pub fn in_quad(t: f32) -> f32 {
        t * t
    }
    /// Ease in then out. QML `Easing.InOutQuad`.
    pub fn in_out_quad(t: f32) -> f32 {
        if t < 0.5 {
            2.0 * t * t
        } else {
            1.0 - (-2.0 * t + 2.0).powi(2) / 2.0
        }
    }
    /// Strong deceleration — the workhorse for fades/expands. QML `Easing.OutCubic`.
    pub fn out_cubic(t: f32) -> f32 {
        1.0 - (1.0 - t).powi(3)
    }
    /// Accelerating cubic. QML `Easing.InCubic`.
    pub fn in_cubic(t: f32) -> f32 {
        t * t * t
    }
    /// Springy overshoot past 1 then settles — great for pop-ins. QML `Easing.OutBack`.
    pub fn out_back(t: f32) -> f32 {
        const C1: f32 = 1.70158;
        const C3: f32 = C1 + 1.0;
        1.0 + C3 * (t - 1.0).powi(3) + C1 * (t - 1.0).powi(2)
    }
    /// Gentle sine ease-out — soft, used for breathing/idle motion.
    pub fn out_sine(t: f32) -> f32 {
        (t * std::f32::consts::FRAC_PI_2).sin()
    }

    /// QML `Easing.InSine`.
    pub fn in_sine(t: f32) -> f32 {
        1.0 - (t * std::f32::consts::FRAC_PI_2).cos()
    }
    /// QML `Easing.InOutSine` — the smooth breathing/ambient curve the Qt apps lean on (bo-controller
    /// uses it 16×). Symmetric ease-in-out.
    pub fn in_out_sine(t: f32) -> f32 {
        -((std::f32::consts::PI * t).cos() - 1.0) / 2.0
    }
    /// A cubic-Bézier easing evaluator matching QML `Easing.Bezier` (and CSS `cubic-bezier`): the two
    /// control points `(x1,y1),(x2,y2)` with endpoints fixed at (0,0)/(1,1). Solves x(u)=t for the curve
    /// parameter by bisection (x is monotonic for well-formed control points), then returns y(u). Lets a
    /// port reproduce a hand-tuned QML Bezier curve exactly instead of approximating with a named easing.
    pub fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
        let comp = |a: f32, b: f32, u: f32| {
            let v = 1.0 - u;
            3.0 * v * v * u * a + 3.0 * v * u * u * b + u * u * u
        };
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..24 {
            let mid = 0.5 * (lo + hi);
            if comp(x1, x2, mid) < t { lo = mid; } else { hi = mid; }
        }
        comp(y1, y2, 0.5 * (lo + hi))
    }
}

/// `animate_bool` with an easing curve: drives a 0→1 value for `id` toward `target` over `duration`
/// seconds, then shapes it with `easing`. Returns the eased value in `[0,1]` (may briefly exceed 1 for
/// `out_back`).
pub fn animate_eased(ctx: &Context, id: Id, target: bool, duration: f32, easing: fn(f32) -> f32) -> f32 {
    let linear = ctx.animate_bool_with_time(id, target, duration);
    easing(linear)
}

/// A spring pop-in scale (`out_back`) for something appearing/disappearing — scale a rect by this
/// around its center. `0` when closed, overshoots ~1.05 then settles at `1.0`.
pub fn pop_scale(ctx: &Context, id: Id, open: bool, duration: f32) -> f32 {
    animate_eased(ctx, id, open, duration, ease::out_back)
}

/// Smoothly approach `target` colour over roughly `duration` seconds — the equivalent of QML's
/// `Behavior on color { ColorAnimation }`. Keeps the shown colour in egui memory keyed by `id`, so
/// hover/press/state changes glide instead of snapping.
pub fn animate_color(ctx: &Context, id: Id, target: Color32, duration: f32) -> Color32 {
    let tgt = [target.r() as f32, target.g() as f32, target.b() as f32, target.a() as f32];
    let cur: [f32; 4] = ctx.data_mut(|d| d.get_temp(id)).unwrap_or(tgt);
    let dt = ctx.input(|i| i.stable_dt).clamp(0.0, 0.1);
    // Exponential approach; the factor 4 makes it substantially settle within `duration`.
    let k = if duration <= 1e-4 { 1.0 } else { (1.0 - (-dt * 4.0 / duration).exp()).clamp(0.0, 1.0) };
    let next = [
        cur[0] + (tgt[0] - cur[0]) * k,
        cur[1] + (tgt[1] - cur[1]) * k,
        cur[2] + (tgt[2] - cur[2]) * k,
        cur[3] + (tgt[3] - cur[3]) * k,
    ];
    ctx.data_mut(|d| d.insert_temp(id, next));
    let dist: f32 = (0..4).map(|i| (next[i] - tgt[i]).abs()).sum();
    if dist > 0.6 {
        ctx.request_repaint(); // keep gliding until we arrive
    }
    Color32::from_rgba_unmultiplied(
        next[0].round() as u8,
        next[1].round() as u8,
        next[2].round() as u8,
        next[3].round() as u8,
    )
}

/// A fill colour that eases between rest/hover/press based on `response` — drop-in smoothness for any
/// custom-painted button or cell (matching the QML 120ms hover `ColorAnimation`). Use for painter-drawn
/// widgets where egui's own widget animation doesn't apply.
pub fn interactive_fill(ctx: &Context, response: &Response, rest: Color32, hover: Color32, press: Color32) -> Color32 {
    let target = if response.is_pointer_button_down_on() {
        press
    } else if response.hovered() {
        hover
    } else {
        rest
    };
    animate_color(ctx, response.id.with("motion_fill"), target, 0.12)
}

#[cfg(test)]
mod tests {
    use super::ease::*;

    fn endpoints(f: fn(f32) -> f32) {
        assert!((f(0.0)).abs() < 1e-4, "f(0)=0");
        assert!((f(1.0) - 1.0).abs() < 1e-4, "f(1)=1");
    }

    #[test]
    fn curves_hit_their_endpoints() {
        for f in [linear, out_quad, in_quad, in_out_quad, out_cubic, in_cubic, out_back, out_sine, in_sine, in_out_sine] {
            endpoints(f);
        }
    }

    #[test]
    fn out_curves_decelerate_and_in_curves_accelerate() {
        // out_* is above the diagonal (ahead early); in_* is below (behind early)
        assert!(out_quad(0.25) > 0.25);
        assert!(out_cubic(0.25) > 0.25);
        assert!(in_quad(0.25) < 0.25);
        assert!(in_cubic(0.25) < 0.25);
    }

    #[test]
    fn out_back_overshoots_past_one() {
        // the spring goes beyond 1.0 somewhere before settling
        let peak = (70..=95).map(|i| out_back(i as f32 / 100.0)).fold(0.0f32, f32::max);
        assert!(peak > 1.0, "out_back should overshoot, peak={peak}");
    }

    #[test]
    fn in_out_sine_is_symmetric_and_bezier_matches_endpoints() {
        assert!((in_out_sine(0.0)).abs() < 1e-4 && (in_out_sine(1.0) - 1.0).abs() < 1e-4);
        assert!((in_out_sine(0.5) - 0.5).abs() < 1e-3, "InOutSine passes through the midpoint");
        // a standard ease (0.25,0.1,0.25,1.0) hits the endpoints and stays in-range
        assert!(cubic_bezier(0.25, 0.1, 0.25, 1.0, 0.0).abs() < 1e-3);
        assert!((cubic_bezier(0.25, 0.1, 0.25, 1.0, 1.0) - 1.0).abs() < 1e-3);
        let mid = cubic_bezier(0.25, 0.1, 0.25, 1.0, 0.5);
        assert!(mid > 0.0 && mid < 1.0);
    }
}

/// Press feedback: the dip-and-ripple every tappable key in the fleet does.
///
/// This exists because it was written twice. snapclone's `GlassButton` and
/// bo-suite's `StepKey` were ported independently, three sessions apart, and
/// came out line-for-line identical — same fields, same tick, same
/// `(1.0 - press_scale) * dt / press_ms` ramp — differing only in three
/// constants. That is the "two lists of the same thing" failure the port
/// programme keeps finding in the Qt code, committed in the port itself.
///
/// The constants stay per-app and are NOT unified: B&O's key dips 8% over 110ms
/// with a 340ms ripple, snapclone's dips 12% over 90ms with 320ms. Those are
/// house feels, deliberately different, and flattening them here would be the
/// opposite mistake — a shared kit that makes every app feel the same is a
/// worse outcome than a little duplication.
///
/// What IS shared is the behaviour, and the two rules that are easy to get
/// wrong in it:
///
/// * The ripple OUTLIVES the release. Cutting it on finger-up means a quick tap
///   produces no visible feedback at all — the interaction that most needs it.
/// * The ripple fades as it grows, or it ends as a hard disc sitting on the key.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PressFeel {
    /// How long the dip takes, and how long springing back takes.
    pub press_ms: f32,
    /// Ripple lifetime. Longer than `press_ms` on purpose: feedback should
    /// still be visible after the key has sprung back, or the two animations
    /// cancel each other out.
    pub ripple_ms: f32,
    /// How far the body scales down while held, 0..1.
    pub press_scale: f32,
}

impl PressFeel {
    pub const fn new(press_ms: f32, ripple_ms: f32, press_scale: f32) -> Self {
        PressFeel { press_ms, ripple_ms, press_scale }
    }
}

/// The animation state for one pressable thing.
#[derive(Debug, Clone, PartialEq)]
pub struct PressFeedback {
    feel: PressFeel,
    scale: f32,
    down: bool,
    ripple_ms: Option<f32>,
    ripple_at: (f32, f32),
}

impl PressFeedback {
    pub fn new(feel: PressFeel) -> Self {
        PressFeedback { feel, scale: 1.0, down: false, ripple_ms: None, ripple_at: (0.0, 0.0) }
    }

    pub fn feel(&self) -> PressFeel { self.feel }

    /// Press at a point in widget-local coordinates.
    ///
    /// The ripple is keyed to the ACTUAL touch point, which is what a
    /// stylesheet `:pressed` state cannot know and what makes the feedback feel
    /// like it came from your finger.
    pub fn press(&mut self, x: f32, y: f32) {
        self.down = true;
        self.ripple_at = (x, y);
        self.ripple_ms = Some(0.0);
    }

    pub fn release(&mut self) { self.down = false; }
    pub fn is_down(&self) -> bool { self.down }

    pub fn tick(&mut self, dt_ms: f32) {
        let target = if self.down { self.feel.press_scale } else { 1.0 };
        let step = (1.0 - self.feel.press_scale) * dt_ms / self.feel.press_ms;
        if self.scale > target { self.scale = (self.scale - step).max(target); }
        else if self.scale < target { self.scale = (self.scale + step).min(target); }

        if let Some(t) = self.ripple_ms {
            let t = t + dt_ms;
            // Deliberately not cut on release — see the type doc.
            self.ripple_ms = if t >= self.feel.ripple_ms { None } else { Some(t) };
        }
    }

    pub fn scale(&self) -> f32 { self.scale }
    pub fn rippling(&self) -> bool { self.ripple_ms.is_some() }
    pub fn ripple_origin(&self) -> (f32, f32) { self.ripple_at }

    /// Ripple radius fraction, eased so it blooms rather than expanding evenly.
    pub fn ripple_t(&self) -> f32 {
        self.ripple_ms
            .map(|t| ease::out_cubic((t / self.feel.ripple_ms).clamp(0.0, 1.0)))
            .unwrap_or(0.0)
    }

    /// Ripple opacity at a given peak — fades as it expands.
    pub fn ripple_alpha(&self, peak: f32) -> f32 {
        if !self.rippling() { return 0.0; }
        (1.0 - self.ripple_t()) * peak
    }
}

#[cfg(test)]
mod press_tests {
    use super::*;

    const FEEL: PressFeel = PressFeel::new(100.0, 300.0, 0.9);

    #[test]
    fn it_dips_on_press_and_springs_back_on_release() {
        let mut p = PressFeedback::new(FEEL);
        assert_eq!(p.scale(), 1.0);
        p.press(5.0, 5.0);
        p.tick(FEEL.press_ms);
        assert!((p.scale() - FEEL.press_scale).abs() < 0.01);
        p.release();
        p.tick(FEEL.press_ms);
        assert!((p.scale() - 1.0).abs() < 0.01);
    }

    #[test]
    fn the_ripple_outlives_the_release() {
        // Cutting it on finger-up means a quick tap produces no visible
        // feedback at all — the interaction that most needs it.
        let mut p = PressFeedback::new(FEEL);
        p.press(1.0, 2.0);
        p.tick(20.0);
        p.release();
        p.tick(20.0);
        assert!(p.rippling());
        p.tick(FEEL.ripple_ms);
        assert!(!p.rippling());
    }

    #[test]
    fn the_ripple_expands_while_fading() {
        // Otherwise it ends as a hard disc sitting on the key.
        let mut p = PressFeedback::new(FEEL);
        p.press(0.0, 0.0);
        p.tick(FEEL.ripple_ms * 0.2);
        let (t1, a1) = (p.ripple_t(), p.ripple_alpha(0.3));
        p.tick(FEEL.ripple_ms * 0.4);
        assert!(p.ripple_t() > t1);
        assert!(p.ripple_alpha(0.3) < a1);
    }

    #[test]
    fn the_ripple_starts_where_the_finger_did() {
        let mut p = PressFeedback::new(FEEL);
        p.press(7.0, 23.0);
        assert_eq!(p.ripple_origin(), (7.0, 23.0));
    }

    #[test]
    fn the_scale_never_overshoots_its_bounds() {
        let mut p = PressFeedback::new(FEEL);
        p.press(0.0, 0.0);
        for _ in 0..50 { p.tick(FEEL.press_ms); }
        assert!(p.scale() >= FEEL.press_scale);
        p.release();
        for _ in 0..50 { p.tick(FEEL.press_ms); }
        assert!(p.scale() <= 1.0);
    }

    #[test]
    fn two_apps_can_feel_different_while_sharing_the_behaviour() {
        // The constants are house feels and stay per-app. A shared kit that
        // makes every app feel the same is a worse outcome than a little
        // duplication.
        let bo = PressFeedback::new(PressFeel::new(90.0, 320.0, 0.92));
        let snap = PressFeedback::new(PressFeel::new(110.0, 340.0, 0.88));
        assert_ne!(bo.feel().press_scale, snap.feel().press_scale);
        assert_ne!(bo.feel().press_ms, snap.feel().press_ms);
    }

    #[test]
    fn a_settled_key_reports_no_ripple() {
        let p = PressFeedback::new(FEEL);
        assert!(!p.rippling());
        assert_eq!(p.ripple_alpha(0.3), 0.0);
        assert_eq!(p.ripple_t(), 0.0);
    }
}

/// A one-dimensional reveal: something that eases in, and STAYS DRAWN while it
/// eases out.
///
/// Written three times before it was noticed — snapclone's accordion card
/// (`SUBMENU_MS`, 180), snapclone's bottom sheet (`SHEET_MS`, 220) and
/// bo-suite's device panel (`UNFOLD_MS`, 220) all ran the same
/// `if t < target { t += dt/ms } else if t > target { t -= dt/ms }` ramp and the
/// same `t > 0.0` visibility test.
///
/// The visibility test is the part worth sharing. Dropping a panel the moment
/// its flag goes false makes it VANISH rather than close, and every one of those
/// three had to learn that separately. `visible()` is true while there is any
/// reveal left, so the caller draws through the whole exit.
///
/// Durations stay with the caller: a submenu is quicker than a sheet, and
/// flattening them would make everything move at one speed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reveal {
    t: f32,
    open: bool,
    duration_ms: f32,
}

impl Reveal {
    pub fn new(duration_ms: f32) -> Self {
        // A zero duration would divide by zero on the first tick; treat it as
        // "instant" rather than as a bug the caller has to remember.
        Reveal { t: 0.0, open: false, duration_ms: duration_ms.max(1.0) }
    }

    /// Start open, for things that are on screen before they animate.
    pub fn new_open(duration_ms: f32) -> Self {
        Reveal { t: 1.0, open: true, duration_ms: duration_ms.max(1.0) }
    }

    pub fn open(&mut self) { self.open = true; }
    pub fn close(&mut self) { self.open = false; }
    pub fn toggle(&mut self) { self.open = !self.open; }
    pub fn set(&mut self, open: bool) { self.open = open; }
    pub fn is_open(&self) -> bool { self.open }

    /// Snap to the target without animating — for a resize or a state restore,
    /// where animating from nothing would look like the thing just appeared.
    pub fn jump(&mut self) { self.t = if self.open { 1.0 } else { 0.0 }; }

    pub fn tick(&mut self, dt_ms: f32) {
        let target = if self.open { 1.0 } else { 0.0 };
        let step = dt_ms / self.duration_ms;
        if self.t < target { self.t = (self.t + step).min(target); }
        else if self.t > target { self.t = (self.t - step).max(target); }
    }

    /// Raw progress, 0..=1.
    pub fn raw(&self) -> f32 { self.t }

    /// Eased progress — what a caller should scale a height or an offset by.
    pub fn eased(&self) -> f32 { ease::out_cubic(self.t.clamp(0.0, 1.0)) }

    /// Still on screen? True through the whole exit, which is the rule all
    /// three call sites had to discover independently.
    pub fn visible(&self) -> bool { self.open || self.t > 0.0 }

    /// Neither opening nor closing.
    pub fn settled(&self) -> bool { self.t <= 0.0 || self.t >= 1.0 }
}

/// A slow oscillation between two levels — the "this is alive" glow.
///
/// Five independent implementations before this existed: bo-suite's dial idle
/// glow and anticipation cue, bo-suite's queue equalizer, snapclone's peel hint
/// and its voice-bar pulse. Every one was `sin` mapped into a floor..ceiling
/// band; they differed only in period and in how loud they were allowed to get.
///
/// Period and levels stay with the caller for the same reason the press
/// constants did. A container's glow must be slower and dimmer than a status
/// pulse — a fast bright breath on a panel reads as an alert.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Breath {
    phase: f32,
    period_ms: f32,
    floor: f32,
    ceiling: f32,
}

impl Breath {
    pub fn new(period_ms: f32, floor: f32, ceiling: f32) -> Self {
        Breath { phase: 0.0, period_ms: period_ms.max(1.0), floor, ceiling }
    }

    pub fn tick(&mut self, dt_ms: f32) {
        self.phase = (self.phase + dt_ms / self.period_ms).rem_euclid(1.0);
    }

    /// 0..=1 through the cycle, for callers that want to offset several
    /// oscillators from one clock — an equalizer's bars rising in unison is a
    /// progress bar, not an equalizer.
    pub fn phase(&self) -> f32 { self.phase }

    /// The level, between `floor` and `ceiling`.
    pub fn value(&self) -> f32 { self.at(0.0) }

    /// The level at a phase offset in turns, for those extra oscillators.
    pub fn at(&self, offset_turns: f32) -> f32 {
        let t = (self.phase + offset_turns) * std::f32::consts::TAU;
        let unit = (t.sin() + 1.0) * 0.5;
        self.floor + (self.ceiling - self.floor) * unit
    }

    /// Restart from the trough — so a thing that just became active begins by
    /// brightening rather than at whatever level the clock happened to be at.
    pub fn restart(&mut self) { self.phase = 0.75; }
}

#[cfg(test)]
mod reveal_breath_tests {
    use super::*;

    #[test]
    fn a_reveal_stays_visible_through_its_whole_exit() {
        // The rule all three call sites had to learn separately: dropping it at
        // open=false makes the panel VANISH rather than close.
        let mut r = Reveal::new(200.0);
        r.open();
        r.tick(200.0);
        assert_eq!(r.eased(), 1.0);
        r.close();
        r.tick(50.0);
        assert!(r.visible(), "still leaving");
        assert!(r.raw() > 0.0);
        r.tick(200.0);
        assert!(!r.visible());
    }

    #[test]
    fn a_reveal_eases_rather_than_ramping_linearly() {
        let mut r = Reveal::new(200.0);
        r.open();
        r.tick(50.0);
        assert!(r.eased() > r.raw(), "out_cubic leads a linear ramp early");
    }

    #[test]
    fn a_slow_frame_does_not_skip_the_whole_reveal() {
        // Frame-time driven, so one hitch advances by its own duration rather
        // than jumping to the end.
        let mut r = Reveal::new(400.0);
        r.open();
        r.tick(100.0);
        assert!(!r.settled());
        assert!(r.raw() < 1.0);
    }

    #[test]
    fn a_zero_duration_does_not_divide_by_zero() {
        let mut r = Reveal::new(0.0);
        r.open();
        r.tick(16.0);
        assert!(r.raw().is_finite());
        assert_eq!(r.raw(), 1.0);
    }

    #[test]
    fn jump_snaps_without_animating() {
        // For a resize or a state restore, where animating from nothing looks
        // like the thing just appeared.
        let mut r = Reveal::new(200.0);
        r.open();
        r.jump();
        assert_eq!(r.raw(), 1.0);
        assert!(r.settled());
    }

    #[test]
    fn reversing_mid_animation_starts_from_where_it_got_to() {
        // Not from the beginning — a menu half-open that is closed again should
        // close from half, or it jumps open first.
        let mut r = Reveal::new(200.0);
        r.open();
        r.tick(100.0);
        let half = r.raw();
        r.close();
        r.tick(10.0);
        assert!(r.raw() < half && r.raw() > 0.0);
    }

    #[test]
    fn a_breath_stays_inside_its_band() {
        let mut b = Breath::new(1000.0, 0.35, 0.6);
        for _ in 0..500 {
            b.tick(7.0);
            let v = b.value();
            assert!((0.35..=0.6).contains(&v), "out of band: {v}");
        }
    }

    #[test]
    fn a_breath_actually_breathes() {
        // A constant value would pass the band test and be useless.
        let mut b = Breath::new(1000.0, 0.0, 1.0);
        let mut lo = f32::INFINITY;
        let mut hi: f32 = 0.0;
        for _ in 0..200 { b.tick(10.0); lo = lo.min(b.value()); hi = hi.max(b.value()); }
        assert!(hi - lo > 0.9, "range was {}", hi - lo);
    }

    #[test]
    fn the_phase_wraps_rather_than_growing_without_bound() {
        // A phase that accumulates for hours loses float precision and the
        // glow starts stuttering.
        let mut b = Breath::new(1000.0, 0.0, 1.0);
        for _ in 0..100_000 { b.tick(16.0); }
        assert!((0.0..1.0).contains(&b.phase()));
    }

    #[test]
    fn offset_oscillators_do_not_move_in_unison() {
        // Three bars rising together is a progress bar, not an equalizer.
        let mut b = Breath::new(1000.0, 0.35, 0.8);
        b.tick(120.0);
        assert!((b.at(0.0) - b.at(0.33)).abs() > 0.05);
    }

    #[test]
    fn restarting_begins_at_the_trough() {
        // So a thing that just became active starts by brightening rather than
        // at whatever level the clock happened to be at.
        let mut b = Breath::new(1000.0, 0.2, 0.9);
        b.tick(400.0);
        b.restart();
        assert!((b.value() - 0.2).abs() < 0.01);
    }

    #[test]
    fn two_callers_can_breathe_at_different_speeds_and_levels() {
        // A container's glow must be slower and dimmer than a status pulse: a
        // fast bright breath on a panel reads as an alert.
        let card = Breath::new(1800.0, 0.0, 0.35);
        let status = Breath::new(600.0, 0.0, 1.0);
        assert!(card.period_ms > status.period_ms);
        assert!(card.ceiling < status.ceiling);
    }
}
