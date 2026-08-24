//! program_panel — the Program tab, which said "not built yet" until now.
//!
//! FL's grammar splits a channel's content in two: the step grid for hits,
//! the piano roll for held notes, chords and slides. The Rack tab is the
//! first; this is the second. Same wiring rule as `rack_panel`: the views
//! (`studio_egui::roll`) own nothing and return edits, the model
//! (`Pattern::add_note` and friends) decides, and this panel only carries
//! values between them — `s.rack != before` flags the loop re-render, and
//! the per-frame rack sync broadcasts the change like any other rack edit.

use studio_egui::roll::{self, RollEdit};
use uikit_egui::PaletteColors;

use crate::Studio;

/// The visible key range: `TOP` down through `ROWS` rows. ±24 semitones
/// around the channel's own pitch — four octaves, which covers a bass line
/// and a lead over it; the panel scrolls for anything the viewport cannot
/// show at once.
const TOP: i8 = 24;
const ROWS: usize = 49;

/// One frame of the Program panel. `playhead` is the loop column the
/// audition sink reports (already wrapped), or `None` when stopped — the
/// same value the Rack tab draws, so the two tabs agree about where the
/// music is.
pub fn show(ui: &mut egui::Ui, s: &mut Studio, c: &PaletteColors, playhead: Option<u32>) {
    let before = s.rack.clone();

    // ── channel row ─────────────────────────────────────────────────────
    // The roll edits ONE channel's notes; which one is the same selection
    // the Rack tab's strip and the mod lane already share, so switching
    // tabs never silently retargets an edit.
    ui.horizontal_wrapped(|ui| {
        ui.label("channel:");
        let mut selected = s.selected_channel;
        for (i, ch) in s.rack.channels.iter().enumerate() {
            if ui.add(egui::Button::selectable(selected == i, &ch.name)).clicked() {
                selected = i;
            }
        }
        s.selected_channel = selected.min(s.rack.channels.len().saturating_sub(1));
        ui.separator();
        ui.colored_label(c.text_weak,
            "drag to draw a note · click to remove · grab the right edge to resize · right-click for slide");
    });

    let Some(pattern) = s.rack.patterns.get(s.current_pattern) else {
        ui.colored_label(c.text_weak, "No pattern. Add one on the Rack tab.");
        return;
    };
    let pattern = pattern.clone(); // the view takes &Pattern; edits come back as values
    let channel = s.selected_channel;

    // ── the roll ────────────────────────────────────────────────────────
    let mut edits = Vec::new();
    egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
        edits = roll::roll(ui, &pattern, channel, TOP, ROWS,
                           &mut s.roll_drag, playhead, c);
    });
    if let Some(p) = s.rack.patterns.get_mut(s.current_pattern) {
        for e in edits {
            match e {
                RollEdit::Add { channel, note } => p.add_note(channel, note),
                RollEdit::Remove { channel, start, key } => { p.remove_note(channel, start, key); }
                RollEdit::Resize { channel, start, key, len } => p.resize_note(channel, start, key, len),
                RollEdit::ToggleSlide { channel, start, key } => p.toggle_note_slide(channel, start, key),
            }
        }
    }

    if s.rack != before {
        s.loop_dirty = true;
    }
}
