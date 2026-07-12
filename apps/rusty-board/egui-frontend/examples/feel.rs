//! feel.rs — a runnable eframe harness for feel-testing the rusty-board keyboard: the motion key
//! feedback (press glides colour), long-press accent variants (the slide_menu grid popup), and the
//! cardinal-swipe layouts (swipe a key on the `cardinal`/`cardinal8` layouts). Layouts are switched via
//! the sidebar tabs on the left (not a whole-board swipe — see Board::poll_swipe's doc comment for why).
//!
//! Run:  cargo run --example feel        (from apps/rusty-board/egui-frontend)

use eframe::egui;
use rusty_board_egui::{apply_theme, keyboard_ui, Board};
use uikit_egui::Adaptive;

struct Feel {
    board: Board,
    typed: String,
}

impl eframe::App for Feel {
    // this eframe's App trait requires ui(); the app is driven from update() below.
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}

    #[allow(deprecated)]
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        apply_theme(ctx);

        egui::Panel::top("out").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("layout: {}", self.board.current().name)).strong());
                ui.separator();
                ui.label("hold a key for accents · swipe a key on the cardinal layouts · tap a sidebar tab to switch layout");
                if ui.button("clear").clicked() {
                    self.typed.clear();
                }
            });
            ui.add_space(4.0);
            let shown = if self.typed.is_empty() { "…" } else { self.typed.as_str() };
            ui.label(egui::RichText::new(shown).monospace().size(18.0));
            ui.add_space(4.0);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let adaptive = Adaptive::detect(ctx);
            // Layout switching lives in its OWN strip (the sidebar), never contested with a key's
            // gesture space — a whole-board swipe used to race per-key cardinal swipes (see
            // Board::poll_swipe's doc comment) and intermittently "eat" a key's swipe on real touch
            // hardware. A sidebar tap has no such race.
            ui.horizontal(|ui| {
                let sidebar_w = 64.0_f32.max(adaptive.min_touch_target * 1.3);
                ui.allocate_ui(egui::vec2(sidebar_w, ui.available_height()), |ui| {
                    let names = self.board.layout_names();
                    let tabs: Vec<uikit_egui::vertical_tabs::Tab> = names.iter().map(|n| uikit_egui::vertical_tabs::Tab::new(n)).collect();
                    let colors = uikit_egui::palette_colors(self.board.palette());
                    if let Some(i) = uikit_egui::vertical_tabs::show(ui, &tabs, self.board.layout_index(), adaptive.min_touch_target, &colors) {
                        self.board.set_layout(i);
                    }
                });
                ui.separator();
                ui.vertical(|ui| {
                    if let Some(e) = keyboard_ui(ui, self.board.current(), &adaptive, self.board.palette()) {
                        if e == "\u{8}" {
                            self.typed.pop();
                        } else {
                            self.typed.push_str(&e);
                        }
                    }
                });
            });
        });
    }
}

fn main() -> eframe::Result<()> {
    eframe::run_native(
        "rusty-board · feel",
        eframe::NativeOptions::default(),
        Box::new(|_cc| Ok(Box::new(Feel { board: Board::new(), typed: String::new() }))),
    )
}
