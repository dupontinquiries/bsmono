//! steps — a strip that says which step of a sequence you are on, and what was decided
//! at the ones behind you.
//!
//! `1 Take · photo  ›  2 Compose · 5s · “hi”  ›  3 Choose  ›  4 Send` — the current cell
//! strong in the accent and underlined, the done ones plain, the rest weak. One `LayoutJob`
//! rather than a label per cell: a `horizontal_wrapped` of eight labels painted them on top
//! of one another at phone width (snapclone's layout probe caught it), and a single text
//! run cannot overlap itself. A wrapping `Label`, so a strip that does break onto a second
//! line is given the height for it.
//!
//! Extracted from snapclone's send flow; the cells are the caller's — this only draws them.

use crate::PaletteColors;
use egui::text::{LayoutJob, TextFormat};
use egui::Ui;

/// One cell of the strip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepCell {
    /// `1 Take · photo` — the number, the name, and what was decided if anything.
    pub label: String,
    pub current: bool,
    pub done: bool,
}

impl StepCell {
    pub fn new(label: impl Into<String>, current: bool, done: bool) -> StepCell {
        StepCell { label: label.into(), current, done }
    }
}

/// Draw the strip.
pub fn strip(ui: &mut Ui, cells: &[StepCell], c: &PaletteColors) {
    let font = egui::TextStyle::Small.resolve(ui.style());
    let mut job = LayoutJob::default();
    job.wrap.max_width = ui.available_width().max(120.0);
    for (i, cell) in cells.iter().enumerate() {
        let color = if cell.current { c.accent } else if cell.done { c.text } else { c.text_weak };
        let mut fmt = TextFormat { font_id: font.clone(), color, ..Default::default() };
        if cell.current {
            fmt.underline = egui::Stroke::new(1.0_f32, c.accent);
        }
        job.append(&cell.label, 0.0, fmt);
        if i + 1 < cells.len() {
            job.append("  \u{203a}  ", 0.0, TextFormat { font_id: font.clone(), color: c.text_weak, ..Default::default() });
        }
    }
    ui.add(egui::Label::new(job).wrap());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_strip_draws_as_one_run_that_cannot_overlap_itself() {
        let c = crate::default_palette();
        let cells = [
            StepCell::new("1 Take · photo", false, true),
            StepCell::new("2 Compose · 5s · \u{201c}hi\u{201d}", true, false),
            StepCell::new("3 Choose", false, false),
            StepCell::new("4 Send", false, false),
        ];
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(360.0, 200.0)));
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| strip(ui, &cells, &c));
        });
        let texts = out.shapes.iter().filter(|s| matches!(s.shape, egui::Shape::Text(_))).count();
        assert_eq!(texts, 1, "one galley, not one per cell");
    }
}
