//! bo_suite_egui.rs — the deployable egui bo-suite app (built behind the `app` feature so a plain
//! `cargo build`/`cargo test` on this crate stays eframe-free). Defaults to the Amber palette (warm
//! gold) matching bo-controller's Qt/QML surface, instead of the shared default.
//!
//! Run:  cargo run --bin bo-suite-egui --features app   (from apps/bo-suite/egui-frontend)

use eframe::egui;
use bo_suite_egui::{Controller, Device, Scene};
use uikit_egui::Adaptive;

struct App {
    controller: Controller,
    /// The audio host. The controller returns REQUESTS; this performs them,
    /// which is what keeps the view constructible and testable with no
    /// process table — see `playback::Request`.
    player: bo_suite_egui::playback::Player,
    /// Said once per failure, not per frame.
    last_note: String,
}

impl App {
    fn new() -> Self {
        let devices = vec![
            Device { name: "Living Room".into(), volume: 62, playing: true },
            Device { name: "Kitchen".into(), volume: 40, playing: false },
            Device { name: "Studio".into(), volume: 78, playing: true },
        ];
        let scenes = vec![
            Scene { name: "Morning".into() },
            Scene { name: "Focus".into() },
            Scene { name: "Evening Wind-down".into() },
        ];
        Self {
            controller: Controller::new(devices, scenes),
            player: bo_suite_egui::playback::Player::new(),
            last_note: String::new(),
        }
    }
}

impl App {
    /// Carry out whatever the view asked for, and report back what the
    /// player did.
    fn pump_playback(&mut self) {
        use bo_suite_egui::playback::{plan, Request, State, DEFAULT_PLAYER};
        if let Some(req) = self.controller.pending_playback.take() {
            match req {
                Request::Play { url, sink } => {
                    if url.trim().is_empty() {
                        // NOT attempted. Handing an empty path to a player
                        // gets "no such file", which reads as a broken
                        // player rather than as the missing catalogue it
                        // actually is.
                        self.last_note =
                            "no track resolved — the library has titles, not files yet".into();
                        self.controller.transport.pause();
                    } else {
                        let p = plan(DEFAULT_PLAYER, &url, &sink);
                        for c in &p.setup { eprintln!("bo-suite: {c}"); }
                        eprintln!("bo-suite: {}", p.play);
                        self.player.start(&p);
                    }
                }
                Request::Stop => self.player.stop_now(),
                Request::ReleaseRoute => self.player.release_route("sh"),
            }
        }
        if let Some(state) = self.player.shared.take_state() {
            if let State::Failed(why) = state {
                // The player's own words. "Playback failed" tells nobody
                // anything; "Failed to open audio device" tells them
                // everything.
                self.last_note = why;
                self.controller.transport.pause();
            }
        }
        if self.player.shared.take_ended() {
            // A track that finished is not a track that was stopped — only
            // this advances anything, or pausing would skip.
            self.controller.transport.pause();
        }
        if !self.last_note.is_empty() {
            eprintln!("bo-suite: {}", std::mem::take(&mut self.last_note));
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        uikit_egui::apply_palette(ctx, snapclone_theme::palette_for(snapclone_theme::PaletteId::Amber));
        let mut prefs = settings_kit::Prefs::default();
        prefs.palette = "amber".to_string();
        self.controller.apply_settings(ctx, &prefs);
        egui::CentralPanel::default().show(ctx, |ui| {
            let adaptive = Adaptive::detect(ctx);
            self.controller.ui(ui, &adaptive);
        });
        self.pump_playback();
    }
}

/// Must match packaging/org.bosuite.BoSuite.desktop exactly. A mismatch fails
/// SILENTLY: the app runs and has no icon, no dock grouping and the wrong
/// name in the switcher.
const APP_ID: &str = "org.bosuite.BoSuite";

fn main() -> eframe::Result<()> {
    eframe::run_native(
        "bo-suite (egui)",
        eframe::NativeOptions {
        // Glow SAID, not left to eframe: its 0.34 defaults include
        // wgpu but not glow, and wgpu's default device descriptor
        // asks for 8 colour attachments — which dies at startup on
        // a phone GPU allowing 5. A glow-only feature list is not
        // enough; gitmind-egui declared one and still got wgpu
        // through feature unification.
        renderer: eframe::Renderer::Glow,
        viewport: uikit_egui::desktop::viewport(APP_ID, "Bo Suite", [900.0, 700.0]),
        ..Default::default()
    },
        Box::new(|_cc| Ok(Box::new(App::new()))),
    )
}
