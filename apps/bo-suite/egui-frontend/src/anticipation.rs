//! anticipation — the dot that shows the controller is paying attention.
//!
//! Ported from `AnticipationDot.qml`, whose own header states the design: a tiny
//! dot in the header strip whose opacity **breathes** when a strong prediction is
//! queued, mounted next to the device name "so it doesn't compete with any
//! actual control".
//!
//! The bands are the contract, and they are quoted from that header:
//!
//! ```text
//!   < 0.30    → hidden (just noise; no breathing)
//!   0.30-0.55 → dim,  slow  breath
//!   0.55-0.80 → mid,  mid   breath
//!   > 0.80    → bright, faster breath
//! ```
//!
//! ## Why hidden below 0.30 is the important one
//!
//! A prediction engine always has *a* top prediction — there is no "no idea"
//! state — so a dot that renders whatever the confidence happens to be is lit
//! permanently, and a permanently lit indicator carries no information. The
//! floor is what makes the dot mean something when it appears.
//!
//! ## Breathing rate is the second channel
//!
//! Opacity alone would need a user to remember which brightness meant what.
//! Rate is comparative — a faster breath reads as *more* without a reference —
//! so the two together survive being glanced at, which is the only way this
//! indicator is ever looked at.

/// Below this a prediction is noise and the dot stays hidden.
pub const FLOOR: f32 = 0.30;

/// How confident, in the QML's bands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band {
    /// Nothing worth showing.
    Hidden,
    Dim,
    Mid,
    Bright,
}

impl Band {
    pub fn of(confidence: f32) -> Band {
        // Ordered high to low so the boundaries read the way the header writes
        // them, and NaN falls through to Hidden rather than matching a band —
        // a confidence that is not a number is not a strong prediction.
        if confidence > 0.80 { Band::Bright }
        else if confidence >= 0.55 { Band::Mid }
        else if confidence >= FLOOR { Band::Dim }
        else { Band::Hidden }
    }

    /// Peak opacity for the breath.
    pub fn opacity(self) -> f32 {
        match self { Band::Hidden => 0.0, Band::Dim => 0.35, Band::Mid => 0.6, Band::Bright => 0.9 }
    }

    /// Seconds per full breath. Faster means more confident, and rate is the
    /// channel that works without a reference to compare against.
    pub fn period_secs(self) -> f32 {
        match self { Band::Hidden => 0.0, Band::Dim => 4.0, Band::Mid => 2.6, Band::Bright => 1.6 }
    }

    pub fn visible(self) -> bool { self != Band::Hidden }
}

/// The dot's live state.
#[derive(Debug, Clone, Default)]
pub struct AnticipationDot {
    pub confidence: f32,
    /// What we would say if tapped. Empty when there is nothing to say.
    pub top_action: String,
    /// Phase of the breath, 0..1.
    phase: f32,
    /// Set while the tap tooltip is showing.
    reveal_for: f32,
}

/// How long a tapped tooltip stays up. Long enough to read a short phrase,
/// short enough that it does not become a thing to dismiss.
pub const REVEAL_SECS: f32 = 2.5;

impl AnticipationDot {
    pub fn band(&self) -> Band { Band::of(self.confidence) }

    /// Advance the breath.
    ///
    /// The phase is kept across a band change rather than reset, so a
    /// prediction strengthening mid-breath speeds up smoothly instead of
    /// snapping to full brightness — the dot is meant to be quiet, and a jump
    /// is the one thing that would draw the eye.
    pub fn tick(&mut self, dt: f32) {
        let period = self.band().period_secs();
        if period > 0.0 {
            self.phase = (self.phase + dt.max(0.0) / period).fract();
        }
        if self.reveal_for > 0.0 { self.reveal_for = (self.reveal_for - dt.max(0.0)).max(0.0); }
    }

    /// Current opacity, 0 when hidden.
    pub fn opacity(&self) -> f32 {
        let b = self.band();
        if !b.visible() { return 0.0; }
        // A sine breath rather than a triangle: the eye notices the corner of a
        // linear ramp, which is exactly the attention this is trying not to take.
        let breath = (self.phase * std::f32::consts::TAU).sin() * 0.5 + 0.5;
        // Never fully out — a dot that vanishes and returns reads as a glitch
        // rather than a rhythm.
        b.opacity() * (0.45 + 0.55 * breath)
    }

