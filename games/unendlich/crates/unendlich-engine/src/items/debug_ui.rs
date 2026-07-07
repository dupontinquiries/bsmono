//! egui debug inventory — a palette you click to drop stacks into the hotbar,
//! a visible hotbar with the selected slot highlighted, and a full-grid view.
//!
//! Feature-gated behind `egui_debug`; this file is only compiled when that
//! feature (and therefore the `egui` dependency) is present. It is written
//! against the stable egui widget subset (`label`, `button`, `horizontal`,
//! `RichText`, `Color32`, `Window`) to survive version drift, but has not been
//! compiled here — expect to nudge an API name or two for your egui version.
//!
//! It takes a `&mut egui::Ui`, so mount it however you run egui (side panel,
//! window, overlay). [`debug_inventory_window`] is a turnkey wrapper if you
//! already have an `egui::Context`.

use egui::{Color32, RichText};

use super::{Inventory, ItemId, ItemRegistry, ItemStack, PaletteEntry};

fn swatch(color: [u8; 3]) -> RichText {
    RichText::new("\u{25A0}") // ■
        .color(Color32::from_rgb(color[0], color[1], color[2]))
        .size(18.0)
}

fn name_of(palette: &[PaletteEntry], item: ItemId) -> String {
    palette
        .iter()
        .find(|e| e.item == item)
        .map(|e| e.name.to_string())
        .unwrap_or_else(|| format!("item#{}", item.0))
}

fn color_of(palette: &[PaletteEntry], item: ItemId) -> Option<[u8; 3]> {
    palette.iter().find(|e| e.item == item).map(|e| e.color)
}

fn slot_caption(palette: &[PaletteEntry], slot: Option<ItemStack>) -> String {
    match slot {
        Some(s) => {
            let n: String = name_of(palette, s.item).chars().take(6).collect();
            format!("{}\n{}", n, s.count)
        }
        None => "—\n".to_string(),
    }
}

/// Draw the debug inventory into an existing `Ui`.
pub fn debug_inventory_ui<R: ItemRegistry>(
    ui: &mut egui::Ui,
    inv: &mut Inventory,
    palette: &[PaletteEntry],
    reg: &R,
) {
    ui.heading("Debug Inventory");

    // Palette — click to add to the inventory.
    ui.label("Palette — click to add:");
    for e in palette {
        ui.horizontal(|ui| {
            ui.label(swatch(e.color));
            ui.label(e.name);
            let max = reg.max_stack(e.item).max(1);
            if ui.button(format!("+{}", max)).clicked() {
                inv.add_item(e.item, max, reg);
            }
            if ui.button("+1").clicked() {
                inv.add_item(e.item, 1, reg);
            }
        });
    }

    ui.separator();

    // Hotbar — visible slots, selected one highlighted, click to select.
    let selected = inv.selected();
    ui.label(format!("Hotbar (selected: {})", selected + 1));
    ui.horizontal(|ui| {
        for i in 0..inv.hotbar_size() {
            let caption = slot_caption(palette, inv.slot(i));
            let mut text = RichText::new(format!("{}\n{}", i + 1, caption));
            if i == selected {
                text = text.strong().underline();
            }
            if let Some(c) = inv.slot(i).and_then(|s| color_of(palette, s.item)) {
                text = text.color(Color32::from_rgb(c[0], c[1], c[2]));
            }
            if ui.button(text).clicked() {
                inv.set_selected(i);
            }
        }
    });

    match inv.held() {
        Some(h) => ui.label(format!("Held: {} x{}", name_of(palette, h.item), h.count)),
        None => ui.label("Held: (empty)"),
    };

    ui.separator();

    // Full grid view (read-only) — quick way to confirm contents while testing.
    ui.collapsing("All non-empty slots", |ui| {
        let mut any = false;
        for i in 0..inv.len() {
            if let Some(s) = inv.slot(i) {
                any = true;
                ui.horizontal(|ui| {
                    ui.label(format!("#{:>2}", i));
                    if let Some(c) = color_of(palette, s.item) {
                        ui.label(swatch(c));
                    }
                    ui.label(format!("{} x{}", name_of(palette, s.item), s.count));
                });
            }
        }
        if !any {
            ui.label("(empty)");
        }
    });

    if ui.button("Clear all").clicked() {
        inv.clear();
    }
}

/// Turnkey window wrapper, if you already have an `egui::Context` each frame.
pub fn debug_inventory_window<R: ItemRegistry>(
    ctx: &egui::Context,
    open: &mut bool,
    inv: &mut Inventory,
    palette: &[PaletteEntry],
    reg: &R,
) {
    egui::Window::new("Debug Inventory")
        .open(open)
        .resizable(true)
        .show(ctx, |ui| debug_inventory_ui(ui, inv, palette, reg));
}
