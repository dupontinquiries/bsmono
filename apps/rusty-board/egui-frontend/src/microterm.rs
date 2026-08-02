//! microterm — the strip above the keys, ported from `src/ui/microterm.rs`.
//!
//! One line of screen that changes what it is: a status message, a live voice
//! partial with its level, tappable steno candidates, an LLM prompt, a terminal
//! or a web page. rusty-board is `primary` in the manifest and this was the
//! largest surface the port still lacked.
//!
//! ## The content is a sum type for a reason
//!
//! Six pages, one at a time, and the GTK version has a `GtkStack` page per kind.
//! Modelling it as a sum type rather than a struct of optionals means *"showing
//! a voice partial and steno candidates at once"* is unrepresentable — which is
//! what the stack enforces visually and what a struct of `Option`s would let
//! drift.
//!
//! ## Steno candidates and the fuzzy flag
//!
//! `StenoCandidate { word, fuzzy }` — fuzzy marks a **vowel-approximate** match.
//! It is carried through to the chip so the user can see that "cat" came from an
//! exact phoneme run and "cot" was a guess. Dropping the flag would make a guess
//! look like a certainty, which in a keyboard means committing the wrong word.
//!
//! Exact candidates sort ahead of fuzzy ones: the chip nearest the thumb should
//! be the one most likely to be right.

/// A steno suggestion.
#[derive(Debug, Clone, PartialEq)]
pub struct StenoCandidate {
    pub word: String,
    /// Vowel-approximate rather than exact. Shown, because a guess that looks
    /// like a certainty commits the wrong word.
    pub fuzzy: bool,
}

/// What the strip is showing.
#[derive(Debug, Clone, PartialEq)]
pub enum Content {
    Status(String),
    VoicePreview { partial: String, level: f32 },
    StenoSuggestions { buffer: String, candidates: Vec<StenoCandidate> },
    LlmPrompt { draft: String },
    Terminal,
    WebPage(String),
}

impl Default for Content {
    fn default() -> Self { Content::Status(String::new()) }
}

impl Content {
    /// The page name, matching the GTK stack's children so the two cannot
    /// drift about what "the steno page" is called.
    pub fn page(&self) -> &'static str {
        match self {
            Content::Status(_) => "status",
            Content::VoicePreview { .. } => "voice",
            Content::StenoSuggestions { .. } => "steno",
            Content::LlmPrompt { .. } => "llm",
            Content::Terminal => "terminal",
            Content::WebPage(_) => "webpage",
        }
    }

    /// Is there anything worth showing?
    ///
    /// An empty status is not content — the strip should collapse rather than
    /// occupy a row of a phone screen with nothing in it.
    pub fn is_empty(&self) -> bool {
        match self {
            Content::Status(s) => s.trim().is_empty(),
            Content::VoicePreview { partial, level } => partial.trim().is_empty() && *level <= 0.0,
            Content::StenoSuggestions { buffer, candidates } =>
                buffer.trim().is_empty() && candidates.is_empty(),
            Content::LlmPrompt { .. } | Content::Terminal => false,
            Content::WebPage(u) => u.trim().is_empty(),
        }
    }
}

/// The strip.
#[derive(Debug, Clone, Default)]
pub struct Microterm {
    content: Content,
    visible: bool,
}

impl Microterm {
    pub fn content(&self) -> &Content { &self.content }

    /// `set_content`. Showing content implicitly reveals the strip — the GTK
    /// version's callers all call `show()` right after, and forgetting it is a
    /// silent no-op that looks like the feature not working.
    pub fn set_content(&mut self, c: Content) {
        self.visible = !c.is_empty();
        self.content = c;
    }

    pub fn show(&mut self) { if !self.content.is_empty() { self.visible = true; } }
    pub fn hide(&mut self) { self.visible = false; }
    pub fn is_visible(&self) -> bool { self.visible && !self.content.is_empty() }

    /// Candidates in tap order: exact before fuzzy, then as supplied.
    ///
    /// The chip nearest the thumb should be the one most likely to be right,
    /// and within each group the engine's order is its ranking — re-sorting it
    /// alphabetically would throw that away.
    pub fn ordered_candidates(&self) -> Vec<&StenoCandidate> {
        match &self.content {
            Content::StenoSuggestions { candidates, .. } => {
                let mut v: Vec<&StenoCandidate> = candidates.iter().collect();
                v.sort_by_key(|c| c.fuzzy);   // stable: false (exact) first
                v
            }
            _ => Vec::new(),
        }
    }

    /// Tapping a candidate commits the word and clears the buffer.
    ///
    /// Returns the word so the caller can insert it. Clearing is not optional:
    /// leaving the phoneme run in place means the next keystroke extends a run
    /// the user has already resolved.
    pub fn tap_candidate(&mut self, word: &str) -> Option<String> {
        let hit = match &self.content {
            Content::StenoSuggestions { candidates, .. } =>
                candidates.iter().any(|c| c.word == word),
            _ => false,
        };
        if !hit { return None; }
        self.set_content(Content::Status(String::new()));
        Some(word.to_string())
    }

    /// The voice level, clamped for the meter. Out-of-range input from a
    /// hot mic must not draw past the widget.
    pub fn level(&self) -> f32 {
        match &self.content {
            Content::VoicePreview { level, .. } => level.clamp(0.0, 1.0),
            _ => 0.0,
        }
    }
}

