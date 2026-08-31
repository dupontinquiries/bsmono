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
    /// The real library: a scan of this machine's music directory.
    catalog: bo_suite_egui::music_scan::FsCatalog,
    /// The recogniser, when one is configured and voice is on.
    voice: bo_suite_egui::voice_source::VoiceSource,
    /// What the recogniser was last started with, so a settings change
    /// restarts it and an unchanged one does not — respawning a process
    /// every frame is the shape of bug that empties a battery.
    voice_cmd: String,
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
        // THE LIBRARY, from disk. `CatalogSource` had two implementations
        // and both were test fakes, so this frontend had never shown a real
        // one — and the tracks it did show had no files behind them.
        let catalog = bo_suite_egui::music_scan::FsCatalog::discover();
        let mut controller = Controller::new(devices, scenes);
        if catalog.is_empty() {
            eprintln!("bo-suite: no music found — looked in $XDG_MUSIC_DIR, ~/Music, ~/music");
        } else {
            eprintln!("bo-suite: {} track(s) from {}", catalog.len(),
                      catalog.root.as_ref().map(|r| r.display().to_string())
                          .unwrap_or_default());
            // The first page, so the list is not empty before anyone
            // scrolls. `answer_tracks_from` needs a pending request; asking
            // for offset 0 is that request.
            controller.pending_tracks =
                Some(bo_suite_egui::all_tracks::TracksAction::Load { offset: 0 });
            controller.answer_tracks_from(&catalog);
            // The id-to-file map the transport resolves through. Built from
            // the same scan the rows came from, so a row and its file
            // cannot disagree.
            let paths = (0..catalog.len() as i64)
                .filter_map(|i| catalog.path_of(i)
                    .map(|p| (i, p.display().to_string())))
                .collect();
            controller.set_track_paths(paths);
        }
        Self {
            controller,
            player: bo_suite_egui::playback::Player::new(),
            last_note: String::new(),
            catalog,
            voice: bo_suite_egui::voice_source::VoiceSource::new(),
            voice_cmd: String::new(),
        }
    }
}

impl App {
    /// Keep the recogniser in step with the settings, and act on what it
    /// heard.
    ///
    /// `Controller::heard` had no caller until now: voice control was a
    /// complete, tested parser with no source, which is a switch that
    /// does nothing when you turn it on — worse than a missing feature,
    /// because the switch implies otherwise.
    fn pump_voice(&mut self) {
        // WANTED is both halves. Turning voice off must stop the process,
        // not merely ignore what it says: a recogniser still running after
        // the switch is off is a microphone nobody thinks is on.
        let prefs = &self.controller.voice;
        let wanted = if prefs.enabled { prefs.recognizer.trim().to_string() }
                     else { String::new() };
        if wanted != self.voice_cmd {
            self.voice_cmd = wanted.clone();
            self.voice.stop_now();
            if !wanted.is_empty() {
                eprintln!("bo-suite: listening with: {wanted}");
                self.voice.start(&wanted);
            }
        }
        if let Some(why) = self.voice.heard.take_failure() {
            // Said once. A recogniser that quits because its model is
            // missing otherwise leaves a switch reading "on" and a room
            // where nothing happens.
            self.last_note = why;
        }
        for phrase in self.voice.heard.drain() {
            // Logged before acting, so a phrase that did nothing can be
            // told apart from one that was never heard — the two look
            // identical from the room.
            eprintln!("bo-suite: heard {phrase:?}");
            let _ = self.controller.heard(&phrase);
        }
    }

    /// Carry out whatever the view asked for, and report back what the
    /// player did.
    fn pump_playback(&mut self) {
        use bo_suite_egui::playback::{plan, Request, State};
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
                        // Whatever is installed, not whatever I assumed.
                        // mpv was hardcoded and is not always present.
                        let Some(player) = bo_suite_egui::playback::discover_player()
                        else {
                            self.last_note = "no player found — install mpv, ffplay \
                                              or paplay".into();
                            self.controller.transport.pause();
                            return;
                        };
                        let p = plan(player, &url, &sink);
                        for c in &p.setup { eprintln!("bo-suite: {c}"); }
                        eprintln!("bo-suite: {}", p.play);
                        self.player.start(&p);
                    }
                }
                Request::Stop => self.player.stop_now(),
                Request::ReleaseRoute => self.player.release_route("sh"),
            }
        }
        // THE PLAYER'S CLOCK INTO THE BAR. `Transport::report` is the sync
        // point between them and had no caller, so the bar extrapolated
        // from zero forever and `duration` was never set — `fraction()`
        // was permanently 0 and the handle never moved.
        //
        // The local `tick` still runs between reports: mpv prints about
        // once a second, and a bar that only moved then would stutter.
        if let Some((pos, dur)) = self.player.shared.take_progress() {
            self.controller.transport.duration = dur;
            self.controller.transport.report(pos);
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
            // A track that finished is not a track that was stopped — the
            // player distinguishes them precisely so this can advance
            // without pausing skipping a track. The view decides WHAT is
            // next (repeat mode, queue head); this only reports that the
            // last one is over.
            self.controller.on_track_ended();
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
        // Pages the list asks for, answered from the scan.
        if self.controller.pending_tracks.is_some() {
            self.controller.answer_tracks_from(&self.catalog);
        }
        self.pump_voice();
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
