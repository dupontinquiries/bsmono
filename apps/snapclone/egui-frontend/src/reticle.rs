//! reticle — `FocusReticle`, ported properly.
//!
//! `camera.rs` already had a focus point and a fade timer, which is the shape
//! of this and not the substance. The Qt widget has three STATES — searching,
//! locked, failed — and a reticle that fades on a timer regardless of outcome
//! never tells the user whether the camera actually focused. That is the whole
//! job: it is feedback, not decoration.
//!
//! ## Two rules in the header that are easy to drop
//!
//! **`triggerAt` is idempotent.** *"Repeated triggerAt before a sweep finishes
//! just moves the reticle without restarting the pulse phase."* Tapping around
//! the viewfinder while the camera hunts should slide the reticle, not make it
//! stutter — restarting the pulse on every tap turns a smooth pulse into a
//! flicker that reads as the app struggling.
//!
//! **A result with no sweep is ignored.** *"If the reticle isn't visible, this
//! is a no-op (we shouldn't be hearing about a focus result if we never
//! triggered a sweep)."* Autofocus results arrive from the camera whether or
//! not the user asked, and drawing a green reticle in the middle of the screen
//! for a focus nobody requested is a confusing flash.
//!
//! ## Locked and failed both linger, and that is deliberate
//!
//! 1.2 seconds before the fade starts. A failure that vanished as fast as it
//! appeared would be missed by exactly the person who needed to see it — the
//! one whose photo is about to be blurry.

/// How long a result stays fully visible before fading.
pub const LINGER_MS: f32 = 1_200.0;
/// The fade itself.
pub const FADE_MS: f32 = 400.0;
/// One pulse cycle while searching.
pub const PULSE_MS: f32 = 900.0;

/// What the autofocus is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusState {
    /// Sweep in progress — pulsing.
    Searching,
    /// Sweep completed.
    Locked,
    /// Sweep failed.
    Failed,
}

impl FocusState {
    pub fn is_result(self) -> bool { self != FocusState::Searching }
}

/// The reticle.
#[derive(Debug, Clone, PartialEq)]
pub struct Reticle {
    /// Centre, normalised 0..=1 so it survives a rotation or a resize.
    at: Option<(f32, f32)>,
    state: FocusState,
    /// Pulse phase, 0..1, looped while searching.
    phase: f32,
    /// Time since the result arrived. `None` while still searching.
    since_result_ms: Option<f32>,
}

impl Default for Reticle {
    fn default() -> Self {
        Reticle { at: None, state: FocusState::Searching, phase: 0.0,
                  since_result_ms: None }
    }
}

impl Reticle {
    pub fn at(&self) -> Option<(f32, f32)> { self.at }
    pub fn state(&self) -> FocusState { self.state }
    pub fn visible(&self) -> bool { self.at.is_some() && self.alpha() > 0.001 }

    /// Tap to focus here.
    ///
    /// Idempotent while a sweep is running: moves the reticle and leaves the
    /// pulse phase alone, so tapping around the viewfinder slides it rather
    /// than making it stutter.
    pub fn trigger_at(&mut self, x: f32, y: f32) {
        let fresh = self.at.is_none() || self.state.is_result();
        self.at = Some((x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)));
        if fresh {
            self.state = FocusState::Searching;
            self.phase = 0.0;
            self.since_result_ms = None;
        }
    }

    /// The camera reported a result.
    ///
    /// A no-op when nothing is showing: results arrive whether or not the user
    /// asked, and a green reticle for a focus nobody requested is a confusing
    /// flash in the middle of the frame.
    pub fn set_state(&mut self, s: FocusState) -> bool {
        if self.at.is_none() { return false; }
        if !s.is_result() { return false; }
        // A second result for the same sweep does not restart the linger — the
        // camera can report twice and the user should see one reticle, not a
        // reticle that refuses to leave.
        if self.state.is_result() { return false; }
        self.state = s;
        self.since_result_ms = Some(0.0);
        true
    }

    pub fn tick(&mut self, dt_ms: f32) {
        if self.at.is_none() { return; }
        match self.since_result_ms {
            None => {
                // Searching: pulse, indefinitely. There is no timeout here on
                // purpose — the camera decides when a sweep ends, and a reticle
                // that gave up on its own would disagree with the hardware.
                self.phase = (self.phase + dt_ms / PULSE_MS).rem_euclid(1.0);
            }
            Some(t) => {
                let t = t + dt_ms;
                self.since_result_ms = Some(t);
                if t >= LINGER_MS + FADE_MS { self.at = None; }
            }
        }
    }

    /// Opacity, 0..=1.
    pub fn alpha(&self) -> f32 {
        if self.at.is_none() { return 0.0; }
        match self.since_result_ms {
            None => 1.0,
            Some(t) if t <= LINGER_MS => 1.0,
            Some(t) => (1.0 - (t - LINGER_MS) / FADE_MS).clamp(0.0, 1.0),
        }
    }

    /// Pulse scale while searching, 1.0 once a result is in.
    ///
    /// A result state is STEADY — the Qt header says so for both locked and
    /// failed. A reticle still pulsing after the answer arrived reads as still
    /// hunting.
    pub fn pulse_scale(&self) -> f32 {
        if self.state.is_result() { return 1.0; }
        let t = self.phase * std::f32::consts::TAU;
        1.0 + 0.06 * t.sin()
    }

    /// Clear without a result — the camera stopped, the page changed.
    pub fn clear(&mut self) { *self = Reticle::default(); }
}

