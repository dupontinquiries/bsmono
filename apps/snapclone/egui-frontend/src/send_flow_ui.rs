//! send_flow_ui — the strip that says which step you are on, and the receipt.
//!
//! Drawn at the top of the camera page's controls and at the top of the picker
//! page while a snap is waiting, from the same `SendFlow`, so both pages tell
//! the same story. Plain egui, palette colours from `theme`.

use crate::send_flow::{SendFlow, Step};
use uikit_egui::PaletteColors;

/// Four cells in ONE galley: `① Take · img_017.jpg  ›  ② Compose  ›  ③ Choose  ›  ④ Send`.
/// The current one strong in the accent, the done ones plain, the rest weak.
///
/// One `LayoutJob` rather than a label per cell: the layout probe caught a
/// `horizontal_wrapped` of eight labels painting them on top of one another
/// at phone width, and a single text run cannot overlap itself.
pub fn stepper_strip(ui: &mut egui::Ui, flow: &SendFlow, c: &PaletteColors) {
    use egui::text::{LayoutJob, TextFormat};
    let font = egui::TextStyle::Small.resolve(ui.style());
    let mut job = LayoutJob::default();
    job.wrap.max_width = ui.available_width().max(120.0);
    let cells = flow.stepper();
    for (i, cell) in cells.iter().enumerate() {
        let color = if cell.current { c.accent } else if cell.done { c.text } else { c.text_weak };
        let mut fmt = TextFormat { font_id: font.clone(), color, ..Default::default() };
        if cell.current {
            fmt.underline = egui::Stroke::new(1.0, c.accent);
        }
        job.append(&cell.label(), 0.0, fmt);
        if i + 1 < cells.len() {
            job.append(
                "  \u{203a}  ",
                0.0,
                TextFormat { font_id: font.clone(), color: c.text_weak, ..Default::default() },
            );
        }
    }
    ui.label(job);
    let _ = Step::Capture; // the enum is the strip's contract; keep the import honest
}

/// The receipt: one line per recipient with its outcome, then the headline —
/// the Qt status bar's "Snap sent!" or the honest count.
pub fn receipt(ui: &mut egui::Ui, flow: &SendFlow, c: &PaletteColors) {
    use crate::send_flow::Outcome;
    for (who, outcome) in flow.recipients() {
        let (mark, col) = match outcome {
            Outcome::Pending => ("\u{2026}", c.text_weak),
            Outcome::Delivered => ("\u{2713}", c.text),
            Outcome::Failed => ("\u{2717}", c.warning),
        };
        ui.colored_label(col, format!("{mark} {who}"));
    }
    let r = flow.receipt();
    let col = if r.all_failed() { c.warning } else if r.pending > 0 { c.text_weak } else { c.accent };
    ui.label(egui::RichText::new(r.headline()).strong().color(col));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composer::{Duration, Snap};
    use crate::send_flow::Captured;

    /// Both surfaces draw headless at every step, with something painted each time.
    #[test]
    fn the_strip_and_the_receipt_draw_at_every_step() {
        let mut flow = SendFlow::default();
        let mut steps = Vec::new();
        steps.push(flow.clone());
        flow.on_captured(Captured { path: "a.jpg".into(), at_ms: 0 });
        steps.push(flow.clone());
        flow.on_composed(Snap { caption: "hi".into(), color: [255, 255, 255], caption_y: 0.5, duration: Duration::Seconds(5) });
        steps.push(flow.clone());
        flow.on_send(&["ada".into(), "bob".into()], 1);
        steps.push(flow.clone());
        flow.on_delivered("ada", 2);
        flow.on_failed("bob", 3);
        steps.push(flow.clone());
        let c = PaletteColors::from_palette(snapclone_theme::palette_by_id("midnight"));
        for (i, f) in steps.iter().enumerate() {
            let ctx = egui::Context::default();
            let mut input = egui::RawInput::default();
            input.screen_rect = Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 300.0)));
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    stepper_strip(ui, f, &c);
                    receipt(ui, f, &c);
                });
            });
            assert!(!out.shapes.is_empty(), "step {i} drew nothing");
        }
    }
}
