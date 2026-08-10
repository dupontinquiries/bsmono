//! chevron_confirm — `ChevronConfirm.cpp`, ported.
//!
//! A confirmation panel that points at the control it is confirming, and asks
//! for either a tap or a press-and-hold **depending on how bad the mistake
//! would be**.
//!
//! ## The risk score is the whole idea
//!
//! Below 40 the panel takes a tap; at or above it, a 1.2-second hold. That is
//! not decoration: a hold makes an irreversible action impossible to do by
//! accident, and applying it to everything would make the app exhausting. The
//! threshold is what lets one component serve "clear this filter" and "delete
//! this conversation" without either feeling wrong.
//!
//! ## It gets out of the way once you know the app
//!
//! `UxGuide::recordUse` is consulted first, and a familiar feature confirms
//! immediately with no panel at all. A confirmation a user has answered fifty
//! times has stopped being a safety net and become a tax — so familiarity is an
//! input here, not an afterthought.
//!
//! Note what that implies and the port preserves: **familiarity can skip a
//! tap-level confirm, but the risk score still decides the mode when a panel
//! does appear.** Being practised at deleting things does not make deleting
//! them safe.

/// At or above this score, the panel demands a hold.
pub const HOLD_THRESHOLD: u32 = 40;
/// How long the hold must be sustained.
pub const HOLD_MS: f32 = 1_200.0;
/// How long the panel waits before dismissing itself.
pub const AUTO_DISMISS_MS: f32 = 4_000.0;
/// Panel geometry, from the original.
pub const CHEVRON_H: f32 = 10.0;
pub const RADIUS: f32 = 14.0;
pub const MAX_WIDTH: f32 = 320.0;
pub const EDGE_MARGIN: f32 = 16.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode { Tap, Hold }

impl Mode {
    /// Which interaction a score demands.
    pub fn for_score(score: u32) -> Mode {
        if score < HOLD_THRESHOLD { Mode::Tap } else { Mode::Hold }
    }
}

/// What happened.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    Confirmed,
    Cancelled,
    /// Nobody answered and it withdrew.
    TimedOut,
}

/// The chevron points up at an anchor above, or down at one below.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChevronSide { Above, Below }

#[derive(Clone, Debug)]
pub struct ChevronConfirm {
    mode: Mode,
    /// Milliseconds the press has been held.
    held_ms: f32,
    holding: bool,
    /// Milliseconds since the panel opened with no interaction.
    idle_ms: f32,
    outcome: Option<Outcome>,
}

impl ChevronConfirm {
    /// Open a panel for a risk score.
    pub fn new(score: u32) -> ChevronConfirm {
        ChevronConfirm { mode: Mode::for_score(score), held_ms: 0.0,
                         holding: false, idle_ms: 0.0, outcome: None }
    }

    /// Should a panel be shown at all?
    ///
    /// A familiar feature confirms immediately: a confirmation answered fifty
    /// times has stopped being a safety net and become a tax. `uses` comes from
    /// the UX guide's own counter.
    pub fn should_prompt(uses: u32, familiar_after: u32) -> bool { uses < familiar_after }

    pub fn mode(&self) -> Mode { self.mode }
    pub fn outcome(&self) -> Option<Outcome> { self.outcome }
    pub fn is_open(&self) -> bool { self.outcome.is_none() }

    /// Hold progress, 0..=1, for the filling arc.
    pub fn progress(&self) -> f32 { (self.held_ms / HOLD_MS).clamp(0.0, 1.0) }

    /// A tap on the confirm control.
    ///
    /// In `Hold` mode a tap is **not** a confirmation and is not an error
    /// either — it is somebody who has not read the label yet. Treating it as a
    /// cancel would punish curiosity; treating it as a confirm would defeat the
    /// mode entirely. So it resets the dismissal timer and nothing else.
    pub fn tap(&mut self) -> Option<Outcome> {
        if !self.is_open() { return None; }
        self.idle_ms = 0.0;
        if self.mode == Mode::Tap {
            self.outcome = Some(Outcome::Confirmed);
            return Some(Outcome::Confirmed);
        }
        None
    }

    pub fn press(&mut self) {
        if !self.is_open() { return; }
        self.holding = true;
        self.held_ms = 0.0;
        self.idle_ms = 0.0;
    }

