//! cards_view — the note-card deck, ported over `src/note_cards.rs`.
//!
//! `note_cards.rs` is Qt-free and already does the hard parts: splitting
//! captured text into atomic cards, merging two, and diffing an old deck against
//! a new one into `CardDelta`s so only the changed sections re-render. Mounted
//! by `#[path]` rather than reimplemented.
//!
//! The view adds what a deck needs on screen:
//!
//! * **Code cards are never truncated in preview.** A prose card can be cut to
//!   a line and still read; half a fenced block is misleading, because the part
//!   you cannot see is often the part that matters.
//! * **Provenance is shown when it exists.** A card captured from a page carries
//!   its URL, and a note whose source you cannot recover is a note you cannot
//!   trust six months later.
//! * **Selection survives a re-split.** Capturing more text re-runs
//!   `split_into_cards`, and a selection held by index would silently jump to a
//!   different card — so it is held by title+kind and re-resolved.

#[path = "../../src/note_cards.rs"]
#[allow(dead_code, unused_imports)]
pub mod note_cards;

pub use note_cards::{Card, CardDelta, CardKind, CardSource};

/// How much of a card body to preview.
pub const PREVIEW_CHARS: usize = 180;

/// Deck view state.
#[derive(Debug, Clone, Default)]
pub struct Deck {
    pub cards: Vec<Card>,
    /// Selection by identity rather than index — see `reselect`.
    selected: Option<(String, CardKind)>,
    pub query: String,
    /// Kinds to hide.
    pub hide_code: bool,
}

impl Deck {
    /// Replace the deck, keeping the selection if that card still exists.
    ///
    /// Capturing more text re-runs the splitter and every index shifts, so a
    /// selection held by position would silently jump to a different card — the
    /// user edits the wrong note and does not know it.
    pub fn set_cards(&mut self, cards: Vec<Card>) {
        self.cards = cards;
        if let Some((t, k)) = self.selected.clone() {
            if !self.cards.iter().any(|c| c.title == t && c.kind == k) {
                self.selected = None;
            }
        }
    }

    pub fn select(&mut self, c: &Card) { self.selected = Some((c.title.clone(), c.kind)); }
    pub fn clear_selection(&mut self) { self.selected = None; }

    pub fn selected(&self) -> Option<&Card> {
        let (t, k) = self.selected.as_ref()?;
        self.cards.iter().find(|c| &c.title == t && c.kind == *k)
    }

    pub fn is_selected(&self, c: &Card) -> bool {
        self.selected.as_ref().map(|(t, k)| &c.title == t && c.kind == *k).unwrap_or(false)
    }

    /// Cards matching the query and filters.
    pub fn visible(&self) -> Vec<&Card> {
        let q = self.query.trim().to_lowercase();
        self.cards.iter()
            .filter(|c| !(self.hide_code && c.kind == CardKind::Code))
            .filter(|c| q.is_empty()
                || c.title.to_lowercase().contains(&q)
                || c.body.to_lowercase().contains(&q))
            .collect()
    }

    /// Merge the selected card with the one after it, as the deck's join does.
    ///
    /// Returns false at the end of the deck rather than merging with nothing,
    /// which would silently do nothing and read as a broken button.
    pub fn merge_with_next(&mut self) -> bool {
        let Some(cur) = self.selected().cloned() else { return false };
        let Some(i) = self.cards.iter().position(|c| c.title == cur.title && c.kind == cur.kind)
            else { return false };
        if i + 1 >= self.cards.len() { return false; }
        let merged = note_cards::merge_cards(&self.cards[i], &self.cards[i + 1]);
        self.cards.splice(i..=i + 1, [merged.clone()]);
        self.select(&merged);
        true
    }
}

/// A card's preview text.
///
/// Code is returned whole: a prose card cut to a line still reads, but half a
/// fenced block is misleading, because the part you cannot see is often the part
/// that matters.
pub fn preview(c: &Card) -> &str {
    if c.kind == CardKind::Code { return &c.body; }
    let max = PREVIEW_CHARS.min(c.body.len());
    let mut end = max;
    while end > 0 && !c.body.is_char_boundary(end) { end -= 1; }
    &c.body[..end]
}

/// Does this card know where it came from?
pub fn has_provenance(c: &Card) -> bool {
    !c.source.url.trim().is_empty() || !c.source.paragraph_id.trim().is_empty()
}

