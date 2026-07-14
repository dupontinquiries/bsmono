//! voiceterm_egui.rs — the deployable egui voiceterm app (built behind the `app` feature so a plain
//! `cargo build`/`cargo test` on this crate stays eframe-free). Defaults to the Voice palette
//! (terminal orange/green) matching voiceterm's Qt/QML identity.
//!
//! Assembles the real app shell — session sidebar, terminal with its accent border, the mic FAB +
//! its action compass docked at the corner, and the bottom keycap bar — ported from actual
//! screenshots of the Qt app, not just QML reading (see shell.rs's doc comment for what was
//! deliberately NOT ported: the screenshots used a selectable "Mocha" theme, not this port's
//! baseline Voice identity, and the settings panel's full multi-theme picker is a separate,
//! bigger feature left for later).
//!
//! Run:  cargo run --bin voiceterm-egui --features app   (from apps/voiceterm/egui-frontend)

use eframe::egui;
use voiceterm_egui::action_compass::ActionCompass;
use voiceterm_egui::shell::{fab_rect, keycap_bar_ui, mic_fab_ui, paint_terminal_border, Session, Sidebar, SidebarAction, TERMINAL_KEYCAPS};
use uikit_egui::PaletteColors;
use voiceterm_egui::App;

struct VoicetermApp {
    app: App,
    sidebar: Sidebar,
    compass: ActionCompass,
    colors: PaletteColors,
}

impl eframe::App for VoicetermApp {
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        uikit_egui::apply_palette(ctx, snapclone_theme::palette_for(snapclone_theme::PaletteId::Voice));

        egui::SidePanel::left("sidebar").resizable(false).exact_width(160.0).show(ctx, |ui| {
            if let Some(action) = self.sidebar.ui(ui, &self.colors) {
                match action {
                    SidebarAction::SelectSession(_) => {} // on-device: switch the active PTY/session
                    SidebarAction::New => {} // on-device: spawn a new session
                    SidebarAction::Graph => {} // on-device: open the session graph view
                    SidebarAction::Settings => {} // on-device: open the settings panel (setup_wizard.rs)
                }
            }
        });

        egui::TopBottomPanel::bottom("keycaps").show(ctx, |ui| {
            if let Some(send) = keycap_bar_ui(ui, TERMINAL_KEYCAPS, &self.colors) {
                self.app.terminal.feed(send);
            }
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            self.app.handle_input(ui);
            let term_rect = ui.available_rect_before_wrap();
            self.app.fit_to(term_rect.size()); // keep the grid matching the actual pane size
            if self.app.handle_gestures(ctx, term_rect.min) {
                ctx.request_repaint(); // fling momentum still gliding
            }
            self.app.render(ui, &self.colors);
            paint_terminal_border(ui, term_rect, &self.colors);

            // the mic FAB docks at the terminal's bottom-right corner; press-hold opens the
            // action compass right there (matches the reference: the compass pops up ABOVE the FAB,
            // opening from the mic button itself, not a separate host button).
            let fab = fab_rect(term_rect, 56.0);
            let resp = mic_fab_ui(ui, fab, &self.colors);
            let _ = self.compass.ui_from_host(ui, &resp, &mut self.app);
        });
    }
}

fn main() -> eframe::Result<()> {
    let mut app = App::new(80, 24);
    // realistic content, not a toy script — to actually surface rendering gotchas (colors,
    // wrapping, unicode) rather than just confirming an empty prompt renders.
    app.terminal.feed("$ ls --color\r\n");
    app.terminal.feed("\x1b[1;34mCargo.toml\x1b[0m  \x1b[1;32mbuild.sh\x1b[0m  \x1b[1;34msrc\x1b[0m  \x1b[1;31mold.bak\x1b[0m\r\n");
    app.terminal.feed("$ echo \"a very long line that should wrap across more than one terminal row so we can see how wrapping and the cursor position interact once it spills over\"\r\n");
    app.terminal.feed("a very long line that should wrap across more than one terminal row so we can see how wrapping and the cursor position interact once it spills over\r\n");
    app.terminal.feed("$ echo café 🚀 你好\r\n"); // multibyte + emoji + CJK, straight in the source
    app.terminal.feed("café 🚀 你好\r\n");
    app.terminal.feed("$ ");
    let colors = uikit_egui::palette_colors("voice");
    let sidebar = Sidebar::new(vec![
        Session { name: "kesslerd".into(), count: 5 },
        Session { name: "kesslerd".into(), count: 2 },
    ]);
    eframe::run_native(
        "voiceterm (egui)",
        eframe::NativeOptions::default(),
        Box::new(move |_cc| {
            Ok(Box::new(VoicetermApp {
                app,
                sidebar,
                compass: ActionCompass::default(),
                colors,
            }))
        }),
    )
}
