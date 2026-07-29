//! thinkerwatch.rs — a rolling view of what the Thinker is writing, condensed, with a forecast.
//!
//! The operator watching a long generation wants two things the raw stream does not give: what is
//! happening RIGHT NOW in one line, and some sense of how much longer. This keeps a window of the most
//! recent output, hands it to the Liaison to condense, and offers a forecast.
//!
//! ## The forecast is the dangerous part
//!
//! A prediction rendered beside facts reads as a fact. So the forecast here is derived from OBSERVABLE
//! STRUCTURE — an unclosed code fence means code is still coming; a function signature just written
//! means a body next — rather than from asking a model to speculate. Structural inference is cheap,
//! checkable, and wrong in ways a reader can see. A model asked "what happens next" will always answer,
//! confidently, including when there is no basis at all.
//!
//! When there is no structural signal, the forecast is `None` and nothing is shown. An empty forecast is
//! more useful than a confident guess, because the absence itself is information.

/// Words kept in the rolling window.
pub const WINDOW_WORDS: usize = 100;
/// Minimum gap between condensations. Summarising every token would cost more than the generation.
pub const POLL_MS: u64 = 2500;

/// A rolling window of the Thinker's most recent output.
#[derive(Clone, Debug, Default)]
pub struct ThinkerWindow {
    words: std::collections::VecDeque<String>,
    total_words: usize,
    last_poll: Option<std::time::Instant>,
}

impl ThinkerWindow {
    pub fn new() -> Self { Self::default() }

    /// Feed newly generated text.
    pub fn push(&mut self, chunk: &str) {
        for w in chunk.split_whitespace() {
            self.words.push_back(w.to_string());
            self.total_words += 1;
            if self.words.len() > WINDOW_WORDS { self.words.pop_front(); }
        }
    }

    pub fn text(&self) -> String { self.words.iter().cloned().collect::<Vec<_>>().join(" ") }
    pub fn total_words(&self) -> usize { self.total_words }
    pub fn is_empty(&self) -> bool { self.words.is_empty() }

    /// Is a condensation due? Rate-limited, and never on an empty window.
    ///
    /// Summarising every token would cost more than the generation it is describing — the Liaison is
    /// cheap, not free, and it competes for the same GPU unless it is CPU-placed.
    pub fn poll_due(&self, now: std::time::Instant) -> bool {
        if self.is_empty() { return false; }
        match self.last_poll { None => true, Some(t) => now.duration_since(t).as_millis() as u64 >= POLL_MS }
    }

    pub fn mark_polled(&mut self, now: std::time::Instant) { self.last_poll = Some(now); }
}

/// What the output's STRUCTURE says is probably coming next.
///
/// Deliberately a small closed set. Each variant corresponds to a signal that is present or absent in
/// the text — nothing here is a guess about intent.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Forecast {
    /// An unclosed code fence: more code before prose resumes.
    FinishingCodeBlock,
    /// A signature or opening brace with no matching close: a body is coming.
    WritingBody,
    /// An unfinished enumerated list.
    ContinuingList,
    /// A sentence ended and nothing is open: it may be nearly done.
    MayBeWrappingUp,
}

impl Forecast {
    pub fn describe(self) -> &'static str {
        match self {
            Forecast::FinishingCodeBlock => "still inside a code block",
            Forecast::WritingBody => "opened a block it has not closed",
            Forecast::ContinuingList => "part-way through a list",
            Forecast::MayBeWrappingUp => "nothing left open — may be finishing",
        }
    }

    /// The label, marked as inference rather than observation.
    ///
    /// The tilde is doing real work: it separates "this is what it wrote" from "this is what I think
    /// comes next", at a glance, in a strip too small for a sentence of hedging.
    pub fn label(self) -> String { format!("~ {}", self.describe()) }
}

/// Infer a forecast from the text's structure, or None when nothing is open.
///
/// Counts fences and braces over the WHOLE generation, not the window — a code block opened 300 words
/// ago is still open, and a window-local count would report it closed the moment the opening scrolled
/// out. That would be confidently wrong in exactly the situation the forecast exists for.
pub fn forecast(full_text: &str) -> Option<Forecast> {
    let fences = full_text.matches("```").count();
    if fences % 2 == 1 { return Some(Forecast::FinishingCodeBlock); }

    let opens = full_text.matches('{').count();
    let closes = full_text.matches('}').count();
    if opens > closes { return Some(Forecast::WritingBody); }

    let trimmed = full_text.trim_end();
    if trimmed.is_empty() { return None; }
    // An unfinished list item: the last line starts like a list and has no terminator.
    if let Some(last) = trimmed.lines().last() {
        let l = last.trim_start();
        let listy = l.starts_with("- ") || l.starts_with("* ")
            || l.chars().next().is_some_and(|c| c.is_ascii_digit()) && l.contains(". ");
        if listy && !l.ends_with('.') { return Some(Forecast::ContinuingList); }
    }
    if trimmed.ends_with('.') || trimmed.ends_with('!') || trimmed.ends_with('?') {
        return Some(Forecast::MayBeWrappingUp);
    }
    None
}

