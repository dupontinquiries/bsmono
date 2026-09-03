//! dictation_session: what a dictation session does with each
//! recognition result, and the instrument for deciding when the
//! acoustic fast path can be trusted.
//!
//! ## The rule this module exists to enforce
//!
//! `acoustic_index` tier1 is a fingerprint→cached-transcript lookup: a
//! PHRASE RECOGNIZER, not an ASR. It cannot carry dictated content,
//! because dictated content is novel every time and there is nothing
//! to match. What it is good at is a small closed vocabulary — which is
//! exactly what `Ctl` is.
//!
//! So: **the fast path may only ever answer with a control verb.** A
//! fast-path result that is not in the control vocabulary is discarded,
//! not inserted. That single restriction is what makes it safe to
//! enable `promote` here before it is safe anywhere else in the app.
//!
//! ## Why dictation is the right place to enable promote first
//!
//! docs/design/voice_perf_findings.md records why `promote` is off: in
//! shadow mode, hits at distance 0.001 carry the wrong transcript, so
//! flipping it on would fire the WRONG COMMAND. Inside dictation the
//! blast radius of a wrong hit is a newline or an indent, landing
//! inside an undo boundary that already exists
//! (`undo_stack::record_voice`). Wrong-and-instantly-undoable beats
//! right-in-1.2-seconds for a verb you say every few sentences.
//!
//! Enabling it needs evidence, not a decision, so `Divergence` here
//! accumulates exactly the agree/disagree record that findings doc asks
//! for — scoped to the control vocabulary rather than to every phrase
//! the user has ever said.
//!
//! ## Not here
//!
//! Filler and hallucination filtering. `voice-kit` already owns that and
//! callers should pre-filter; taking the dependency would cost this
//! module its place in the Qt-less test target. `Router::route` treats
//! an empty transcript as a drop and otherwise assumes what it is
//! handed is real speech.
//!
//! PURE: std only.

use crate::dictate::{Ctl, Mode};

/// Which recognizer produced a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// acoustic_index nearest-neighbour hit. Fast (sub-ms) and
    /// restricted to the control vocabulary by policy.
    Fast,
    /// whisper inference. ~1.0–1.8s, the only source of content.
    Whisper,
}

/// One recognition result arriving at the session.
#[derive(Debug, Clone, PartialEq)]
pub struct Recognition {
    pub text: String,
    pub tier: Tier,
    /// 0..1. For `Fast`, derive from the index distance against the
    /// promote threshold; for `Whisper`, 1.0 unless the caller has
    /// something better.
    pub confidence: f32,
}

impl Recognition {
    pub fn fast(text: &str, confidence: f32) -> Self {
        Recognition { text: text.into(), tier: Tier::Fast, confidence }
    }
    pub fn whisper(text: &str) -> Self {
        Recognition { text: text.into(), tier: Tier::Whisper, confidence: 1.0 }
    }
}

/// Why a result was not acted on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropReason {
    /// Nothing was said.
    Empty,
    /// The fast path returned something outside the control vocabulary.
    /// This is the policy that makes the whole arrangement safe, and it
    /// is expected to fire constantly — every content utterance trips
    /// it. Not an error.
    FastPathContent,
    /// A control verb below the confidence bar.
    LowConfidence,
    /// The session has already ended; later results are stale.
    Ended,
    /// A duplicate of a control we already acted on this utterance.
    AlreadyActed,
}

/// What the caller should do with a result.
#[derive(Debug, Clone, PartialEq)]
pub enum Route {
    /// Act on a control verb now. From `Fast` this is the low-latency
    /// win the whole design is for.
    Control(Ctl),
    /// Insert as dictated content. Only ever from `Whisper`.
    Content(String),
    /// Do nothing.
    Dropped(DropReason),
}

/// Router policy.
#[derive(Debug, Clone, Copy)]
pub struct Policy {
    /// Minimum confidence for a fast-path control to act. Higher than
    /// you would set for whisper, because a fast hit has no second
    /// opinion behind it.
    pub fast_control_min: f32,
    /// Whether the fast path may act at all. This is the local
    /// equivalent of `WEB2QML_TIER1_PROMOTE`, scoped to dictation
    /// controls. Default OFF — same posture as the global flag, so
    /// enabling it is always a deliberate act.
    pub fast_enabled: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Policy { fast_control_min: 0.85, fast_enabled: false }
    }
}

/// Accumulated evidence about whether the fast path agrees with whisper
/// on the control vocabulary.
///
/// The unit is one utterance for which BOTH a fast result and a whisper
/// result arrived. `record` is called with the pair; the ratio is what
/// justifies (or refuses) turning `fast_enabled` on.
#[derive(Debug, Clone, Default)]
pub struct Divergence {
    pub agree: usize,
    pub disagree: usize,
    /// Bounded sample of disagreements, for reading in a log:
    /// (fast text, whisper text).
    pub samples: Vec<(String, String)>,
    cap: usize,
}

