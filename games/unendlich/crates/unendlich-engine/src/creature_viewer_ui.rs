//! egui creature viewer — the standalone "stats tool" panel and, later, the
//! in-game inspector. It is a THIN shell over [`crate::creature_viewer`]: pick an
//! archetype / element / seed, hatch the egg, show the stats, and paint the
//! blueprint as a flat 2D projection. All the geometry and selection logic lives
//! in `creature_viewer` and is unit-tested; this file only translates that into
//! egui widgets and painter calls.
//!
//! Feature-gated behind `egui_debug` (so the `egui` dep is only pulled when that
//! feature is on). Written against the stable egui 0.34 widget subset
//! (`ComboBox`, `DragValue`, `label`, `button`, `allocate_painter`,
//! `rect_filled`) but NOT compiled in the test sandbox — expect to nudge an API
//! name or two for your exact egui build. Mount it however you run egui: a
//! `SidePanel`, a `Window`, or the in-game overlay — it just needs an
//! `&mut egui::Ui`.

use egui::{Color32, ComboBox, DragValue, Sense, Vec2};

use crate::creature_design::Arche;
use crate::creature_viewer::{project, stats, ViewAxis, ViewerState};
use crate::sim::elements::Affinity;

fn col(c: [f32; 3]) -> Color32 {
    let to = |v: f32| (v.clamp(0.0, 1.0) * 255.0) as u8;
    Color32::from_rgb(to(c[0]), to(c[1]), to(c[2]))
}

/// Archetypes offered in the picker (None = a random roll over the general fauna).
const ARCHES: [Option<Arche>; 11] = [
    None,
    Some(Arche::Biped),
    Some(Arche::Quadruped),
    Some(Arche::Critter),
    Some(Arche::Serpent),
    Some(Arche::Heavy),
    Some(Arche::Bird),
    Some(Arche::Canid),
    Some(Arche::Dragon),
    Some(Arche::Dinosaur),
    Some(Arche::Lauscher),
];

fn arche_label(a: Option<Arche>) -> &'static str {
    match a {
        None => "random",
        Some(a) => a.label(),
    }
}

fn element_label(e: Option<Affinity>) -> &'static str {
    match e {
        None => "random / wild",
        Some(a) => a.label(),
    }
}