    /// Tapped. Returns the text to show, or `None` when there is nothing.
    pub fn tap(&mut self) -> Option<&str> {
        if !self.band().visible() || self.top_action.trim().is_empty() { return None; }
        self.reveal_for = REVEAL_SECS;
        Some(&self.top_action)
    }

    pub fn revealed(&self) -> Option<&str> {
        if self.reveal_for > 0.0 { Some(&self.top_action) } else { None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dot(c: f32) -> AnticipationDot {
        AnticipationDot { confidence: c, top_action: "warm the kitchen speaker".into(),
                          ..Default::default() }
    }

    #[test]
    fn the_bands_are_the_qml_headers() {
        // Quoted from AnticipationDot.qml. A port that shifts a boundary makes
        // the dot behave differently for the same prediction.
        assert_eq!(Band::of(0.0), Band::Hidden);
        assert_eq!(Band::of(0.29), Band::Hidden);
        assert_eq!(Band::of(0.30), Band::Dim);
        assert_eq!(Band::of(0.54), Band::Dim);
        assert_eq!(Band::of(0.55), Band::Mid);
        assert_eq!(Band::of(0.80), Band::Mid, "0.80 is not yet bright");
        assert_eq!(Band::of(0.81), Band::Bright);
    }

    #[test]
    fn a_weak_prediction_shows_nothing_at_all() {
        // A prediction engine always has A top prediction — there is no "no
        // idea" state — so a dot without a floor is lit permanently, and a
        // permanently lit indicator carries no information.
        let mut d = dot(0.1);
        d.tick(1.0);
        assert_eq!(d.opacity(), 0.0);
        assert_eq!(d.tap(), None, "and there is nothing to reveal");
    }

    #[test]
    fn a_stronger_prediction_breathes_faster() {
        // Rate is the channel that works without a reference to compare
        // against; opacity alone would need the user to remember which
        // brightness meant what.
        assert!(Band::Bright.period_secs() < Band::Mid.period_secs());
        assert!(Band::Mid.period_secs() < Band::Dim.period_secs());
    }

    #[test]
    fn the_breath_never_goes_fully_out() {
        // A dot that vanishes and returns reads as a glitch rather than a
        // rhythm.
        let mut d = dot(0.9);
        let mut min = f32::MAX;
        for _ in 0..200 { d.tick(0.02); min = min.min(d.opacity()); }
        assert!(min > 0.0, "went dark at {min}");
        assert!(min < Band::Bright.opacity(), "and it does breathe");
    }

    #[test]
    fn strengthening_mid_breath_does_not_snap() {
        // The dot is meant to be quiet, and a jump is the one thing that would
        // draw the eye. The phase carries across a band change.
        let mut d = dot(0.35);
        for _ in 0..40 { d.tick(0.02); }
        let before = d.opacity();
        d.confidence = 0.9;
        let after = d.opacity();
        // brighter, but proportionally — not reset to the top of a new breath
        assert!(after > before);
        let ratio = after / before.max(1e-6);
        let band_ratio = Band::Bright.opacity() / Band::Dim.opacity();
        assert!((ratio - band_ratio).abs() < 0.01, "phase was reset: {ratio} vs {band_ratio}");
    }

    #[test]
    fn tapping_reveals_the_action_and_it_expires() {
        let mut d = dot(0.9);
        assert_eq!(d.tap(), Some("warm the kitchen speaker"));
        assert!(d.revealed().is_some());
        d.tick(REVEAL_SECS + 0.1);
        assert!(d.revealed().is_none(), "it goes away rather than needing dismissal");
    }

    #[test]
    fn a_confident_prediction_with_no_text_reveals_nothing() {
        // The engine can be sure something will happen without a phrase for it,
        // and an empty tooltip is worse than no tooltip.
        let mut d = AnticipationDot { confidence: 0.95, top_action: "  ".into(), ..Default::default() };
        assert_eq!(d.tap(), None);
    }

    #[test]
    fn a_nonsense_confidence_hides_rather_than_matching_a_band() {
        // NaN reaching a comparison chain is how an indicator ends up lit for
        // no reason.
        assert_eq!(Band::of(f32::NAN), Band::Hidden);
        assert_eq!(Band::of(-1.0), Band::Hidden);
        assert_eq!(Band::of(2.0), Band::Bright, "over-confident is still bright");
    }
}
