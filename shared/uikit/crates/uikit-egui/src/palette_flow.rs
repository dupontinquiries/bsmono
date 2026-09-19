//! palette_flow — Ctrl+P as one key that opens the command palette, then
//! settles into voice on its own if you pause, rather than needing a
//! hold gesture at all.
//!
//! ## Why this replaced the hold-duration design
//!
//! The earlier design (`ctrl_p_gesture::GestureTracker`, still present
//! and still real infrastructure — a keyboard adapter over the shared
//! `cmdstrip::Hold` touch-hold primitive) required physically holding
//! Ctrl+P down past a threshold to trigger voice. Replaced by direct
//! design feedback for a simpler flow: tap once to open (always the
//! command palette, typed-search mode), and if you just PAUSE for a
//! moment without typing, it settles into listening on its own — no key
//! to hold, no timing to get right with your hand.
//!
//! ## The exact flow, worked through with the person who designed it
//!
//! ```text
//! Off --Ctrl+P--> Command --idle 750ms--> Voice
//!                    ^                       |
//!                    |                       |
//!                    +------ typing ---------+
//!                    |  (cancels in-flight capture,
//!                    |   escalates next dwell to 5000ms)
//!                    |
//!                    +--idle 5000ms--> Voice (again)
//! ```
//!
//! Ctrl+P from either open state (`Command` or `Voice`) closes back to
//! `Off` and resets the escalation — a fresh open always gets the
//! shorter 750ms dwell.
//!
//! ## Why the dwell escalates rather than staying fixed
//!
//! A fixed 750ms would mean anyone who types, pauses to think, types
//! more, pauses again — ordinary composition — keeps getting yanked into
//! voice mode on every natural pause. Escalating to 1500ms after the
//! FIRST time typing pulls the mode back is a cheap, real fix: it reads
//! as "already tried typing once this session" without needing to
//! actually parse or remember what was typed.
//!
//! ## What this module does not decide
//!
//! Whether crossing into `Voice` should call `voice_popup::VoicePopup::
//! start()`, or whether leaving `Voice` (via typing OR via Ctrl+P closing
//! it) should call `cancel()` — those are the caller's job, signaled
//! through [`Transition`] rather than performed here, so this state
//! machine stays pure and testable without a running voice engine.

/// Which surface is currently open, if either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Off,
    Command,
    Voice,
}

/// What just happened, for the caller to act on. `tick`/`on_type`/
/// `ctrl_p_tapped` all return one of these rather than mutating shared
/// state the caller has to diff against the previous frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    /// Nothing changed this call.
    None,
    /// Opened fresh, from `Off`. Always lands in `Command`.
    Opened,
    /// Closed back to `Off`. Carries which mode it closed FROM, since a
    /// close from `Voice` needs to cancel an in-flight capture and a
    /// close from `Command` does not.
    Closed(Mode),
    /// Typing pulled the mode back from `Voice` to `Command`. The
    /// caller should cancel any in-flight capture — the person started
    /// typing, which reads as "I changed my mind about speaking."
    SwitchedToCommand,
    /// The dwell timer crossed its threshold: `Command` -> `Voice`. The
    /// caller should start listening.
    SwitchedToVoice,
}

/// The first dwell, from a fresh `Command` open.
pub const FIRST_DWELL_MS: f32 = 750.0;
/// The dwell once the keyboard has been used at all this session.
///
/// Was 1500ms, and that was too short: 750ms is a pause for breath and
/// 1500ms is a pause for thought, so voice kept taking the palette from
/// under someone mid-command. Five seconds is long enough to stop and think
/// without losing the keyboard, and short enough that putting your hands
/// down still hands voice back without a deliberate action.
pub const ESCALATED_DWELL_MS: f32 = 5000.0;

