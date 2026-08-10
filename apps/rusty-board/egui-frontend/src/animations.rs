//! animations — `ui/animations.rs`, ported from libadwaita to egui.
//!
//! The keyboard's four animations: key-press pulse, suggestion-bar slide,
//! recording pulse, and commit flash.
//!
//! ## The optimisation that shaped the original does not port — the constraint does
//!
//! The GTK version moved the key-press pulse from a `SpringAnimation` with a
//! callback target onto a CSS class, and its note says exactly why: the
//! callback fired *"~13 main-loop callbacks per invocation (60Hz × 220ms), each
//! setting widget.opacity. At a typical 5 cps typing rate that was ~65
//! callbacks/sec just for press feedback"*.
//!
//! egui has no compositor to hand a keyframe to — it redraws each frame anyway,
//! so per-frame evaluation costs nothing extra and the CSS trick has no
//! analogue. What ports is the constraint that motivated it: **a keyboard fires
//! these faster than they finish**, so the state per key must be tiny and
//! restarting must be free.
//!
//! ## Restart, never queue
//!
//! The C++ removes the class before re-adding it, *"in case a previous press is
//! still mid-animation — re-adding restarts the keyframe"*. Its note names the
//! failure the old version had: *"animations overlapped into a queue when the
//! user typed faster than they completed."* A queue means the flash lags the
//! keystroke and keeps flashing after you stop — the tell of a keyboard that
//! feels slow even at full frame rate. Restart is the whole behaviour.

/// Key-press dip, then back.
pub const PRESS_MS: f32 = 220.0;
/// Suggestion bar slide-in.
pub const SLIDE_MS: f32 = 220.0;
/// Recording pulse half-cycle.
pub const RECORD_MS: f32 = 800.0;
/// Commit flash.
pub const COMMIT_MS: f32 = 380.0;
/// How far the press dips.
pub const PRESS_DEPTH: f32 = 0.55;
/// Recording pulse floor, from `TimedAnimation::new(0.5, 1.0, 800)`.
pub const RECORD_FLOOR: f32 = 0.5;

/// A one-shot animation on a single element.
///
/// Copy and eight bytes, because a keyboard holds one per key and they are
/// created faster than they finish.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OneShot {
    age_ms: f32,
    running: bool,
}

impl OneShot {
    /// Start, or restart if already running.
    ///
    /// Restart rather than queue: the GTK version removes the CSS class before
    /// re-adding for exactly this, because overlapping presses otherwise made
    /// the flash lag the keystroke and keep flashing after you stopped.
    pub fn start(&mut self) { self.age_ms = 0.0; self.running = true; }

    pub fn is_running(&self) -> bool { self.running }

    pub fn tick(&mut self, dt_ms: f32, duration_ms: f32) {
        if !self.running || dt_ms <= 0.0 { return; }
        // Clamped so a resumed suspend completes the animation rather than
        // leaving a key stuck dim.
        self.age_ms += dt_ms.min(duration_ms.max(1.0));
        if self.age_ms >= duration_ms { self.running = false; }
    }

    pub fn progress(&self, duration_ms: f32) -> f32 {
        if !self.running { return 1.0; }
        (self.age_ms / duration_ms.max(1.0)).clamp(0.0, 1.0)
    }
}

/// Opacity for a key being pressed: dips and returns.
///
/// Symmetric, unlike the press response in snapclone's touch feedback — a key
/// is struck and released in one motion, so there is no separate "still down"
/// state to hold at.
pub fn press_opacity(a: &OneShot) -> f32 {
    if !a.is_running() { return 1.0; }
    let t = a.progress(PRESS_MS);
    // A triangle rather than a curve: at 220ms nobody perceives the easing, and
    // a keyboard evaluates this for every visible key every frame.
    let depth = 1.0 - (2.0 * t - 1.0).abs();
    1.0 - (1.0 - PRESS_DEPTH) * depth
}

