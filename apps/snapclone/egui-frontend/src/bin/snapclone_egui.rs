//! snapclone_egui.rs — the deployable egui snapclone app (built behind the `app` feature so a plain
//! `cargo build`/`cargo test` on this crate stays eframe-free). Unlike gitmind/rusty-board/voiceterm/
//! bo-suite/bot-arena, snapclone isn't a PORT of a Qt/GTK twin — it's the founding app these uikit-egui
//! patterns (live backgrounds, page/subpage nav) originated from, so it uses the shared default palette
//! rather than a bespoke one matching some other UI it doesn't have.
//!
//! Run:  cargo run --bin snapclone-egui --features app   (from apps/snapclone/egui-frontend)

use eframe::egui;
use snapclone_egui::{Peer, Snapclone};
use uikit_egui::Adaptive;

struct App {
    snap: Snapclone,
}

impl App {
    fn new() -> Self {
        let peers = vec![
            Peer { name: "kitchen-tablet".into(), online: true },
            Peer { name: "studio-mbp".into(), online: true },
            Peer { name: "porch-cam".into(), online: false },
        ];
        Self { snap: Snapclone::new(peers) }
    }
}

impl eframe::App for App {
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let prefs = settings_kit::Prefs::default();
        self.snap.apply_settings(ctx, &prefs);
        egui::CentralPanel::default().show(ctx, |ui| {
            let adaptive = Adaptive::detect(ctx);
            self.snap.ui(ui, &adaptive);
        });
    }
}

fn main() -> eframe::Result<()> {
    eframe::run_native(
        "snapclone (egui)",
        eframe::NativeOptions::default(),
        Box::new(|_cc| Ok(Box::new(App::new()))),
    )
}
