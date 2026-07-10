//! logscope_touch — the reverse port, as runnable code.
//!
//! logscope's scene is mouse-first: *drag to orbit, scroll to zoom*. This
//! example wraps a logscope-style node strand in the SAME `GestureLayer`
//! snapclone uses, so on a touchscreen (the Fairphone/pmOS fleet) you can flick
//! to page between nodes and pull to step tabs — without touching logscope's
//! rendering. Toggle the touch layer with the checkbox to feel the difference.
//!
//! `cargo run -p uikit-egui --example logscope_touch` (needs a display).

use eframe::egui;
use egui::{FontId, Pos2, Rect, Sense, Stroke, Vec2};
use uikit_egui::{apply_palette, carved, paint_barrier, GestureLayer, Skin};
use snapclone_theme::palette_by_id;

const TABS: [&str; 4] = ["Sources", "Modify", "Rotate", "Bloc"];

struct Node {
    title: &'static str,
    status: &'static str,
    verdict: Verdict,
}
#[derive(Clone, Copy)]
enum Verdict { Ok, Fail, Stale }

struct Demo {
    touch_on: bool,
    tab: usize,
    focus: usize,
    pan: Vec2,
    zoom: f32,
    tabs_gesture: GestureLayer,
    nodes: Vec<Node>,
    skin: Skin,
}

impl Default for Demo {
    fn default() -> Self {
        let nodes = vec![
            Node { title: "staged keep-set", status: "main · 14 comps", verdict: Verdict::Ok },
            Node { title: "render geom_probe", status: "shaders 80/80", verdict: Verdict::Ok },
            Node { title: "unclosed { coord_main", status: "rustcheck Tier-0", verdict: Verdict::Fail },
            Node { title: "registry-kit", status: "linkme · tests pass", verdict: Verdict::Ok },
            Node { title: "snapclone→opendht", status: "cmake repath", verdict: Verdict::Stale },
        ];
        Self {
            touch_on: true,
            tab: 0,
            focus: 0,
            pan: Vec2::new(40.0, 24.0),
            zoom: 1.0,
            tabs_gesture: GestureLayer::horizontal_page(320.0),
            nodes,
            skin: Skin::carved(),
        }
    }
}

impl eframe::App for Demo {
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}

    #[allow(deprecated)]
    fn update(&mut self, ctx: &egui::Context, _f: &mut eframe::Frame) {
        // logscope uses its own cool palette; we borrow Modern here for contrast
        let pal = palette_by_id("modern");
        let c = apply_palette(ctx, pal);

        egui::TopBottomPanel::top("bar").show(ctx, |ui| {
            uikit_egui::row(ui, |ui| {
                ui.checkbox(&mut self.touch_on, "Touch layer (uikit::gesture)");
                ui.separator();
                for (i, t) in TABS.iter().enumerate() {
                    if carved::pill(ui, t, c.bg_card, c.accent, c.text, &self.skin, i == self.tab).clicked() {
                        self.tab = i;
                    }
                }
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let surface = ui.max_rect();
            let resp = ui.interact(surface, ui.id().with("scene"), Sense::click_and_drag());

            // ---- logscope's mouse-first interactions (ALWAYS on) ----
            if resp.dragged() {
                self.pan += resp.drag_delta(); // orbit/pan
            }
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if resp.hovered() && scroll.abs() > 0.0 {
                self.zoom = (self.zoom * (1.0 + scroll * 0.001)).clamp(0.55, 2.2); // zoom
            }

            // ---- the added touch layer (only when enabled) ----
            if self.touch_on {
                let g = self.tabs_gesture.update(ui, surface, true);
                if let Some(dir) = g.detent {
                    let n = TABS.len() as i32;
                    self.tab = ((self.tab as i32 + dir.sign()).rem_euclid(n)) as usize;
                }
                if let Some(dir) = g.page_jump {
                    let n = self.nodes.len() as i32;
                    self.focus = ((self.focus as i32 + dir.sign()).clamp(0, n - 1)) as usize;
                }
                if let Some(edge) = g.barrier_edge {
                    paint_barrier(ui, surface, g.barrier_fraction, edge, c.accent);
                }
            }

            // ---- draw the node strand ----
            let painter = ui.painter_at(surface);
            let nx = surface.left() + self.pan.x + 120.0 * self.zoom;
            let step = 96.0 * self.zoom;
            for i in 0..self.nodes.len() {
                let y = surface.top() + self.pan.y + i as f32 * step;
                if i + 1 < self.nodes.len() {
                    painter.line_segment(
                        [Pos2::new(nx, y + 30.0), Pos2::new(nx, y + step - 6.0)],
                        Stroke::new(1.0 * self.zoom, c.border),
                    );
                }
                let w = 158.0 * self.zoom;
                let rect = Rect::from_center_size(Pos2::new(nx, y + 14.0), Vec2::new(w, 56.0 * self.zoom));
                let state = if i == self.focus { carved::State::Down } else { carved::State::Rest };
                carved::paint_carved(ui, rect, c.bg_card, c.accent, &self.skin, state, 0.0);
                if i == self.focus {
                    painter.rect_stroke(rect, egui::Rounding::same(8.0), Stroke::new(1.0, c.accent));
                }
                let vc = match self.nodes[i].verdict {
                    Verdict::Ok => c.success,
                    Verdict::Fail => c.danger,
                    Verdict::Stale => c.warning,
                };
                painter.circle_filled(rect.left_top() + Vec2::new(12.0, 14.0), 5.0, vc);
                painter.text(rect.left_top() + Vec2::new(24.0, 12.0), egui::Align2::LEFT_CENTER, self.nodes[i].title, FontId::proportional(11.0 * self.zoom.max(0.7)), c.text);
                painter.text(rect.left_top() + Vec2::new(24.0, 30.0), egui::Align2::LEFT_CENTER, self.nodes[i].status, FontId::monospace(9.0 * self.zoom.max(0.7)), c.text_weak);
            }

            let hint = if self.touch_on {
                "Nodes appear as you converse — drag to orbit, scroll to zoom. Touch layer ON: flick to page nodes, pull to step tabs."
            } else {
                "Nodes appear as you converse — drag to orbit, scroll to zoom. (Touch layer OFF: gestures inert — logscope today.)"
            };
            painter.text(surface.left_bottom() + Vec2::new(12.0, -14.0), egui::Align2::LEFT_CENTER, hint, FontId::monospace(10.0), c.text_weak);
        });
    }
}

fn main() -> eframe::Result<()> {
    eframe::run_native(
        "logscope_touch (reverse port)",
        eframe::NativeOptions::default(),
        Box::new(|_cc| Ok(Box::<Demo>::default())),
    )
}
