//! typing_grace — don't take the microphone back while someone is typing.
//!
//! ## The bug
//!
//! `sync_voice_routing` returns the mic to the command vocabulary the instant
//! glove mode stops dictating, and its comment explains why: *"voiceterm is a
//! voice-driven app, and a terminal that stopped listening after one
//! dictation would need a tap to wake"*. That reasoning is right and the
//! behaviour still hurt, because it has no idea whether a keyboard is in use.
//!
//! The result: you start typing, the command vocabulary is live the whole
//! time, and a word you said — or a word the recogniser thought it heard —
//! fires a command in the middle of a sentence you were entering by hand.
//!
//! ## The fix is a grace period, not a mode
//!
//! A "keyboard mode" toggle would be a third thing to remember and would get
//! left in the wrong position. Instead: **any keyboard activity silences the
//! command vocabulary for [`GRACE`], and the timer restarts on every
//! keystroke.** Stop typing for five seconds and voice comes back on its own,
//! which preserves the no-tap-to-wake property the original comment was
//! protecting.
//!
//! ## Navigation counts as typing
//!
//! Arrow keys, Tab, Page Up/Down, Home/End and Escape all reset the timer even
//! though they insert nothing. Moving through a menu is *using the keyboard*,
//! and a grace period that expired while someone arrowed down a long list
//! would re-arm the mic at the exact moment they were about to press Enter —
//! the worst possible instant, because "enter" is also a word.
//!
//! What does NOT reset it: modifier keys held alone, and mouse or touch input.
//! A held Shift is not activity, and a tap is how you reach for voice in the
//! first place.
//!
//! ## No clock in here
//!
//! The caller passes `now`. That keeps this testable without sleeping and
//! without a fake clock, which for a five-second timer is the difference
//! between a test suite that runs in microseconds and one that runs in
//! minutes.

/// How long the keyboard holds the microphone off after the last keypress.
///
/// Five seconds is long enough to cross a pause for thought mid-command and
/// short enough that putting your hands down gives voice back without a
/// deliberate action.
pub const GRACE_MS: u64 = 5_000;

/// What kind of input arrived. Only some of it counts as typing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// A character was entered.
    Text,
    /// Enter, Backspace, Delete — editing without inserting a character.
    Edit,
    /// Arrows, Tab, Page Up/Down, Home/End, Escape. Moving through a menu or
    /// a buffer is using the keyboard.
    Navigation,
    /// Shift/Ctrl/Alt/Super with nothing else. Holding a modifier is not
    /// activity — a hand resting on Ctrl would hold the mic off forever.
    ModifierOnly,
    /// Mouse or touch. Deliberately not typing: a tap is how someone reaches
    /// for voice, so counting it would make the feature unreachable by the
    /// gesture it is designed around.
    Pointer,
}

impl Input {
    /// Does this restart the grace period?
    pub fn is_typing(self) -> bool {
        matches!(self, Input::Text | Input::Edit | Input::Navigation)
    }
}

/// Tracks the last keyboard activity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TypingGrace {
    /// Milliseconds, in the caller's own timebase. `None` = never typed.
    last_ms: Option<u64>,
}

impl TypingGrace {
    pub fn new() -> TypingGrace {
        TypingGrace { last_ms: None }
    }

    /// Record an input event at `now_ms`. Returns whether it counted.
    ///
    /// A non-monotonic `now` is ignored rather than trusted: the timebase is
    /// the caller's, and letting a clock that went backwards shorten the grace
    /// period would make the bug reappear intermittently, which is worse than
    /// it never being fixed.
    pub fn on_input(&mut self, kind: Input, now_ms: u64) -> bool {
        if !kind.is_typing() {
            return false;
        }
        match self.last_ms {
            Some(prev) if now_ms < prev => false,
            _ => {
                self.last_ms = Some(now_ms);
                true
            }
        }
    }

    /// Is the keyboard holding the microphone off?
    pub fn suppressing(&self, now_ms: u64) -> bool {
        self.remaining_ms(now_ms) > 0
    }

    /// Milliseconds left before voice returns. 0 when it already has.
    ///
    /// Exposed so the UI can show the countdown: a mic icon that dims and
    /// comes back on its own is self-explanatory, where one that simply
    /// stopped working is not.
    pub fn remaining_ms(&self, now_ms: u64) -> u64 {
        match self.last_ms {
            None => 0,
            Some(last) => {
                let elapsed = now_ms.saturating_sub(last);
                GRACE_MS.saturating_sub(elapsed)
            }
        }
    }

    /// Give the microphone back immediately — for an explicit "listen" button
    /// or wake word. The user asking for voice outranks the heuristic that
    /// guesses they did not want it.
    pub fn release(&mut self) {
        self.last_ms = None;
    }
}