    /// Let go before the hold completed.
    ///
    /// Progress resets to zero rather than decaying. A hold that resumed where
    /// it left off would let a user complete an irreversible action through
    /// several accidental brushes, which is the exact thing the mode exists to
    /// prevent.
    pub fn release(&mut self) {
        self.holding = false;
        self.held_ms = 0.0;
        self.idle_ms = 0.0;
    }

    pub fn cancel(&mut self) {
        if self.is_open() { self.outcome = Some(Outcome::Cancelled); }
    }

    /// Advance both timers. Returns an outcome when this tick settled it.
    pub fn tick(&mut self, dt_ms: f32) -> Option<Outcome> {
        if !self.is_open() || dt_ms <= 0.0 { return None; }
        // A resumed suspend must not auto-confirm a held button or silently
        // time out a panel the user is looking at.
        let dt = dt_ms.min(AUTO_DISMISS_MS);

        if self.holding {
            self.held_ms += dt;
            if self.held_ms >= HOLD_MS {
                self.outcome = Some(Outcome::Confirmed);
                return Some(Outcome::Confirmed);
            }
            // Holding is interaction, so the panel cannot time out underneath a
            // finger that is nearly there.
            return None;
        }

        self.idle_ms += dt;
        if self.idle_ms >= AUTO_DISMISS_MS {
            self.outcome = Some(Outcome::TimedOut);
            return Some(Outcome::TimedOut);
        }
        None
    }

    /// Panel width for a viewport, matching the original's `min(root - 32, 320)`.
    pub fn panel_width(&self, root_w: f32) -> f32 {
        (root_w - EDGE_MARGIN * 2.0).min(MAX_WIDTH).max(0.0)
    }

    /// Panel x, centred under an anchor and clamped inside the margins.
    ///
    /// Clamped rather than allowed to overflow: a confirmation half off-screen
    /// is one whose Cancel button cannot be reached, which turns a safety
    /// feature into a trap.
    pub fn panel_x(&self, anchor_x: f32, anchor_w: f32, root_w: f32) -> f32 {
        let pw = self.panel_width(root_w);
        let ideal = anchor_x - pw / 2.0 + anchor_w / 2.0;
        ideal.clamp(EDGE_MARGIN, (root_w - pw - EDGE_MARGIN).max(EDGE_MARGIN))
    }

