// drivemind-egui binary — eframe bootstrap: spawn the worker thread, wire
// the two channels, apply the shared palette, hand everything to draw().

use drivemind_egui::shell::{worker, ShellState};
use std::sync::mpsc;

struct App {
    state: ShellState,
    colors: uikit_egui::PaletteColors,
}

impl eframe::App for App {
    // This fleet's eframe requires `ui` alongside `update`; the app is
    // driven from update(), same as dialer-egui and every other egui
    // frontend in the tree.
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Same CentralPanel form as dialer-egui (the deprecation warning on
        // .show is tree-wide on this pinned eframe and tolerated there too).
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(self.colors.bg_base).inner_margin(10.0))
            .show(ctx, |ui| {
                drivemind_egui::draw(ui, &mut self.state, &self.colors);
            });
    }
}

fn main() -> eframe::Result<()> {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("drivemind")
            .with_inner_size([420.0, 760.0]), // phone-shaped by default
        ..Default::default()
    };
    eframe::run_native(
        "drivemind",
        opts,
        Box::new(move |cc| {
            let colors = uikit_egui::apply_palette(
                &cc.egui_ctx,
                snapclone_theme::palette_for(snapclone_theme::PaletteId::Night),
            );
            let (task_tx, task_rx) = mpsc::channel();
            let (msg_tx, msg_rx) = mpsc::channel();
            let ctx = cc.egui_ctx.clone();
            std::thread::spawn(move || worker(task_rx, msg_tx, move || ctx.request_repaint()));
            Ok(Box::new(App { state: ShellState::new(task_tx, msg_rx), colors }))
        }),
    )
}
