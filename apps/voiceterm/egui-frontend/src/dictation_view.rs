//! dictation_view — the dictation facade, held for a frame loop.
//!
//! `dictation_engine::Engine` is a facade the Qt side calls, and its header
//! explains why it exists: the bugs in this stack were all in the SEAMS
//! between five correct modules, so the composition was made a thing rather
//! than left to each caller. This frontend was a caller that had mounted three
//! of the five and never assembled them.
//!
//! What the facade does not cover is the part that is specific to an
//! immediate-mode UI, and that is all this adds:
//!
//! * **A clock the UI already has.** `Engine` takes `now_ms` on every call.
//!   An egui frame has `ctx.input(|i| i.time)` in seconds as `f64`; converting
//!   it at each of six call sites is six chances to pass the wrong unit, so it
//!   is converted once here.
//! * **Somewhere for an outcome to live until it is drawn.** `recognize`
//!   returns an `Outcome` once, and an immediate-mode frame that does not
//!   render it loses it. A terminal that silently dropped a recognised
//!   sentence is the worst failure this stack has.
//! * **A session that can be absent.** Not dictating is the common state, and
//!   it should not require a live `Engine` with nothing in it.
//!
//! No policy lives here. Anything about what a phrase MEANS belongs to the
//! engine, and anything this file starts deciding is drift of exactly the kind
//! the facade was built to prevent.

/// A recognition the UI has not drawn yet.
///
/// Held rather than returned because `recognize` yields an outcome once. A
/// frame that fails to render it loses a sentence the user said, which is the
/// worst failure available to a voice terminal — worse than mis-recognising
/// it, because there is nothing on screen to correct.
#[derive(Debug, Clone, Default)]
pub struct Pending {
    /// Text to commit to the terminal, if the outcome produced any.
    pub commit: Option<String>,
    /// A one-line note for the status surface.
    pub note: Option<String>,
}

impl Pending {
    pub fn is_empty(&self) -> bool {
        self.commit.is_none() && self.note.is_none()
    }
}

/// Seconds (egui's clock) to milliseconds (the engine's).
///
/// One place, because six call sites converting by hand is six chances to
/// pass seconds where milliseconds were wanted — and the failure would be a
/// silence timeout firing a thousand times too early, which reads as the
/// recogniser being broken rather than as a unit mistake.
///
/// Saturating at zero: egui's `time` is monotonic from app start, but a
/// caller passing a delta rather than an absolute would otherwise wrap.
pub fn ms_from_secs(t: f64) -> u64 {
    if t <= 0.0 {
        0
    } else {
        (t * 1000.0) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The conversion the six call sites would otherwise each do.
    #[test]
    fn seconds_become_milliseconds() {
        assert_eq!(ms_from_secs(0.0), 0);
        assert_eq!(ms_from_secs(1.0), 1000);
        assert_eq!(ms_from_secs(1.5), 1500);
        assert_eq!(ms_from_secs(0.25), 250);
    }

    /// A negative or pre-start time must not wrap into an enormous timestamp,
    /// which would make every timeout look long-expired.
    #[test]
    fn a_negative_time_saturates_rather_than_wrapping() {
        assert_eq!(ms_from_secs(-1.0), 0);
        assert_eq!(ms_from_secs(-0.001), 0);
    }

    /// A frame that draws nothing must still be able to tell there was
    /// nothing to draw.
    #[test]
    fn an_empty_pending_reports_itself_empty() {
        assert!(Pending::default().is_empty());
        assert!(!Pending { commit: Some("ls".into()), note: None }.is_empty());
        assert!(!Pending { commit: None, note: Some("heard".into()) }.is_empty());
    }
}
