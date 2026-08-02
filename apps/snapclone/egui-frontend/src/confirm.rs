//! confirm — confirmation friction that decreases as you learn a feature.
//!
//! Ported from `ChevronConfirm` and `UxGuide::familiarityFor`. The panel slides
//! in anchored to the button that was pressed and auto-dismisses after 4
//! seconds, and **which** confirmation you get depends on how familiar you are
//! with that specific feature:
//!
//! ```text
//!   < 40   TapConfirm   — a panel with Cancel/Confirm; an explicit tap
//!   40-80  HoldConfirm  — press and hold 1.2s with a progress ring
//!   >= 80  NoConfirm    — proceeds immediately, with a haptic pulse
//! ```
//!
//! ## Why per-feature and not per-user
//!
//! A user who has exported a vCard two hundred times is still a novice at
//! deleting a conversation. One global "expert" flag removes the guard from
//! everything the first time someone gets good at anything, which is exactly
//! when a destructive action they have never performed is most dangerous.
//!
//! ## Why failures count
//!
//! `failCount` exists so a feature someone keeps getting wrong does not graduate
//! on use count alone. Twenty uses and ten cancellations is not familiarity —
//! it is a feature that is confusing them, and dropping the confirmation there
//! would be exactly backwards.

/// The three tiers. Boundaries are the header's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// New: an explicit tap on Confirm.
    Tap,
    /// Intermediate: press and hold.
    Hold,
    /// Expert: proceeds immediately.
    None,
}

/// Hold duration for `Mode::Hold`, from the header.
pub const HOLD_SECS: f32 = 1.2;
/// Auto-dismiss, from the header.
pub const DISMISS_SECS: f32 = 4.0;

/// What is known about one feature.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Familiarity {
    pub score: i32,
    pub use_count: u32,
    pub fail_count: u32,
}

impl Familiarity {
    pub fn mode(&self) -> Mode {
        // Clamped, because a score outside 0..100 is a storage bug and the
        // dangerous direction is the one that removes the guard.
        match self.score.clamp(0, 100) {
            s if s >= 80 => Mode::None,
            s if s >= 40 => Mode::Hold,
            _ => Mode::Tap,
        }
    }

    /// Record a completion.
    pub fn record_use(&mut self) {
        self.use_count += 1;
        self.score = (self.score + 6).min(100);
    }

    /// Record a cancellation or failure.
    ///
    /// Costs more than a use gains. A feature someone keeps backing out of is
    /// one they are unsure about, and letting twenty uses with ten cancellations
    /// graduate to NoConfirm would drop the guard precisely where it is needed.
    pub fn record_fail(&mut self) {
        self.fail_count += 1;
        self.score = (self.score - 10).max(0);
    }
}

/// A confirmation in progress.
#[derive(Debug, Clone)]
pub struct Confirm {
    pub feature: String,
    pub label: String,
    pub mode: Mode,
    held: f32,
    age: f32,
    holding: bool,
}

/// What the confirmation produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Confirmed,
    Cancelled,
    /// Still waiting.
    Pending,
}

impl Confirm {
    /// Begin. Returns `None` for an expert, who is not asked at all — the
    /// caller performs the action immediately.
    pub fn begin(feature: &str, label: &str, f: &Familiarity) -> Option<Confirm> {
        if f.mode() == Mode::None { return None; }
        Some(Confirm { feature: feature.into(), label: label.into(), mode: f.mode(),
                       held: 0.0, age: 0.0, holding: false })
    }

    pub fn press(&mut self) { self.holding = true; }

    /// Release. In Hold mode a short release is a cancel, not a confirm.
    pub fn release(&mut self) -> Outcome {
        self.holding = false;
        match self.mode {
            // A tap on Confirm IS the confirmation.
            Mode::Tap => Outcome::Confirmed,
            Mode::Hold if self.held >= HOLD_SECS => Outcome::Confirmed,
            // Letting go early is the user changing their mind mid-gesture,
            // which is the whole reason hold-to-confirm is safer than a tap.
            Mode::Hold => { self.held = 0.0; Outcome::Cancelled }
            Mode::None => Outcome::Confirmed,
        }
    }

    /// Advance. Returns `Cancelled` when the panel times out.
    pub fn tick(&mut self, dt: f32) -> Outcome {
        let dt = dt.max(0.0);
        self.age += dt;
        if self.holding { self.held += dt; }
        // Timing out is a CANCEL. A confirmation that completes itself after
        // four seconds of no input is not a confirmation.
        if self.age >= DISMISS_SECS { return Outcome::Cancelled; }
        Outcome::Pending
    }