pub struct PaletteFlow {
    mode: Mode,
    /// Milliseconds since the last reset — typing, or entering
    /// `Command` fresh. Only accumulates while `mode == Command`;
    /// meaningless (and un-ticked) otherwise.
    idle_ms: f32,
    /// The CURRENT dwell threshold `tick` compares `idle_ms` against.
    /// Starts at `FIRST_DWELL_MS`; escalates to `ESCALATED_DWELL_MS` on
    /// the first `on_type` call that pulls `Voice` back to `Command`.
    dwell_ms: f32,
}

impl Default for PaletteFlow {
    fn default() -> Self {
        PaletteFlow { mode: Mode::Off, idle_ms: 0.0, dwell_ms: FIRST_DWELL_MS }
    }
}

impl PaletteFlow {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// How far into the current dwell window `Command` mode is, 0.0 at
    /// entry to 1.0 at the threshold and beyond -- the raw input to a
    /// "mic is about to switch on" indicator. 0.0 outside `Command`
    /// (nothing dwelling toward voice in `Off` or `Voice` -- `Voice` is
    /// already there, not approaching it).
    pub fn dwell_progress(&self) -> f32 {
        if self.mode != Mode::Command {
            return 0.0;
        }
        (self.idle_ms / self.dwell_ms).min(1.0)
    }

    /// An explicit manual override -- right-click or hold on the mic
    /// indicator, say -- rather than waiting for Ctrl+P, typing, or the
    /// dwell timer. Always goes straight to `Off` from either open mode,
    /// resetting the escalation the same way a real Ctrl+P close does;
    /// a no-op from `Off`. Returns the same `Transition::Closed`/`None`
    /// shape `ctrl_p_tapped`'s close branch does, so a caller already
    /// handling that transition (cancelling an in-flight capture on
    /// `Closed(Mode::Voice)`) does not need a second match arm for this
    /// path.
    pub fn force_off(&mut self) -> Transition {
        match self.mode {
            Mode::Off => Transition::None,
            Mode::Command | Mode::Voice => self.close(),
        }
    }

    /// Ctrl+P was tapped. Opens (to `Command`, resetting the escalation)
    /// from `Off`; closes (to `Off`) from either open mode.
    pub fn ctrl_p_tapped(&mut self) -> Transition {
        match self.mode {
            Mode::Off => {
                self.mode = Mode::Command;
                self.idle_ms = 0.0;
                self.dwell_ms = FIRST_DWELL_MS;
                Transition::Opened
            }
            Mode::Command | Mode::Voice => self.close(),
        }
    }

    fn close(&mut self) -> Transition {
        let was = self.mode;
        self.mode = Mode::Off;
        Transition::Closed(was)
    }

    /// A text/typing event landed while the palette is open.
    ///
    /// Returns `Transition::None` while `Off` or already `Command` -- typing
    /// while already in typed-search mode is just... typing -- but it still
    /// resets the timer AND escalates the dwell.
    ///
    /// The escalation used to happen only when typing pulled the mode back
    /// from `Voice`. So the common case was unprotected: open the palette,
    /// start typing, pause 750ms to think, and voice took it mid-command
    /// without ever having been in `Voice` for the escalation to trigger on.
    /// Using the keyboard at all is the signal, not losing a race to voice.
    pub fn on_type(&mut self) -> Transition {
        self.keyboard_used()
    }

    /// An arrow key, Tab, Page Up/Down, Home/End -- moving through the
    /// palette's own list rather than typing into it.
    ///
    /// Resets the dwell exactly as typing does, and this is the half that was
    /// missing entirely: arrowing down a list of results inserts no text, so
    /// `on_type` never fired and the timer ran out underneath the selection.
    /// Voice would then arm at the precise moment before Enter -- the worst
    /// instant available, since "enter" is also a word.
    pub fn on_navigate(&mut self) -> Transition {
        self.keyboard_used()
    }

