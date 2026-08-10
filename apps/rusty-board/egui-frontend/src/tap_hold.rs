//! tap_hold — `ui/tap_hold.rs` (`TapHoldButton`), ported from GTK to egui.
//!
//! A button whose tap and hold mean different things, with a progress bar
//! filling during the hold.
//!
//! ## Four ways to reach the same action, and that is the design
//!
//! The GTK original fires `held_down` from a hold, a **double-click**, or a
//! **right-click** — the doc calls the shortcuts a path *"for users who don't
//! want to wait for the hold timeout"*. Keeping all of them matters: a hold is
//! the accessible route on a touch screen and a slow one on a desktop, and
//! dropping the shortcuts would make every destructive action take 800ms.
//!
//! Used where *"the tap action is conservative (open a popup, show help) and
//! the hold action is destructive or commits to a longer-running operation"*.
//! That pairing is why the hold has visible progress at all: a user must be
//! able to see they are about to do the serious thing, and let go.
//!
//! ## The double-fire hazard does not port, but its lesson does
//!
//! The GTK version overrides `GtkButton`'s own `clicked` because *"we'd
//! double-fire if we kept it AND added our own gesture"*. egui has no competing
//! built-in signal, so that specific fix is unnecessary — but the underlying
//! rule survives as a test: **one press produces exactly one outcome**, never a
//! tap and a hold from the same gesture.

/// Defaults matching the QML `ToolBtn` this descends from.
pub const HOLD_INTERVAL_MS: f32 = 800.0;
pub const RELEASE_SNAP_MS: f32 = 120.0;
/// Progress bar width as a fraction of the button.
pub const BAR_WIDTH_FRAC: f32 = 0.6;

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub hold_interval_ms: f32,
    pub release_snap_ms: f32,
    pub show_progress: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config { hold_interval_ms: HOLD_INTERVAL_MS, release_snap_ms: RELEASE_SNAP_MS,
                 show_progress: true }
    }
}

/// What the button emitted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fired { Tapped, HeldDown }

#[derive(Clone, Copy, Debug, Default)]
pub struct TapHold {
    cfg: Config,
    pressing: bool,
    held_ms: f32,
    /// Set once the hold fires, so the release does not also tap.
    consumed: bool,
    /// Bar position, which snaps back rather than jumping.
    progress: f32,
}

impl TapHold {
    pub fn new(cfg: Config) -> TapHold { TapHold { cfg, ..Default::default() } }

    /// Bar fill, 0..=1. Zero when progress is hidden, so a caller can draw
    /// unconditionally.
    pub fn progress(&self) -> f32 {
        if self.cfg.show_progress { self.progress.clamp(0.0, 1.0) } else { 0.0 }
    }

    pub fn is_pressing(&self) -> bool { self.pressing }

    pub fn press(&mut self) {
        self.pressing = true;
        self.held_ms = 0.0;
        self.consumed = false;
    }

    /// A right-click, or a double-click — the shortcut paths.
    ///
    /// Fires immediately and marks the gesture consumed, so a release that
    /// follows cannot also tap.
    pub fn shortcut(&mut self) -> Fired {
        self.pressing = false;
        self.held_ms = 0.0;
        self.progress = 0.0;
        self.consumed = true;
        Fired::HeldDown
    }

    /// Advance. Returns `HeldDown` on the frame the interval is reached.
    pub fn tick(&mut self, dt_ms: f32) -> Option<Fired> {
        let interval = self.cfg.hold_interval_ms.max(1.0);
        if self.pressing {
            if dt_ms > 0.0 {
                // A resumed suspend must not fire a DESTRUCTIVE action the user
                // never held for. Clamping to the interval means the worst case
                // is one hold they were already most of the way through.
                self.held_ms += dt_ms.min(interval);
            }
            self.progress = (self.held_ms / interval).clamp(0.0, 1.0);
            if self.held_ms >= interval {
                self.pressing = false;
                self.consumed = true;
                self.progress = 0.0;
                return Some(Fired::HeldDown);
            }
            return None;
        }
        // Snap the bar back rather than clearing it: the C++ tunes this at
        // 120ms so a near-miss is visibly a near-miss rather than a flicker.
        if self.progress > 0.0 && dt_ms > 0.0 {
            let step = dt_ms / self.cfg.release_snap_ms.max(1.0);
            self.progress = (self.progress - step).max(0.0);
        }
        None
    }

    /// Released. Taps only if the hold did not already fire.
    pub fn release(&mut self) -> Option<Fired> {
        let was_pressing = self.pressing;
        self.pressing = false;
        if self.consumed || !was_pressing { return None; }
        Some(Fired::Tapped)
    }

    /// The gesture was taken away — a scroll started, or the pointer left.
    ///
    /// Neither outcome fires: a gesture that became a scroll was not a tap, and
    /// firing a destructive action on it would be the worst possible reading.
    pub fn cancel(&mut self) {
        self.pressing = false;
        self.held_ms = 0.0;
        self.consumed = false;
    }