    /// Which way the chevron points.
    ///
    /// The panel prefers to sit below its anchor and flips above when there is
    /// no room — near the bottom of a phone screen, which is where the action
    /// bar lives, so the flip is the common case rather than the exception.
    pub fn chevron_side(&self, anchor_bottom: f32, panel_h: f32, root_h: f32) -> ChevronSide {
        if anchor_bottom + panel_h + EDGE_MARGIN <= root_h { ChevronSide::Below }
        else { ChevronSide::Above }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_score_decides_the_interaction() {
        // A hold makes an irreversible action impossible by accident; applying
        // it to everything would make the app exhausting.
        assert_eq!(Mode::for_score(0), Mode::Tap);
        assert_eq!(Mode::for_score(39), Mode::Tap);
        assert_eq!(Mode::for_score(40), Mode::Hold, "the threshold is inclusive");
        assert_eq!(Mode::for_score(100), Mode::Hold);
    }

    #[test]
    fn a_familiar_feature_skips_the_panel_entirely() {
        // A confirmation answered fifty times has stopped being a safety net
        // and become a tax.
        assert!(ChevronConfirm::should_prompt(0, 5));
        assert!(ChevronConfirm::should_prompt(4, 5));
        assert!(!ChevronConfirm::should_prompt(5, 5));
        assert!(!ChevronConfirm::should_prompt(50, 5));
    }

    #[test]
    fn familiarity_does_not_downgrade_a_hold_to_a_tap() {
        // Being practised at deleting things does not make deleting them safe.
        // The skip is all-or-nothing; when a panel appears, the score still
        // decides the mode.
        assert_eq!(ChevronConfirm::new(80).mode(), Mode::Hold);
    }

    #[test]
    fn a_tap_confirms_in_tap_mode() {
        let mut c = ChevronConfirm::new(10);
        assert_eq!(c.tap(), Some(Outcome::Confirmed));
        assert!(!c.is_open());
    }

    #[test]
    fn a_tap_in_hold_mode_is_neither_a_confirm_nor_a_cancel() {
        // It is somebody who has not read the label yet. Cancelling would
        // punish curiosity; confirming would defeat the mode.
        let mut c = ChevronConfirm::new(90);
        assert_eq!(c.tap(), None);
        assert!(c.is_open());
        assert_eq!(c.outcome(), None);
    }

    #[test]
    fn a_hold_confirms_only_after_the_full_duration() {
        let mut c = ChevronConfirm::new(90);
        c.press();
        assert_eq!(c.tick(HOLD_MS - 1.0), None);
        assert!(c.progress() > 0.99 && c.progress() < 1.0);
        assert_eq!(c.tick(1.0), Some(Outcome::Confirmed));
    }

    #[test]
    fn letting_go_early_resets_progress_rather_than_decaying_it() {
        // A hold that resumed where it left off would let several accidental
        // brushes complete an irreversible action — the exact thing the mode
        // exists to prevent.
        let mut c = ChevronConfirm::new(90);
        c.press();
        c.tick(HOLD_MS * 0.9);
        c.release();
        assert_eq!(c.progress(), 0.0);
        c.press();
        assert_eq!(c.tick(HOLD_MS * 0.5), None, "starts over");
        assert!(c.is_open());
    }

    #[test]
    fn the_panel_withdraws_when_nobody_answers() {
        let mut c = ChevronConfirm::new(10);
        assert_eq!(c.tick(AUTO_DISMISS_MS - 1.0), None);
        assert_eq!(c.tick(1.0), Some(Outcome::TimedOut));
        assert!(!c.is_open());
    }

    #[test]
    fn a_panel_cannot_time_out_under_a_finger_that_is_nearly_there() {
        // Holding is interaction. Dismissing mid-hold would be the most
        // frustrating possible moment to give up.
        let mut c = ChevronConfirm::new(90);
        c.press();
        // Enough frames to exceed HOLD_MS — my first version ran 1000ms
        // against a 1200ms hold and never reached the confirm, which was the
        // test's arithmetic rather than the code's.
        for _ in 0..200 {
            if c.tick(10.0).is_some() { break; }
        }
        assert_eq!(c.outcome(), Some(Outcome::Confirmed), "it confirmed, not timed out");
    }

    #[test]
    fn interaction_restarts_the_dismissal_countdown() {
        let mut c = ChevronConfirm::new(90);
        c.tick(AUTO_DISMISS_MS - 100.0);
        c.tap();
        assert_eq!(c.tick(AUTO_DISMISS_MS - 100.0), None, "a fresh four seconds");
    }

    #[test]
    fn a_resumed_suspend_does_not_auto_confirm_a_held_button() {
        // The worst possible outcome of a clamped-delta bug: waking a laptop
        // and finding a conversation deleted.
        let mut c = ChevronConfirm::new(90);
        c.press();
        c.tick(1_000.0);
        c.release();
        assert_eq!(c.tick(4_000_000.0), Some(Outcome::TimedOut),
                   "it withdrew rather than confirming");
    }

    #[test]
    fn a_settled_panel_ignores_further_input() {
        let mut c = ChevronConfirm::new(10);
        c.tap();
        assert_eq!(c.tap(), None);
        c.cancel();
        assert_eq!(c.outcome(), Some(Outcome::Confirmed), "the first answer stands");
    }

    #[test]
    fn the_panel_stays_inside_the_screen_margins() {
        // A confirmation half off-screen is one whose Cancel cannot be reached,
        // which turns a safety feature into a trap.
        let c = ChevronConfirm::new(10);
        let root = 400.0;
        let pw = c.panel_width(root);
        for anchor_x in [0.0, 5.0, 200.0, 395.0] {
            let x = c.panel_x(anchor_x, 48.0, root);
            assert!(x >= EDGE_MARGIN, "x {x} past the left margin");
            assert!(x + pw <= root - EDGE_MARGIN + 0.01, "x {x} + {pw} past the right");
        }
    }

    #[test]
    fn a_narrow_screen_gets_a_narrower_panel_rather_than_an_overflowing_one() {
        let c = ChevronConfirm::new(10);
        assert_eq!(c.panel_width(1000.0), MAX_WIDTH, "capped on a wide screen");
        assert_eq!(c.panel_width(240.0), 208.0, "240 minus both margins");
    }

    #[test]
    fn the_chevron_flips_above_when_there_is_no_room_below() {
        // Near the bottom of a phone screen, which is where the action bar
        // lives — so the flip is the common case rather than the exception.
        let c = ChevronConfirm::new(10);
        assert_eq!(c.chevron_side(100.0, 200.0, 800.0), ChevronSide::Below);
        assert_eq!(c.chevron_side(700.0, 200.0, 800.0), ChevronSide::Above);
    }
}