    /// Any keyboard activity: reset the idle timer and hold the longer dwell.
    fn keyboard_used(&mut self) -> Transition {
        self.idle_ms = 0.0;
        let t = self.voice_to_command();
        // Escalate even when the mode did not change -- see `on_type`.
        if self.mode != Mode::Off {
            self.dwell_ms = ESCALATED_DWELL_MS;
        }
        t
    }

    /// Voice failed to actually start (the caller's `voice_popup.start()`
    /// refused — password mode, say). Same underlying transition as
    /// `on_type`'s `Voice -> Command` step and the same escalation, under
    /// its own name so a caller does not have to pretend a keystroke
    /// happened just to get the same effect back onto `Command`.
    pub fn voice_refused(&mut self) -> Transition {
        self.voice_to_command()
    }

    fn voice_to_command(&mut self) -> Transition {
        if self.mode == Mode::Voice {
            self.mode = Mode::Command;
            self.dwell_ms = ESCALATED_DWELL_MS;
            return Transition::SwitchedToCommand;
        }
        Transition::None
    }

    /// The dwell currently in force, in ms. For the UI's warming ring and for
    /// tests that should not hardcode which constant is active.
    pub fn dwell_ms(&self) -> f32 {
        self.dwell_ms
    }

    /// Advance idle time by one frame's worth of milliseconds. Only has
    /// an effect while `mode == Command` -- `Off` and `Voice` do not
    /// accumulate or check a dwell timer at all.
    pub fn tick(&mut self, dt_ms: f32) -> Transition {
        if self.mode != Mode::Command {
            return Transition::None;
        }
        self.idle_ms += dt_ms.max(0.0);
        if self.idle_ms >= self.dwell_ms {
            self.mode = Mode::Voice;
            self.idle_ms = 0.0;
            return Transition::SwitchedToVoice;
        }
        Transition::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_p_opens_to_command_from_off() {
        let mut f = PaletteFlow::new();
        assert_eq!(f.mode(), Mode::Off);
        assert_eq!(f.ctrl_p_tapped(), Transition::Opened);
        assert_eq!(f.mode(), Mode::Command);
    }

    #[test]
    fn idle_past_the_first_dwell_switches_to_voice() {
        let mut f = PaletteFlow::new();
        f.ctrl_p_tapped();
        assert_eq!(f.tick(FIRST_DWELL_MS - 1.0), Transition::None, "must not fire early");
        assert_eq!(f.tick(1.0), Transition::SwitchedToVoice, "must fire once the threshold is crossed");
        assert_eq!(f.mode(), Mode::Voice);
    }

    #[test]
    fn tick_does_nothing_while_off_or_voice() {
        let mut f = PaletteFlow::new();
        // Off: ticking a huge amount of time must not open anything.
        assert_eq!(f.tick(100_000.0), Transition::None);
        assert_eq!(f.mode(), Mode::Off);

        f.ctrl_p_tapped();
        f.tick(FIRST_DWELL_MS); // -> Voice
        assert_eq!(f.mode(), Mode::Voice);
        // Voice: ticking must not do anything further (no third state).
        assert_eq!(f.tick(100_000.0), Transition::None);
        assert_eq!(f.mode(), Mode::Voice);
    }

    #[test]
    fn typing_while_voice_switches_back_to_command_and_escalates() {
        let mut f = PaletteFlow::new();
        f.ctrl_p_tapped();
        f.tick(FIRST_DWELL_MS); // -> Voice
        assert_eq!(f.on_type(), Transition::SwitchedToCommand);
        assert_eq!(f.mode(), Mode::Command);
        // The escalation: the SAME 750ms that worked the first time must
        // NOT trigger Voice again -- this is the whole point of the ask.
        assert_eq!(f.tick(FIRST_DWELL_MS), Transition::None, "must not re-trigger at the original dwell");
        // But the escalated 1500ms threshold does fire.
        assert_eq!(f.tick(ESCALATED_DWELL_MS - FIRST_DWELL_MS), Transition::SwitchedToVoice);
    }

    #[test]
    fn typing_while_already_command_is_a_plain_no_op() {
        let mut f = PaletteFlow::new();
        f.ctrl_p_tapped(); // -> Command
        assert_eq!(f.on_type(), Transition::None, "typing in Command is just typing");
        assert_eq!(f.mode(), Mode::Command);
    }

    #[test]
    fn typing_resets_the_idle_clock_even_without_a_mode_switch() {
        // A person typing steadily in Command mode must never drift into
        // Voice just because enough WALL time passed -- each keystroke
        // has to reset the idle counter, not merely be ignored.
        let mut f = PaletteFlow::new();
        f.ctrl_p_tapped();
        f.tick(FIRST_DWELL_MS - 50.0); // close to the threshold
        f.on_type(); // keystroke -- resets idle_ms to 0
        assert_eq!(f.tick(FIRST_DWELL_MS - 50.0), Transition::None, "reset clock must not be near the threshold yet");
    }

    #[test]
    fn ctrl_p_closes_from_either_open_mode_and_reports_which() {
        let mut f = PaletteFlow::new();
        f.ctrl_p_tapped(); // -> Command
        assert_eq!(f.ctrl_p_tapped(), Transition::Closed(Mode::Command));
        assert_eq!(f.mode(), Mode::Off);

        f.ctrl_p_tapped();
        f.tick(FIRST_DWELL_MS); // -> Voice
        assert_eq!(f.ctrl_p_tapped(), Transition::Closed(Mode::Voice));
        assert_eq!(f.mode(), Mode::Off);
    }

    #[test]
    fn closing_and_reopening_resets_the_escalation() {
        // A fresh open must always get the SHORT dwell again, even if a
        // previous session escalated to 1500ms -- the escalation is
        // per-session (since the last full close), not permanent.
        let mut f = PaletteFlow::new();
        f.ctrl_p_tapped();
        f.tick(FIRST_DWELL_MS); // -> Voice
        f.on_type(); // -> Command, dwell escalated to 1500ms
        f.ctrl_p_tapped(); // close fully -> Off

        f.ctrl_p_tapped(); // reopen -> Command
        assert_eq!(f.tick(FIRST_DWELL_MS), Transition::SwitchedToVoice, "a fresh open must use the SHORT dwell, not the escalated one");
    }

    #[test]
    fn a_second_escalation_within_one_session_does_not_escalate_further() {
        // Only one escalation step exists (750 -> 1500), not an
        // ever-doubling series -- pinned explicitly so a future edit
        // that turns this into 1500 -> 3000 -> 6000 fails a named test
        // rather than silently changing the feel.
        let mut f = PaletteFlow::new();
        f.ctrl_p_tapped();
        f.tick(FIRST_DWELL_MS); // -> Voice (1st time)
        f.on_type(); // -> Command, escalate to 1500
        f.tick(ESCALATED_DWELL_MS); // -> Voice (2nd time)
        f.on_type(); // -> Command, escalate again -- but to what?
        // Must still be 1500, not 3000: the SAME escalated threshold
        // fires again, it does not escalate a second time.
        assert_eq!(f.tick(ESCALATED_DWELL_MS), Transition::SwitchedToVoice);
    }

    #[test]
    fn voice_refused_has_the_same_effect_as_on_type_without_needing_a_fake_keystroke() {
        let mut f = PaletteFlow::new();
        f.ctrl_p_tapped();
        f.tick(FIRST_DWELL_MS); // -> Voice
        assert_eq!(f.voice_refused(), Transition::SwitchedToCommand);
        assert_eq!(f.mode(), Mode::Command);
        // Same escalation as on_type's Voice -> Command step -- the
        // original 750ms must not re-trigger.
        assert_eq!(f.tick(FIRST_DWELL_MS), Transition::None);
        assert_eq!(f.tick(ESCALATED_DWELL_MS - FIRST_DWELL_MS), Transition::SwitchedToVoice);
    }

    #[test]
    fn voice_refused_is_a_no_op_outside_voice_mode() {
        let mut f = PaletteFlow::new();
        assert_eq!(f.voice_refused(), Transition::None, "Off must be unaffected");
        f.ctrl_p_tapped();
        assert_eq!(f.voice_refused(), Transition::None, "Command must be unaffected");
        assert_eq!(f.mode(), Mode::Command);
    }

    #[test]
    fn dwell_progress_is_zero_outside_command() {
        let mut f = PaletteFlow::new();
        assert_eq!(f.dwell_progress(), 0.0, "Off");
        f.ctrl_p_tapped();
        f.tick(FIRST_DWELL_MS); // -> Voice
        assert_eq!(f.dwell_progress(), 0.0, "Voice is already there, not approaching it");
    }

    #[test]
    fn dwell_progress_tracks_the_ratio_and_clamps_at_one() {
        let mut f = PaletteFlow::new();
        f.ctrl_p_tapped();
        assert_eq!(f.dwell_progress(), 0.0);
        f.tick(FIRST_DWELL_MS / 2.0);
        assert!((f.dwell_progress() - 0.5).abs() < 0.01);
        f.tick(FIRST_DWELL_MS * 10.0); // well past -- but this also crosses
                                        // into Voice, so progress must read 0
                                        // again (Voice, not approaching it)
        assert_eq!(f.dwell_progress(), 0.0, "once switched to Voice, progress is not 'past 1.0', it is the wrong question");
    }

    #[test]
    fn dwell_progress_uses_the_escalated_threshold_after_typing() {
        let mut f = PaletteFlow::new();
        f.ctrl_p_tapped();
        f.tick(FIRST_DWELL_MS); // -> Voice
        f.on_type(); // -> Command, dwell escalated
        f.tick(FIRST_DWELL_MS);
        // Against the constant, not a literal: this asserted 0.5 (750 of 1500)
        // and went red when the escalated dwell became 5 s (4b1486d71), which
        // changed the number and not the property.
        let want = FIRST_DWELL_MS / ESCALATED_DWELL_MS;
        assert!((f.dwell_progress() - want).abs() < 0.01, "progress must be against the ESCALATED dwell, not the original");
        assert!(want < 0.5, "and the escalated dwell really is the longer one");
    }

    #[test]
    fn force_off_closes_from_either_open_mode_and_is_a_no_op_from_off() {
        let mut f = PaletteFlow::new();
        assert_eq!(f.force_off(), Transition::None, "already Off");

        f.ctrl_p_tapped(); // -> Command
        assert_eq!(f.force_off(), Transition::Closed(Mode::Command));
        assert_eq!(f.mode(), Mode::Off);

        f.ctrl_p_tapped();
        f.tick(FIRST_DWELL_MS); // -> Voice
        assert_eq!(f.force_off(), Transition::Closed(Mode::Voice));
        assert_eq!(f.mode(), Mode::Off);
    }

    #[test]
    fn force_off_resets_the_escalation_the_same_as_a_real_close() {
        let mut f = PaletteFlow::new();
        f.ctrl_p_tapped();
        f.tick(FIRST_DWELL_MS); // -> Voice
        f.on_type(); // -> Command, escalated to 1500
        f.force_off(); // -> Off

        f.ctrl_p_tapped(); // reopen -> Command
        assert_eq!(f.tick(FIRST_DWELL_MS), Transition::SwitchedToVoice, "a fresh open after force_off must use the SHORT dwell");
    }

    /// THE REPORTED BUG. Open, start typing, pause to think -- and voice took
    /// the palette at 750ms. The escalation existed but only triggered when
    /// typing pulled the mode back from Voice, so the ordinary case of typing
    /// from a fresh open was never protected.
    #[test]
    fn typing_from_a_fresh_open_earns_the_long_dwell_immediately() {
        let mut f = PaletteFlow::new();
        f.ctrl_p_tapped();
        assert_eq!(f.dwell_ms(), FIRST_DWELL_MS, "before any keystroke");
        f.on_type();
        assert_eq!(f.dwell_ms(), ESCALATED_DWELL_MS, "one keystroke is the signal");
        // A pause for thought must not lose the keyboard.
        assert_eq!(f.tick(FIRST_DWELL_MS + 1.0), Transition::None);
        assert_eq!(f.mode(), Mode::Command, "still typing mode after 750ms");
        assert_eq!(f.tick(ESCALATED_DWELL_MS), Transition::SwitchedToVoice,
            "and voice still returns on its own");
    }

    /// Five seconds, as asked. Pinned as a value because the whole complaint
    /// was that the number was too small.
    #[test]
    fn the_keyboard_dwell_is_five_seconds() {
        assert_eq!(ESCALATED_DWELL_MS, 5000.0);
        assert!(ESCALATED_DWELL_MS > FIRST_DWELL_MS * 4.0);
    }

    /// THE MISSING HALF. Arrowing through results inserts no text, so on_type
    /// never fired and the dwell ran out under the selection -- arming voice
    /// at the instant before Enter, which is also a word.
    #[test]
    fn navigating_the_list_resets_the_dwell_just_like_typing() {
        let mut f = PaletteFlow::new();
        f.ctrl_p_tapped();
        f.on_navigate();
        assert_eq!(f.dwell_ms(), ESCALATED_DWELL_MS);
        // Walk a long list: never armed, however long it takes.
        for _ in 0..40 {
            assert_eq!(f.tick(400.0), Transition::None);
            f.on_navigate();
        }
        assert_eq!(f.mode(), Mode::Command, "still the keyboard's after 16s of arrowing");
    }

    /// Navigation pulls voice back too, not just text.
    #[test]
    fn an_arrow_key_takes_the_palette_back_from_voice() {
        let mut f = PaletteFlow::new();
        f.ctrl_p_tapped();
        assert_eq!(f.tick(FIRST_DWELL_MS), Transition::SwitchedToVoice);
        assert_eq!(f.on_navigate(), Transition::SwitchedToCommand);
        assert_eq!(f.mode(), Mode::Command);
    }

    /// Typing mid-dwell restarts the clock rather than letting the earlier
    /// idle time count toward the threshold.
    #[test]
    fn each_keystroke_restarts_the_timer() {
        let mut f = PaletteFlow::new();
        f.ctrl_p_tapped();
        for _ in 0..10 {
            f.on_type();
            assert_eq!(f.tick(ESCALATED_DWELL_MS - 100.0), Transition::None);
        }
        assert_eq!(f.mode(), Mode::Command);
    }

    /// Closing and reopening is a new session, so the short dwell is back --
    /// someone who opens the palette and says nothing still gets voice fast.
    #[test]
    fn a_fresh_session_returns_to_the_short_dwell() {
        let mut f = PaletteFlow::new();
        f.ctrl_p_tapped();
        f.on_type();
        assert_eq!(f.dwell_ms(), ESCALATED_DWELL_MS);
        f.force_off();
        f.ctrl_p_tapped();
        assert_eq!(f.dwell_ms(), FIRST_DWELL_MS, "a new open is not penalised");
    }

    /// Keyboard events while the palette is CLOSED must not arm anything or
    /// change a dwell that is not running.
    #[test]
    fn keyboard_events_while_off_do_nothing() {
        let mut f = PaletteFlow::new();
        assert_eq!(f.on_type(), Transition::None);
        assert_eq!(f.on_navigate(), Transition::None);
        assert_eq!(f.mode(), Mode::Off);
        assert_eq!(f.tick(100_000.0), Transition::None, "Off never arms");
    }
}
