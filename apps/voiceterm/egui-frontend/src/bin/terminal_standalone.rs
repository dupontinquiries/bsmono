//! terminal-standalone — the pure terminal core (uikit-term's `App`) with NO app-specific chrome
//! at all: no sidebar, no FAB, no compass, no keycap bar. Exists to prove the kit genuinely works
//! standalone, not just wired inside voiceterm's shell — and as a minimal, lightweight terminal
//! any other app (logscope, gitmind, av1tuner-m1, unendlich analytics) can point to as "this is
//! what embedding uikit-term looks like at its simplest."
//!
//! Run:  cargo run --bin terminal-standalone --features app   (from apps/voiceterm/egui-frontend)

use eframe::egui;
use uikit_term::App;

struct StandaloneTerminal {
    app: App,
    colors: uikit_egui::PaletteColors,
}

impl eframe::App for StandaloneTerminal {
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // System is a neutral, theme-agnostic identity — this bin isn't "voiceterm branded", it's
        // the bare terminal kit, so it defaults to the shared neutral palette rather than Voice.
        uikit_egui::apply_palette(ctx, snapclone_theme::palette_for(snapclone_theme::PaletteId::System));

        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(cmd) = self.app.handle_input(ui) {
                // no real PTY here — echo the command back so typing is visibly alive
                self.app.terminal.feed(&format!("{cmd}\r\n$ "));
            }
            let term_rect = ui.available_rect_before_wrap();
            self.app.fit_to(term_rect.size());
            if self.app.handle_gestures(ctx, term_rect.min) {
                ctx.request_repaint();
            }
            self.app.render(ui, &self.colors);
        });
    }
}

fn main() -> eframe::Result<()> {
    let mut app = App::new(80, 24);
    app.terminal.feed("terminal-standalone — the bare uikit-term kit, no app chrome\r\n$ ");
    let colors = uikit_egui::palette_colors("system");
    eframe::run_native(
        "terminal (standalone)",
        eframe::NativeOptions::default(),
        Box::new(move |_cc| Ok(Box::new(StandaloneTerminal { app, colors }))),
    )
}
