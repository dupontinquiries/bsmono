//! snapclone_egui.rs — the deployable egui snapclone app (built behind the `app` feature so a plain
//! `cargo build`/`cargo test` on this crate stays eframe-free). Unlike gitmind/rusty-board/voiceterm/
//! bo-suite/bot-arena, snapclone isn't a PORT of a Qt/GTK twin — it's the founding app these uikit-egui
//! patterns (live backgrounds, page/subpage nav) originated from, so it uses the shared default palette
//! rather than a bespoke one matching some other UI it doesn't have.
//!
//! Run:  cargo run --bin snapclone-egui --features app   (from apps/snapclone/egui-frontend)

use eframe::egui;
use snapclone_egui::capture;
use snapclone_egui::host::{HostCommand, HostEvent};
use snapclone_egui::Snapclone;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use uikit_egui::Adaptive;

/// The capture engine's host-side half: the thread that runs `capture::run`,
/// and the two handles that implement the stop contract (flag + kill).
///
/// Start/stop is driven from `update` by `frame_pump::should_stream` — the
/// pipeline runs while the camera page is visible in a focused window and
/// stops when it is not, because a phone streaming its camera into a page
/// nobody is looking at is warm for nothing. gst-launch restarts in well
/// under a page-swipe, so the restart cost is the cheap side of that trade.
struct CaptureRig {
    shared: Arc<capture::Shared>,
    stop: Arc<AtomicBool>,
    kill: capture::KillSwitch,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl CaptureRig {
    fn idle() -> CaptureRig {
        CaptureRig { shared: capture::Shared::new(), stop: Arc::new(AtomicBool::new(false)),
                     kill: capture::KillSwitch::new(), thread: None }
    }

    fn running(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }

    fn start(&mut self, plan: Vec<capture::SourceCandidate>, w: u32, h: u32) {
        self.stop_now();
        self.stop = Arc::new(AtomicBool::new(false));
        self.shared = capture::Shared::new();
        self.kill = capture::KillSwitch::new();
        let (shared, stop, kill) = (self.shared.clone(), self.stop.clone(), self.kill.clone());
        self.thread = Some(std::thread::spawn(move || {
            let opener = capture::gst_opener(w, h, kill);
            capture::run(&plan, w, h, opener, &shared, &stop,
                         std::time::Duration::from_millis(500));
        }));
    }

    /// Both halves of the stop contract, then reap the thread. The join is
    /// bounded because the kill turns any blocked read into an EOF.
    fn stop_now(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.kill.stop();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct App {
    snap: Snapclone,
    rig: CaptureRig,
    /// The plan is printed once per start, not per frame: the pipeline
    /// strings are the debuggable artifact, and a log that repeats them at
    /// 30 Hz buries the stderr lines gst-launch itself contributes.
    plan_logged: bool,
    /// Last reported focus, so the change is sent once rather than every frame.
    focused: bool,
    /// The discovery socket, or None when the port was unavailable. Absent
    /// rather than fatal: an app that refuses to start because something else
    /// holds 9877 is worse than one that runs without discovery and says so.
    gossip: Option<snapclone_egui::gossip_wire::GossipSocket>,
    /// When we last announced ourselves, in wall-clock seconds.
    last_announce: i64,
    /// Kept because announcing requires the signing key, and the key must not
    /// travel through `Snapclone` — the app holds `local_id`, which is public,
    /// and has no business holding a secret it never needs.
    identity: Option<snapclone_egui::identity::Identity>,
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
            eprintln!("snapclone: new identity {} — identity.conf holds a signing key, \
                       keep it", i.sc_id);
        }
        let keys = match &id {
            snapclone_egui::identity::Loaded::Existing(i)
            | snapclone_egui::identity::Loaded::Created(i) => Some(i.clone()),
            snapclone_egui::identity::Loaded::Unavailable => None,
        };
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
        // THE CAMERA LIST. `App.cameras` starts empty and only
        // `CamerasFound` fills it — and nothing in the tree sent that
        // event, so the picker was permanently blank and no USB webcam
        // could appear on any machine. Everything downstream
        // (camera_select, camera_source, camera_policy) was built and
        // tested against a list nobody produced.
        //
        // Sent once at startup rather than polled: hotplug needs a
        // netlink or inotify watch, which is a bigger change, and a
        // camera present at launch is the case that was broken.
        snap.apply(HostEvent::CamerasFound(snapclone_egui::camera_enumerate::enumerate()));
        let port = snap.settings.current.network.gossip_port;
        let gossip = match snapclone_egui::gossip_wire::GossipSocket::bind(port) {
            Ok(s) => Some(s),
            Err(e) => {
                eprintln!("snapclone: no discovery — could not bind UDP {port}: {e}");
                None
            }
        };
        if let Some(g) = gossip.as_ref() {
            if !g.joined_group() {
                // Worth its own line: discovery will look broken and the cause
                // is the interface, not the app.
                eprintln!("snapclone: could not join the discovery multicast group — \
                           peers on this network will not be found");
            }
        }
        Self { snap, rig: CaptureRig::idle(), plan_logged: false, focused: true,
               gossip, last_announce: 0, warned_unsigned: false, identity: keys }
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
                        // The address the packet CAME from, when the sender did
                        // not name one. Without this the record's host is empty,
                        // parse_dotted_quad declines it, and a peer that
                        // verified perfectly is still not dialable.
                        record: a.sender_record_from(from.ip()),
                        // The admission check wants the datagram size and we
                        // have already consumed the bytes, so this is the
                        // encoded length of what we understood. It is an
                        // approximation and the cap it feeds was already
                        // enforced by the socket's own buffer.
                        datagram_len: len,
                        public: a.public,
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
        // No key, no announcement. Broadcasting unsigned would be traffic every
        // correctly-configured peer refuses, which reads as a flaky network
        // rather than as a missing identity.
        let Some(keys) = self.identity.clone() else { return };
        if !due { return; }
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
            // Filled by the signer, not here — `verified` is only ever true on
            // something that came off the wire and checked out.
            public: None,
            verified: false,
        };
        let to = snapclone_egui::gossip_wire::GossipSocket::group_addr(
            self.snap.settings.current.network.gossip_port);
        if let Ok(addr) = to.parse() {
            if let Some(s) = self.gossip.as_ref() {
                if let Err(e) = s.send_signed_to(&a, &keys.secret, &keys.public, addr) {
                    eprintln!("snapclone: could not announce: {e}");
                }
            }
        }
    }

