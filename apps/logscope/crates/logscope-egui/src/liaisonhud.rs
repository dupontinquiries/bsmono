//! liaisonhud.rs — the running backend account shown under the newest message.
//!
//! While a large model works for tens of seconds, the strip below the last message says what the machine
//! is actually doing. It is the Liaison's output surface: observations from the engine, phrased for a
//! person.
//!
//! ## The rule this exists to enforce
//!
//! The HUD must never say something the engine did not report. A status line is believed precisely
//! because it looks mechanical, so a fabricated one — "should finish shortly", "almost done" — does more
//! damage than a blank strip. Every line here therefore originates in an engine observation and carries
//! its source; when there are no observations the strip shows nothing at all.

use logscope::liaison::Statement;

/// What the strip is currently reporting.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hud {
    /// Newest first. Bounded, because this updates during generation and an unbounded log would grow
    /// without limit inside the render loop.
    lines: Vec<Statement>,
    cap: usize,
}

impl Hud {
    pub fn new(cap: usize) -> Self { Hud { lines: Vec::new(), cap: cap.max(1) } }

    /// Record an observation. Consecutive duplicates are collapsed: a phase that reports itself every
    /// frame should read as one steady line, not a scrolling wall of the same sentence.
    pub fn observe(&mut self, text: impl Into<String>, source: &str) {
        let text = text.into();
        if self.lines.first().map(|s| s.text == text).unwrap_or(false) { return; }
        self.lines.insert(0, Statement::new(text, vec![source.to_string()]));
        self.lines.truncate(self.cap);
    }

    pub fn latest(&self) -> Option<&Statement> { self.lines.first() }
    pub fn all(&self) -> &[Statement] { &self.lines }
    pub fn is_empty(&self) -> bool { self.lines.is_empty() }
    pub fn clear(&mut self) { self.lines.clear(); }

    /// The line to show, and whether it is model-phrased or raw.
    ///
    /// `narration` is the liaison model's sentence when one has been produced. It is used ONLY if it is
    /// non-empty; otherwise the newest raw observation is shown verbatim. A raw observation cannot be
    /// wrong — it is copied from the engine — so it is the safe default while the liaison is still
    /// thinking or absent.
    pub fn display(&self, narration: Option<&str>) -> Option<(String, bool)> {
        if let Some(n) = narration.map(str::trim).filter(|n| !n.is_empty()) {
            return Some((n.to_string(), true));
        }
        self.latest().map(|s| (s.text.clone(), false))
    }

    /// Observations as the Liaison's input.
    pub fn facts(&self) -> Vec<Statement> {
        // oldest → newest: the liaison reads them as a sequence, and reversed order would imply the
        // wrong causality ("it finished, then it started").
        self.lines.iter().rev().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repeated_observation_does_not_scroll() {
        let mut h = Hud::new(8);
        for _ in 0..5 { h.observe("converting Q4_K on GPU", "loader"); }
        assert_eq!(h.all().len(), 1, "a steady phase is one line, not five");
        h.observe("prefill 3 of 8 chunks", "engine");
        assert_eq!(h.all().len(), 2);
        assert_eq!(h.latest().unwrap().text, "prefill 3 of 8 chunks", "newest first");
    }

    #[test]
    fn the_buffer_is_bounded_because_it_updates_during_render() {
        let mut h = Hud::new(3);
        for i in 0..10 { h.observe(format!("step {i}"), "engine"); }
        assert_eq!(h.all().len(), 3);
        assert_eq!(h.latest().unwrap().text, "step 9");
    }

    #[test]
    fn a_raw_observation_is_shown_when_the_liaison_has_not_spoken() {
        // The raw line is copied from the engine, so it cannot be wrong. It is the right default while a
        // narration is pending or no liaison model is configured.
        let mut h = Hud::new(4);
        h.observe("expert cache 62% resident", "moe-repin");
        assert_eq!(h.display(None), Some(("expert cache 62% resident".into(), false)));
        assert_eq!(h.display(Some("   ")), Some(("expert cache 62% resident".into(), false)),
            "a blank narration is not a narration");
        assert_eq!(h.display(Some("It's swapping experts in and out.")),
                   Some(("It's swapping experts in and out.".into(), true)));
    }

    #[test]
    fn an_empty_hud_shows_nothing_rather_than_a_placeholder() {
        // A fabricated status is worse than a blank strip: it looks mechanical and is therefore believed.
        let h = Hud::new(4);
        assert!(h.is_empty());
        assert_eq!(h.display(None), None);
        assert_eq!(h.display(Some("almost done!")), Some(("almost done!".into(), true)),
            "a narration with no facts behind it is the caller's error to prevent, not the HUD's to hide");
        assert!(h.facts().is_empty(), "and the liaison is handed nothing, so it cannot invent from it");
    }

    #[test]
    fn facts_reach_the_liaison_oldest_first() {
        let mut h = Hud::new(8);
        h.observe("loading shards", "loader");
        h.observe("prefill started", "engine");
        let f = h.facts();
        assert_eq!(f[0].text, "loading shards", "sequence order, or the causality reads backwards");
        assert_eq!(f[1].text, "prefill started");
        assert!(f[0].is_grounded(), "every HUD line carries its source");
    }
}