    /// Hold progress 0..1, for the ring.
    pub fn progress(&self) -> f32 {
        if self.mode != Mode::Hold { return 0.0; }
        (self.held / HOLD_SECS).clamp(0.0, 1.0)
    }

    pub fn cancel(&mut self) -> Outcome { Outcome::Cancelled }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(score: i32) -> Familiarity { Familiarity { score, ..Default::default() } }

    #[test]
    fn the_tiers_are_the_headers() {
        assert_eq!(f(0).mode(), Mode::Tap);
        assert_eq!(f(39).mode(), Mode::Tap);
        assert_eq!(f(40).mode(), Mode::Hold);
        assert_eq!(f(79).mode(), Mode::Hold);
        assert_eq!(f(80).mode(), Mode::None);
    }

    #[test]
    fn an_expert_is_not_asked_at_all() {
        assert!(Confirm::begin("export", "Export all contacts?", &f(90)).is_none());
        assert!(Confirm::begin("export", "Export all contacts?", &f(79)).is_some());
    }

    #[test]
    fn familiarity_is_per_feature_not_per_user() {
        // Someone who has exported two hundred vCards is still a novice at
        // deleting a conversation — and that is exactly when a destructive
        // action they have never performed is most dangerous.
        let mut export = Familiarity::default();
        for _ in 0..20 { export.record_use(); }
        assert_eq!(export.mode(), Mode::None);
        let delete = Familiarity::default();
        assert_eq!(delete.mode(), Mode::Tap, "a different feature is still new");
    }

    #[test]
    fn cancellations_hold_a_feature_back() {
        // Twenty uses and ten cancellations is not familiarity — it is a
        // feature that is confusing someone, and dropping the guard there is
        // exactly backwards.
        let mut a = Familiarity::default();
        for _ in 0..20 { a.record_use(); }
        let mut b = Familiarity::default();
        for _ in 0..20 { b.record_use(); b.record_fail(); if b.fail_count >= 10 { break } }
        assert_eq!(a.mode(), Mode::None);
        assert_ne!(b.mode(), Mode::None, "score {}", b.score);
    }

    #[test]
    fn a_score_can_never_leave_its_range() {
        // Out of range is a storage bug, and the dangerous direction is the one
        // that removes the guard.
        let mut a = Familiarity::default();
        for _ in 0..100 { a.record_use(); }
        assert!(a.score <= 100);
        for _ in 0..100 { a.record_fail(); }
        assert!(a.score >= 0);
        assert_eq!(a.mode(), Mode::Tap, "back to the strictest guard");
        assert_eq!(Familiarity { score: 9999, ..Default::default() }.mode(), Mode::None);
        assert_eq!(Familiarity { score: -50, ..Default::default() }.mode(), Mode::Tap);
    }

    #[test]
    fn a_short_hold_cancels_rather_than_confirms() {
        // Letting go early is changing your mind mid-gesture, which is the whole
        // reason hold-to-confirm is safer than a tap.
        let mut c = Confirm::begin("delete", "Delete?", &f(50)).unwrap();
        c.press();
        c.tick(0.3);
        assert_eq!(c.release(), Outcome::Cancelled);
        assert_eq!(c.progress(), 0.0, "and the ring resets");
    }

    #[test]
    fn a_full_hold_confirms() {
        let mut c = Confirm::begin("delete", "Delete?", &f(50)).unwrap();
        c.press();
        c.tick(HOLD_SECS + 0.05);
        assert!(c.progress() >= 1.0);
        assert_eq!(c.release(), Outcome::Confirmed);
    }

    #[test]
    fn a_timeout_cancels_rather_than_completing() {
        // A confirmation that completes itself after four seconds of no input
        // is not a confirmation.
        let mut c = Confirm::begin("delete", "Delete?", &f(10)).unwrap();
        assert_eq!(c.tick(1.0), Outcome::Pending);
        assert_eq!(c.tick(DISMISS_SECS), Outcome::Cancelled);
    }

    #[test]
    fn a_tap_tier_confirms_on_release_because_the_tap_is_the_answer() {
        let mut c = Confirm::begin("delete", "Delete?", &f(10)).unwrap();
        assert_eq!(c.mode, Mode::Tap);
        c.press();
        assert_eq!(c.release(), Outcome::Confirmed);
    }
}
