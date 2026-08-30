//! snapclone_egui.rs — the deployable egui snapclone app (built behind the `app` feature so a plain
//! `cargo build`/`cargo test` on this crate stays eframe-free). Unlike gitmind/rusty-board/voiceterm/
//! bo-suite/bot-arena, snapclone isn't a PORT of a Qt/GTK twin — it's the founding app these uikit-egui
//! patterns (live backgrounds, page/subpage nav) originated from, so it uses the shared default palette
//! rather than a bespoke one matching some other UI it doesn't have.
//!
//! Run:  cargo run --bin snapclone-egui --features app   (from apps/snapclone/egui-frontend)

use eframe::egui;
use snapclone_egui::host::{HostCommand, HostEvent};
use snapclone_egui::{Peer, Snapclone};
use uikit_egui::Adaptive;

struct App {
    snap: Snapclone,
    /// Last reported focus, so the change is sent once rather than every frame.
    focused: bool,
}

impl App {
    fn new() -> Self {
        // Placeholder roster. Real peers arrive as `HostEvent::GossipHeard`
        // once there is something listening on the subnet — see the note in
        // `handle` about what this binary can and cannot yet produce.
        let peers = vec![
            Peer { name: "kitchen-tablet".into(), online: true },
            Peer { name: "studio-mbp".into(), online: true },
            Peer { name: "porch-cam".into(), online: false },
        ];
        let mut snap = Snapclone::new(peers);
        // Settings off disk, through the seam rather than by assignment: the
        // event sets `current` AND the saved baseline, so a freshly loaded
        // config does not come up reporting unsaved changes with the Save
        // button lit.
        snap.apply(HostEvent::SettingsLoaded(Box::new(snapclone_egui::settings_store::load())));
        Self { snap, focused: true }
    }

    /// What the app asked the world to do.
    ///
    /// Deliberately a debug sink and labelled as one. Capture, export and
    /// retry all need a camera pipeline, an archiver and a socket that this
    /// binary does not have yet; printing is not a stand-in for them, it is a
    /// way to SEE that the request reached the boundary — which is exactly
    /// what could not be observed while nothing drained these at all.
    fn handle(&mut self, cmds: Vec<HostCommand>) {
        for c in cmds {
            match c {
                // NOT app-internal after all, despite what this comment
                // used to claim. `MarkViewed`'s own doc says "mark as
                // seen IN STORAGE" and `MarkExpired`'s says "storage does
                // different things with each" — so these are the two
                // commands that most need a store, and they were the two
                // being silently discarded while everything else was
                // reported.
                //
                // The in-memory state exists and is tested (`snaps.rs`),
                // so today a snap is viewed until restart and unviewed
                // afterwards. Until there is a persistence layer they
                // report like the rest: a comment claiming something is
                // handled is worse than no arm at all, because it stops
                // anyone looking.
                other @ (HostCommand::MarkViewed(_) | HostCommand::MarkExpired(_)) => {
                    eprintln!("snapclone: {other:?} — no store yet, viewed state is in-memory only");
                }
                other => eprintln!("snapclone: unhandled host command: {other:?}"),
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let prefs = settings_kit::Prefs::default();
        self.snap.apply_settings(ctx, &prefs);

        // Wall clock, not frame time. `ui()` already advances gossip, sends,
        // posture, help and touch feedback per frame; this is the second-
        // resolution clock the relative-time rendering and the screensaver's
        // arming window key off, and nothing was setting it.
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let cmds = self.snap.apply(HostEvent::Tick { now_secs });
        self.handle(cmds);

        let focused = ctx.input(|i| i.focused);
        if focused != self.focused {
            self.focused = focused;
            let cmds = self.snap.apply(HostEvent::WindowFocus(focused));
            self.handle(cmds);
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            let adaptive = Adaptive::detect(ctx);
            self.snap.ui(ui, &adaptive);
        });

        // The frame just ran, and it may have set a request flag. Draining
        // here rather than waiting for the next Tick keeps a shutter press
        // from sitting unhandled for a frame.
        let cmds = self.snap.drain_commands();
        self.handle(cmds);
    }
}

/// Must match `packaging/org.snapclone.SnapClone.desktop` exactly — no
/// extension, same case. A mismatch fails SILENTLY: the app runs perfectly and
/// has no icon, no dock grouping and the wrong name, and nobody connects those
/// symptoms to a string.
const APP_ID: &str = "org.snapclone.SnapClone";

fn main() -> eframe::Result<()> {
    let opts = eframe::NativeOptions {
        // NativeOptions::default() sets no app id, which on Wayland means the
        // compositor can never find the .desktop entry — there is no WM_CLASS to
        // fall back on there, so this IS the desktop integration.
        viewport: uikit_egui::desktop::viewport(APP_ID, "SnapClone", [420.0, 820.0]),
        ..Default::default()
    };
    eframe::run_native(APP_ID, opts, Box::new(|_cc| Ok(Box::new(App::new()))))
}
