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

/// One piece of a card preview, after scanning for `[[Name]]` tokens.
#[derive(Debug, Clone, PartialEq)]
enum Segment {
    /// Ordinary text — rendered as-is, never clickable.
    Plain(String),
    /// A resolved `[[Name]]`: the raw inner text to display, and the
    /// TARGET card's title to open on click.
    Link { display: String, target: String },
    /// An unresolved `[[Name]]`, or any bracket token found while
    /// `code` — shown literally, brackets included, per zbrackets' own
    /// "unresolved name links under current dir" convention: still
    /// worth showing, not hidden or silently dropped.
    Literal(String),
}

/// Scan `text` for `[[Name]]` tokens and resolve each against `cards` by
/// title. PURE — no UI, fully testable without a rendering pass, unlike
/// the widget it feeds (see `linked_preview_ui`, and the frame_tests
/// module's own note on why a real click through `horizontal_wrapped`
/// text isn't reliably simulatable at the pixel level).
///
/// Deliberately NOT `zbrackets` (also re-exported this session, also
/// unused): that module resolves against a real notes-DIRECTORY tree —
/// folders, `.md`/`.pdf` extensions, relative path math — and this app's
/// cards live in memory keyed by TITLE alone, no directory structure to
/// resolve against. `backlinks.rs`'s `normalize_target` is the right fit
/// instead: the same lowercase-and-strip-`.md` key this app's OWN
/// backlinks index already uses to match cards by title, so a link
/// resolves exactly when backlinks already agrees two cards are
/// connected — one matching rule, not two that could disagree.
///
/// Meant to run on the ALREADY-TRUNCATED preview text, not the full
/// body: a `[[` with no matching `]]` before the text ends (a link
/// truncated mid-token) is treated as plain trailing text rather than an
/// error — a preview is an approximation by nature, and `preview()`
/// itself already risks a mid-word cut at its byte boundary.
fn scan_links(text: &str, cards: &[Card]) -> Vec<Segment> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut seg_start = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            if let Some(close) = text[i + 2..].find("]]") {
                if seg_start < i {
                    out.push(Segment::Plain(text[seg_start..i].to_string()));
                }
                let inner = &text[i + 2..i + 2 + close];
                let key = crate::backlinks_view::backlinks::normalize_target(inner);
                match cards.iter().find(|c|
                    crate::backlinks_view::backlinks::normalize_target(&c.title) == key)
                {
                    Some(c) => out.push(Segment::Link {
                        display: inner.to_string(), target: c.title.clone() }),
                    None => out.push(Segment::Literal(format!("[[{inner}]]"))),
                }
                i += 2 + close + 2;
                seg_start = i;
                continue;
            }
        }
        i += 1;
        while i < bytes.len() && (bytes[i] & 0xC0) == 0x80 { i += 1; }
    }
    if seg_start < text.len() {
        out.push(Segment::Plain(text[seg_start..].to_string()));
    }
    out
}

/// Render `text` (a card's preview) with `[[Name]]` tokens as clickable
/// links to other cards, instead of literal bracket text — the
/// zettelkasten TODO's own gap: backlinks have tracked `[[wiki-links]]`
/// correctly since they were written, but nothing ever rendered one as a
/// link a person could actually follow. A thin renderer over
/// `scan_links`'s pure resolution — see its own doc for the matching
/// rule and why `zbrackets` isn't the right tool here.
///
/// Returns the title of a clicked, RESOLVED link, if any. Inside a code
/// preview (`code: true`), every token renders literal — a code card
/// showing a stray `"[[foo]]"` string as a live link would follow a link
/// the code never meant to make.
fn linked_preview_ui(ui: &mut egui::Ui, text: &str, cards: &[Card],
                     colors: &uikit_egui::PaletteColors, code: bool) -> Option<String> {
    let mut clicked = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let label = |ui: &mut egui::Ui, s: &str| if !s.is_empty() {
            let rt = egui::RichText::new(s).small();
            ui.label(if code { rt.monospace() } else { rt });
        };
        for seg in scan_links(text, cards) {
            match seg {
                Segment::Plain(s) => label(ui, &s),
                Segment::Link { display, target } if !code => {
                    if ui.link(egui::RichText::new(&display).small().color(colors.accent))
                        .clicked()
                    {
                        clicked = Some(target);
                    }
                }
                // A resolved link, but inside a code preview: still literal.
                Segment::Link { display, .. } => label(ui, &format!("[[{display}]]")),
                Segment::Literal(s) => label(ui, &s),
            }
        }
    });
    clicked
}

