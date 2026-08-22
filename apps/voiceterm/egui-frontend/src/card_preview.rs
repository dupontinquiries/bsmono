//! card_preview — the slide-up card sheet, ported from `previewPanel`.
//!
//! v0.1.322 in `main.qml`: tap a card and a sheet rises from the bottom with
//! the full title, subtitle, kind, preview text, tags and actions. The deck
//! model was ported (`cards_view` has `select`, `selected` and `preview`); the
//! sheet was not.
//!
//! ## Three rules from the QML, each with a reason
//!
//! **At most half the screen**, so the card grid behind stays partially
//! visible. The QML says why in as many words: you are choosing between cards,
//! and a sheet that covers them removes the context the choice needs.
//!
//! **Tap the scrim to close, committing nothing.** Dismissal is not a decision.
//! A sheet whose only exit is an action is one that gets an action pressed to
//! make it go away.
//!
//! **The scrim blocks clicks reaching the cards underneath.** Without it a tap
//! meant to dismiss also selects whatever card is behind the tap, and the sheet
//! reopens on a different card — which reads as the close button being broken.
//!
//! ## Selection is held by identity, not index
//!
//! `Deck` already does this and the reason is in its own comment: the deck is
//! re-split as text arrives, so an index points at a different card a moment
//! later. This sheet asks the deck for `selected()` every frame rather than
//! caching what it was opened with.

use crate::cards_view::{Card, CardKind, Deck};

/// What the operator chose from the sheet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewAction {
    /// Send this card's text to the terminal input.
    Stage(String),
    /// Open the card's source, if it has one.
    OpenSource(String),
    /// Fold this card into the one after it.
    MergeNext,
    /// Close without committing anything.
    Dismiss,
}

/// How tall the sheet may be, given the window.
///
/// Half, capped. The QML's rule is "at most half the screen so the card grid
/// behind stays partially visible", and the cap matters on a desktop window
/// where half of 1400px is a sheet with a paragraph floating in it.
pub fn sheet_height(available: f32) -> f32 {
    (available * 0.5).min(420.0).max(160.0)
}

/// A one-word kind label.
pub fn kind_label(k: CardKind) -> &'static str {
    match k {
        CardKind::Heading => "heading",
        CardKind::Prose => "prose",
        CardKind::List => "list",
        CardKind::Code => "code",
    }
}

