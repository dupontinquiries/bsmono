//! visible_mode — Full / Tray / Dismissed, and every way between them.
//!
//! `docs/single-hand-mode.md` specifies three top-level states and names each transition
//! into them. The GTK build holds it in an `Rc<Cell<VisibleMode>>` that a drag handler flips
//! from inside a closure; this port ported the TRAY (`tray.rs`, the widget with its three
//! gesture vocabularies) and not the machine that decides when the tray is on screen. The
//! result is a keyboard with a minimized mode nothing can enter.
//!
//! ## The states
//!
//! | State | What is on screen | Entered by |
//! |---|---|---|
//! | `Full` | the letter board, all features | IM Activate from Dismissed; Double-Activate from Tray; Expand in Tray |
//! | `Tray` | corner panel: expand, spacebar, close | a long upward fling from Full |
//! | `Dismissed` | nothing | Close in Tray; IM Deactivate from Full |
//!
//! ## Double-Activate, and why it needs a clock
//!
//! The doc: *two `zwp_input_method_v1::activate` events within 700ms with no intervening
//! Deactivate*. KWin fires that when you double-tap a text field, or tap two fields in quick
//! succession — the second tap counts as a re-focus. It is the way back from Tray without
//! reaching for the expand button, which matters because the Tray exists precisely for when
//! reaching is the problem.
//!
//! So an Activate is not idempotent, and the rule cannot be evaluated without knowing when
//! the previous one arrived. [`Machine::activate`] takes the time; nothing here reads a
//! clock itself.
//!
//! ## What this does NOT decide
//!
//! Whether the compositor is showing the surface. That is `shell/layer.rs`'s map/unmap
//! dance, driven from this state — `Dismissed` means "we have asked for nothing on screen",
//! and the shell is what makes it so. Keeping them separate is what lets this be tested
//! without a Wayland connection.

/// One of the three top-level states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisibleMode {
    Full,
    Tray,
    Dismissed,
}

/// The doc's window for a double-activate.
pub const DOUBLE_ACTIVATE_MS: u64 = 700;

/// What the caller must do about a transition.
///
/// Returned rather than performed, because the actions live in three different places — the
/// shell maps and unmaps, the screen swaps its content, the tray resets its selection state
/// — and a state machine that reached into all three could not be tested without them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Nothing changed.
    None,
    /// Show the full board.
    ShowFull,
    /// Show the tray.
    ShowTray,
    /// Put nothing on screen.
    Hide,
}

#[derive(Debug, Clone, Copy)]
pub struct Machine {
    mode: VisibleMode,
    /// When the last Activate arrived, for the double-activate window.
    last_activate_ms: Option<u64>,
}

impl Default for Machine {
    fn default() -> Self {
        // Dismissed, because a keyboard that starts visible is one that appears over an app
        // nobody asked it to cover. The IM tells us when there is a text field.
        Machine { mode: VisibleMode::Dismissed, last_activate_ms: None }
    }
}

impl Machine {
    pub fn new() -> Machine {
        Machine::default()
    }

    pub fn mode(&self) -> VisibleMode {
        self.mode
    }

    pub fn is_visible(&self) -> bool {
        self.mode != VisibleMode::Dismissed
    }

    fn go(&mut self, to: VisibleMode) -> Effect {
        if self.mode == to {
            return Effect::None;
        }
        self.mode = to;
        match to {
            VisibleMode::Full => Effect::ShowFull,
            VisibleMode::Tray => Effect::ShowTray,
            VisibleMode::Dismissed => Effect::Hide,
        }
    }

    /// `zwp_input_method_v1::activate` — a text field took focus.
    ///
    /// From Dismissed this shows the board. From TRAY, only a second activate inside
    /// [`DOUBLE_ACTIVATE_MS`] restores the full board: a single one leaves the tray alone,
    /// because tapping the field you were already typing into is not a request to undo the
    /// minimizing you just did.
    pub fn activate(&mut self, now_ms: u64) -> Effect {
        let double = self
            .last_activate_ms
            .is_some_and(|t| now_ms.saturating_sub(t) <= DOUBLE_ACTIVATE_MS);
        self.last_activate_ms = Some(now_ms);
        match self.mode {
            VisibleMode::Dismissed => self.go(VisibleMode::Full),
            VisibleMode::Tray if double => self.go(VisibleMode::Full),
            _ => Effect::None,
        }
    }

    /// `zwp_input_method_v1::deactivate` — focus left the text field.
    ///
    /// From Full this dismisses. From TRAY it does not: the tray is a deliberate,
    /// user-chosen state, and a focus change while it is up should not throw away the
    /// choice — the close button is how the tray goes away. It also clears the
    /// double-activate memory, per the doc's "with no intervening Deactivate".
    pub fn deactivate(&mut self) -> Effect {
        self.last_activate_ms = None;
        match self.mode {
            VisibleMode::Full => self.go(VisibleMode::Dismissed),
            _ => Effect::None,
        }
    }