impl Divergence {
    pub fn new(cap: usize) -> Self {
        Divergence { cap, ..Default::default() }
    }

    /// Record one shadow comparison. Compares the RESOLVED control, not
    /// the raw text: "newline" and "new line" are the same decision, and
    /// counting them as divergence would hide the failures that matter.
    pub fn record(&mut self, fast_text: &str, whisper_text: &str) {
        let f = classify(fast_text);
        let w = classify(whisper_text);
        if f == w {
            self.agree += 1;
        } else {
            self.disagree += 1;
            if self.samples.len() < self.cap.max(1) {
                self.samples.push((fast_text.to_string(), whisper_text.to_string()));
            }
        }
    }

    pub fn total(&self) -> usize {
        self.agree + self.disagree
    }

    /// Agreement rate, or None below `min_samples` — a 100% rate over
    /// three utterances is not evidence of anything.
    pub fn rate(&self, min_samples: usize) -> Option<f32> {
        if self.total() < min_samples.max(1) {
            return None;
        }
        Some(self.agree as f32 / self.total() as f32)
    }

    /// Whether the evidence supports enabling the fast path. Returns
    /// None while still gathering, so a caller can distinguish "not yet
    /// known" from "known to be bad" — the findings doc's mistake was
    /// treating an unmeasured path as a merely-unflipped switch.
    pub fn promote_safe(&self, min_samples: usize, bar: f32) -> Option<bool> {
        self.rate(min_samples).map(|r| r >= bar)
    }
}

/// Resolve a transcript to a control verb, or None if it is content.
/// Uses the strict whole-utterance rule: a recognizer classifies a whole
/// audio burst, so a control only counts if it IS the utterance.
pub fn classify(text: &str) -> Option<Ctl> {
    let n: String = text
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| !matches!(c, '.' | ',' | '!' | '?' | ';' | ':'))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    Ctl::phrases().iter().find(|(p, _)| *p == n).map(|(_, c)| *c)
}

/// A live dictation session.
#[derive(Debug, Clone)]
pub struct Session {
    pub mode: Mode,
    pub policy: Policy,
    pub divergence: Divergence,
    ended: bool,
    /// Control acted on for the utterance in flight, so a whisper result
    /// arriving after the fast path already handled the same burst does
    /// not fire it twice. Cleared by `next_utterance`.
    acted: Option<Ctl>,
}

impl Session {
    pub fn new(mode: Mode, policy: Policy) -> Self {
        Session {
            mode,
            policy,
            divergence: Divergence::new(32),
            ended: false,
            acted: None,
        }
    }

    pub fn ended(&self) -> bool {
        self.ended
    }

    /// Begin a new audio burst. Clears the per-utterance duplicate
    /// guard; the caller invokes this when capture starts, not when a
    /// result arrives.
    pub fn next_utterance(&mut self) {
        self.acted = None;
    }

    /// Route one recognition result.
    pub fn route(&mut self, r: &Recognition) -> Route {
        if self.ended {
            return Route::Dropped(DropReason::Ended);
        }
        if r.text.trim().is_empty() {
            return Route::Dropped(DropReason::Empty);
        }
        let ctl = classify(&r.text);

        match r.tier {
            Tier::Fast => {
                // The load-bearing restriction: the fast path speaks
                // only the control vocabulary.
                let Some(c) = ctl else {
                    return Route::Dropped(DropReason::FastPathContent);
                };
                if !self.policy.fast_enabled {
                    return Route::Dropped(DropReason::FastPathContent);
                }
                if r.confidence < self.policy.fast_control_min {
                    return Route::Dropped(DropReason::LowConfidence);
                }
                self.commit(c)
            }
            Tier::Whisper => match ctl {
                Some(c) => {
                    if self.acted == Some(c) {
                        // the fast path already handled this burst
                        return Route::Dropped(DropReason::AlreadyActed);
                    }
                    self.commit(c)
                }
                None => Route::Content(r.text.trim().to_string()),
            },
        }
    }

    fn commit(&mut self, c: Ctl) -> Route {
        self.acted = Some(c);
        if c == Ctl::Stop {
            self.ended = true;
        }
        Route::Control(c)
    }