/// Should the router be listening for commands right now?
///
/// The decision `sync_voice_routing` needs, as a free function so it can be
/// tested without a `GloveCursor`. `dictating` wins outright: an armed field
/// is an explicit request and typing does not revoke it — the two are
/// routinely used together, saying a word into a field while editing another.
pub fn should_listen_for_commands(grace: &TypingGrace, dictating: bool, now_ms: u64) -> bool {
    dictating || !grace.suppressing(now_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE BUG. Typing must take the command vocabulary offline, or a word
    /// heard mid-sentence fires a command.
    #[test]
    fn typing_suppresses_the_command_vocabulary() {
        let mut g = TypingGrace::new();
        assert!(!g.suppressing(0), "silence does not suppress");
        g.on_input(Input::Text, 1_000);
        assert!(g.suppressing(1_000));
        assert!(g.suppressing(1_000 + GRACE_MS - 1), "still held just before the grace ends");
    }

    /// ...and gives it back on its own, which is what keeps the no-tap-to-wake
    /// property the original behaviour was protecting.
    #[test]
    fn voice_returns_by_itself_after_five_seconds() {
        let mut g = TypingGrace::new();
        g.on_input(Input::Text, 1_000);
        assert!(!g.suppressing(1_000 + GRACE_MS), "exactly at the boundary it is back");
        assert!(!g.suppressing(9_999));
        assert_eq!(g.remaining_ms(1_000 + GRACE_MS), 0);
    }

    /// Every keystroke restarts it. Typing a paragraph must not let voice back
    /// in halfway through because the FIRST key was six seconds ago.
    #[test]
    fn the_timer_restarts_on_every_keystroke() {
        let mut g = TypingGrace::new();
        let mut t = 0;
        for _ in 0..20 {
            g.on_input(Input::Text, t);
            t += 400; // a steady 2.5 keys/sec
            assert!(g.suppressing(t), "held throughout at t={t}");
        }
        assert!(!g.suppressing(t + GRACE_MS));
    }

    /// NAVIGATION COUNTS. Arrowing down a menu is using the keyboard, and a
    /// grace period that lapsed there would re-arm the mic at the instant
    /// before Enter -- which is also a word.
    #[test]
    fn arrow_keys_and_menu_navigation_reset_the_timer() {
        for kind in [Input::Navigation, Input::Edit, Input::Text] {
            let mut g = TypingGrace::new();
            assert!(g.on_input(kind, 500), "{kind:?} must count as typing");
            assert!(g.suppressing(500 + GRACE_MS - 1), "{kind:?} must hold the mic");
        }
        // A long walk down a list stays suppressed the whole way.
        let mut g = TypingGrace::new();
        let mut t = 0;
        for _ in 0..30 {
            g.on_input(Input::Navigation, t);
            t += 300;
        }
        assert!(g.suppressing(t), "still held after 30 arrow presses");
    }

    /// A hand resting on Ctrl is not activity, or the mic never comes back.
    #[test]
    fn a_held_modifier_alone_does_not_hold_the_microphone() {
        let mut g = TypingGrace::new();
        assert!(!g.on_input(Input::ModifierOnly, 100));
        assert!(!g.suppressing(100));
    }

    /// A tap is how you reach for voice; counting it would make the feature
    /// unreachable by its own gesture.
    #[test]
    fn pointer_input_is_not_typing() {
        let mut g = TypingGrace::new();
        assert!(!g.on_input(Input::Pointer, 100));
        assert!(!g.suppressing(100));
    }

    /// An explicit ask outranks the heuristic.
    #[test]
    fn release_gives_the_microphone_back_at_once() {
        let mut g = TypingGrace::new();
        g.on_input(Input::Text, 1_000);
        assert!(g.suppressing(1_500));
        g.release();
        assert!(!g.suppressing(1_500));
    }

    /// The countdown is for the UI: a mic that dims and returns on its own
    /// explains itself; one that silently stops working does not.
    #[test]
    fn the_remaining_time_counts_down() {
        let mut g = TypingGrace::new();
        g.on_input(Input::Text, 1_000);
        assert_eq!(g.remaining_ms(1_000), GRACE_MS);
        assert_eq!(g.remaining_ms(3_000), GRACE_MS - 2_000);
        assert_eq!(g.remaining_ms(100_000), 0, "never negative");
        assert_eq!(TypingGrace::new().remaining_ms(50), 0, "nothing typed, nothing left");
    }

    /// A clock that went backwards must not shorten the grace -- that would
    /// make the bug reappear intermittently, which is worse than not fixing it.
    #[test]
    fn a_backwards_clock_does_not_cut_the_grace_short() {
        let mut g = TypingGrace::new();
        g.on_input(Input::Text, 10_000);
        assert!(!g.on_input(Input::Text, 5_000), "an earlier timestamp is ignored");
        assert!(g.suppressing(10_000 + GRACE_MS - 1), "still measured from the later key");
        assert_eq!(g.remaining_ms(5_000), GRACE_MS, "saturating, not wrapping");
    }

    /// An armed field wins: dictation is an explicit request, and typing in
    /// one field while a word goes into another is a normal thing to do.
    #[test]
    fn dictation_outranks_the_typing_grace() {
        let mut g = TypingGrace::new();
        g.on_input(Input::Text, 1_000);
        assert!(!should_listen_for_commands(&g, false, 1_500), "commands are held off");
        assert!(should_listen_for_commands(&g, true, 1_500), "but dictation is not");
    }

    /// With nobody typing, nothing changes from today's behaviour.
    #[test]
    fn an_idle_keyboard_leaves_voice_exactly_as_it_was() {
        let g = TypingGrace::new();
        assert!(should_listen_for_commands(&g, false, 0));
        assert!(should_listen_for_commands(&g, false, 1_000_000));
    }
}