    /// A long upward fling off the top of the full board.
    pub fn fling_up(&mut self) -> Effect {
        match self.mode {
            VisibleMode::Full => self.go(VisibleMode::Tray),
            _ => Effect::None,
        }
    }

    /// The tray's expand button.
    pub fn expand(&mut self) -> Effect {
        match self.mode {
            VisibleMode::Tray => self.go(VisibleMode::Full),
            _ => Effect::None,
        }
    }

    /// The tray's close button.
    pub fn close(&mut self) -> Effect {
        match self.mode {
            VisibleMode::Tray => self.go(VisibleMode::Dismissed),
            _ => Effect::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The straight line through the doc's table.
    #[test]
    fn the_documented_path_round_trips() {
        let mut m = Machine::new();
        assert_eq!(m.mode(), VisibleMode::Dismissed, "a keyboard must not start over an app");
        assert!(!m.is_visible());

        assert_eq!(m.activate(0), Effect::ShowFull);
        assert_eq!(m.mode(), VisibleMode::Full);

        assert_eq!(m.fling_up(), Effect::ShowTray);
        assert_eq!(m.mode(), VisibleMode::Tray);
        assert!(m.is_visible(), "the tray is on screen");

        assert_eq!(m.expand(), Effect::ShowFull);
        assert_eq!(m.fling_up(), Effect::ShowTray);
        assert_eq!(m.close(), Effect::Hide);
        assert_eq!(m.mode(), VisibleMode::Dismissed);
    }

    /// Two activates within 700ms and no Deactivate between them. It is the way back from
    /// the tray without reaching for the expand button — which matters, because the tray
    /// exists precisely for when reaching is the problem.
    #[test]
    fn a_double_activate_restores_the_full_board_from_the_tray() {
        let mut m = Machine::new();
        m.activate(1_000);
        m.fling_up();
        assert_eq!(m.mode(), VisibleMode::Tray);

        // One activate is not enough: tapping the field you were already typing into is not
        // a request to undo the minimizing you just did.
        assert_eq!(m.activate(2_000), Effect::None);
        assert_eq!(m.mode(), VisibleMode::Tray);

        // A second inside the window is.
        assert_eq!(m.activate(2_000 + DOUBLE_ACTIVATE_MS), Effect::ShowFull);
        assert_eq!(m.mode(), VisibleMode::Full);
    }

    /// Just outside the window is two separate taps, not a double.
    #[test]
    fn the_double_activate_window_is_bounded() {
        let mut m = Machine::new();
        m.activate(0);
        m.fling_up();
        m.activate(10_000);
        assert_eq!(m.activate(10_000 + DOUBLE_ACTIVATE_MS + 1), Effect::None, "too slow to be a double");
        assert_eq!(m.mode(), VisibleMode::Tray);
        // and the pair that IS in the window still works afterwards
        m.activate(20_000);
        assert_eq!(m.activate(20_000 + 1), Effect::ShowFull);
    }

    /// The doc says "with no intervening Deactivate", and that clause is the whole
    /// difference between a double-tap on one field and two taps on two fields with a focus
    /// loss in between.
    #[test]
    fn a_deactivate_between_them_breaks_the_pair() {
        let mut m = Machine::new();
        m.activate(0);
        m.fling_up();
        m.activate(1_000);
        assert_eq!(m.deactivate(), Effect::None, "a focus change must not close the tray");
        assert_eq!(m.mode(), VisibleMode::Tray, "the tray is a choice, not a side effect");
        assert_eq!(m.activate(1_100), Effect::None, "the deactivate should have broken the pair");
        assert_eq!(m.mode(), VisibleMode::Tray);
    }

    /// A focus change while the tray is up must not throw away a state the user chose. The
    /// close button is how the tray goes away.
    #[test]
    fn only_full_is_dismissed_by_losing_focus() {
        let mut m = Machine::new();
        m.activate(0);
        assert_eq!(m.deactivate(), Effect::Hide);
        assert_eq!(m.mode(), VisibleMode::Dismissed);

        m.activate(10);
        m.fling_up();
        assert_eq!(m.deactivate(), Effect::None);
        assert_eq!(m.mode(), VisibleMode::Tray);

        // And from Dismissed a deactivate is nothing at all.
        m.close();
        assert_eq!(m.deactivate(), Effect::None);
    }

    /// Every transition is guarded by the state it starts in — a fling in the tray, an
    /// expand in Full, a close while dismissed are all no-ops rather than surprises.
    #[test]
    fn transitions_that_do_not_apply_do_nothing() {
        let mut m = Machine::new();
        assert_eq!(m.fling_up(), Effect::None, "there is no board to minimize");
        assert_eq!(m.expand(), Effect::None);
        assert_eq!(m.close(), Effect::None);
        assert_eq!(m.mode(), VisibleMode::Dismissed);

        m.activate(0);
        assert_eq!(m.expand(), Effect::None, "already full");
        assert_eq!(m.close(), Effect::None, "close is the tray's button");
        assert_eq!(m.mode(), VisibleMode::Full);

        // Re-entering the state you are in reports None rather than a redundant show.
        assert_eq!(m.activate(1), Effect::None);
    }
}
