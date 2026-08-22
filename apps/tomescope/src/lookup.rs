//! Offline word lookup: a small bundled glossary plus per-book index-term
//! lookup (from the book's own back-of-book index, if the format
//! supplied one — currently FB2/EPUB via `IndexTerm`). No network access;
//! this is intentionally not a full dictionary, just enough for quick
//! in-context definitions of common English words and any book-supplied
//! glossary terms.

use crate::content::{BookContent, IndexTerm, LocusRange};
use crate::reader::Selection;
use eframe::egui;
use std::collections::HashMap;

/// A tiny built-in glossary. Real deployments would ship a bundled
/// wordlist asset; this is a representative seed covering common
/// literary/academic vocabulary so the lookup path has something to show
/// even with zero external assets.
pub fn builtin_glossary() -> HashMap<&'static str, &'static str> {
    let pairs: &[(&str, &str)] = &[
        ("ephemeral", "Lasting for a very short time."),
        ("ubiquitous", "Present, appearing, or found everywhere."),
        ("paradigm", "A typical example or pattern of something; a model."),
        ("juxtapose", "To place two things side by side, especially for contrasting effect."),
        ("cognizant", "Having knowledge or being aware of something."),
        ("empirical", "Based on observation or experience rather than theory."),
        ("axiom", "A statement regarded as self-evidently true, taken as a starting point."),
        ("corollary", "A proposition that follows naturally from one already proven."),
        ("heuristic", "A practical method not guaranteed to be optimal, used for quick solutions."),
        ("anachronism", "Something placed in the wrong historical time period."),
        ("verisimilitude", "The appearance of being true or real."),
        ("dichotomy", "A division into two contrasting parts."),
        ("nascent", "Just coming into existence and beginning to develop."),
        ("mercurial", "Subject to sudden or unpredictable changes of mood."),
        ("quixotic", "Extremely idealistic, unrealistic, and impractical."),
        ("synthesis", "The combination of ideas to form a theory or system."),
        ("catalyst", "A person or thing that precipitates an event or change."),
        ("dogmatic", "Inclined to lay down principles as incontrovertibly true."),
        ("pragmatic", "Dealing with things sensibly and realistically."),
        ("plausible", "Seeming reasonable or probable."),
    ];
    pairs.iter().copied().collect()
}

pub struct Lookup {
    glossary: HashMap<&'static str, &'static str>,
}

