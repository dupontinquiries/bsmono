//! rusty_board_egui.rs — the deployable egui rusty-board app (built behind the `app` feature so a
//! plain `cargo build`/`cargo test` on this crate stays eframe-free). Same content as examples/feel.rs
//! (kept in sync manually — the example is the quick dev-iteration harness, this bin is what the
//! build orchestrator (manifest/projects.toml) actually builds+runs for a deploy).
//!
//! Run:  cargo run --bin rusty-board-egui --features app   (from apps/rusty-board/egui-frontend)

use eframe::egui;
use rusty_board_egui::real_layout::{Press, RealBoard};
use rusty_board_egui::{apply_theme, keyboard_ui, Board};
use uikit_egui::Adaptive;

struct Feel {
    board: Board,
    /// The REAL layout files (resources/layouts/*.json), with shift/caps + the symbols toggle —
    /// shown when the sidebar's real-layout tabs are active (indices past the demo layouts).
    real: RealBoard,
    real_active: bool,
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
                    // demo layouts first, then the REAL layout files (shift/symbols live)
                    let mut names: Vec<String> = self.board.layout_names().iter().map(|s| s.to_string()).collect();
                    let demo_n = names.len();
                    for f in &self.real.files {
                        if f.name != "symbols" { // symbols is reached via the ?123 key, not a tab
                            names.push(format!("{} ⌨", f.display_name));
                        }
                    }
                    let tabs: Vec<uikit_egui::vertical_tabs::Tab> = names.iter().map(|n| uikit_egui::vertical_tabs::Tab::new(n)).collect();
                    let sel = if self.real_active { demo_n + self.real.active.min(1) } else { self.board.layout_index() };
                    let colors = uikit_egui::palette_colors(self.board.palette());
                    if let Some(i) = uikit_egui::vertical_tabs::show(ui, &tabs, sel, adaptive.min_touch_target, &colors) {
                        if i < demo_n {
                            self.real_active = false;
                            self.board.set_layout(i);
                        } else {
                            self.real_active = true;
                            self.real.active = i - demo_n; // qwerty=0, dvorak=1 (symbols excluded)
                            self.real.shift = rusty_board_egui::real_layout::Shift::Off;
                        }
                    }
                });
                ui.separator();
                ui.vertical(|ui| {
                    if self.real_active {
                        // the real layout files: render via the projection, feed emissions through
                        // the board's state machine (shift/symbols absorbed), act on the rest
                        let render = self.real.render();
                        if let Some(e) = keyboard_ui(ui, &render, &adaptive, self.board.palette()) {
                            match self.real.press(&e) {
                                Some(Press::Text(t)) => self.typed.push_str(&t),
                                Some(Press::Backspace) => { self.typed.pop(); }
                                Some(Press::Return) => self.typed.push('\n'),
                                Some(Press::Voice) | None => {}
                                Some(_) => {}
                            }
                        }
                    } else if let Some(e) = keyboard_ui(ui, self.board.current(), &adaptive, self.board.palette()) {
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
        Box::new(|_cc| Ok(Box::new(Feel { board: Board::new(), real: RealBoard::new(), real_active: false, typed: String::new() }))),
    )
}