    /// Start, stop and drain the capture engine, from the frame loop.
    ///
    /// On the frame loop for the same reason `pump_gossip` is: the drain is
    /// latest-wins and non-blocking, so an idle camera costs three mutex
    /// peeks, and the whole path is steppable in one place. Only the
    /// blocking pipe read lives on the rig's thread, because it has to.
    fn pump_capture(&mut self) {
        // "Camera" by name rather than index 0, so a nav reorder cannot
        // silently leave the pipeline gated on the wrong page.
        let camera_page = snapclone_egui::PAGES
            .iter().position(|n| *n == "Camera").unwrap_or(0);
        let visible = self.snap.nav.page() == camera_page;
        // `screen_on` is not knowable from eframe; `true` defers to focus,
        // which a locked phone drops anyway.
        let want = snapclone_egui::frame_pump::should_stream(visible, self.focused, true)
            && self.snap.camera.is_some();

        if want && !self.rig.running() {
            let choice = self.snap.camera.clone().expect("gated on is_some above");
            // No PipeWire probe yet: pw_nodes empty means a phone's libcamera
            // nodes only enter the plan via the settings override (a node
            // name in camera.device). The v4l2 half probes for real when the
            // tool exists; absent tool contributes nothing, not an error.
            let v4l2 = std::process::Command::new("v4l2-ctl").arg("--list-devices")
                .output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                .unwrap_or_default();
            let plan = snapclone_egui::camera_source::pipelines_for(&choice, &[], &v4l2);
            if !self.plan_logged {
                self.plan_logged = true;
                for c in &plan {
                    eprintln!("snapclone: capture candidate [{}]: {}", c.label, c.pipeline);
                }
            }
            let (w, h) = (choice.format.width, choice.format.height);
            self.rig.start(plan, w, h);
        } else if !want && self.rig.running() {
            self.rig.stop_now();
            let cmds = self.snap.apply(HostEvent::CaptureStatus(capture::Status::Idle));
            self.handle(cmds);
        }

        // Drain, newest only. `CameraWorked` before the frame so the health
        // record exists by the time anything reads it this tick.
        if self.rig.shared.take_worked() {
            if let Some(cam) = self.snap.camera.as_ref() {
                let id = cam.device.id.clone();
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64).unwrap_or(0);
                let cmds = self.snap.apply(HostEvent::CameraWorked { id, now_ms });
                self.handle(cmds);
            }
        }
        if let Some(st) = self.rig.shared.take_status() {
            let cmds = self.snap.apply(HostEvent::CaptureStatus(st));
            self.handle(cmds);
        }
        if let Some(f) = self.rig.shared.take_frame() {
            let cmds = self.snap.apply(HostEvent::CameraFrame(f));
            self.handle(cmds);
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
        // Events a command handler wants to send back. Collected rather than
        // applied inline: `apply` drains, and calling it from inside a drain
        // would run the drain again half way through this loop.
        let mut pending: Vec<HostEvent> = Vec::new();
        for c in cmds {
            match c {
                // REAL, not a debug sink. This is the whole point of the
                // exercise: a contact scanned or renamed now reaches the disk,
                // so it is still there next launch. A write failure is
                // reported rather than swallowed — a silent one means the user
                // adds someone twice and never learns why.
                // THE SHUTTER PRODUCES A FILE. Not the composer flow yet —
                // that is its own thread of work — but a press that lands a
                // viewable image on disk is the difference between a camera
                // and a viewfinder. PPM because it is 12 bytes of header over
                // the RGBA we already hold: no encoder dependency, and every
                // image tool opens it. The path is printed so the photo is
                // findable the moment it exists.
                HostCommand::CaptureSnap => {
                    let outcome = match self.rig.shared.peek_frame() {
                        Some(f) if f.pixels_plausible() => save_ppm(&f),
                        Some(_) => Err("the frame arrived torn".to_string()),
                        None => Err("no live frame".to_string()),
                    };
                    match &outcome {
                        Ok(path) => eprintln!("snapclone: snap saved: {path}"),
                        Err(e) => eprintln!("snapclone: snap NOT saved: {e}"),
                    }
                    // Back through the seam, not just to stderr: the person
                    // who pressed the shutter is looking at the camera page.
                    // Queued rather than applied inline — `handle` is called
                    // FROM a drain, and re-entering `apply` here would drain
                    // a second time mid-iteration.
                    pending.push(HostEvent::SnapSaved(outcome));
                }
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
                        eprintln!("snapclone: gossip refused — unsigned, or the sender's \
                                   sc-ID is not its key's fingerprint. Qt peers do not sign \
                                   and will not appear online.");
                    }
                }
                HostCommand::GossipAdmitted(true) => {}
                HostCommand::PeerKeyChanged(id) => {
                    eprintln!("snapclone: {id} is signing with a different key than the one \
                               on file. They may have reinstalled — or someone is \
                               impersonating them. Refused; delete and re-add the contact \
                               if you know it was a reinstall.");
                }
                HostCommand::IdentityUnavailable => {
                    eprintln!("snapclone: no identity — /dev/urandom was unreadable. \
                               Nobody can add this device and the QR code will be blank.");
                }
                // NOT app-internal, despite what this comment used to say —
                // and re-applied here after the merge, because the incoming
                // branch predates the fix and reintroduced it.
                //
                // `MarkViewed`'s own doc says "mark as seen IN STORAGE" and
                // `MarkExpired`'s says "storage does different things with
                // each". So these are the two commands that most need a
                // store, and they were the two being silently discarded while
                // everything around them is now persisted for real.
                //
                // The rest of this match got a disk in this very merge, which
                // makes the gap sharper rather than softer: viewed state is
                // still in memory only, so a snap is viewed until restart and
                // unviewed afterwards.
                other @ (HostCommand::MarkViewed(_) | HostCommand::MarkExpired(_)) => {
                    eprintln!("snapclone: {other:?} — no store yet, viewed state is in-memory only");
                }
                other => eprintln!("snapclone: unhandled host command: {other:?}"),
            }
        }
        // Feed back what the handlers produced. Bounded by construction —
        // nothing in this list produces a command that produces another
        // event — but recursion through `handle` would not be, so the reply
        // goes through `apply` and its commands are reported, not re-run.
        for ev in pending {
            let cmds = self.snap.apply(ev);
            for c in cmds {
                eprintln!("snapclone: after a reply event: {c:?}");
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

/// Write one frame as binary PPM under the app's data dir, named by wall
/// clock so a burst of presses cannot overwrite each other within a second
/// (the millisecond field sees to that).
fn save_ppm(f: &snapclone_egui::frame_pump::RawFrame) -> Result<String, String> {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis()).unwrap_or(0);
    let dir = config_kit::data_dir("snapclone").join("snaps");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(format!("snap-{ms}.ppm"));
    let mut out = Vec::with_capacity(20 + (f.width * f.height * 3) as usize);
    out.extend_from_slice(format!("P6\n{} {}\n255\n", f.width, f.height).as_bytes());
    // RGBA to RGB: PPM has no alpha, and the camera's alpha is opaque anyway.
    for px in f.rgba.chunks_exact(4) {
        out.extend_from_slice(&px[..3]);
    }
    std::fs::write(&path, out).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path.display().to_string())
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
        self.pump_capture();

        // Discovery is time-driven, not input-driven, so the frame has to
        // come round on its own or an idle window never polls the socket —
        // and a live viewfinder needs the frame loop at frame rate, or the
        // preview updates only when the mouse moves.
        ctx.request_repaint_after(std::time::Duration::from_millis(
            if self.rig.running() { 33 } else { 500 }));

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
        // Glow explicitly. Left to eframe the renderer depends on whether
        // egui-wgpu happens to be linked by feature unification, and when
        // it is, eframe asks for `wgpu::Limits::default()` — 8 colour
        // attachments — without clamping to the adapter. That killed
        // gitmind-egui at startup on an Asahi GPU allowing 5, and this
        // crate declares glow and no wgpu, so it was exposed to the same
        // accident.
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(APP_ID, opts, Box::new(|_cc| Ok(Box::new(App::new()))))
}
