//! The window. Deliberately thin: the engine is `host::Host`, the pages are
//! the `Shell`, and this file opens a window around the two.
//!
//! It used to carry its own `Page` enum and its own copy of the page
//! dispatch, beside the `Shell` that already did both — two routers, one of
//! them (this one) dropping the Places page's actions on the floor and
//! feeding every page demo numbers. Now there is one router and the numbers
//! come from the same engine the GTK build runs.

use ambient_learner_egui::host::Host;
use ambient_learner_egui::Shell;

struct App {
    shell: Shell,
    host: Host,
    colors: uikit_egui::PaletteColors,
    started: std::time::Instant,
}

/// How often to wake for new readings while nothing on screen is
/// animating. The sensor worker polls at 10 Hz and the model worker
/// re-predicts at 5 Hz; 4 Hz on screen is enough to follow either and
/// cheap enough for a phone in a pocket.
const POLL: std::time::Duration = std::time::Duration::from_millis(250);

impl eframe::App for App {
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}

    fn update(&mut self, ctx: &egui::Context, _f: &mut eframe::Frame) {
        // Engine → pages, before drawing: the frame shows this tick's world.
        self.host.tick(&mut self.shell);
        let t_ms = self.started.elapsed().as_secs_f32() * 1000.0;
        egui::CentralPanel::default().show(ctx, |ui| {
            // The mood background sits BEHIND everything, and only on Live.
            // On the data pages it would compete with numbers people are
            // reading, which is the opposite of what an ambient layer is for.
            if self.shell.router.current() == ambient_learner_egui::ui::egui::chrome::Section::Live {
                ambient_learner_egui::ui::egui::ambience::ambience_ui(
                    ui, &self.shell.ambience, &self.shell.sparkles, t_ms, ui.max_rect(), &self.colors);
            }
            self.shell.ui(ui, &self.colors, t_ms);
        });
        // Pages → engine, after drawing: what was pressed this frame.
        for a in std::mem::take(&mut self.shell.pending) {
            self.host.apply(a, &mut self.shell);
        }
        if self.shell.back_unhandled {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        // Wake for the next reading. The easings request their own repaints
        // while they move; this is the floor under them.
        ctx.request_repaint_after(POLL);
    }
}

fn main() -> eframe::Result<()> {
    // Reverse-DNS app id, matching the .desktop entry this needs to name
    // exactly (packaging/org.ambientlearner.AmbientLearner.desktop) — see
    // uikit_egui::desktop's own docs for why an unset id is not cosmetic on
    // Wayland, where there is no WM_CLASS fallback at all.
    const APP_ID: &str = "org.ambientlearner.AmbientLearner";
    let opts = eframe::NativeOptions {
        // Glow SAID, not left to eframe: its 0.34 defaults include
        // wgpu but not glow, and wgpu's default device descriptor
        // asks for 8 colour attachments — which dies at startup on
        // a phone GPU allowing 5. A glow-only feature list is not
        // enough; gitmind-egui declared one and still got wgpu
        // through feature unification.
        renderer: eframe::Renderer::Glow,
        viewport: uikit_egui::desktop::viewport(APP_ID, "Ambient Learner", [420.0, 720.0]),
        ..Default::default()
    };
    // The engine comes up before the window: the first frame already has a
    // location name, a sensor count and the database's counts to show.
    let host = Host::open();
    eframe::run_native("ambient-learner-egui", opts, Box::new(move |cc| {
        Ok(Box::new(App {
            shell: Shell::default(),
            host,
            // ctx variant, from the CreationContext: same colours, plus the
            // symbol fonts. Installed once at construction rather than per
            // frame, which is all this app needs since it never re-themes.
            colors: uikit_egui::palette_colors_ctx(&cc.egui_ctx, "cloudscape"),
            started: std::time::Instant::now(),
        }))
    }))
}