/// What the strip shows.
#[derive(Clone, PartialEq, Debug)]
pub struct Watch {
    /// The Liaison's one-line condensation, or None before the first poll returns.
    pub gist: Option<String>,
    pub forecast: Option<Forecast>,
    pub words_so_far: usize,
}

impl Watch {
    /// The strip text. Short enough for a status bar.
    pub fn line(&self) -> String {
        let g = self.gist.as_deref().unwrap_or("…");
        match self.forecast {
            Some(f) => format!("{g}  ·  {}  ·  {}w", f.label(), self.words_so_far),
            None => format!("{g}  ·  {}w", self.words_so_far),
        }
    }
}

/// The prompt asking the Liaison to condense the window.
pub const CONDENSE_PROMPT: &str = "\
Below is the most recent output from another model, mid-generation. Say in ONE short clause what it is \
currently doing — not what it has concluded, which you cannot know from a fragment. Under ten words. Do \
not speculate about what comes next; that is inferred separately from the text's structure.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_keeps_only_the_most_recent_words_but_counts_all_of_them() {
        let mut w = ThinkerWindow::new();
        for i in 0..250 { w.push(&format!("word{i} ")); }
        assert_eq!(w.text().split_whitespace().count(), WINDOW_WORDS);
        assert_eq!(w.total_words(), 250, "the running total is what tells you how long this has gone on");
        assert!(w.text().starts_with("word150"), "oldest dropped first");
    }

    #[test]
    fn a_forecast_counts_fences_over_the_WHOLE_text_not_the_window() {
        // A code block opened 300 words ago is still open. Counting within the window would report it
        // closed the moment the opening scrolled out — confidently wrong in exactly the situation the
        // forecast exists for.
        let long = format!("```rust\n{}", "filler ".repeat(500));
        assert_eq!(forecast(&long), Some(Forecast::FinishingCodeBlock));
        let closed = format!("```rust\n{}\n```", "filler ".repeat(500));
        assert_ne!(forecast(&closed), Some(Forecast::FinishingCodeBlock));
    }

    #[test]
    fn no_structural_signal_means_NO_forecast_rather_than_a_guess() {
        // An empty forecast is more useful than a confident one, because the absence is information. A
        // model asked "what comes next" would always answer, including when there is no basis.
        assert_eq!(forecast(""), None);
        assert_eq!(forecast("the quick brown fox"), None, "mid-sentence, nothing open");
    }

    #[test]
    fn an_unclosed_brace_predicts_a_body() {
        assert_eq!(forecast("fn main() {"), Some(Forecast::WritingBody));
        assert_eq!(forecast("fn main() { }"), None);
    }

    #[test]
    fn a_finished_sentence_with_nothing_open_suggests_wrapping_up() {
        assert_eq!(forecast("That completes the change."), Some(Forecast::MayBeWrappingUp));
    }

    #[test]
    fn an_unfinished_list_item_continues_the_list() {
        assert_eq!(forecast("- first thing.\n- second thing"), Some(Forecast::ContinuingList));
        assert_ne!(forecast("- first thing.\n- second thing."), Some(Forecast::ContinuingList));
    }

    #[test]
    fn the_forecast_is_marked_as_INFERENCE_in_the_label() {
        // A prediction rendered beside facts reads as a fact. The tilde separates "what it wrote" from
        // "what I think comes next" at a glance, in a strip too small for a sentence of hedging.
        assert!(Forecast::WritingBody.label().starts_with("~ "));
        for f in [Forecast::FinishingCodeBlock, Forecast::ContinuingList, Forecast::MayBeWrappingUp] {
            assert!(f.label().starts_with("~ "), "{f:?}");
        }
    }

    #[test]
    fn condensing_is_rate_limited_and_never_runs_on_an_empty_window() {
        // Summarising every token would cost more than the generation it describes.
        let mut w = ThinkerWindow::new();
        let t0 = std::time::Instant::now();
        assert!(!w.poll_due(t0), "nothing to condense yet");
        w.push("some output");
        assert!(w.poll_due(t0), "first poll is immediate");
        w.mark_polled(t0);
        assert!(!w.poll_due(t0), "and then rate-limited");
        assert!(w.poll_due(t0 + std::time::Duration::from_millis(POLL_MS + 1)));
    }

    #[test]
    fn the_strip_reads_sensibly_before_the_first_condensation_returns() {
        // The first poll takes a moment; the strip must not be blank or claim something it does not know.
        let w = Watch { gist: None, forecast: Some(Forecast::WritingBody), words_so_far: 42 };
        let l = w.line();
        assert!(l.contains('…') && l.contains("42w") && l.contains('~'));
    }

    #[test]
    fn the_condense_prompt_forbids_the_liaison_from_forecasting() {
        // Two sources of "what happens next" would disagree, and the model's version would sound better
        // while being groundless.
        assert!(CONDENSE_PROMPT.contains("Do not speculate about what comes next"));
        assert!(CONDENSE_PROMPT.contains("not what it has concluded"));
    }
}
