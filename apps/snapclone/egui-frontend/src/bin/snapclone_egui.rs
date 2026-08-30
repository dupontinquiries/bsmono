//! snapclone_egui.rs — the deployable egui snapclone app (built behind the `app` feature so a plain
//! `cargo build`/`cargo test` on this crate stays eframe-free). Unlike gitmind/rusty-board/voiceterm/
//! bo-suite/bot-arena, snapclone isn't a PORT of a Qt/GTK twin — it's the founding app these uikit-egui
//! patterns (live backgrounds, page/subpage nav) originated from, so it uses the shared default palette
//! rather than a bespoke one matching some other UI it doesn't have.
//!
//! Run:  cargo run --bin snapclone-egui --features app   (from apps/snapclone/egui-frontend)

use eframe::egui;
use snapclone_egui::host::{HostCommand, HostEvent};
use snapclone_egui::Snapclone;
use uikit_egui::Adaptive;

struct App {
    snap: Snapclone,
    /// Last reported focus, so the change is sent once rather than every frame.
    focused: bool,
}

impl App {
    fn new() -> Self {
        // NO PLACEHOLDER ROSTER. kitchen-tablet, studio-mbp and porch-cam were
        // the three names this binary invented on every launch, and they were
        // most of why the app read as a demo: the roster was furniture, and a
        // contact you actually added disappeared when you quit.
        //
        // The list now starts empty and is filled from disk. Empty is the
        // correct first-run state and the Friends page already has an empty
        // view that names the ports to open — which is real help, unlike three
        // devices that do not exist.
        let mut snap = Snapclone::new(Vec::new());

        // Identity before contacts, because the QR code the user shows is
        // drawn from it and a fresh install's first act is usually to show it.
        let id = snapclone_egui::identity::load_or_create();
        if let snapclone_egui::identity::Loaded::Created(ref i) = id {
            eprintln!("snapclone: new identity {} (config: identity.conf)", i.sc_id);
        }
        let cmds = snap.apply(HostEvent::IdentityLoaded(id));
        Self::report(cmds);

        let book = snapclone_egui::contact_book::load();
        let cmds = snap.apply(HostEvent::ContactsLoaded(Box::new(book)));
        Self::report(cmds);

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
                // REAL, not a debug sink. This is the whole point of the
                // exercise: a contact scanned or renamed now reaches the disk,
                // so it is still there next launch. A write failure is
                // reported rather than swallowed — a silent one means the user
                // adds someone twice and never learns why.
                HostCommand::SaveContacts(book) => {
                    if let Err(e) = snapclone_egui::contact_book::store(&book) {
                        eprintln!("snapclone: could not save contacts: {e}");
                    }
                }
                HostCommand::IdentityUnavailable => {
                    eprintln!("snapclone: no identity — /dev/urandom was unreadable. \
                               Nobody can add this device and the QR code will be blank.");
                }
                // These two the app can answer for itself: they are bookkeeping
                // about what the user already saw, not work for a backend.
                HostCommand::MarkViewed(_) | HostCommand::MarkExpired(_) => {}
                other => eprintln!("snapclone: unhandled host command: {other:?}"),
            }
        }
    }

    /// `handle` before there is a `self` to handle with. Start-up events can
    /// produce commands too — a first-run identity is exactly when something
    /// might need saying — and dropping them silently would be the same class
    /// of bug the drain exists to prevent.
    fn report(cmds: Vec<HostCommand>) {
        for c in cmds {
            eprintln!("snapclone: startup: {c:?}");
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
