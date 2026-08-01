//! voiceconfirm.rs — showing a voice interjection before it acts, on voiceterm's shape.
//!
//! `apps/voiceterm/qml/ConfirmDialog.qml` already solved the hard part, and the reusable idea is not
//! the layout — it is `timeoutMs` paired with `timeoutAction`. **A confirm that waits forever blocks
//! the run.** Voice is the case where that matters most: the operator has already moved on, and a
//! dialog still sitting there is a stalled panel plus a correction nobody wants applied.
//!
//! So a voice confirm has a deadline and a default, and the default is DISCARD.
//!
//! ## Why voice needs confirming at all when typing does not
//!
//! Typed text is what the operator wrote. Transcribed speech is a guess, and a wrong guess injected
//! into a debate is worse than a typo in a chat message — it redirects reasoning rather than appearing
//! in a line someone can reread. So the transcript is shown, and low-confidence transcription is
//! flagged rather than silently sent.
//!
//! ## What the panel must show
//!
//! Not just the words. The operator needs to know **who it will reach** — `panelvoice::Share` decides
//! whether one debater hears it or all of them, and sharing with everyone ends the round's blind
//! control. That consequence belongs on the dialog, not in a manual.

/// What happens if the operator does nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OnTimeout { Discard, Send }

/// Milliseconds before the default fires.
///
/// voiceterm uses 10s for a save confirm. Shorter here, because an injection is about a turn in
/// progress: past a few seconds the turn has moved and the correction lands on the wrong thing —
/// `injecttime` puts voice's shelf life at 2.5s for the same reason.
pub const TIMEOUT_MS: u64 = 4_000;

/// Below this, the transcript is likely wrong enough to show as uncertain.
pub const LOW_CONFIDENCE: f32 = 0.75;

/// A pending interjection awaiting confirmation.
#[derive(Clone, PartialEq, Debug)]
pub struct Pending {
    pub transcript: String,
    /// Recogniser confidence, 0..1.
    pub confidence: f32,
    pub age_ms: u64,
    /// Who it will reach if sent.
    pub reach: Reach,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Reach { One(String), AllButControl(String), Everyone }

impl Reach {
    /// The consequence line, shown on the dialog rather than left to a manual.
    pub fn consequence(&self) -> String {
        match self {
            Reach::One(who) => format!("only {who} hears this; the rest stay blind"),
            Reach::AllButControl(c) => format!("everyone except {c}, which stays blind as a control"),
            Reach::Everyone => "EVERYONE — this ends the round's ability to tell a corroborated \
answer from your hint arriving several times".into(),
        }
    }
    /// Does a blind member survive?
    pub fn keeps_control(&self) -> bool { !matches!(self, Reach::Everyone) }
}

impl Pending {
    /// Default action once the deadline passes.
    ///
    /// DISCARD, always. Sending on a timeout would apply an unreviewed transcription to a turn that has
    /// moved — the two failures the dialog exists to prevent, combined.
    pub fn on_timeout(&self) -> OnTimeout { OnTimeout::Discard }

    pub fn expired(&self) -> bool { self.age_ms >= TIMEOUT_MS }

    /// Should the transcript be shown as uncertain?
    pub fn uncertain(&self) -> bool { self.confidence < LOW_CONFIDENCE }

    /// Milliseconds left, for a countdown the operator can see.
    ///
    /// Shown because a dialog that vanishes without warning reads as a bug, and one that vanishes with
    /// a visible countdown reads as a deadline.
    pub fn remaining_ms(&self) -> u64 { TIMEOUT_MS.saturating_sub(self.age_ms) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(t: &str, conf: f32, age: u64, reach: Reach) -> Pending {
        Pending { transcript: t.into(), confidence: conf, age_ms: age, reach }
    }

    #[test]
    fn the_default_on_timeout_is_DISCARD() {
        // Sending would apply an unreviewed transcription to a turn that has moved — both failures the
        // dialog exists to prevent, at once. voiceterm defaults its save confirm to discard for the
        // same reason.
        let x = p("no not that", 0.9, TIMEOUT_MS + 1, Reach::Everyone);
        assert_eq!(x.on_timeout(), OnTimeout::Discard);
        assert!(x.expired());
    }

    #[test]
    fn the_timeout_is_shorter_than_voiceterms_because_a_TURN_is_moving() {
        // voiceterm's 10s suits a save confirm, where the document is not changing. An injection is
        // about a turn in progress, and injecttime puts voice's shelf life at 2.5s.
        assert!(TIMEOUT_MS < 10_000);
    }

    #[test]
    fn a_low_confidence_transcript_is_FLAGGED_not_silently_sent() {
        // Transcribed speech is a guess, and a wrong guess injected into a debate redirects reasoning
        // rather than sitting in a line someone can reread.
        assert!(p("use a hash map", 0.4, 0, Reach::Everyone).uncertain());
        assert!(!p("use a hash map", 0.95, 0, Reach::Everyone).uncertain());
    }

    #[test]
    fn sharing_with_EVERYONE_states_what_it_costs() {
        // The consequence belongs on the dialog. An operator picking "all" should see that it ends the
        // blind control, not discover it later when the reading says Uncontrolled.
        let r = Reach::Everyone;
        assert!(!r.keeps_control());
        assert!(r.consequence().contains("ends the round's ability"));
    }

    #[test]
    fn narrower_reaches_keep_the_control_and_say_so() {
        assert!(Reach::One("m2".into()).keeps_control());
        assert!(Reach::One("m2".into()).consequence().contains("the rest stay blind"));
        assert!(Reach::AllButControl("c".into()).keeps_control());
    }

    #[test]
    fn the_countdown_is_visible_rather_than_the_dialog_just_vanishing() {
        // A dialog that disappears without warning reads as a bug; one with a countdown reads as a
        // deadline, and the operator can act on the difference.
        let x = p("hm", 0.9, 1_000, Reach::Everyone);
        assert_eq!(x.remaining_ms(), TIMEOUT_MS - 1_000);
        assert_eq!(p("hm", 0.9, 99_999, Reach::Everyone).remaining_ms(), 0, "saturates, never wraps");
    }
}