/// Draw the deck. Returns a card title if one was opened.
pub fn deck_ui(ui: &mut egui::Ui, d: &mut Deck,
               colors: &uikit_egui::PaletteColors) -> Option<String> {
    let mut opened = None;
    ui.horizontal(|ui| {
        ui.text_edit_singleline(&mut d.query);
        ui.checkbox(&mut d.hide_code, "hide code");
        if d.selected().is_some() && ui.small_button("merge ↓").clicked() {
            d.merge_with_next();
        }
    });
    ui.separator();
    let cards: Vec<Card> = d.visible().into_iter().cloned().collect();
    if cards.is_empty() {
        ui.label(egui::RichText::new(if d.query.is_empty() { "no cards yet" }
                                     else { "nothing matches" }).weak());
        return None;
    }
    egui::ScrollArea::vertical().show(ui, |ui| {
        for c in &cards {
            let sel = d.is_selected(c);
            let resp = ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(&c.title).strong());
                    if has_provenance(c) {
                        ui.label(egui::RichText::new("↗").color(colors.accent).small());
                    }
                });
                let body = preview(c);
                if c.kind == CardKind::Code {
                    ui.label(egui::RichText::new(body).monospace().small());
                } else {
                    ui.label(egui::RichText::new(body).small());
                }
            }).response;
            if sel {
                ui.painter().rect_stroke(resp.rect, 2.0,
                    egui::Stroke::new(1.0, colors.accent), egui::StrokeKind::Inside);
            }
            if resp.interact(egui::Sense::click()).clicked() {
                d.select(c);
                opened = Some(c.title.clone());
            }
            ui.separator();
        }
    });
    opened
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(title: &str, body: &str, kind: CardKind, url: &str) -> Card {
        Card { title: title.into(), body: body.into(), kind,
               source: CardSource { url: url.into(), paragraph_id: String::new() } }
    }

    fn deck() -> Deck {
        let mut d = Deck::default();
        d.set_cards(vec![
            card("Intro", "Some prose that goes on for a while.", CardKind::Prose, "https://x/1"),
            card("fn main", "fn main() {\n    println!(\"hi\");\n}", CardKind::Code, ""),
            card("Notes", "More prose here.", CardKind::Prose, ""),
        ]);
        d
    }

    #[test]
    fn a_code_card_is_never_truncated_in_preview() {
        // Half a fenced block is misleading: the part you cannot see is often
        // the part that matters.
        let long_code = card("x", &"let a = 1;\n".repeat(200), CardKind::Code, "");
        assert_eq!(preview(&long_code), long_code.body, "code is whole");
        let long_prose = card("y", &"word ".repeat(200), CardKind::Prose, "");
        assert!(preview(&long_prose).len() <= PREVIEW_CHARS);
    }

    #[test]
    fn a_preview_never_splits_a_character() {
        // Captured web text is full of non-ASCII, and slicing a String by byte
        // index panics mid-character.
        let c = card("t", &"café — naïve “quote” ①②③ ".repeat(40), CardKind::Prose, "");
        let p = preview(&c);
        assert!(c.body.starts_with(p));
    }

    #[test]
    fn selection_survives_a_resplit() {
        // Capturing more text re-runs the splitter and every index shifts. A
        // selection held by POSITION would jump to a different card and the
        // user would edit the wrong note without knowing.
        let mut d = deck();
        let target = d.cards[2].clone();
        d.select(&target);
        assert_eq!(d.selected().map(|c| c.title.clone()), Some("Notes".into()));
        // a new capture inserts a card BEFORE it
        let mut cards = d.cards.clone();
        cards.insert(0, card("New", "fresh", CardKind::Prose, ""));
        d.set_cards(cards);
        assert_eq!(d.selected().map(|c| c.title.clone()), Some("Notes".into()),
                   "still the same card, not the one now at index 2");
    }

    #[test]
    fn a_selection_that_no_longer_exists_is_dropped() {
        // Otherwise the deck reports a selected card the list does not contain.
        let mut d = deck();
        let t = d.cards[0].clone();
        d.select(&t);
        d.set_cards(vec![card("Only", "x", CardKind::Prose, "")]);
        assert!(d.selected().is_none());
    }

    #[test]
    fn merging_at_the_end_of_the_deck_is_refused() {
        // Merging with nothing would silently do nothing and read as a broken
        // button.
        let mut d = deck();
        let last = d.cards[2].clone();
        d.select(&last);
        assert!(!d.merge_with_next());
        assert_eq!(d.cards.len(), 3, "unchanged");
        // and with no selection at all
        d.clear_selection();
        assert!(!d.merge_with_next());
    }

    #[test]
    fn merging_joins_two_cards_into_one_and_keeps_it_selected() {
        let mut d = deck();
        let first = d.cards[0].clone();
        d.select(&first);
        assert!(d.merge_with_next());
        assert_eq!(d.cards.len(), 2);
        assert!(d.selected().is_some(), "the merged card stays selected");
    }

    #[test]
    fn provenance_is_reported_only_when_it_exists() {
        // A note whose source you cannot recover is one you cannot trust six
        // months later — but claiming a source that is blank is worse.
        assert!(has_provenance(&card("a", "b", CardKind::Prose, "https://x")));
        assert!(!has_provenance(&card("a", "b", CardKind::Prose, "  ")));
        let mut c = card("a", "b", CardKind::Prose, "");
        c.source.paragraph_id = "p12".into();
        assert!(has_provenance(&c), "a paragraph id counts");
    }

    #[test]
    fn the_code_filter_and_the_query_compose() {
        let mut d = deck();
        assert_eq!(d.visible().len(), 3);
        d.hide_code = true;
        assert_eq!(d.visible().len(), 2);
        d.query = "prose".into();
        assert_eq!(d.visible().len(), 2, "both prose bodies mention it");
        d.query = "println".into();
        assert!(d.visible().is_empty(), "the code card is still hidden");
    }
}