/// Top margin for a suggestion bar sliding up from `distance` to zero.
pub fn slide_offset(a: &OneShot, distance: f32) -> f32 {
    if !a.is_running() { return 0.0; }
    let t = a.progress(SLIDE_MS);
    // Ease out: the bar arrives and settles rather than stopping dead.
    distance * (1.0 - t).powi(3)
}

/// Commit flash opacity, a single fade back to rest.
pub fn commit_opacity(a: &OneShot) -> f32 {
    if !a.is_running() { return 1.0; }
    let t = a.progress(COMMIT_MS);
    1.0 + (1.0 - t) * 0.4
}

/// A continuous pulse, for the recording indicator.
///
/// Phase-based and wrapping rather than a counter, so it runs for an hour
/// without accumulating a float large enough to stop moving.
#[derive(Clone, Copy, Debug, Default)]
pub struct Pulse {
    phase_ms: f32,
    running: bool,
}

impl Pulse {
    pub fn start(&mut self) { self.running = true; }
    /// Stop and reset, so a later recording starts at a KNOWN point rather than
    /// wherever the previous one happened to end — resuming mid-cycle makes two
    /// recordings look different for no reason.
    pub fn stop(&mut self) { self.running = false; self.phase_ms = 0.0; }
    pub fn is_running(&self) -> bool { self.running }

    pub fn tick(&mut self, dt_ms: f32) {
        if !self.running || dt_ms <= 0.0 { return; }
        let cycle = RECORD_MS * 2.0;
        self.phase_ms = (self.phase_ms + dt_ms.min(cycle)) % cycle;
    }