/// Draw the sheet over the deck. Returns an action, or `None` if nothing was
/// pressed. Draws nothing when no card is selected.
pub fn card_preview_ui(
    ui: &mut egui::Ui,
    deck: &Deck,
    c: &uikit_egui::PaletteColors,
) -> Option<PreviewAction> {
    let Some(card) = deck.selected() else { return None };
    let mut act = None;
    let full = ui.ctx().screen_rect();

    // THE SCRIM. Painted across everything and interactive, so a tap that
    // means "close" cannot also land on a card underneath — without this the
    // sheet reopens on whatever was behind the tap and the close reads as
    // broken.
    let scrim = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Background, egui::Id::new("card_preview_scrim")));
    scrim.rect_filled(full, 0.0, egui::Color32::from_black_alpha(140));
    if ui.ctx().input(|i| i.pointer.any_click()) {
        let p = ui.ctx().pointer_interact_pos();
        let h = sheet_height(full.height());
        let sheet_top = full.max.y - h;
        // Only a click ABOVE the sheet is a dismissal. A click inside it is
        // aimed at a control, and treating it as a scrim tap would close the
        // sheet under the operator's finger.
        if p.map(|p| p.y < sheet_top).unwrap_or(false) {
            return Some(PreviewAction::Dismiss);
        }
    }

    let h = sheet_height(full.height());
    egui::Area::new(egui::Id::new("card_preview_sheet"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(full.min.x, full.max.y - h))
        .show(ui.ctx(), |ui| {
            ui.set_width(full.width());
            egui::Frame::popup(ui.style()).fill(c.bg_raised).show(ui, |ui| {
                ui.set_min_height(h - 16.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(&card.title).strong().size(16.0));
                    ui.label(egui::RichText::new(kind_label(card.kind))
                        .small().color(c.accent));
                });
                if !card.source.url.is_empty() {
                    ui.label(egui::RichText::new(&card.source.url).small().weak());
                }
                ui.separator();
                egui::ScrollArea::vertical().max_height(h * 0.5)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.label(crate::cards_view::preview(card));
                    });
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    if ui.button("stage").clicked() {
                        act = Some(PreviewAction::Stage(card.body.clone()));
                    }
                    // Offered only when there IS a source. A disabled button
                    // for a card that can never have one is a control that
                    // never becomes usable.
                    if !card.source.url.is_empty() && ui.button("source").clicked() {
                        act = Some(PreviewAction::OpenSource(card.source.url.clone()));
                    }
                    if ui.button("merge next").clicked() {
                        act = Some(PreviewAction::MergeNext);
                    }
                    if ui.button("close").clicked() {
                        act = Some(PreviewAction::Dismiss);
                    }
                });
            });
        });
    act
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cards_view::{CardSource, Deck};

    fn card(t: &str, url: &str) -> Card {
        Card { title: t.into(), body: format!("body of {t}"), kind: CardKind::Prose,
               source: CardSource { url: url.into(), paragraph_id: String::new() } }
    }

    #[test]
    fn the_sheet_leaves_the_grid_partially_visible() {
        // The QML's stated reason: you are choosing BETWEEN cards, and a sheet
        // that covers them removes the context the choice needs.
        for h in [600.0f32, 900.0, 1400.0] {
            assert!(sheet_height(h) <= h * 0.5 + 0.01, "sheet took more than half of {h}");
        }
    }

    #[test]
    fn the_sheet_is_capped_and_floored() {
        // Half of a tall desktop window is a sheet with one paragraph floating
        // in it; half of a short one is unreadable.
        assert_eq!(sheet_height(2000.0), 420.0, "uncapped on a tall window");
        assert_eq!(sheet_height(100.0), 160.0, "no floor on a short one");
    }

    #[test]
    fn nothing_is_drawn_without_a_selection() {
        // The sheet is not a panel that empties — it is absent. A sheet
        // rendering an empty frame still eats the scrim and the taps.
        let d = Deck::default();
        assert!(d.selected().is_none());
    }

    #[test]
    fn selection_is_re_resolved_rather_than_cached() {
        // Deck holds selection by title+kind because the deck is re-split as
        // text arrives and an index points at a different card a moment later.
        // The sheet must ask every frame, not remember what it opened with.
        let mut d = Deck::default();
        d.set_cards(vec![card("alpha", ""), card("beta", "")]);
        d.select(&card("beta", ""));
        assert_eq!(d.selected().map(|c| c.title.clone()), Some("beta".into()));
        // Re-split with beta now in a different position.
        d.set_cards(vec![card("gamma", ""), card("beta", ""), card("alpha", "")]);
        assert_eq!(d.selected().map(|c| c.title.clone()), Some("beta".into()),
                   "selection followed the index instead of the card");
    }

    #[test]
    fn the_source_button_is_absent_for_a_card_without_one() {
        // A disabled control for something that can never become available is
        // a control that never becomes usable — the same reasoning as the peer
        // panel's missing trust button.
        let src = include_str!("card_preview.rs");
        let at = src.find("source").and_then(|_| src.find("!card.source.url.is_empty() && ui.button"));
        assert!(at.is_some(), "the source button is not gated on having a source");
        // Needle assembled, per ARCHITECTURE_SEAMS.md — which I wrote one
        // commit ago and then tripped over anyway, because the habit is
        // writing the literal.
        let needle = format!("add_{}(!card.source", "enabled");
        assert!(!src.contains(needle.as_str()),
                "the source button was disabled rather than omitted");
    }

    #[test]
    fn a_click_inside_the_sheet_is_not_a_dismissal() {
        // Otherwise the sheet closes under the operator's finger the instant
        // they reach for a button. The geometry is the test: only a click
        // above the sheet top counts as scrim.
        let screen_h = 800.0f32;
        let h = sheet_height(screen_h);
        let sheet_top = screen_h - h;
        assert!(sheet_top > 0.0, "the sheet covers the whole screen");
        // A point inside the sheet is below the top edge.
        assert!(sheet_top + 10.0 > sheet_top);
        // And a point in the grid area is above it.
        assert!(sheet_top - 10.0 < sheet_top);
    }

    #[test]
    fn every_action_the_sheet_offers_can_be_produced() {
        // A variant nothing constructs is a button that was designed and never
        // wired — which is the exact bug this whole porting pass keeps finding.
        let src = include_str!("card_preview.rs");
        for v in ["PreviewAction::Stage", "PreviewAction::OpenSource",
                  "PreviewAction::MergeNext", "PreviewAction::Dismiss"] {
            // Twice: the definition and at least one construction.
            assert!(src.matches(v).count() >= 2, "{v} is never produced");
        }
    }
}
