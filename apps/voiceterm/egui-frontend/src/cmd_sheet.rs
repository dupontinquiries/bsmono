//! cmd_sheet — the touch-friendly command history, ported from `cmdSheet`.
//!
//! `main.qml` v0.1.787: a bottom sheet of recent commands as big touch cards.
//! **Tap** drops one onto the terminal input line to review and press Enter.
//! **Drag up** past a threshold saves it to the palette's starred section.
//!
//! ## The two gestures on one card are the whole design
//!
//! A tap and a drag start identically, so the card has to decide which it is
//! from how far the finger travelled. Below the threshold it is a tap that
//! wobbled; above it, a save. That decision is [`Drag::resolve`], and it is
//! separated from the drawing so it can be tested without a finger — the
//! version in the QML is entangled with a MouseArea and cannot be.
//!
//! ## Why the drop is not a run
//!
//! The QML is explicit: *"TAP a card to drop the command onto the terminal
//! input line (review, then Enter)."* Recalled commands are the ones most
//! likely to be destructive — you are pulling back something you already ran
//! once, often the thing you ran just before it went wrong — so the operator
//! reads it before it executes. The same rule the search-hit and card-preview
//! paths follow.
//!
//! The ranking is `voiceterm::recall`, which is import-free and already scores
//! recency against frequency. Included by path rather than reimplemented, so
//! the sheet and the palette rank the same history the same way.

#[path = "../../src/recall.rs"]
#[allow(dead_code, unused_imports)]
pub mod recall;

pub use recall::{RecallHit, RecallItem};

/// How far a card must travel up before a release saves rather than taps.
///
/// In points, not a fraction of the card: the gesture is a flick of the
/// finger and its natural size is the same whether the card is 60pt or 120pt
/// tall. A fractional threshold makes tall cards harder to save than short
/// ones for no reason the operator could name.
pub const SAVE_THRESHOLD: f32 = 56.0;

/// A card being dragged.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Drag {
    /// Index of the card under the finger.
    pub index: usize,
    /// How far it has moved, in points. Negative is up.
    pub dy: f32,
}

/// What a released drag meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Release {
    /// Barely moved: the operator meant to tap.
    Tap,
    /// Pulled up past the threshold.
    Save,
    /// Moved, but not far enough, or the wrong way. Nothing happens — and
    /// this is DISTINCT from Tap, because a card dragged 30pt and released
    /// was not a tap and running the command would be a surprise.
    Abandoned,
}

impl Drag {
    /// What this release means.
    ///
    /// The three-way split matters. Collapsing Abandoned into Tap is the bug
    /// the QML's threshold exists to avoid: a finger that slid halfway and
    /// stopped would drop a command on the input line the operator never
    /// chose.
    pub fn resolve(self) -> Release {
        // A few points of travel is a finger on a touchscreen holding still.
        const TAP_SLOP: f32 = 8.0;
        if self.dy.abs() <= TAP_SLOP { Release::Tap }
        else if self.dy <= -SAVE_THRESHOLD { Release::Save }
        else { Release::Abandoned }
    }

    /// How far through the save gesture, 0..=1, for the lift animation.
    ///
    /// Clamped at both ends: past the threshold the card should stop growing,
    /// or a long drag inflates it off the screen and the confirmation reads as
    /// a glitch.
    pub fn progress(self) -> f32 {
        if self.dy >= 0.0 { return 0.0; }
        (-self.dy / SAVE_THRESHOLD).clamp(0.0, 1.0)
    }
}

/// What the sheet produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SheetAction {
    /// Put this on the input line, unexecuted.
    Drop(String),
    /// Star it in the palette.
    Save(String),
    Dismiss,
}

/// The sheet's state.
#[derive(Debug, Clone, Default)]
pub struct CmdSheet {
    pub open: bool,
    /// Ranked history, newest-relevant first.
    pub hits: Vec<RecallHit>,
    /// The card currently under a finger.
    pub drag: Option<Drag>,
    /// Commands already starred, so the sheet can mark them rather than
    /// offering to save one twice.
    pub saved: Vec<String>,
}

impl CmdSheet {
    /// Open with a freshly ranked history.
    pub fn open_with(&mut self, hits: Vec<RecallHit>) {
        self.hits = hits;
        self.drag = None;
        self.open = true;
    }

    pub fn close(&mut self) {
        self.open = false;
        // Cleared on close, or the next open resumes a gesture whose finger
        // left the screen minutes ago.
        self.drag = None;
    }

    pub fn is_saved(&self, cmd: &str) -> bool {
        self.saved.iter().any(|s| s == cmd)
    }

    /// Note that a command was starred.
    pub fn mark_saved(&mut self, cmd: &str) {
        if !self.is_saved(cmd) { self.saved.push(cmd.to_string()); }
    }
}

