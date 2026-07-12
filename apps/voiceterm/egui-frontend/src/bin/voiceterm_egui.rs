//! voiceterm_egui.rs — the deployable egui voiceterm app (built behind the `app` feature so a plain
//! `cargo build`/`cargo test` on this crate stays eframe-free). Same content as examples/feel.rs (kept
//! in sync manually — the example is the quick dev-iteration harness, this bin is what the build
//! orchestrator (manifest/projects.toml) actually builds+runs for a deploy). Defaults to the Voice
//! palette (terminal orange/green) matching voiceterm's Qt/QML surface, instead of the shared default.
//!
//! Run:  cargo run --bin voiceterm-egui --features app   (from apps/voiceterm/egui-frontend)

use eframe::egui;
use voiceterm_egui::action_compass::ActionCompass;
use voiceterm_egui::confirm_dialog::{ConfirmDialog, Outcome};
use voiceterm_egui::repl_voice_glow::ReplVoiceGlow;
use uikit_egui::PaletteColors;
use voiceterm_egui::App;

struct Feel {
    app: App,
    compass: ActionCompass,
    confirm: Option<ConfirmDialog>,
    glow: ReplVoiceGlow,
    colors: PaletteColors,
    last_action: String,
}

impl eframe::App for Feel {
    // this eframe's App trait requires ui(); the app is driven from update() below.
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}

    #[allow(deprecated)]
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        uikit_egui::apply_palette(ctx, snapclone_theme::palette_for(snapclone_theme::PaletteId::Voice));
        egui::Panel::top("bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if let Some(a) = self.compass.ui(ui, &mut self.app) {
                    self.last_action = format!("{a:?}");
                }
                ui.separator();
                if ui.button("confirm dialog").clicked() {
                    self.confirm = Some(
                        ConfirmDialog::new("Discard changes?", "This can't be undone.", "Discard").with_timeout(8.0),
                    );
                }
                if ui.button("toggle glow").clicked() {
                    self.glow.active = !self.glow.active;
                    self.glow.reset("python");
                }
                if ui.button("search").clicked() {
                    self.app.open_search();
                }
                if !self.last_action.is_empty() {
                    ui.label(egui::RichText::new(format!("· {}", self.last_action)).weak());
                }
            });
            self.app.search_ui(ui);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            // typing / paste flow into the terminal input line
            self.app.handle_input(ui);
            self.app.render(ui, &self.colors);
        });

        if let Some(dialog) = &mut self.confirm {
            if dialog.show(ctx) != Outcome::Pending {
                self.confirm = None;
            }
        }
        self.glow.show(ctx);
    }
}

fn main() -> eframe::Result<()> {
    let mut app = App::new(80, 24);
    app.terminal.feed("$ echo hello\r\nhello\r\n$ ls\r\nfeel.rs  main.rs  Cargo.toml\r\n$ ");
    let colors = uikit_egui::palette_colors("voice");
    eframe::run_native(
        "voiceterm · feel",
        eframe::NativeOptions::default(),
        Box::new(move |_cc| {
            Ok(Box::new(Feel {
                app,
                compass: ActionCompass::default(),
                confirm: None,
                glow: ReplVoiceGlow::default(),
                colors,
                last_action: String::new(),
            }))
        }),
    )
}