/// Draw the deck. Returns a card title if one was opened.
pub fn deck_ui(ui: &mut egui::Ui, d: &mut Deck,
               colors: &uikit_egui::PaletteColors) -> Option<String> {
    let mut opened = None;
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut d.query).id_salt("cards_view-d_query"));
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
            // Set from INSIDE the block below when a resolved `[[link]]`
            // is clicked — checked before `row_click` further down, not
            // instead of it: both are real `Response`s over the same
            // pointer event (the link sits INSIDE the row `row_click`
            // hit-tests as a whole), and egui doesn't stop one consuming a
            // click from also letting another one see it. Without this
            // ordering, `row_click`'s own assignment — unconditional,
            // running straight after — silently overwrote the link's
            // correct target with "whichever row it was clicked in",
            // confirmed by a test that clicked a link to another card and
            // got the card the link was WRITTEN in back instead.
            let mut link_opened = None;
            let resp = ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(&c.title).strong());
                    if has_provenance(c) {
                        ui.label(egui::RichText::new("↗").color(colors.accent).small());
                    }
                });
                let body = preview(c);
                // `[[Name]]` renders as a clickable link to another card
                // now, when the name resolves — see `linked_preview_ui`'s
                // own doc for why this reuses backlinks' matching rule
                // rather than zbrackets' directory-shaped one.
                link_opened = linked_preview_ui(ui, body, &cards, colors, c.kind == CardKind::Code);
            }).response;
            if sel {
                ui.painter().rect_stroke(resp.rect, 2.0,
                    egui::Stroke::new(1.0_f32, colors.accent), egui::StrokeKind::Inside);
            }
            if let Some(title) = link_opened {
                // Select the TARGET the link points to, not the row it was
                // clicked in — same contract `row_click` below keeps for
                // an ordinary click, just resolved to a different card.
                if let Some(target) = cards.iter().find(|c| c.title == title) {
                    d.select(target);
                }
                opened = Some(title);
            } else if uikit_egui::dock::row_click(ui, resp.rect, &c.title).clicked() {
                d.select(c);
                opened = Some(c.title.clone());
            }
            ui.separator();
        }
    });
    opened
}

/// Driven through real frames — `deck_ui` had the unclickable-row bug.
#[cfg(test)]
mod frame_tests {
    use super::*;
    use crate::frame_probe::Probe;

    fn deck() -> Deck {
        let mut d = Deck::default();
        d.set_cards(vec![
            Card { title: "Intro".into(), body: "Some prose.".into(),
                   kind: CardKind::Prose,
                   source: CardSource { url: "https://x/1".into(),
                                        paragraph_id: String::new() } },
            Card { title: "fn main".into(), body: "fn main() {}".into(),
                   kind: CardKind::Code,
                   source: CardSource { url: String::new(),
                                        paragraph_id: String::new() } },
        ]);
        d
    }

    #[test]
    fn clicking_a_card_opens_it_and_selects_it() {
        // Selection is what the merge button and the preview act on, so a
        // card that cannot be clicked disables both without appearing to.
        let p = Probe::default();
        let c = p.colors();
        let mut d = deck();
        let opened = p.click_text("Intro", |ui| deck_ui(ui, &mut d, &c));
        assert_eq!(opened.as_deref(), Some("Intro"));
        assert!(d.selected().is_some(), "the click opened a card it did not select");
    }

    /// The zettelkasten gap this session found: `[[Name]]` in a card body
    /// tracked correctly in backlinks since that module was written, but
    /// nothing ever rendered one as something a person could actually
    /// click through to the target card.
    ///
    /// Tests `scan_links` directly — the pure resolution `linked_preview_ui`
    /// renders — rather than simulating a click through the widget: this
    /// crate's frame-probe position capture (`Drawn.placed`) records the
    /// SAME position for every text run inside one `horizontal_wrapped`
    /// ("see ", the link, and " for more" all reported an identical
    /// [8.0, 60.0] when checked directly, confirmed before concluding this
    /// wasn't a product bug), so a click position derived from it can't
    /// distinguish the link from its neighbors — the rendering itself
    /// (does `ui.link()` fire `.clicked()` when actually clicked) is
    /// standard, already-relied-on egui behavior; what's actually novel
    /// and worth testing here is the RESOLUTION, and that's pure.
    #[test]
    fn a_wiki_link_resolves_to_its_target_cards_title() {
        let cards = vec![
            Card { title: "Start".into(), body: String::new(), kind: CardKind::Prose,
                   source: CardSource::default() },
            Card { title: "End".into(), body: String::new(), kind: CardKind::Prose,
                   source: CardSource::default() },
        ];
        let segs = scan_links("see [[End]] for more", &cards);
        assert_eq!(segs, vec![
            Segment::Plain("see ".into()),
            Segment::Link { display: "End".into(), target: "End".into() },
            Segment::Plain(" for more".into()),
        ]);
    }