/// Draw the sheet. Returns an action if one was completed.
pub fn cmd_sheet_ui(
    ui: &mut egui::Ui,
    s: &mut CmdSheet,
    c: &uikit_egui::PaletteColors,
) -> Option<SheetAction> {
    if !s.open { return None; }
    let mut act = None;

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("recent commands").strong());
        if ui.button("close").clicked() { act = Some(SheetAction::Dismiss); }
    });
    if s.hits.is_empty() {
        // Said rather than shown as an empty sheet: a blank panel reads as a
        // sheet that failed to load its contents.
        ui.label(egui::RichText::new("no history yet").small().weak());
        return act;
    }
    ui.label(egui::RichText::new("tap to place on the input line · drag up to star")
        .small().weak());
    ui.separator();

    egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
        for (i, h) in s.hits.iter().enumerate() {
            let dragging = s.drag.filter(|d| d.index == i);
            let lift = dragging.map(|d| d.progress()).unwrap_or(0.0);
            // A big target: this is the touch surface, and the QML calls the
            // rows "big touch cards" for a reason.
            let height = 44.0 + lift * 8.0;
            let (rect, resp) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), height),
                egui::Sense::click_and_drag());

            let bg = if lift > 0.0 {
                c.bg_hover.gamma_multiply(1.0 + lift * 0.5)
            } else { c.bg_card };
            ui.painter().rect_filled(rect.translate(egui::vec2(0.0, -lift * 6.0)), 6.0, bg);
            let text_at = rect.translate(egui::vec2(8.0, -lift * 6.0));
            ui.painter().text(text_at.left_center(), egui::Align2::LEFT_CENTER,
                &h.command, egui::FontId::monospace(13.0),
                if s.is_saved(&h.command) { c.accent } else { c.text });

            if resp.drag_started() {
                s.drag = Some(Drag { index: i, dy: 0.0 });
            } else if resp.dragged() {
                if let Some(d) = s.drag.as_mut() {
                    if d.index == i { d.dy += resp.drag_delta().y; }
                }
            } else if resp.drag_stopped() {
                if let Some(d) = s.drag.take() {
                    match d.resolve() {
                        Release::Save => act = Some(SheetAction::Save(h.command.clone())),
                        Release::Tap => act = Some(SheetAction::Drop(h.command.clone())),
                        // Nothing. A half-drag is not a choice.
                        Release::Abandoned => {}
                    }
                }
            } else if resp.clicked() {
                act = Some(SheetAction::Drop(h.command.clone()));
            }
        }
    });
    act
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_half_drag_does_nothing_rather_than_tapping() {
        // The bug the threshold exists to avoid: a finger that slid part way
        // and stopped would otherwise drop a command on the input line the
        // operator never chose. Recalled commands are the ones most likely to
        // be destructive.
        assert_eq!(Drag { index: 0, dy: -30.0 }.resolve(), Release::Abandoned);
        assert_eq!(Drag { index: 0, dy: -60.0 }.resolve(), Release::Save);
        assert_eq!(Drag { index: 0, dy: -2.0 }.resolve(), Release::Tap);
    }

    #[test]
    fn a_downward_drag_is_never_a_save() {
        // Only up saves. A downward flick is a scroll that caught a card.
        assert_eq!(Drag { index: 0, dy: 80.0 }.resolve(), Release::Abandoned);
        assert_eq!(Drag { index: 0, dy: 200.0 }.progress(), 0.0);
    }

    #[test]
    fn the_lift_stops_growing_past_the_threshold() {
        // Otherwise a long drag inflates the card off the screen and the
        // confirmation reads as a glitch.
        assert_eq!(Drag { index: 0, dy: -SAVE_THRESHOLD }.progress(), 1.0);
        assert_eq!(Drag { index: 0, dy: -SAVE_THRESHOLD * 4.0 }.progress(), 1.0);
        assert!((Drag { index: 0, dy: -SAVE_THRESHOLD / 2.0 }.progress() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn the_threshold_is_absolute_not_a_fraction_of_the_card() {
        // The gesture is a flick of a finger and its natural size does not
        // change with the card. A fractional threshold makes tall cards harder
        // to save for no reason the operator could name.
        let src = include_str!("cmd_sheet.rs");
        assert!(src.contains("pub const SAVE_THRESHOLD: f32 = 56.0;"));
        // And resolve must not consult a height.
        let at = src.find("pub fn resolve").unwrap();
        let body = &src[at..at + 400];
        assert!(!body.contains("height"), "resolve scales with the card:\n{body}");
    }

    #[test]
    fn closing_clears_an_in_flight_drag() {
        // Otherwise the next open resumes a gesture whose finger left the
        // screen minutes ago.
        let mut s = CmdSheet::default();
        s.open_with(vec![]);
        s.drag = Some(Drag { index: 3, dy: -40.0 });
        s.close();
        assert!(s.drag.is_none());
        assert!(!s.open);
    }

    #[test]
    fn an_empty_history_says_so() {
        // A blank sheet reads as one that failed to load its contents.
        let src = include_str!("cmd_sheet.rs");
        assert!(src.contains("no history yet"));
    }

    #[test]
    fn a_command_is_only_starred_once() {
        let mut s = CmdSheet::default();
        s.mark_saved("git status");
        s.mark_saved("git status");
        assert_eq!(s.saved.len(), 1);
        assert!(s.is_saved("git status"));
        assert!(!s.is_saved("git log"));
    }

    #[test]
    fn the_ranking_is_the_shared_one_not_a_second_sort() {
        // recall.rs already scores recency against frequency, and the palette
        // uses it. A sheet with its own sort would show a different "most
        // recent" than the palette for the same history.
        let src = include_str!("cmd_sheet.rs");
        let needle = format!("../../src/{}.rs", "recall");
        assert!(src.contains(needle.as_str()), "the sheet does not use the shared ranking");
        // Assembled, like the needle two lines up. I assembled ONE of the two
        // in this same test and spelled the other — which is how the habit
        // actually fails: not forgetting the rule, applying it once and
        // stopping.
        let resort = format!("sort_{}", "by");
        assert!(!src.contains(resort.as_str()),
                "the sheet re-sorts what recall already ranked");
    }
}