/// Draw the full viewer into `ui`, editing `state` in place. Call every frame.
pub fn creature_viewer_ui(ui: &mut egui::Ui, state: &mut ViewerState) {
    ui.heading("Creature Viewer");

    // ---- Controls ----
    ui.horizontal(|ui| {
        ComboBox::from_label("Archetype")
            .selected_text(arche_label(state.arche))
            .show_ui(ui, |ui| {
                for a in ARCHES {
                    ui.selectable_value(&mut state.arche, a, arche_label(a));
                }
            });
        ComboBox::from_label("Element")
            .selected_text(element_label(state.element))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut state.element, None, element_label(None));
                for e in Affinity::CHOOSABLE {
                    ui.selectable_value(&mut state.element, Some(e), e.label());
                }
            });
    });

    ui.horizontal(|ui| {
        ui.label("Seed:");
        ui.add(DragValue::new(&mut state.seed).speed(1.0));
        if ui.button("Reroll").clicked() {
            state.reroll();
        }
    });

    ui.horizontal(|ui| {
        for v in [ViewAxis::Side, ViewAxis::Front, ViewAxis::Top] {
            let name = match v {
                ViewAxis::Side => "Side",
                ViewAxis::Front => "Front",
                ViewAxis::Top => "Top",
            };
            ui.selectable_value(&mut state.view, v, name);
        }
    });

    // ---- Hatch + stats ----
    let bp = state.hatch();
    let s = stats(&bp);
    ui.separator();
    ui.label(format!("arch: {}    element: {}    age: {}", s.arch, s.element, s.age));
    ui.label(format!(
        "boxes: {}    looseness: {:.2}    standing height: {:.2}",
        s.box_count, s.looseness, s.standing_height
    ));
    ui.label(format!(
        "size  L {:.2} × W {:.2} × H {:.2}  (blocks)",
        s.length, s.width, s.height
    ));

    // ---- 2D projection ----
    ui.separator();
    let desired = Vec2::splat(ui.available_width().min(360.0).max(160.0));
    let (response, painter) = ui.allocate_painter(desired, Sense::hover());
    let rect = response.rect;
    // Faint backdrop so the silhouette reads against the panel.
    painter.rect_filled(rect, 4.0, Color32::from_gray(28));
    let side = rect.width().min(rect.height());
    let origin = rect.min;
    for b in project(&bp, state.view) {
        let min = origin + Vec2::new(b.x * side, b.y * side);
        let size = Vec2::new(b.w * side, b.h * side);
        painter.rect_filled(egui::Rect::from_min_size(min, size), 1.0, col(b.color));
    }

    // ---- Genome (diploid genetics) ----
    ui.separator();
    ui.label("Genome — two alleles per trait; the bar marker is the expressed phenotype");
    // Environment switches: toggle these to watch epigenetic adaptation shift the
    // expressed traits (bits are caller-defined; here: cold/heat/wet/hunger).
    ui.horizontal(|ui| {
        ui.label("environment:");
        for (bit, name) in [(0u8, "cold"), (1, "heat"), (2, "wet"), (3, "hunger")] {
            let mut on = state.env & (1 << bit) != 0;
            if ui.checkbox(&mut on, name).changed() {
                if on {
                    state.env |= 1 << bit;
                } else {
                    state.env &= !(1 << bit);
                }
            }
        }
    });

    let genome = state.genome();
    let readouts = genome.readouts(state.seed, state.env);
    let row_h = 16.0;
    let bar_w = ui.available_width().min(320.0).max(160.0);
    for (name, r) in crate::genetics::GENOME_TRAITS.iter().zip(readouts.iter()) {
        ui.horizontal(|ui| {
            ui.allocate_space(Vec2::new(2.0, row_h));
            ui.label(format!("{name:<8}"));
            let (resp, p) = ui.allocate_painter(Vec2::new(bar_w, row_h), Sense::hover());
            let bar = resp.rect;
            p.rect_filled(bar, 2.0, Color32::from_gray(36));
            // Two faint allele ticks (value position), heavier = more dominant.
            for (val, dom) in [(r.value_a, r.dom_a), (r.value_b, r.dom_b)] {
                let x = bar.min.x + val * bar.width();
                let shade = (90.0 + 150.0 * dom) as u8;
                p.rect_filled(
                    egui::Rect::from_min_size(egui::pos2(x - 1.5, bar.min.y + 2.0), Vec2::new(3.0, row_h - 4.0)),
                    0.0,
                    Color32::from_gray(shade),
                );
            }
            // The expressed phenotype: a bright marker.
            let px = bar.min.x + r.phenotype * bar.width();
            p.rect_filled(
                egui::Rect::from_min_size(egui::pos2(px - 1.0, bar.min.y), Vec2::new(2.0, row_h)),
                0.0,
                Color32::from_rgb(120, 220, 255),
            );
            ui.label(if r.homozygous { "homozygous" } else { "heterozygous" });
        });
    }

    // Expressed coat colour from the colour genes, as a swatch.
    let ph = genome.express(state.seed, state.env);
    ui.horizontal(|ui| {
        ui.label("expressed coat:");
        let (resp, p) = ui.allocate_painter(Vec2::new(40.0, 16.0), Sense::hover());
        p.rect_filled(resp.rect, 3.0, col(ph.color));
        ui.label(format!("height {:.2}  eyes {:.2}  ears {:.2}", ph.height, ph.eyesight, ph.hearing));
    });
}

/// Turnkey wrapper: float the viewer in its own window given an `egui::Context`.
/// Handy for the standalone stats tool; in-game you'll likely embed
/// [`creature_viewer_ui`] in your existing overlay instead.
pub fn creature_viewer_window(ctx: &egui::Context, state: &mut ViewerState, open: &mut bool) {
    egui::Window::new("Creature Viewer")
        .open(open)
        .resizable(true)
        .show(ctx, |ui| creature_viewer_ui(ui, state));
}