/// Draw the strip. Returns a tapped word, if any.
pub fn microterm_ui(ui: &mut egui::Ui, m: &mut Microterm,
                    colors: &uikit_egui::PaletteColors) -> Option<String> {
    if !m.is_visible() { return None; }
    let mut tapped = None;
    match m.content().clone() {
        Content::Status(s) => { ui.label(egui::RichText::new(s).small()); }
        Content::VoicePreview { partial, .. } => {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::Vec2::new(40.0, 6.0), egui::Sense::hover());
                ui.painter().rect_filled(r, 2.0, colors.bg_card);
                let mut f = r;
                f.set_width(r.width() * m.level());
                ui.painter().rect_filled(f, 2.0, colors.accent);
                ui.label(egui::RichText::new(partial).italics().small());
            });
        }
        Content::StenoSuggestions { buffer, .. } => {
            ui.horizontal_wrapped(|ui| {
                if !buffer.trim().is_empty() {
                    ui.label(egui::RichText::new(&buffer).weak().small());
                }
                for c in m.ordered_candidates().into_iter().cloned().collect::<Vec<_>>() {
                    let t = if c.fuzzy { egui::RichText::new(&c.word).color(colors.text_weak) }
                            else { egui::RichText::new(&c.word).color(colors.accent) };
                    if ui.button(t).clicked() { tapped = Some(c.word.clone()); }
                }
            });
        }
        Content::LlmPrompt { mut draft } => {
            if ui.text_edit_singleline(&mut draft).changed() {
                m.set_content(Content::LlmPrompt { draft });
            }
        }
        Content::Terminal => { ui.label(egui::RichText::new("terminal").small()); }
        Content::WebPage(u) => { ui.label(egui::RichText::new(u).small()); }
    }
    if let Some(w) = tapped.clone() { m.tap_candidate(&w); }
    tapped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(w: &str, fuzzy: bool) -> StenoCandidate {
        StenoCandidate { word: w.into(), fuzzy }
    }

    fn steno() -> Content {
        Content::StenoSuggestions {
            buffer: "k a t".into(),
            candidates: vec![cand("cot", true), cand("cat", false),
                             cand("cut", true), cand("cart", false)],
        }
    }

    #[test]
    fn exact_candidates_sit_nearest_the_thumb() {
        // The chip most likely to be right should be the easiest to hit, and
        // within each group the engine's order IS its ranking.
        let mut m = Microterm::default();
        m.set_content(steno());
        let o: Vec<&str> = m.ordered_candidates().iter().map(|c| c.word.as_str()).collect();
        assert_eq!(o, vec!["cat", "cart", "cot", "cut"]);
    }

    #[test]
    fn the_fuzzy_flag_survives_to_the_chip() {
        // A guess that looks like a certainty commits the wrong word.
        let mut m = Microterm::default();
        m.set_content(steno());
        let o = m.ordered_candidates();
        assert!(!o[0].fuzzy, "cat is exact");
        assert!(o.last().unwrap().fuzzy, "and the tail is approximate");
    }

    #[test]
    fn tapping_a_candidate_clears_the_phoneme_run() {
        // Leaving the run in place means the next keystroke extends something
        // the user has already resolved.
        let mut m = Microterm::default();
        m.set_content(steno());
        assert_eq!(m.tap_candidate("cat").as_deref(), Some("cat"));
        assert!(matches!(m.content(), Content::Status(_)), "buffer gone");
        // and a word that was never offered is refused rather than committed
        m.set_content(steno());
        assert_eq!(m.tap_candidate("dog"), None);
        assert!(matches!(m.content(), Content::StenoSuggestions { .. }), "unchanged");
    }

    #[test]
    fn empty_content_collapses_the_strip() {
        // An empty status is not content, and a blank row on a phone screen is
        // a row you cannot get back.
        let mut m = Microterm::default();
        m.set_content(Content::Status("  ".into()));
        assert!(!m.is_visible());
        m.show();
        assert!(!m.is_visible(), "show() cannot reveal nothing");
        m.set_content(Content::Status("saved".into()));
        assert!(m.is_visible(), "content reveals the strip without a separate show()");
    }

    #[test]
    fn a_terminal_page_is_never_empty_even_with_no_output() {
        // Unlike a status, an open terminal IS the content.
        let mut m = Microterm::default();
        m.set_content(Content::Terminal);
        assert!(m.is_visible());
        assert!(!Content::Terminal.is_empty());
    }

    #[test]
    fn page_names_match_the_gtk_stack() {
        // The two frontends must not disagree about what "the steno page" is
        // called, or a bug report names a page one of them does not have.
        assert_eq!(Content::Status(String::new()).page(), "status");
        assert_eq!(Content::VoicePreview { partial: String::new(), level: 0.0 }.page(), "voice");
        assert_eq!(steno().page(), "steno");
        assert_eq!(Content::LlmPrompt { draft: String::new() }.page(), "llm");
        assert_eq!(Content::Terminal.page(), "terminal");
        assert_eq!(Content::WebPage("x".into()).page(), "webpage");
    }

    #[test]
    fn a_hot_mic_cannot_draw_past_the_meter() {
        let mut m = Microterm::default();
        m.set_content(Content::VoicePreview { partial: "hello".into(), level: 4.0 });
        assert_eq!(m.level(), 1.0);
        m.set_content(Content::VoicePreview { partial: "hello".into(), level: -2.0 });
        assert_eq!(m.level(), 0.0);
        // and a non-voice page has no level rather than a stale one
        m.set_content(Content::Terminal);
        assert_eq!(m.level(), 0.0);
    }

    #[test]
    fn a_silent_voice_preview_with_no_words_is_empty() {
        // Otherwise the strip sits open showing nothing while the mic is armed.
        let c = Content::VoicePreview { partial: "  ".into(), level: 0.0 };
        assert!(c.is_empty());
        let heard = Content::VoicePreview { partial: "  ".into(), level: 0.3 };
        assert!(!heard.is_empty(), "level alone is worth showing");
    }
}