impl Default for Lookup {
    fn default() -> Self {
        Self {
            glossary: builtin_glossary(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum LookupResult {
    Glossary { term: String, definition: String },
    BookIndex { term: String, locations: Vec<usize> },
    NotFound { term: String },
}

impl Lookup {
    /// Look up a word: check the book's own index first (more specific to
    /// this text), then fall back to the built-in glossary.
    pub fn lookup(&self, word: &str, content: &BookContent) -> LookupResult {
        let normalized = word.trim().to_lowercase();
        let normalized = normalized.trim_matches(|c: char| !c.is_alphanumeric());
        if normalized.is_empty() {
            return LookupResult::NotFound { term: word.to_string() };
        }

        let index_hits: Vec<usize> = content
            .index_terms
            .iter()
            .filter(|t: &&IndexTerm| t.term.to_lowercase() == normalized)
            .flat_map(|t| t.blocks.clone())
            .collect();
        if !index_hits.is_empty() {
            return LookupResult::BookIndex {
                term: normalized.to_string(),
                locations: index_hits,
            };
        }

        if let Some(def) = self.glossary.get(normalized) {
            return LookupResult::Glossary {
                term: normalized.to_string(),
                definition: def.to_string(),
            };
        }

        LookupResult::NotFound {
            term: normalized.to_string(),
        }
    }

    /// Full-text prefix search over the glossary, for a "browse dictionary"
    /// panel.
    pub fn glossary_prefix(&self, prefix: &str) -> Vec<(&'static str, &'static str)> {
        let prefix = prefix.to_lowercase();
        let mut out: Vec<(&'static str, &'static str)> = self
            .glossary
            .iter()
            .filter(|(k, _)| k.starts_with(&prefix))
            .map(|(k, v)| (*k, *v))
            .collect();
        out.sort_by_key(|(k, _)| *k);
        out
    }
}

/// Text under a selection, for lookup. Single-block only: a lookup target is
/// a word or short phrase, so a selection spanning multiple blocks returns
/// `None` rather than guessing at how to join them — asking "what does this
/// mean" about a range that crosses a paragraph break is almost always a
/// drag that overshot, not an intentional multi-block query.
///
/// Slices the block's own `plain_text()` directly rather than going through
/// `BookContent::snippet` — `snippet` unconditionally prefixes/suffixes an
/// ellipsis whenever the range doesn't start at 0 or end at the block's
/// length (that's correct for its actual job, a context snippet around a
/// search hit, but wrong here: `Lookup::lookup` would then look up
/// `"…ephemeral…"` instead of `"ephemeral"`, silently failing every glossary
/// hit except a selection that happens to cover a whole block). Caught by
/// `a_lookup_looks_up_the_real_selected_word_not_the_whole_block` below,
/// which is the one test in this pair worth keeping if a future edit ever
/// reintroduces the `snippet`-based version.
pub fn selection_text(content: &BookContent, range: LocusRange) -> Option<String> {
    if !range.single_block() {
        return None;
    }
    let block = content.blocks.get(range.from.block)?;
    let chars: Vec<char> = block.plain_text().chars().collect();
    let a = range.from.ch.min(chars.len());
    let z = range.to.ch.min(chars.len());
    if z <= a {
        return None;
    }
    let text: String = chars[a..z].iter().collect();
    if text.trim().is_empty() { None } else { Some(text) }
}

/// Draw the "define" popover for the current selection, if `open`. Mirrors
/// `reader::draw_footnote_popover`'s shape (a dismissable `egui::Window`) so
/// the two popovers feel like the same feature family rather than two
/// different UI languages for "here is some info about what you tapped".
///
/// Looks up fresh from `selection` every call rather than caching the result
/// at open-time: the selection can still be dragged while this is open (nothing
/// stops it), and re-deriving each frame means the shown definition always
/// matches what is actually highlighted rather than freezing on whatever was
/// selected the instant the button was pressed.
pub fn draw_lookup_popover(
    ctx: &egui::Context,
    content: &BookContent,
    lookup: &Lookup,
    selection: &Selection,
    open: &mut bool,
    colors: &uikit_egui::PaletteColors,
) {
    if !*open {
        return;
    }
    let Some(range) = selection.range() else {
        *open = false;
        return;
    };
    let mut still_open = true;
    egui::Window::new("Define")
        .id(egui::Id::new("tomescope_lookup_popover"))
        .collapsible(false)
        .resizable(false)
        .open(&mut still_open)
        .show(ctx, |ui| {
            ui.set_max_width(320.0);
            match selection_text(content, range) {
                None => {
                    ui.colored_label(colors.text_weak, "select a single word or short phrase");
                }
                Some(word) => match lookup.lookup(&word, content) {
                    LookupResult::Glossary { term, definition } => {
                        ui.label(egui::RichText::new(term).color(colors.text).strong());
                        ui.label(egui::RichText::new(definition).color(colors.text));
                    }
                    LookupResult::BookIndex { term, locations } => {
                        ui.label(egui::RichText::new(term).color(colors.text).strong());
                        ui.colored_label(
                            colors.text_weak,
                            format!("in this book's index — {} mention(s)", locations.len()),
                        );
                    }
                    LookupResult::NotFound { term } => {
                        ui.colored_label(colors.text_weak, format!("no definition found for \"{term}\""));
                    }
                },
            }
        });
    if !still_open {
        *open = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{Block, BookContent, Locus, Span};
    use crate::reader::Selection;

    fn one_paragraph(text: &str) -> BookContent {
        BookContent {
            blocks: vec![Block::Paragraph { spans: vec![Span::plain(text)] }],
            toc: vec![],
            images: vec![],
            footnotes: vec![],
            index_terms: vec![],
            raster: None,
            page_labels: vec![],
        }
    }

    #[test]
    fn selection_text_extracts_the_highlighted_word() {
        let content = one_paragraph("an ephemeral moment");
        // "ephemeral" is chars 3..12
        let range = LocusRange {
            from: Locus { block: 0, ch: 3 },
            to: Locus { block: 0, ch: 12 },
        };
        assert_eq!(selection_text(&content, range).as_deref(), Some("ephemeral"));
    }

    #[test]
    fn selection_text_refuses_a_multi_block_range() {
        let content = BookContent {
            blocks: vec![
                Block::Paragraph { spans: vec![Span::plain("first block")] },
                Block::Paragraph { spans: vec![Span::plain("second block")] },
            ],
            toc: vec![],
            images: vec![],
            footnotes: vec![],
            index_terms: vec![],
            raster: None,
            page_labels: vec![],
        };
        let range = LocusRange { from: Locus { block: 0, ch: 0 }, to: Locus { block: 1, ch: 5 } };
        assert_eq!(selection_text(&content, range), None);
    }

    #[test]
    fn selection_text_is_none_for_an_empty_range() {
        let content = one_paragraph("hello world");
        let range = LocusRange { from: Locus { block: 0, ch: 4 }, to: Locus { block: 0, ch: 4 } };
        assert_eq!(selection_text(&content, range), None);
    }

    #[test]
    fn a_lookup_looks_up_the_real_selected_word_not_the_whole_block() {
        // Regression guard for the actual wiring bug this module exists to
        // avoid: passing the whole block's text to `lookup()` instead of
        // just the selected word would silently "work" (NotFound is still a
        // valid LookupResult) while showing the wrong thing to the user.
        let content = one_paragraph("this word is ephemeral indeed");
        let range = LocusRange {
            from: Locus { block: 0, ch: 13 },
            to: Locus { block: 0, ch: 22 },
        };
        let word = selection_text(&content, range).unwrap();
        assert_eq!(word, "ephemeral");
        let lookup = Lookup::default();
        match lookup.lookup(&word, &content) {
            LookupResult::Glossary { term, .. } => assert_eq!(term, "ephemeral"),
            other => panic!("expected a glossary hit for the selected word, got {other:?}"),
        }
    }

    fn empty_content_for_popover() -> BookContent {
        BookContent {
            blocks: vec![],
            toc: vec![],
            images: vec![],
            footnotes: vec![],
            index_terms: vec![],
            raster: None,
            page_labels: vec![],
        }
    }

    /// Smoke test: the popover must not panic when the selection is
    /// active-but-empty (anchor == head, a click with no drag) — this is the
    /// single most common way it will actually be invoked in the running
    /// app, since a tap that barely moves is still `is_active()`.
    #[test]
    fn popover_does_not_panic_on_a_zero_width_selection() {
        let content = empty_content_for_popover();
        let lookup = Lookup::default();
        let sel = Selection { anchor: Some(Locus::START), head: Some(Locus::START) };
        let mut open = true;
        let ctx = egui::Context::default();
        let colors = uikit_egui::palette_colors("dark");
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            let ctx = ui.ctx().clone();
            draw_lookup_popover(&ctx, &content, &lookup, &sel, &mut open, &colors);
        });
    }

    fn empty_content() -> BookContent {
        BookContent {
            blocks: vec![],
            toc: vec![],
            images: vec![],
            footnotes: vec![],
            index_terms: vec![IndexTerm {
                term: "Heuristic".to_string(),
                blocks: vec![3, 8],
            }],
            raster: None,
            page_labels: vec![],
        }
    }

    #[test]
    fn book_index_takes_priority() {
        let lookup = Lookup::default();
        let content = empty_content();
        match lookup.lookup("heuristic", &content) {
            LookupResult::BookIndex { locations, .. } => assert_eq!(locations, vec![3, 8]),
            _ => panic!("expected book index hit"),
        }
    }

    #[test]
    fn glossary_fallback() {
        let lookup = Lookup::default();
        let content = BookContent {
            blocks: vec![],
            toc: vec![],
            images: vec![],
            footnotes: vec![],
            index_terms: vec![],
            raster: None,
            page_labels: vec![],
        };
        match lookup.lookup("ephemeral", &content) {
            LookupResult::Glossary { definition, .. } => assert!(definition.contains("short time")),
            _ => panic!("expected glossary hit"),
        }
    }

    #[test]
    fn unknown_word_not_found() {
        let lookup = Lookup::default();
        let content = BookContent {
            blocks: vec![],
            toc: vec![],
            images: vec![],
            footnotes: vec![],
            index_terms: vec![],
            raster: None,
            page_labels: vec![],
        };
        matches!(lookup.lookup("zzzznotaword", &content), LookupResult::NotFound { .. });
    }
}
