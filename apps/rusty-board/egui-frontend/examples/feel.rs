//! feel.rs — a runnable eframe harness for feel-testing the rusty-board keyboard: the motion key
//! feedback (press glides colour), long-press accent variants (the slide_menu grid popup), and the
//! cardinal-swipe layouts (swipe a key on the `cardinal`/`cardinal8` layouts). Swipe the whole board
//! left/right to cycle layouts.
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
                ui.label("hold a key for accents · swipe a key on the cardinal layouts · swipe the board to switch");
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
            let surface = ui.max_rect();
            // swipe the board sideways to cycle qwerty → symbols → cardinal → cardinal8
            self.board.poll_swipe(ui, surface);
            if let Some(e) = keyboard_ui(ui, self.board.current(), &adaptive, self.board.palette()) {
                if e == "\u{8}" {
                    self.typed.pop();
                } else {
                    self.typed.push_str(&e);
                }
            }
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