/// Draw the reticle over a viewfinder rect.
pub fn reticle_ui(ui: &mut egui::Ui, r: &Reticle, view: egui::Rect,
                  c: &uikit_egui::PaletteColors) {
    let Some((nx, ny)) = r.at() else { return };
    let centre = egui::pos2(view.left() + nx * view.width(),
                            view.top() + ny * view.height());
    reticle_ui_at(ui, r, centre, c);
}

/// The reticle at a screen point the caller mapped — the viewfinder's
/// `to_screen`, which knows the mirror, the turn and the zoom window that
/// `reticle_ui`'s fraction-of-the-view placement does not.
pub fn reticle_ui_at(ui: &mut egui::Ui, r: &Reticle, centre: egui::Pos2,
                     c: &uikit_egui::PaletteColors) {
    if r.at().is_none() { return; }
    let a = r.alpha();
    if a <= 0.001 { return; }
    let size = 34.0 * r.pulse_scale();
    // Colour carries the state: the shape alone cannot say whether focus
    // succeeded, and that is the one thing the user needs from it.
    let col = match r.state() {
        FocusState::Searching => c.accent,
        FocusState::Locked => c.success,
        FocusState::Failed => c.danger,
    };
    let col = egui::Color32::from_rgba_unmultiplied(
        col.r(), col.g(), col.b(), (a * 255.0) as u8);
    ui.painter().rect_stroke(egui::Rect::from_center_size(centre, egui::vec2(size, size)),
                             4.0, egui::Stroke::new(2.0_f32, col), egui::StrokeKind::Middle);
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_viewfinder_draws_this_reticle_and_not_a_timer() {
        // The changeover was started and left one call short: Viewfinder held
        // a Reticle, tap() called trigger_at, tick() advanced it — and the
        // paint still used a fixed-timer fade, so this module never reached a
        // screen. The old path is gone; this fails if it comes back.
        let cam = include_str!("camera.rs");
        assert!(cam.contains("reticle::reticle_ui_at("), "the reticle is not drawn");
        assert!(!cam.contains("let a = vf.reticle_alpha();"),
                "the timer-based fade is back alongside the reticle");
    }

    #[test]
    fn a_result_outlives_a_search() {
        // Why the timer version was not good enough: it faded on a schedule
        // whatever the camera did, so it could not tell "focused" from "gave
        // up". A result must linger long enough to read.
        let mut searching = Reticle::default();
        searching.trigger_at(0.5, 0.5);
        let mut locked = Reticle::default();
        locked.trigger_at(0.5, 0.5);
        locked.set_state(FocusState::Locked);

        // Advance both by the same amount and the settled one must still be
        // showing when the searching one has faded further.
        for _ in 0..12 {
            searching.tick(100.0);
            locked.tick(100.0);
        }
        assert!(locked.state().is_result());
        assert!(!searching.state().is_result());
        assert!(locked.alpha() >= searching.alpha(),
                "a result faded faster than an unresolved search: {} vs {}",
                locked.alpha(), searching.alpha());
    }

    use super::*;

    fn triggered() -> Reticle {
        let mut r = Reticle::default();
        r.trigger_at(0.5, 0.5);
        r
    }

    #[test]
    fn a_tap_starts_a_searching_reticle() {
        let r = triggered();
        assert_eq!(r.state(), FocusState::Searching);
        assert!(r.visible());
        assert_eq!(r.at(), Some((0.5, 0.5)));
    }

    #[test]
    fn the_position_is_normalised_so_it_survives_a_rotation() {
        let mut r = Reticle::default();
        r.trigger_at(1.4, -0.2);
        assert_eq!(r.at(), Some((1.0, 0.0)));
    }

    #[test]
    fn retapping_during_a_sweep_moves_it_without_restarting_the_pulse() {
        // The Qt header's own word: idempotent. Restarting the pulse on every
        // tap turns a smooth pulse into a flicker that reads as the app
        // struggling.
        let mut r = triggered();
        r.tick(PULSE_MS * 0.4);
        let phase_before = r.pulse_scale();
        r.trigger_at(0.2, 0.8);
        assert_eq!(r.at(), Some((0.2, 0.8)), "it moved");
        assert_eq!(r.pulse_scale(), phase_before, "and the pulse did not restart");
    }

    #[test]
    fn a_result_with_no_sweep_is_ignored() {
        // Autofocus results arrive whether or not the user asked, and a green
        // reticle in the middle of the frame for a focus nobody requested is a
        // confusing flash.
        let mut r = Reticle::default();
        assert!(!r.set_state(FocusState::Locked));
        assert!(!r.visible());
        assert_eq!(r.at(), None);
    }

    #[test]
    fn a_second_result_does_not_restart_the_linger() {
        // The camera can report twice; the user should see one reticle rather
        // than one that refuses to leave.
        let mut r = triggered();
        assert!(r.set_state(FocusState::Locked));
        r.tick(LINGER_MS * 0.8);
        assert!(!r.set_state(FocusState::Failed), "the second is refused");
        assert_eq!(r.state(), FocusState::Locked);
    }

    #[test]
    fn setting_searching_as_a_result_is_refused() {
        // Only Locked and Failed are results; "still searching" is not news.
        let mut r = triggered();
        assert!(!r.set_state(FocusState::Searching));
        assert!(r.state() == FocusState::Searching);
    }

    #[test]
    fn a_result_lingers_before_it_fades() {
        // A failure that vanished as fast as it appeared would be missed by
        // exactly the person who needed to see it.
        let mut r = triggered();
        r.set_state(FocusState::Failed);
        r.tick(LINGER_MS - 1.0);
        assert_eq!(r.alpha(), 1.0, "still fully visible");
        r.tick(FADE_MS * 0.5 + 1.0);
        let mid = r.alpha();
        assert!(mid > 0.0 && mid < 1.0, "fading, at {mid}");
    }

    #[test]
    fn the_reticle_eventually_goes_away() {
        let mut r = triggered();
        r.set_state(FocusState::Locked);
        r.tick(LINGER_MS + FADE_MS + 1.0);
        assert!(!r.visible());
        assert_eq!(r.at(), None);
    }

    #[test]
    fn searching_never_times_out_on_its_own() {
        // The camera decides when a sweep ends. A reticle that gave up by
        // itself would disagree with the hardware and disappear while the lens
        // is still moving.
        let mut r = triggered();
        r.tick(30_000.0);
        assert!(r.visible());
        assert_eq!(r.state(), FocusState::Searching);
    }

    #[test]
    fn a_result_state_is_steady_rather_than_pulsing() {
        // Still pulsing after the answer arrived reads as still hunting.
        let mut r = triggered();
        r.tick(PULSE_MS * 0.3);
        assert_ne!(r.pulse_scale(), 1.0, "searching pulses");
        r.set_state(FocusState::Locked);
        assert_eq!(r.pulse_scale(), 1.0);
    }

    #[test]
    fn tapping_after_a_result_starts_a_fresh_sweep() {
        // The idempotence rule applies DURING a sweep, not after one — a new
        // tap is a new request.
        let mut r = triggered();
        r.set_state(FocusState::Locked);
        r.tick(LINGER_MS * 0.5);
        r.trigger_at(0.1, 0.1);
        assert_eq!(r.state(), FocusState::Searching);
        assert_eq!(r.alpha(), 1.0);
    }

    #[test]
    fn clearing_removes_it_without_a_result() {
        // The camera stopped, or the page changed. Neither is a focus outcome.
        let mut r = triggered();
        r.clear();
        assert!(!r.visible());
        assert_eq!(r.at(), None);
    }

    #[test]
    fn the_three_states_are_distinguishable() {
        // Colour carries the answer; the shape alone cannot say whether focus
        // succeeded, which is the one thing the user needs from it.
        assert!(!FocusState::Searching.is_result());
        assert!(FocusState::Locked.is_result());
        assert!(FocusState::Failed.is_result());
        assert_ne!(FocusState::Locked, FocusState::Failed);
    }
}