    /// Shadow comparison for one burst where both tiers produced text.
    /// Call this regardless of whether the fast path was allowed to act
    /// — that is the point of shadow mode, and it is how the evidence
    /// accumulates while `fast_enabled` is still false.
    pub fn observe_shadow(&mut self, fast_text: &str, whisper_text: &str) {
        self.divergence.record(fast_text, whisper_text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sess() -> Session {
        Session::new(Mode::Code, Policy { fast_enabled: true, ..Default::default() })
    }

    #[test]
    fn fast_path_never_supplies_content() {
        let mut s = sess();
        let r = s.route(&Recognition::fast("let x equals five", 0.99));
        assert_eq!(r, Route::Dropped(DropReason::FastPathContent));
    }

    #[test]
    fn fast_path_acts_on_a_control_verb() {
        let mut s = sess();
        assert_eq!(s.route(&Recognition::fast("new line", 0.95)), Route::Control(Ctl::NewLine));
    }

    #[test]
    fn fast_path_is_off_by_default() {
        let mut s = Session::new(Mode::Code, Policy::default());
        assert_eq!(
            s.route(&Recognition::fast("new line", 1.0)),
            Route::Dropped(DropReason::FastPathContent)
        );
        // whisper still works with the fast path disabled
        assert_eq!(s.route(&Recognition::whisper("new line")), Route::Control(Ctl::NewLine));
    }

    #[test]
    fn low_confidence_control_is_refused() {
        let mut s = sess();
        assert_eq!(
            s.route(&Recognition::fast("new line", 0.5)),
            Route::Dropped(DropReason::LowConfidence)
        );
    }

    #[test]
    fn whisper_supplies_content() {
        let mut s = sess();
        assert_eq!(
            s.route(&Recognition::whisper("  let x equals five  ")),
            Route::Content("let x equals five".into())
        );
    }

    #[test]
    fn whisper_after_fast_does_not_double_fire() {
        let mut s = sess();
        assert_eq!(s.route(&Recognition::fast("new line", 0.99)), Route::Control(Ctl::NewLine));
        // same burst, whisper catches up 1.2s later
        assert_eq!(
            s.route(&Recognition::whisper("newline")),
            Route::Dropped(DropReason::AlreadyActed)
        );
        // next burst is fair game again
        s.next_utterance();
        assert_eq!(s.route(&Recognition::whisper("new line")), Route::Control(Ctl::NewLine));
    }

    #[test]
    fn whisper_catches_a_control_the_fast_path_missed() {
        let mut s = sess();
        assert_eq!(
            s.route(&Recognition::fast("scratched hat", 0.9)),
            Route::Dropped(DropReason::FastPathContent)
        );
        assert_eq!(
            s.route(&Recognition::whisper("scratch that")),
            Route::Control(Ctl::ScratchThat)
        );
    }

    #[test]
    fn stop_ends_the_session_and_later_results_are_stale() {
        let mut s = sess();
        assert_eq!(s.route(&Recognition::whisper("stop dictating")), Route::Control(Ctl::Stop));
        assert!(s.ended());
        s.next_utterance();
        assert_eq!(s.route(&Recognition::whisper("hello")), Route::Dropped(DropReason::Ended));
    }

    #[test]
    fn empty_is_dropped_not_inserted() {
        let mut s = sess();
        assert_eq!(s.route(&Recognition::whisper("   ")), Route::Dropped(DropReason::Empty));
    }

    #[test]
    fn divergence_compares_decisions_not_spellings() {
        let mut d = Divergence::new(8);
        // different words, same decision — not a divergence
        d.record("newline", "new line");
        d.record("next line", "new line");
        assert_eq!((d.agree, d.disagree), (2, 0));
        // both content — also agreement (neither is a control)
        d.record("let x be five", "let x equals five");
        assert_eq!((d.agree, d.disagree), (3, 0));
        // a real one: fast says control, whisper says content
        d.record("indent", "in dental work");
        assert_eq!(d.disagree, 1);
        assert_eq!(d.samples.len(), 1);
    }

    #[test]
    fn divergence_samples_are_bounded() {
        let mut d = Divergence::new(2);
        for i in 0..10 {
            d.record("indent", &format!("something else {i}"));
        }
        assert_eq!(d.disagree, 10);
        assert_eq!(d.samples.len(), 2);
    }

    #[test]
    fn promote_is_unknown_until_there_is_evidence() {
        let mut d = Divergence::new(8);
        assert_eq!(d.promote_safe(20, 0.98), None, "no data is not the same as bad data");
        for _ in 0..19 {
            d.record("new line", "new line");
        }
        assert_eq!(d.promote_safe(20, 0.98), None, "19 of 20 is still gathering");
        d.record("new line", "new line");
        assert_eq!(d.promote_safe(20, 0.98), Some(true));
    }

    #[test]
    fn promote_refuses_below_the_bar() {
        let mut d = Divergence::new(8);
        for _ in 0..19 {
            d.record("new line", "new line");
        }
        d.record("indent", "in tents");
        assert_eq!(d.rate(20), Some(0.95));
        assert_eq!(d.promote_safe(20, 0.98), Some(false));
        assert_eq!(d.promote_safe(20, 0.90), Some(true));
    }

    #[test]
    fn shadow_accumulates_while_fast_path_is_disabled() {
        let mut s = Session::new(Mode::Code, Policy::default());
        assert!(!s.policy.fast_enabled);
        s.observe_shadow("new line", "new line");
        s.observe_shadow("indent", "indent");
        assert_eq!(s.divergence.agree, 2);
    }

    #[test]
    fn classify_is_whole_utterance_only() {
        assert_eq!(classify("New line."), Some(Ctl::NewLine));
        assert_eq!(classify("write a new line to the log"), None);
    }
}