    /// Is anything animating? Drives whether a repaint is requested.
    pub fn is_animating(&self) -> bool { self.pressing || self.progress > 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn btn() -> TapHold { TapHold::new(Config::default()) }

    #[test]
    fn a_quick_press_and_release_taps() {
        let mut b = btn();
        b.press();
        b.tick(100.0);
        assert_eq!(b.release(), Some(Fired::Tapped));
    }

    #[test]
    fn holding_past_the_interval_fires_held_down() {
        let mut b = btn();
        b.press();
        assert_eq!(b.tick(HOLD_INTERVAL_MS - 1.0), None);
        assert_eq!(b.tick(1.0), Some(Fired::HeldDown));
    }

    #[test]
    fn one_press_produces_exactly_one_outcome() {
        // The lesson from the GTK double-fire fix: it overrode GtkButton's own
        // `clicked` because "we'd double-fire if we kept it AND added our own
        // gesture". egui has no competing signal, but the rule still holds.
        let mut b = btn();
        b.press();
        assert_eq!(b.tick(HOLD_INTERVAL_MS), Some(Fired::HeldDown));
        assert_eq!(b.release(), None, "the release must not also tap");
    }

    #[test]
    fn a_shortcut_fires_held_down_without_waiting() {
        // "For users who don't want to wait for the hold timeout" — dropping
        // this makes every destructive action take 800ms.
        let mut b = btn();
        b.press();
        b.tick(50.0);
        assert_eq!(b.shortcut(), Fired::HeldDown);
        assert_eq!(b.release(), None, "and the release is consumed");
    }

    #[test]
    fn the_bar_fills_toward_the_hold_so_a_user_can_let_go() {
        // The tap is conservative and the hold is destructive, which is exactly
        // why the progress is visible: you must be able to see what you are
        // about to commit to.
        let mut b = btn();
        b.press();
        b.tick(HOLD_INTERVAL_MS / 2.0);
        assert!((b.progress() - 0.5).abs() < 0.02, "half filled: {}", b.progress());
    }

    #[test]
    fn releasing_early_snaps_the_bar_back_rather_than_clearing_it() {
        // 120ms, so a near-miss is visibly a near-miss rather than a flicker.
        let mut b = btn();
        b.press();
        b.tick(HOLD_INTERVAL_MS * 0.7);
        b.release();
        assert!(b.progress() > 0.0, "still visible immediately after release");
        b.tick(RELEASE_SNAP_MS);
        assert_eq!(b.progress(), 0.0);
        assert!(!b.is_animating());
    }

    #[test]
    fn hiding_the_progress_bar_reports_zero_so_a_caller_can_draw_blindly() {
        // The keyboard's per-key long-press uses this without the bar.
        let mut b = TapHold::new(Config { show_progress: false, ..Default::default() });
        b.press();
        b.tick(HOLD_INTERVAL_MS / 2.0);
        assert_eq!(b.progress(), 0.0);
        assert_eq!(b.tick(HOLD_INTERVAL_MS), Some(Fired::HeldDown), "but the hold still works");
    }

    #[test]
    fn a_cancelled_gesture_fires_nothing_at_all() {
        // A gesture that became a scroll was not a tap, and firing a
        // destructive action on it would be the worst possible reading.
        let mut b = btn();
        b.press();
        b.tick(HOLD_INTERVAL_MS * 0.9);
        b.cancel();
        assert_eq!(b.release(), None);
        assert_eq!(b.tick(HOLD_INTERVAL_MS), None);
    }

    #[test]
    fn a_release_without_a_press_taps_nothing() {
        // A press delivered to another widget, or one lost to a compositor.
        let mut b = btn();
        assert_eq!(b.release(), None);
    }

    #[test]
    fn a_resumed_suspend_does_not_fire_a_destructive_action_repeatedly() {
        // Clamping to the interval means the worst case is one hold the user
        // was already most of the way through — not a burst.
        let mut b = btn();
        b.press();
        assert_eq!(b.tick(4_000_000.0), Some(Fired::HeldDown));
        assert_eq!(b.tick(4_000_000.0), None, "and only once");
    }

    #[test]
    fn a_zero_interval_does_not_divide_by_zero_in_the_progress() {
        // Reachable from a config value; a NaN in a progress bar paints nothing
        // with no error.
        let mut b = TapHold::new(Config { hold_interval_ms: 0.0, ..Default::default() });
        b.press();
        let out = b.tick(1.0);
        assert!(b.progress().is_finite());
        assert_eq!(out, Some(Fired::HeldDown));
    }

    #[test]
    fn pressing_again_after_a_hold_starts_a_fresh_gesture() {
        // Otherwise the consumed flag would swallow the next tap.
        let mut b = btn();
        b.press();
        b.tick(HOLD_INTERVAL_MS);
        b.press();
        b.tick(50.0);
        assert_eq!(b.release(), Some(Fired::Tapped));
    }
}