    /// Opacity between the floor and full, triangular.
    pub fn opacity(&self) -> f32 {
        if !self.running { return 1.0; }
        let t = self.phase_ms / RECORD_MS;
        let up = if t <= 1.0 { t } else { 2.0 - t };
        RECORD_FLOOR + (1.0 - RECORD_FLOOR) * up
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_press_dips_and_returns_to_full() {
        let mut a = OneShot::default();
        a.start();
        assert!((press_opacity(&a) - 1.0).abs() < 0.01, "starts at rest");
        a.tick(PRESS_MS / 2.0, PRESS_MS);
        assert!((press_opacity(&a) - PRESS_DEPTH).abs() < 0.02, "dips at the midpoint");
        a.tick(PRESS_MS, PRESS_MS);
        assert_eq!(press_opacity(&a), 1.0);
        assert!(!a.is_running());
    }

    #[test]
    fn pressing_again_mid_animation_restarts_rather_than_queueing() {
        // The failure the GTK note records: "animations overlapped into a queue
        // when the user typed faster than they completed" — the flash lags the
        // keystroke and keeps flashing after you stop.
        let mut a = OneShot::default();
        a.start();
        a.tick(PRESS_MS * 0.9, PRESS_MS);
        a.start();
        assert_eq!(a.progress(PRESS_MS), 0.0, "back to the beginning");
        a.tick(PRESS_MS, PRESS_MS);
        assert!(!a.is_running(), "and finishes one interval later, not two");
    }

    #[test]
    fn fast_typing_never_accumulates_state() {
        // Five characters a second was the measured rate; the point of the
        // rewrite was that this must stay free.
        let mut a = OneShot::default();
        for _ in 0..500 {
            a.start();
            a.tick(200.0, PRESS_MS);
        }
        assert!(a.progress(PRESS_MS) <= 1.0);
        assert_eq!(std::mem::size_of::<OneShot>(), 8, "one per key, so it stays small");
    }

    #[test]
    fn a_resumed_suspend_does_not_leave_a_key_stuck_dim() {
        let mut a = OneShot::default();
        a.start();
        a.tick(4_000_000.0, PRESS_MS);
        assert!(!a.is_running());
        assert_eq!(press_opacity(&a), 1.0);
    }

    #[test]
    fn the_suggestion_bar_arrives_and_settles() {
        // Ease out rather than linear: it should stop softly rather than dead.
        let mut a = OneShot::default();
        a.start();
        assert!((slide_offset(&a, 40.0) - 40.0).abs() < 0.1, "starts off-screen");
        a.tick(SLIDE_MS / 2.0, SLIDE_MS);
        assert!(slide_offset(&a, 40.0) < 20.0, "most of the way by halfway");
        a.tick(SLIDE_MS, SLIDE_MS);
        assert_eq!(slide_offset(&a, 40.0), 0.0);
    }

    #[test]
    fn a_settled_animation_reports_its_resting_value_not_its_start() {
        // A caller drawing an idle key must get 1.0, not whatever the animation
        // began at — otherwise every key renders dim until first pressed.
        let a = OneShot::default();
        assert_eq!(press_opacity(&a), 1.0);
        assert_eq!(slide_offset(&a, 40.0), 0.0);
        assert_eq!(commit_opacity(&a), 1.0);
    }

    #[test]
    fn the_recording_pulse_stays_between_its_floor_and_full() {
        // TimedAnimation::new(0.5, 1.0, 800): it dims but never disappears,
        // because an indicator that vanishes reads as recording having stopped.
        let mut p = Pulse::default();
        p.start();
        for _ in 0..500 {
            p.tick(17.0);
            let o = p.opacity();
            assert!(o >= RECORD_FLOOR - 0.01 && o <= 1.01, "opacity {o}");
        }
    }

    #[test]
    fn the_pulse_wraps_rather_than_accumulating() {
        // An hour of recording would otherwise accumulate a float large enough
        // that adding a frame changes nothing, and the pulse would freeze.
        let mut p = Pulse::default();
        p.start();
        for _ in 0..200_000 { p.tick(17.0); }
        assert!(p.phase_ms < RECORD_MS * 2.0 + 1.0, "phase {}", p.phase_ms);
        let before = p.opacity();
        p.tick(17.0);
        assert_ne!(p.opacity(), before, "still moving after an hour");
    }

    #[test]
    fn the_pulse_rises_from_the_floor_as_the_original_animates_it() {
        // TimedAnimation::new(widget, 0.5, 1.0, 800) goes FROM 0.5 TO 1.0. I
        // asserted it started bright and it starts dim — the implementation
        // matched the original and the test did not. Worth pinning the
        // direction, because a pulse that starts bright and dims reads as
        // recording having just stopped rather than just begun.
        let mut p = Pulse::default();
        p.start();
        assert_eq!(p.opacity(), RECORD_FLOOR, "begins at the floor");
        p.tick(RECORD_MS);
        assert!((p.opacity() - 1.0).abs() < 0.01, "and peaks half a cycle later");
    }

    #[test]
    fn stopping_the_pulse_resets_it_to_a_known_point() {
        // Resuming mid-cycle makes two recordings look different for no reason.
        let mut p = Pulse::default();
        p.start();
        p.tick(RECORD_MS * 0.7);
        p.stop();
        assert_eq!(p.opacity(), 1.0, "not running is full opacity");
        p.start();
        assert_eq!(p.opacity(), RECORD_FLOOR, "and a fresh cycle, not the old phase");
    }

    #[test]
    fn a_negative_delta_does_not_run_anything_backwards() {
        let mut a = OneShot::default();
        a.start();
        a.tick(50.0, PRESS_MS);
        let at = a.progress(PRESS_MS);
        a.tick(-500.0, PRESS_MS);
        assert_eq!(a.progress(PRESS_MS), at);
    }

    #[test]
    fn a_zero_duration_does_not_divide_by_zero() {
        // Reachable if a caller passes a config value through.
        let mut a = OneShot::default();
        a.start();
        a.tick(1.0, 0.0);
        assert!(a.progress(0.0).is_finite());
    }
}
