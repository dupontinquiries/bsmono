pub mod caches;
pub mod launch;
pub mod stack;
pub mod vitals;
pub mod wire;

use egui::{RichText, Ui};
use uikit_egui::PaletteColors;

/// Sources of every page, for the test that reads them (see tests/phone.rs).
pub const SOURCES: [(&str, &str); 5] = [
    ("launch", include_str!("launch.rs")),
    ("stack", include_str!("stack.rs")),
    ("vitals", include_str!("vitals.rs")),
    ("wire", include_str!("wire.rs")),
    ("caches", include_str!("caches.rs")),
];

pub fn section(ui: &mut Ui, c: &PaletteColors, title: &str) {
    ui.add_space(8.0);
    ui.label(RichText::new(title).small().strong().color(c.text_weak));
}

pub fn card<R>(ui: &mut Ui, c: &PaletteColors, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(uikit_egui::glass::surface_fill(c.bg_card))
        .stroke(egui::Stroke::new(1.0, c.border))
        .corner_radius(10.0)
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// A row that is one big tap target: title, a weaker second line, optional
/// leading mark. Returns whether it was tapped.
pub fn row(ui: &mut Ui, c: &PaletteColors, selected: bool, mark: &str, title: &str, detail: &str) -> bool {
    let text = if mark.is_empty() { title.to_string() } else { format!("{mark}  {title}") };
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = ui.available_width() - 16.0;
    job.append(&text, 0.0, egui::TextFormat { font_id: egui::FontId::proportional(15.0), color: c.text, ..Default::default() });
    if !detail.is_empty() {
        job.append("\n", 0.0, egui::TextFormat::default());
        job.append(detail, 0.0, egui::TextFormat { font_id: egui::FontId::proportional(12.0), color: c.text_weak, ..Default::default() });
    }
    ui.add_sized([ui.available_width(), 0.0], egui::Button::selectable(selected, job).wrap()).clicked()
}