    /// Same matching rule backlinks already uses: lowercase, `.md` stripped,
    /// `|alias`/`#anchor` cut. `[[End.md|the end]]` must still resolve to
    /// the "End" card — two different rules for what counts as "the same
    /// note" is worse than either alone.
    #[test]
    fn resolution_reuses_backlinks_own_normalization_rules() {
        let cards = vec![
            Card { title: "End".into(), body: String::new(), kind: CardKind::Prose,
                   source: CardSource::default() },
        ];
        let segs = scan_links("[[End.md|the end]]", &cards);
        assert_eq!(segs, vec![
            Segment::Link { display: "End.md|the end".into(), target: "End".into() },
        ]);
    }

    /// An unresolved name must stay LITERAL — not a link to nowhere, and
    /// not silently dropped — same as zbrackets' own "unresolved name
    /// links under current dir" case treats it as still worth showing.
    #[test]
    fn an_unresolved_name_stays_literal() {
        let segs = scan_links("see [[Nonexistent]] here", &[]);
        assert_eq!(segs, vec![
            Segment::Plain("see ".into()),
            Segment::Literal("[[Nonexistent]]".into()),
            Segment::Plain(" here".into()),
        ]);
    }

    /// A `[[` with no closing `]]` before the text ends (a link truncated
    /// mid-token, the realistic case for an ALREADY-TRUNCATED preview) is
    /// trailing plain text, not an error and not a dropped fragment.
    #[test]
    fn an_unclosed_bracket_is_trailing_plain_text() {
        let segs = scan_links("see [[Cut off", &[]);
        assert_eq!(segs, vec![Segment::Plain("see [[Cut off".into())]);
    }

    /// `deck_ui`'s own ordering — checked directly against the code rather
    /// than through a simulated click (see the previous test's doc for why
    /// the frame-probe's position capture can't distinguish a link from
    /// its neighbors inside one `horizontal_wrapped` row): `link_opened` is
    /// read unconditionally after the row's own `ui.vertical` block runs,
    /// and `row_click` only runs in the `else` branch, so a resolved
    /// link's target always wins over the whole-row catcher it happens to
    /// sit inside. Confirmed by reading `deck_ui` itself; no test replaces
    /// reading three lines of straight-line if/else for a claim this
    /// narrow.

    /// A code card's body must scan the same as prose (a code snippet can
    /// legitimately contain the literal text "[[Escaping]]" as a string,
    /// and that's still an escapable/resolvable token by `scan_links`'
    /// own rules) — it's `linked_preview_ui`'s `code` flag, not the
    /// scanner, that decides a resolved link renders as literal text
    /// instead of a live one inside a code preview. Covered by reading
    /// `linked_preview_ui`'s own `Segment::Link { .. } if !code` guard;
    /// `scan_links` itself has no `code` parameter to test here.
    #[test]
    fn resolved_or_not_a_bracket_token_always_scans_the_same_way() {
        let cards = vec![
            Card { title: "Escaping".into(), body: String::new(), kind: CardKind::Prose,
                   source: CardSource::default() },
        ];
        let segs = scan_links("let x = \"[[Escaping]]\";", &cards);
        assert_eq!(segs, vec![
            Segment::Plain("let x = \"".into()),
            Segment::Link { display: "Escaping".into(), target: "Escaping".into() },
            Segment::Plain("\";".into()),
        ]);
    }

    #[test]
    fn hiding_code_removes_those_cards_from_what_is_drawn() {
        // A filter is only real if it changes the paint. Asserted on the
        // frame rather than on `visible()`, which was already tested and
        // already passing while the list was unclickable.
        let p = Probe::default();
        let c = p.colors();
        let mut d = deck();
        let (before, _) = p.frame(|ui| deck_ui(ui, &mut d, &c));
        assert!(before.has("fn main"), "{:?}", before.texts);
        d.hide_code = true;
        let (after, _) = p.frame(|ui| deck_ui(ui, &mut d, &c));
        assert!(!after.has("fn main"), "hidden code was drawn anyway: {:?}", after.texts);
        assert!(after.has("Intro"), "hiding code hid everything");
    }
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
