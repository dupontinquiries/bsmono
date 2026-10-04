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

// Spacing comes from the kit's tokens, not from numbers chosen per call site:
// XS (6) between things that belong together, SM (10) between a heading and its
// card and after every card, MD (18) inside a card and between sections. The
// first version of these pages used 4, 6 and 8 more or less at random, and the
// result read as one continuous column.
use uikit_egui::tokens::{RADIUS_SM, SPACE_MD, SPACE_SM, SPACE_XS};

/// A section heading: room above it (it starts something), little below (it
/// belongs to what follows).
pub fn section(ui: &mut Ui, c: &PaletteColors, title: &str) {
    ui.add_space(SPACE_MD as f32 - SPACE_SM as f32);
    ui.label(RichText::new(title).small().strong().color(c.text_weak));
    ui.add_space(SPACE_XS as f32 - 2.0);
}

/// The kit's glass card: full width, SPACE_MD inside, the hairline where light
/// catches the top edge, SPACE_SM after. This module drew its own flatter one
/// (10pt inside, a plain border) until the padding pass.
pub fn card<R>(ui: &mut Ui, c: &PaletteColors, add: impl FnOnce(&mut Ui) -> R) -> R {
    uikit_egui::glass::card(ui, c.bg_card, |ui| {
        ui.spacing_mut().item_spacing.y = SPACE_XS as f32;
        add(ui)
    })
    .inner
}

/// A recessed well for things that are READ rather than operated: source, logs.
/// Darker than the page, where a card is lighter, so a snippet next to a card
/// next to a callout are three visibly different surfaces.
pub fn well<R>(ui: &mut Ui, c: &PaletteColors, add: impl FnOnce(&mut Ui) -> R) -> R {
    let r = egui::Frame::new()
        .fill(uikit_egui::callout::mix(c.bg_base, egui::Color32::BLACK, 0.35))
        .stroke(egui::Stroke::new(1.0_f32, uikit_egui::callout::mix(c.bg_base, c.border, 0.6)))
        .corner_radius(egui::CornerRadius::same(RADIUS_SM as u8))
        .inner_margin(egui::Margin::same(SPACE_SM as i8))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            add(ui)
        })
        .inner;
    ui.add_space(SPACE_SM as f32);
    r
}

/// A thin rule between entries of one card.
pub fn rule(ui: &mut Ui, c: &PaletteColors) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().rect_filled(r, 0.0, uikit_egui::callout::mix(c.bg_card, c.border, 0.5));
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
