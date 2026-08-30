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
    /// The discovery socket, or None when the port was unavailable. Absent
    /// rather than fatal: an app that refuses to start because something else
    /// holds 9877 is worse than one that runs without discovery and says so.
    gossip: Option<snapclone_egui::gossip_wire::GossipSocket>,
    /// When we last announced ourselves, in wall-clock seconds.
    last_announce: i64,
    /// The unsigned-gossip warning is printed once. A per-datagram log line
    /// at three seconds times every peer on the subnet buries everything
    /// else, and the message is the same every time.
    warned_unsigned: bool,
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
        let port = snap.settings.current.network.gossip_port;
        let gossip = match snapclone_egui::gossip_wire::GossipSocket::bind(port) {
            Ok(s) => Some(s),
            Err(e) => {
                eprintln!("snapclone: no discovery — could not bind UDP {port}: {e}");
                None
            }
        };
        Self { snap, focused: true, gossip, last_announce: 0, warned_unsigned: false }
    }

    /// Drain the discovery socket into the app, and announce ourselves on the
    /// tick the protocol asks for.
    ///
    /// On the frame loop rather than a thread. The socket is non-blocking and
    /// the drain is capped, so an idle network costs a syscall and a busy one
    /// cannot run past the frame that called it — and the whole path stays in
    /// one place you can step through, which a channel out of a background
    /// thread would not be.
    fn pump_gossip(&mut self, now_secs: i64) {
        let Some(sock) = self.gossip.as_mut() else { return };

        // Capped per frame; leftovers wait in the kernel buffer.
        for (from, decoded) in sock.poll(8) {
            match decoded {
                Ok(a) => {
                    let len = a.peers.len();
                    let cmds = self.snap.apply(HostEvent::GossipHeard {
                        record: a.sender_record(),
                        // The admission check wants the datagram size and we
                        // have already consumed the bytes, so this is the
                        // encoded length of what we understood. It is an
                        // approximation and the cap it feeds was already
                        // enforced by the socket's own buffer.
                        datagram_len: len,
                    });
                    self.handle(cmds);
                }
                Err(e) => eprintln!("snapclone: unreadable gossip from {from}: {e:?}"),
            }
        }

        // TICK_MS is the protocol's rate. Announcing faster is a subnet you are
        // shouting at; slower and peers prune you before you say anything.
        let due = now_secs - self.last_announce
            >= (snapclone_egui::gossip::TICK_MS / 1000).max(1) as i64;
        if !due || self.snap.local_id.is_empty() { return; }
        self.last_announce = now_secs;
        let a = snapclone_egui::gossip_wire::Announce {
            id: self.snap.local_id.clone(),
            // The address a peer should dial us back on. Left empty rather than
            // guessed: picking one of several interfaces here would tell peers
            // to come back on the wrong one, and the receiver can see the UDP
            // source address, which is the one that actually worked.
            host: String::new(),
            port: self.snap.settings.current.network.listen_port,
            ts: now_secs.max(0) as u64 * 1000,
            peers: Vec::new(),
        };
        let to = format!("255.255.255.255:{}",
                         self.snap.settings.current.network.gossip_port);
        if let Ok(addr) = to.parse() {
            if let Some(s) = self.gossip.as_ref() {
                if let Err(e) = s.send_to(&a, addr) {
                    eprintln!("snapclone: could not announce: {e}");
                }
            }
        }
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
                // Every gossip record is currently refused, and the reason is
                // in gossip_wire's header: this wire format carries no
                // signature and `gossip::accept` will not admit an unverified
                // record. Reported at a low volume rather than silently, so the
                // gap is visible in a log instead of presenting as "discovery
                // does not work".
                HostCommand::GossipAdmitted(false) => {
                    if !self.warned_unsigned {
                        self.warned_unsigned = true;
                        eprintln!("snapclone: gossip received but refused — the datagram \
                                   format carries no signature and unverified records are \
                                   not admitted. Peers will not appear online.");
                    }
                }
                HostCommand::GossipAdmitted(true) => {}
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
        self.pump_gossip(now_secs);

        // Discovery is time-driven, not input-driven, so the frame has to come
        // round on its own or an idle window never polls the socket.
        ctx.request_repaint_after(std::time::Duration::from_millis(500));

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
