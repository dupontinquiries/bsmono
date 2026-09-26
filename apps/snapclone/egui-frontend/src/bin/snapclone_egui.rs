//! snapclone_egui.rs — the deployable egui snapclone app (built behind the `app` feature so a plain
//! `cargo build`/`cargo test` on this crate stays eframe-free). Unlike gitmind/rusty-board/voiceterm/
//! bo-suite/bot-arena, snapclone isn't a PORT of a Qt/GTK twin — it's the founding app these uikit-egui
//! patterns (live backgrounds, page/subpage nav) originated from, so it uses the shared default palette
//! rather than a bespoke one matching some other UI it doesn't have.
//!
//! Run:  cargo run --bin snapclone-egui --features app   (from apps/snapclone/egui-frontend)

use eframe::egui;
use snapclone_egui::audio;
use snapclone_egui::capture;
use snapclone_egui::host::{HostCommand, HostEvent};
use snapclone_egui::Snapclone;

/// `SNAPCLONE_TEST_PATTERN=1`: a synthetic camera in place of the enumerated ones.
fn test_pattern() -> bool {
    std::env::var("SNAPCLONE_TEST_PATTERN").map(|v| v == "1").unwrap_or(false)
}
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
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
            if test_pattern() {
                // SNAPCLONE_TEST_PATTERN_CODE=<sc-id>: the synthetic camera holds a
                // code up to the lens, so the scanner can be walked without a
                // second phone.
                let code = std::env::var("SNAPCLONE_TEST_PATTERN_CODE").ok();
                let opener = move |_: &capture::SourceCandidate| -> Result<Box<dyn capture::FrameStream>, String> {
                    Ok(Box::new(capture::TestPattern::new(w, h).with_code(code.clone())))
                };
                capture::run(&plan, w, h, opener, &shared, &stop,
                             std::time::Duration::from_millis(500));
                return;
            }
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

/// The voice call's host half: a UDP socket, a mic thread, and a speaker.
///
/// Assembled and torn down with the CALL, not with the process. A mic
/// process running while nobody is in a call is a recording light on some
/// hardware and a wakeup on all of it, and "we hold the mic open the whole
/// time but promise not to listen" is not a property a user can check.
struct VoiceRig {
    sock: Option<snapclone_egui::voice_wire::VoiceSocket>,
    peers: snapclone_egui::voice_wire::Peers,
    mic: Arc<audio::MicShared>,
    mic_stop: Arc<AtomicBool>,
    mic_child: Option<audio::Child>,
    mic_thread: Option<std::thread::JoinHandle<()>>,
    speaker: Option<audio::Speaker>,
    speaker_child: Option<audio::Child>,
    /// Packs mic frames for the wire and unpacks arriving raw ones (voice_codec).
    codec: snapclone_egui::voice_codec::VoiceCodec,
    /// Arriving Opus: ordered, concealed, decoded per speaker (voice_rx), ticked
    /// once per frame interval from `rx_clock`.
    rx: snapclone_egui::voice_rx::VoiceRx,
    rx_clock: std::time::Instant,
    rx_frame: std::time::Duration,
    /// Wall-clock of the last `SinkDepth`, so the tick runs at the cushion's
    /// rate rather than the frame rate — a 120 Hz display must not ask the
    /// play-out layer for audio twice per frame.
    last_sink_tick: std::time::Instant,
    /// Said once per call, not per frame.
    warned_no_audio: bool,
    /// Mic and speaker are up. The SOCKET outlives a call — it is the
    /// punch socket, and its NAT mapping is what our record announces
    /// (`nat_traversal`): STUN discovered it from this socket, and a
    /// mapping measured on a socket that closes when the call ends is a
    /// mapping nobody can use.
    joined: bool,
    warned_no_socket: bool,
}

impl VoiceRig {
    fn idle() -> VoiceRig {
        VoiceRig {
            sock: None, peers: snapclone_egui::voice_wire::Peers::new(),
            mic: audio::MicShared::new(), mic_stop: Arc::new(AtomicBool::new(false)),
            mic_child: None, mic_thread: None, speaker: None, speaker_child: None,
            codec: snapclone_egui::voice_codec::VoiceCodec::new(
                snapclone_egui::sink_cushion::SAMPLE_RATE, snapclone_egui::sink_cushion::frame_samples(snapclone_egui::sink_cushion::FRAME_MS)),
            rx: snapclone_egui::voice_rx::VoiceRx::new(
                snapclone_egui::sink_cushion::SAMPLE_RATE, snapclone_egui::sink_cushion::frame_samples(snapclone_egui::sink_cushion::FRAME_MS)),
            rx_clock: std::time::Instant::now(),
            rx_frame: std::time::Duration::from_millis(snapclone_egui::sink_cushion::FRAME_MS as u64),
            last_sink_tick: std::time::Instant::now(), warned_no_audio: false,
            joined: false, warned_no_socket: false,
        }
    }

    /// Bind the punch/voice socket if it is not already. Said once on
    /// failure; retried every call so a port freed later is picked up.
    fn ensure_socket(&mut self, port: u16) -> bool {
        if self.sock.is_some() { return true; }
        match snapclone_egui::voice_wire::VoiceSocket::bind(port) {
            Ok(s) => { self.sock = Some(s); self.warned_no_socket = false; true }
            Err(e) => {
                if !self.warned_no_socket {
                    self.warned_no_socket = true;
                    eprintln!("snapclone: no voice/punch socket on UDP {port}: {e}");
                }
                false
            }
        }
    }

    fn live(&self) -> bool { self.joined }

    /// Bring up the socket and both devices. A missing audio tool is NOT
    /// fatal to the call: the socket still carries the other direction, and
    /// a call where you can hear but not speak beats no call plus a
    /// traceback.
    fn start(&mut self, port: u16, frame_ms: u32, record_cmd: &str, play_cmd: &str)
             -> Vec<String> {
        let mut notes = Vec::new();
        if !self.ensure_socket(port) {
            notes.push(format!("no voice socket on UDP {port}"));
            return notes;
        }
        self.joined = true;
        self.mic_stop = Arc::new(AtomicBool::new(false));
        self.mic = audio::MicShared::new();
        match audio::spawn(record_cmd, audio::Direction::Record) {
            Ok((pipe, child)) => {
                self.mic_child = Some(child);
                let (shared, stop) = (self.mic.clone(), self.mic_stop.clone());
                self.mic_thread = Some(std::thread::spawn(move || {
                    audio::run_mic(pipe, frame_ms, &shared, &stop);
                }));
            }
            Err(e) => notes.push(format!("no microphone: {e}")),
        }
        match audio::spawn(play_cmd, audio::Direction::Play) {
            Ok((pipe, child)) => {
                self.speaker_child = Some(child);
                self.speaker = Some(audio::Speaker::new(pipe, frame_ms));
            }
            Err(e) => notes.push(format!("no speaker: {e}")),
        }
        notes
    }

    /// Both halves of the stop contract for each child, then reap.
    fn stop(&mut self) {
        self.codec.reset();
        self.rx = snapclone_egui::voice_rx::VoiceRx::new(
            snapclone_egui::sink_cushion::SAMPLE_RATE, snapclone_egui::sink_cushion::frame_samples(snapclone_egui::sink_cushion::FRAME_MS));
        self.mic_stop.store(true, Ordering::Relaxed);
        if let Some(c) = self.mic_child.take() { c.stop(); c.reap(); }
        if let Some(t) = self.mic_thread.take() { let _ = t.join(); }
        // The speaker holds the child's stdin; dropping it is what lets a
        // well-behaved tool finish and exit rather than being killed
        // mid-buffer.
        self.speaker = None;
        if let Some(c) = self.speaker_child.take() { c.stop(); c.reap(); }
        // The socket stays: see `joined`.
        self.joined = false;
        self.warned_no_audio = false;
    }
}

struct App {
    snap: Snapclone,
    /// On-screen keyboard commits -> text, before egui sees them. Without it every text
    /// field keeps only the first character typed on a Wayland on-screen keyboard; see
    /// `uikit_egui::ime::ImeRepair`.
    ime_repair: uikit_egui::ime::ImeRepair,
    /// The accelerometer, once. `None` on a machine without one, and that
    /// case must cost nothing per frame.
    accel: Option<snapclone_egui::glove_input::SysfsAccel>,
    /// Whether the probe has run. Separate from `accel.is_some()` so a
    /// machine with no sensor is not re-probed every frame — a directory
    /// scan of /sys at frame rate is the exact cost the discovery-once
    /// rule exists to avoid.
    glove_probed: bool,
    /// Accepts inbound snaps for the life of the process, unlike the camera
    /// and voice rigs which come up with a page or a call. A snap arriving
    /// while you are on the Friends page is the ordinary case, and a
    /// listener that only ran on the Inbox page would drop it.
    snaps_in: Option<snapclone_egui::snap_transport::Listener>,
    voice: VoiceRig,
    /// `NatTraversal.cpp`'s loop, driven from `pump_nat` on the voice socket.
    nat: snapclone_egui::nat_traversal::Traversal,
    nat_id: String,
    last_stun_ms: Option<u64>,
    /// A resolver in flight for the STUN list. DNS is synchronous and can
    /// block for seconds on a network with a dead resolver — the hotspot
    /// case the server list has IP entries for — and this is the UI
    /// thread. Resolved off it; picked up when ready.
    stun_resolve: Option<Arc<std::sync::Mutex<Option<Vec<std::net::SocketAddr>>>>>,
    /// The bytes of the last shutter press, kept so Send has something to
    /// send. Held by the HOST and not by the app: the controller has no
    /// business carrying a photo in memory purely to hand it back.
    last_snap: Option<Vec<u8>>,
    /// The frame most recently handed to the pump — what the viewfinder is
    /// showing. The shutter reads THIS. `peek_frame` on the capture slot
    /// only sees a frame that arrived since the last `take_frame`, and the
    /// tick takes every frame into the pump at once, so the slot is empty
    /// nearly every time the shutter looks: "snap NOT saved: no live frame"
    /// with a picture on screen. Seen under Xvfb with the test pattern at
    /// 15 fps; at a camera's 30 fps it was a coin toss per press.
    last_live: Option<snapclone_egui::frame_pump::RawFrame>,
    /// Frames seen while the scanner is up, for the every-fourth decode throttle.
    scan_frames: u32,
    /// Sends running on worker threads. A dial can take the full timeout
    /// and must never run on the frame loop.
    sending: Vec<std::thread::JoinHandle<()>>,
    /// Results from those threads.
    send_results: Arc<Mutex<Vec<(String, Result<(), String>)>>>,
    /// An export running on its own thread, and where it reports. Archiving a
    /// month of photos takes seconds; the UI thread must not wait for zip.
    export_result: Arc<Mutex<Option<snapclone_egui::snap_export::Outcome>>>,
    exporting: Option<std::thread::JoinHandle<()>>,
    rig: CaptureRig,
    /// The plan is printed once per start, not per frame: the pipeline
    /// strings are the debuggable artifact, and a log that repeats them at
    /// 30 Hz buries the stderr lines gst-launch itself contributes.
    plan_logged: bool,
    /// Last reported focus, so the change is sent once rather than every frame.
    focused: bool,
    /// When stored snaps were last swept against retention.
    last_sweep: Option<i64>,
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

        // BEFORE anything can populate the feed. A snap filtered after it
        // has been shown has already been shown, and this store exists
        // because a viewed snap coming back is the promise failing in the
        // direction the sender cares about.
        let st = snapclone_egui::snap_state::load();
        let cmds = snap.apply(HostEvent::SnapStateLoaded(Box::new(st)));
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
        let cameras = if test_pattern() {
            // SNAPCLONE_TEST_PATTERN=1: one synthetic device, so the whole
            // capture path — plan, thread, pump, upload, shutter, encode —
            // runs with no camera and no GStreamer. See capture::TestPattern.
            eprintln!("snapclone: SNAPCLONE_TEST_PATTERN — synthetic camera, no device opened");
            vec![snapclone_egui::camera_select::Device {
                id: "test-pattern".into(),
                label: "Test pattern (synthetic)".into(),
                facing: snapclone_egui::camera_select::Facing::Rear,
                capture_group: None,
            }]
        } else {
            snapclone_egui::camera_enumerate::enumerate()
        };
        snap.apply(HostEvent::CamerasFound(cameras));
        // SNAPCLONE_BG=aurora|waves|starfield|gradient|bokeh: pick the live
        // background at launch, for looking at each one under a display
        // without walking to Settings every time.
        if let Ok(b) = std::env::var("SNAPCLONE_BG") {
            snap.bg.style = uikit_egui::BgStyle::parse(&b);
        }
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
        // The snap listener, on the configured listen port — the same one
        // announced in gossip, so a peer that heard us knows where to dial.
        let listen = snap.settings.current.network.listen_port;
        let snaps_in = match snapclone_egui::snap_transport::Listener::start(listen) {
            Ok(l) => Some(l),
            Err(e) => {
                // Not fatal, and worth its own line: outbound snaps still
                // work, so the symptom is one-directional and would
                // otherwise look like the other person ignoring you.
                eprintln!("snapclone: not accepting snaps — could not listen on TCP \
                           {listen}: {e}");
                None
            }
        };

        Self { snap, ime_repair: Default::default(), last_sweep: None, snaps_in, accel: None, glove_probed: false,
               last_snap: None, last_live: None, scan_frames: 0, sending: Vec::new(),
               send_results: Arc::new(Mutex::new(Vec::new())),
               export_result: Arc::new(Mutex::new(None)), exporting: None,
               voice: VoiceRig::idle(), rig: CaptureRig::idle(),
               nat: snapclone_egui::nat_traversal::Traversal::new(""), nat_id: String::new(),
               last_stun_ms: None, stun_resolve: None,
               plan_logged: false, focused: true,
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
    /// Read the accelerometer and let glove mode act on it.
    ///
    /// The whole of cold-weather glove mode was already here — the cursor,
    /// the shake bindings, `apply_glove` and `apply_shake` — and every
    /// piece of it sat unreached because nothing in this app read a
    /// sensor. The accessibility mode existed and could not be turned on
    /// by moving the phone.
    ///
    /// Discovered ONCE, on the first frame. Walking /sys/bus/iio on every
    /// poll is a directory scan twenty times a second for a device that
    /// does not move between sockets — and a machine with no accelerometer
    /// must pay nothing at all, which is what the `None` case buys.
    fn pump_glove(&mut self, ctx: &egui::Context) {
        if !self.glove_probed {
            self.glove_probed = true;
            self.accel = snapclone_egui::glove_input::SysfsAccel::discover();
            self.snap.glove_input.enable_if_present(self.accel.is_some());
        }
        let Some(accel) = self.accel.as_mut() else { return };
        let dt_ms = ctx.input(|i| i.stable_dt).min(0.25) * 1000.0;
        if self.snap.poll_glove(dt_ms, accel).is_some() {
            // A flick moved something, so the next frame has to draw it.
            ctx.request_repaint();
        }
    }

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
                    let admitted = cmds.iter()
                        .any(|c| matches!(c, HostCommand::GossipAdmitted(true)));
                    self.handle(cmds);
                    // THE PUNCH ADDRESS, which nothing was learning. `punch`,
                    // `heard` and every dial path hang off `note_peer_addr`
                    // and no caller existed, so `PunchAddrs` was empty for
                    // the whole life of the port — no hole punch had an
                    // address to punch to, and voice had nowhere to send.
                    //
                    // Only for an ADMITTED record: `gossip::accept` is the
                    // signature check, and learning an address from a
                    // refused datagram would let anyone on the subnet
                    // redirect a peer's traffic by announcing as them.
                    if admitted {
                        if let std::net::IpAddr::V4(v4) = from.ip() {
                            // The port the sender NAMED, not the source port
                            // its gossip came from: gossip is multicast from
                            // an ephemeral port and the listen port is where
                            // it wants to be reached. Falling back to the
                            // source port would send voice to a socket
                            // nobody is reading.
                            let port = if a.port != 0 { a.port } else { from.port() };
                            let cmds = self.snap.apply(HostEvent::PeerAddr {
                                id: a.id.clone(), ip: v4.octets(), port,
                                now_ms: now_secs.max(0) as u64 * 1000,
                            });
                            self.handle(cmds);
                        }
                    }
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
        // Stale addresses go on the same tick. `purge_stale` existed with no
        // caller, so an address learned once was authoritative forever —
        // including for voice admission, where a peer who moved networks
        // would keep an ex-neighbour's address authorised.
        let cmds = self.snap.apply(HostEvent::ExpirePunchAddrs {
            now_ms: now_secs.max(0) as u64 * 1000 });
        self.handle(cmds);
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
        // THE SCANNER NEEDS THE CAMERA. Scanning happens from the Friends page,
        // where the camera page is not visible and the stream was stopped — so
        // "Point at a SnapClone QR code" sat over a viewfinder that could never
        // see one. A live scanner keeps the stream up wherever it is opened.
        let scanning = self.snap.qr.as_ref().is_some_and(|q| q.is_scanning());
        let want = snapclone_egui::frame_pump::should_stream(visible || scanning, self.focused, true)
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
        // THE ENCODE'S ANSWER. It runs on its own thread and would otherwise finish
        // invisibly — a snap that silently succeeded and one that silently failed look
        // identical from the camera page, which is the state this whole chain was in before
        // the frames existed.
        while let Ok(done) = self.snap.encode_done.1.try_recv() {
            use snapclone_egui::video_encode_run::Done;
            self.snap.shutter_note = match done {
                Done::Ok { out, bytes } => {
                    format!("encoded {} \u{2014} {:.1}MB", out.display(), bytes as f64 / 1e6)
                }
                // The reason, not just "failed". `run_blocking` already trimmed ffmpeg's
                // stderr to its tail so the complaint is not buried under a build dump.
                Done::Failed { code, why } => {
                    format!("encode failed{}: {why}", code.map(|c| format!(" ({c})")).unwrap_or_default())
                }
                Done::NotRun(e) => format!("encoder did not start: {e}"),
            };
        }
        if let Some(f) = self.rig.shared.take_frame() {
            self.last_live = Some(f.clone());
            // THE RECORDER, FED. `Recorder::offer` had no caller at all: it counted frames it
            // never saw, `av1_encode` built a command with no input, and video snaps could not
            // work however complete the encoder side was. This is the missing link, and the
            // sink is `video_spill`, which writes each ADMITTED frame — the recorder's rules
            // (no duplicates, no reorders, no resize, no torn buffers) run in front of the
            // disk rather than behind it, which is the point of putting them there.
            //
            // A refusal is not an error here: dropping a duplicate is the recorder working.
            // Only the SPILL's own failures are worth surfacing, and it holds those until the
            // end because a camera thread is not a place to unwind.
            if self.snap.recorder.is_some() {
                let (rec, spill_slot) = (&mut self.snap.recorder, &mut self.snap.spill);
                let rec = rec.as_mut().expect("checked above");
                let spill = spill_slot.get_or_insert_with(|| {
                    snapclone_egui::video_spill::Spill::create(
                        snapclone_egui::video_spill::spill_path(f.seq),
                    )
                });
                let _ = rec.offer(&f, &mut |admitted| spill.write(&admitted.rgba));
            }
            // THE DECODER, while the scanner is up: every fourth frame through
            // the kit's `qr::decode_frame`, and a hit becomes the same
            // `QrPayload` event a pasted code raises. Nothing fed this
            // event before; the scanner had a status line and no eyes.
            if self.snap.qr.as_ref().is_some_and(|q| q.is_scanning()) {
                self.scan_frames = self.scan_frames.wrapping_add(1);
                // COUNTED, so the scanner can say why it has not found anything. Looking and
                // broken produce the same screen otherwise — a preview and no result — and
                // the person holding the phone moves it closer, then turns it, then gives up
                // on a feature that was never running.
                //
                // Both counters here rather than inside the `if`: a frame that is not
                // plausible still ARRIVED, and it is the gap between those two numbers that
                // says "the camera works and the picture is black".
                self.snap.scan_counts.frames += 1;
                if f.pixels_plausible() {
                    self.snap.scan_counts.plausible += 1;
                }
                if self.scan_frames % 4 == 0 && f.pixels_plausible() {
                    self.snap.scan_counts.attempts += 1;
                    if let Some(id) = uikit_egui::qr::decode_frame(&f.rgba, f.width as usize, f.height as usize) {
                        eprintln!("snapclone: scanned {id}");
                        // Kept, because the scanner CLOSES on a hit: without this the status
                        // reverts to "scanner off" the moment it succeeds, which reads as the
                        // scan having failed at the last second.
                        self.snap.scan_counts.hits += 1;
                        self.snap.scan_hit = Some(id.clone());
                        let cmds = self.snap.apply(HostEvent::QrPayload(id));
                        self.handle(cmds);
                    }
                }
            }
            let cmds = self.snap.apply(HostEvent::CameraFrame(f));
            self.handle(cmds);
        }
    }

    /// Bring the voice path up and down with the call, and pump it.
    /// Hand refused datagrams to the traversal: punches and STUN replies
    /// arrive from senders the voice path does not know. Returns what was
    /// neither.
    fn feed_nat(&mut self, refused: Vec<snapclone_egui::voice_wire::Refused>)
        -> Vec<snapclone_egui::voice_wire::Refused> {
        let mut rest = Vec::new();
        for r in refused {
            match r {
                snapclone_egui::voice_wire::Refused::UnknownSender(from, bytes) => {
                    match self.nat.on_datagram(from, &bytes) {
                        Some(ev) => self.on_nat_event(ev),
                        None => rest.push(snapclone_egui::voice_wire::Refused::UnknownSender(from, bytes)),
                    }
                }
                other => rest.push(other),
            }
        }
        rest
    }

    fn on_nat_event(&mut self, ev: snapclone_egui::nat_traversal::Event) {
        use snapclone_egui::nat_traversal::Event;
        match ev {
            Event::Reflexive(addr) => {
                if self.snap.reflexive != Some(addr) {
                    eprintln!("snapclone: our punch socket is {addr} from outside");
                }
                self.snap.reflexive = Some(addr);
                self.snap.stun_failed = false;
            }
            Event::StunFailed => {
                self.snap.stun_failed = true;
                eprintln!("snapclone: no STUN server answered — our public address stays unknown");
            }
            Event::PunchReady { peer, addr } =>
                eprintln!("snapclone: punch volley to {peer} at {addr} complete; keeping it open"),
            Event::PunchArrived { peer, from } => {
                // registerPunchedPeer: where the packets ACTUALLY come from
                // is the address to send to, whatever the record said.
                if let std::net::IpAddr::V4(ip) = from.ip() {
                    self.snap.note_peer_addr(&peer, ip.octets(), from.port(), wall_ms());
                    eprintln!("snapclone: punch from {peer} at {from}");
                }
            }
        }
    }

    /// STUN once we know who we are, again every fifteen minutes, and
    /// when the DHT says nobody answers any more (the network changed);
    /// punch every call participant that has an address; send what is due.
    fn pump_nat(&mut self) {
        let port = self.snap.settings.current.network.listen_port;
        if !self.voice.ensure_socket(port) { return; }
        if self.snap.local_id.is_empty() { return; }
        if self.nat_id != self.snap.local_id {
            self.nat = snapclone_egui::nat_traversal::Traversal::new(&self.snap.local_id);
            self.nat_id = self.snap.local_id.clone();
            self.last_stun_ms = None;
        }
        let now_ms = wall_ms();
        let network_changed = matches!(self.snap.dht_keepalive, Some((n, 0)) if n > 0)
            && self.last_stun_ms.is_some_and(|t| now_ms.saturating_sub(t) > 60_000);
        let stale = self.last_stun_ms.is_none_or(|t| now_ms.saturating_sub(t) > 15 * 60_000);
        let mut outs = Vec::new();
        if (stale || network_changed) && self.stun_resolve.is_none() {
            self.last_stun_ms = Some(now_ms);
            let net = &self.snap.settings.current.network;
            let extra: Vec<String> = net.stun_extra.split_whitespace().map(String::from).collect();
            let names: Vec<String> = snapclone_egui::stun_servers::addresses(net.stun_mask)
                .into_iter().chain(extra).collect();
            let slot = Arc::new(std::sync::Mutex::new(None));
            self.stun_resolve = Some(slot.clone());
            std::thread::spawn(move || {
                let servers: Vec<std::net::SocketAddr> = names.iter()
                    .filter_map(|a| std::net::ToSocketAddrs::to_socket_addrs(a.as_str()).ok()?.find(|s| s.is_ipv4()))
                    .collect();
                if let Ok(mut s) = slot.lock() { *s = Some(servers); }
            });
        }
        if let Some(slot) = self.stun_resolve.as_ref() {
            let ready = slot.lock().ok().and_then(|mut s| s.take());
            if let Some(servers) = ready {
                self.stun_resolve = None;
                outs.extend(self.nat.start_stun(servers, now_ms));
            }
        }
        if self.voice.live() {
            let targets: Vec<(String, snapclone_egui::punch_addrs::PeerAddr)> =
                self.snap.group_voice.participants.iter()
                    .filter(|p| !p.is_self)
                    .filter_map(|p| self.snap.punch.get(&p.id).map(|a| (p.id.clone(), a)))
                    .collect();
            for (id, a) in targets {
                let _ = self.nat.initiate(&id, std::net::SocketAddr::from((a.ip, a.port)), false, now_ms);
            }
        }
        let (due, events) = self.nat.tick(now_ms);
        outs.extend(due);
        if let Some(sock) = self.voice.sock.as_ref() {
            for o in outs {
                let _ = sock.send_raw(&o.bytes, o.to);
            }
        }
        for ev in events { self.on_nat_event(ev); }
    }

    fn pump_voice(&mut self) {
        let joined = self.snap.group_voice.joined;
        let frame_ms = self.snap.sink_latency.frame_ms;
        if joined && !self.voice.live() {
            let port = self.snap.settings.current.network.listen_port;
            for n in self.voice.start(port, frame_ms,
                                      &audio::default_record_cmd(),
                                      &audio::default_play_cmd()) {
                eprintln!("snapclone: {n}");
            }
            if self.voice.live() {
                let cmds = self.snap.apply(HostEvent::VoiceLink(true));
                self.handle(cmds);
                // The codec for THIS call's frame length: Opus when the
                // length is one Opus takes, raw otherwise -- and the host is
                // told which, so "transmitting without a codec" fires exactly
                // when it is so. (This used to say EncoderReady(true) while
                // sending raw S16LE: the warning could never fire.)
                self.voice.codec = snapclone_egui::voice_codec::VoiceCodec::new(
                    snapclone_egui::sink_cushion::SAMPLE_RATE, snapclone_egui::sink_cushion::frame_samples(frame_ms));
                self.voice.rx = snapclone_egui::voice_rx::VoiceRx::new(
                    snapclone_egui::sink_cushion::SAMPLE_RATE, snapclone_egui::sink_cushion::frame_samples(frame_ms));
                self.voice.rx_frame = std::time::Duration::from_millis(frame_ms.max(1) as u64);
                self.voice.rx_clock = std::time::Instant::now();
                let cmds = self.snap.apply(HostEvent::EncoderReady(self.voice.codec.encoder_ready()));
                self.handle(cmds);
            }
        } else if !joined && self.voice.live() {
            self.voice.stop();
            let cmds = self.snap.apply(HostEvent::VoiceLink(false));
            self.handle(cmds);
            // The call is over; the paths held open for it can close. A
            // snap in flight keeps its own (see SendSnap).
            let ids: Vec<String> = self.snap.group_voice.participants.iter()
                .filter(|p| !p.is_self).map(|p| p.id.clone()).collect();
            for id in ids { self.nat.stop(&id); }
        }
        self.pump_nat();
        if !self.voice.live() {
            // Not in a call, but the socket is up and STUN replies and
            // punches still arrive on it. Drain them.
            let empty = snapclone_egui::voice_wire::Peers::new();
            let refused = match self.voice.sock.as_mut() {
                Some(s) => s.poll(&empty, 16).1,
                None => Vec::new(),
            };
            self.feed_nat(refused);
            return;
        }

        // WHO IS IN THE CALL, and where. Rebuilt each frame from the app's
        // own participant list crossed with the punch addresses gossip
        // learned — replaced wholesale rather than patched, so someone who
        // left cannot linger as an authorised sender.
        let entries: Vec<(String, snapclone_egui::punch_addrs::PeerAddr)> =
            self.snap.group_voice.participants.iter()
                .filter(|p| !p.is_self)
                .filter_map(|p| self.snap.punch.get(&p.id).map(|a| (p.id.clone(), a)))
                .collect();
        self.voice.peers.set(entries);

        // MIC → the app's transmit gate → the wire. `voice_frame` is what
        // decides whether anything may leave; a rig that sent every mic
        // frame would transmit for the length of the call.
        for frame in self.voice.mic.take(4) {
            // The LEVEL first: the meter must move even while the gate is
            // closed, or a muted mic and a broken one look identical.
            // RMS, not peak — peak jumps to full scale on one click and
            // sits there, which makes a meter that is always full.
            let rms = if frame.is_empty() {
                0.0
            } else {
                (frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32).sqrt()
            };
            let cmds = self.snap.apply(HostEvent::MicActivity(rms));
            self.handle(cmds);

            // The verdict, paired with the frame it is about. Applying
            // `MicFrame` for a batch and then sending "some" frame would
            // desynchronise the sequence numbers from the audio.
            let cmds = self.snap.apply(HostEvent::MicFrame);
            // The sequence number and spurt flag voice_send assigned this frame:
            // an Opus packet carries them for the receiver's jitter scheduler.
            let send = cmds.iter().find_map(|c| match c {
                HostCommand::VoiceEmit(snapclone_egui::voice_send::Emit::Send { seq, talkspurt_start }) => Some((*seq, *talkspurt_start)),
                _ => None,
            });
            self.handle(cmds);
            if let Some((seq, spurt_start)) = send {
                let packet = self.voice.codec.encode(&frame, seq, spurt_start);
                let cmds = self.snap.apply(HostEvent::VoiceOutbound(packet));
                self.handle(cmds);
            }
        }
        if let Some(why) = self.voice.mic.take_error() {
            eprintln!("snapclone: microphone stopped: {why}");
        }

        // PLAY-OUT: once per frame interval, every speaker's next Opus frame in
        // sequence order -- decoded, or concealed if it never arrived (voice_rx).
        // At most 5 intervals per UI frame; further behind than that, resync
        // the clock rather than dump a burst of catch-up audio.
        if self.voice.live() {
            let mut ticks = 0;
            while self.voice.rx_clock.elapsed() >= self.voice.rx_frame && ticks < 5 {
                self.voice.rx_clock += self.voice.rx_frame;
                ticks += 1;
                for (peer, samples) in self.voice.rx.tick() {
                    let cmds = self.snap.apply(HostEvent::DecodedAudio { peer, samples });
                    self.handle(cmds);
                }
            }
            if ticks == 5 { self.voice.rx_clock = std::time::Instant::now(); }
        }

        // WIRE → the app. The sender's identity is the source address, which
        // is the transport's job to establish — see voice_wire's header on
        // why that is both required and currently weak.
        let (inbound, refused) = {
            let peers = &self.voice.peers;
            match self.voice.sock.as_mut() {
                Some(s) => s.poll(peers, 16),
                None => (Vec::new(), Vec::new()),
            }
        };
        for (sender, payload) in inbound {
            let cmds = self.snap.apply(HostEvent::VoiceReceived { sender, payload });
            self.handle(cmds);
        }
        let refused = self.feed_nat(refused);
        if !refused.is_empty() && !self.voice.warned_no_audio {
            self.voice.warned_no_audio = true;
            eprintln!("snapclone: voice datagrams refused — sender not a known call \
                       peer, or its punch address expired ({} so far)",
                      self.voice.sock.as_ref().map(|s| s.refused_unknown).unwrap_or(0));
        }

        // THE PLAY-OUT TICK, at the cushion's rate rather than the frame
        // rate: a 120 Hz display must not ask the play-out layer for audio
        // twice per frame.
        let due = self.voice.last_sink_tick.elapsed()
            >= std::time::Duration::from_millis(u64::from(self.snap.sink_tick_ms()));
        if due {
            self.voice.last_sink_tick = std::time::Instant::now();
            let depth = self.voice.speaker.as_ref().map(|s| s.depth()).unwrap_or(0);
            let cmds = self.snap.apply(HostEvent::SinkDepth(depth));
            self.handle(cmds);
        }
    }

    /// Deliver whatever the snap listener accepted.
    ///
    /// Attribution happens HERE rather than on the listener thread, because
    /// it needs the peer list and that thread must not borrow the app.
    /// Delete stored snaps older than the retention window (Settings >
    /// Storage > keep days / keep everything). Hourly: retention is measured in
    /// days, and a directory listing a minute is battery for nothing. The file
    /// names carry the capture time (`save_snap`), and only names of that form
    /// are ever candidates (`snap_expiry::due_for_purge`).
    fn sweep_snaps(&mut self, now_secs: i64) {
        const EVERY_SECS: i64 = 3600;
        if self.last_sweep.is_some_and(|t| now_secs - t < EVERY_SECS) {
            return;
        }
        self.last_sweep = Some(now_secs);
        let dir = config_kit::data_dir("snapclone").join("snaps");
        let Ok(entries) = std::fs::read_dir(&dir) else { return };
        let names: Vec<String> = entries.filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().into_string().ok()).collect();
        let due = snapclone_egui::snap_expiry::due_for_purge(
            names.iter().map(|n| n.as_str()), now_secs.max(0) as u64, &self.snap.retention());
        let mut removed = 0;
        for n in &due {
            match std::fs::remove_file(dir.join(n)) {
                Ok(()) => removed += 1,
                Err(e) => eprintln!("snapclone: could not delete expired snap {n}: {e}"),
            }
        }
        if removed > 0 {
            eprintln!("snapclone: deleted {removed} snap(s) past the retention window");
        }
    }

    /// Run the export the user asked for. This command reached the binary's
    /// catch-all ("unhandled host command") -- the Storage page's Export button
    /// logged a line and did nothing. `snap_export` had the plan, the argv and the
    /// outcome messages all along; this is the host that runs them.
    fn start_export(&mut self, dest_dir: String) {
        if self.exporting.as_ref().is_some_and(|h| !h.is_finished()) {
            return; // one at a time; a second tap while zip runs is not a second archive
        }
        let snaps = config_kit::data_dir("snapclone").join("snaps");
        let home = std::env::var("HOME").unwrap_or_default();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs()).unwrap_or(0);
        let slot = self.export_result.clone();
        self.exporting = Some(std::thread::spawn(move || {
            let on_path = |prog: &str| std::env::var_os("PATH").is_some_and(|p| {
                std::env::split_paths(&p).any(|d| d.join(prog).is_file())
            });
            let outcome = snapclone_egui::snap_export::run(
                &snaps, &dest_dir, &home, now, on_path("zip"), on_path("tar"));
            if let Ok(mut r) = slot.lock() { *r = Some(outcome); }
        }));
    }

    /// Hand a finished export's outcome to the app (it shows the message).
    fn pump_export(&mut self) {
        let done = self.export_result.lock().ok().and_then(|mut r| r.take());
        if let Some(o) = done {
            eprintln!("snapclone: export: {}", o.message());
            let cmds = self.snap.apply(HostEvent::ExportFinished(o));
            self.handle(cmds);
        }
    }

    fn pump_snaps(&mut self) {
        let Some(l) = self.snaps_in.as_ref() else { return };
        let arrivals = l.inbox.drain();
        if let Some(e) = l.inbox.take_error() {
            eprintln!("snapclone: snap listener: {e}");
        }
        if arrivals.is_empty() {
            return;
        }
        // Known addresses, from the same punch table gossip fills.
        let known: Vec<(String, snapclone_egui::punch_addrs::PeerAddr)> =
            self.snap.contact_book.iter()
                .filter_map(|c| self.snap.punch.get(&c.sc_id).map(|a| (c.sc_id.clone(), a)))
                .collect();

        let mut senders: Vec<String> = Vec::new();
        for a in arrivals {
            use snapclone_egui::snap_transport::Sender;
            match snapclone_egui::snap_transport::resolve_sender(a.from, &known) {
                Sender::Known(id) => {
                    if !senders.contains(&id) { senders.push(id.clone()); }
                    let cmds = self.snap.apply(HostEvent::SnapArrived {
                        from: id, frame: Box::new(a.frame) });
                    self.handle(cmds);
                }
                // Refused, not guessed — see snap_transport's header. Said
                // out loud because from the sender's side this looks like
                // delivery, and the two people involved need some way to
                // find out it did not happen.
                Sender::Ambiguous(ids) => eprintln!(
                    "snapclone: a snap from {} could be any of {ids:?} — refused, because \
                     an inbound TCP port cannot tell them apart", a.from),
                Sender::Unknown => eprintln!(
                    "snapclone: a snap from {} was refused — not a known contact, or their \
                     address has expired", a.from),
            }
        }
        if !senders.is_empty() {
            // The notification decision is the app's: it knows whether the
            // window is focused and whether it has already said something.
            let cmds = self.snap.apply(HostEvent::SnapsArrived {
                from: senders, focused: self.focused });
            self.handle(cmds);
        }
    }

    /// Collect what the send threads finished with.
    ///
    /// The row the sender watches must be able to reach BOTH terminal
    /// states. Before this only delivery existed as an outcome, so a send
    /// that broke looked identical to one still in flight and simply aged
    /// out — which reads as a timeout whatever actually happened.
    fn pump_sends(&mut self) {
        self.sending.retain(|t| !t.is_finished());
        let done = self.send_results.lock().map(|mut r| std::mem::take(&mut *r))
            .unwrap_or_default();
        if done.is_empty() { return; }
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64).unwrap_or(0);
        for (who, out) in done {
            match out {
                Ok(()) => {
                    // Handed to the socket, which is not the same as seen —
                    // there is no receipt on the wire yet. Marked delivered
                    // because "left this device" is the strongest claim this
                    // build can make, and saying nothing would leave the row
                    // pending forever.
                    let cmds = self.snap.apply(HostEvent::SnapDelivered {
                        id: who.clone(), now_ms });
                    self.handle(cmds);
                }
                Err(e) => {
                    eprintln!("snapclone: snap to {who} failed: {e}");
                    let cmds = self.snap.apply(HostEvent::SnapSendFailed {
                        id: who.clone(), now_ms });
                    self.handle(cmds);
                }
            }
        }
    }

    /// Carry out what the app asked the world to do.
    ///
    /// **This doc used to say the opposite**, and had for some time: that
    /// the handler was "deliberately a debug sink", that capture and
    /// sending needed a pipeline and a socket "this binary does not have
    /// yet", and that printing was a way to see requests reach the
    /// boundary. All of that was true when it was written and none of it
    /// is now — the shutter writes a file, contacts reach the disk, snaps
    /// go out over TCP, voice frames reach a socket.
    ///
    /// A stale doc is worse than a missing one: a reader looking for why
    /// their snap did not save would have found a paragraph telling them
    /// it never could. Corrected here rather than deleted, because the
    /// fact that it drifted is worth knowing — it drifted one arm at a
    /// time, each commit truthfully describing what it added and none of
    /// them looking up.
    ///
    /// What is still only reported: `IdentityUnavailable`, which has
    /// nowhere to go but a log until there is a UI for it, and
    /// `MarkViewed`/`MarkExpired`, which the store handles and this does
    /// not need to echo.
    ///
    /// Events a handler wants to send back are COLLECTED, not applied
    /// inline: `apply` drains, and calling it from inside a drain would
    /// run the drain again half way through the loop.
    fn handle(&mut self, cmds: Vec<HostCommand>) {
        let mut pending: Vec<HostEvent> = Vec::new();
        for c in cmds {
            match c {
                // THE SHUTTER PRODUCES A FILE. Not the composer flow yet —
                // that is its own thread of work — but a press that lands a
                // viewable image on disk is the difference between a camera
                // and a viewfinder. PPM because it is 12 bytes of header over
                // the RGBA we already hold: no encoder dependency, and every
                // image tool opens it. The path is printed so the photo is
                // findable the moment it exists.
                HostCommand::CaptureSnap => {
                    // The frame on screen: the slot if something newer just
                    // landed, else the one the pump was last given.
                    let live = self.rig.shared.peek_frame().or_else(|| self.last_live.clone());
                    let outcome = match live {
                        Some(f) if f.pixels_plausible() => {
                            // ONE ENCODE, used for both the file and the wire.
                            //
                            // The old code called `ppm_bytes` twice — once for
                            // `last_snap` and again inside `save_ppm` — and
                            // relied on the encode being deterministic to make
                            // its "byte for byte what the local file holds"
                            // claim true. It was true, and it was incidental.
                            // Encoding once and using the same buffer makes it
                            // structural: there is no second encode that could
                            // ever differ.
                            //
                            // JPEG rather than raw RGB. A 1080p snap goes from
                            // 6.2 MB on the wire to about 300 KB; see
                            // src/media_encode.rs.
                            match snapclone_egui::media_encode::encode_still(
                                &f, snapclone_egui::media_encode::DEFAULT_QUALITY)
                            {
                                Ok(enc) => {
                                    // The receipt learns the size the moment
                                    // it is known, not from the log.
                                    let cmds = self.snap.apply(HostEvent::SnapEncoded {
                                        bytes: enc.bytes.len() as u64 });
                                    self.handle(cmds);
                                    let ratio = snapclone_egui::media_encode::compression_ratio(
                                        &f, enc.bytes.len());
                                    eprintln!(
                                        "snapclone: snap {}x{} -> {} KB ({:.0}x smaller than raw)",
                                        f.width, f.height, enc.bytes.len() / 1024, ratio);
                                    let r = save_snap(&enc);
                                    self.last_snap = Some(enc.bytes);
                                    r
                                }
                                Err(e) => Err(e),
                            }
                        }
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
                // VOICE OUT. `voice_send` already envelopes one payload per
                // peer; this is the socket that was missing under it.
                HostCommand::SendVoice(per_peer) => {
                    if let Some(s) = self.voice.sock.as_ref() {
                        let unreachable = s.send_all(&self.voice.peers, &per_peer);
                        for id in unreachable {
                            // Worth a line each: a participant with no
                            // current address hears everyone and is heard by
                            // nobody, which presents as THEIR microphone
                            // being broken.
                            eprintln!("snapclone: {id} has no reachable address — \
                                       they will not hear this call");
                        }
                    }
                }
                // AUDIO OUT. A write failure ends the device, not the call:
                // the socket half still works and the other person can still
                // be heard by everyone else.
                // The other half of the codec seam. Raw S16LE in, samples
                // out — matching what this host sends. A payload whose
                // length is not a whole number of samples is refused rather
                // than played as a half-sample of noise.
                HostCommand::OpusArrived { peer, seq, spurt_start, data } => {
                    // Held until its play-out tick (see the rx tick in the
                    // voice pump): ordered, and concealed if it never comes.
                    self.voice.rx.arrive(&peer, seq, spurt_start, data);
                }
                HostCommand::DecodeAudio { peer, codec, pcm } => {
                    // Raw or Opus by the packet's kind, one decoder per
                    // speaker; a payload that is not a whole frame of its
                    // codec is refused (voice_codec::decode). The peer rides
                    // through decode unchanged.
                    if let Some(samples) = self.voice.codec.decode(&peer, codec, &pcm) {
                        pending.push(HostEvent::DecodedAudio { peer, samples });
                    }
                }
                HostCommand::PlayAudio(frames) => {
                    let failed = match self.voice.speaker.as_mut() {
                        Some(sp) => sp.play(&frames).err(),
                        None => None,
                    };
                    if let Some(e) = failed {
                        eprintln!("snapclone: speaker stopped: {e}");
                        self.voice.speaker = None;
                        if let Some(c) = self.voice.speaker_child.take() { c.stop(); c.reap(); }
                    }
                }
                // The transmit gate's verdict for this mic interval. Gated
                // is the normal case and says nothing; the other two are
                // states a silent call needs explained.
                HostCommand::VoiceEmit(emit) => {
                    if matches!(emit, snapclone_egui::voice_send::Emit::NoEncoder) {
                        eprintln!("snapclone: transmitting with no encoder — \
                                   nothing will leave this device");
                    }
                }
                // THE SEND. Off the frame loop, one thread per recipient: a
                // dial to a phone that is asleep costs the whole timeout,
                // and doing that inline would freeze the UI for twenty
                // seconds per person.
                HostCommand::SendSnap { to, snap } => {
                    // THE TIMER GOES ON THE WIRE. `duration_secs` was a literal
                    // 0 here — on this wire "never vanishes" — while the
                    // composer's timer defaulted to five seconds and was shown
                    // to the person as what they were sending. The composed
                    // snap now arrives with the command; a bare palette send
                    // (no composer behind it) uses the composer's default.
                    //
                    // The caption is NOT on the wire yet: `SnapFrame` has no
                    // text field and the Qt predecessor bakes the caption into
                    // the image (`renderComposed`) — rasterising text onto the
                    // JPEG is the next step, in media_encode. Said in the log
                    // so it is not mistaken for sent.
                    let (duration_secs, caption) = match &snap {
                        Some(c) => (c.duration.to_secs().min(u16::MAX as u32) as u16, c.caption.clone()),
                        None => (snapclone_egui::composer::Composer::default().duration.to_secs() as u16, String::new()),
                    };
                    if !caption.is_empty() {
                        eprintln!("snapclone: caption {caption:?} not on the wire yet (no text field in SnapFrame; bake into the image is the next step)");
                    }
                    // Open the path first: on a port-preserving NAT the TCP
                    // SYN that follows rides the UDP mapping the volley
                    // makes (the C++ comment on why TCP works afterwards).
                    for who in &to {
                        if let Some(a) = self.snap.punch.get(who) {
                            let _ = self.nat.initiate(who, std::net::SocketAddr::from((a.ip, a.port)), false, wall_ms());
                        }
                    }
                    let Some(bytes) = self.last_snap.clone() else {
                        eprintln!("snapclone: nothing to send — take a photo first");
                        continue;
                    };
                    let me = self.snap.local_id.clone();
                    let name = self.snap.settings.current.general.display_name.clone();
                    let now_s = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64).unwrap_or(0);
                    for who in to {
                        // AN ADDRESS KNOWN WRONG IS NOT TRIED. Five
                        // failures or an hour stale and the cached one is
                        // skipped, so the person waits for a refusal
                        // instead of a timeout — and the peer falls to
                        // whatever finds it next, gossip or the DHT.
                        if !self.snap.addr_health.usable(&who, now_s * 1000) {
                            if let Ok(mut r) = self.send_results.lock() {
                                r.push((who.clone(),
                                        Err("last known address keeps failing".into())));
                            }
                            continue;
                        }
                        let Some(addr) = self.snap.punch.get(&who) else {
                            if let Ok(mut r) = self.send_results.lock() {
                                r.push((who.clone(), Err("no known address".into())));
                            }
                            continue;
                        };
                        // REAL sequence and ack, from this peer's session.
                        // Both were hardcoded to 0 here while `next_seq`
                        // and `take_ack` sat tested and uncalled — the
                        // fields were on the wire carrying nothing.
                        let (seq, ack) = self.snap.frame_ids(&who);
                        let frame = snapclone_egui::snap_wire::SnapFrame::announce(
                            seq, ack, &me, &name, snapclone_egui::snaps::Media::Image,
                            now_s, duration_secs, bytes.clone());
                        let results = self.send_results.clone();
                        self.sending.push(std::thread::spawn(move || {
                            let out = snapclone_egui::snap_transport::send_snap(addr, &frame);
                            if let Ok(mut r) = results.lock() { r.push((who, out)); }
                        }));
                    }
                }
                // REAL, not a debug sink: a contact scanned or renamed
                // reaches the disk, so it is still there next launch. A
                // write failure is REPORTED rather than swallowed — a
                // silent one means the person adds someone twice and never
                // learns why.
                //
                // (This paragraph had drifted onto the CaptureSnap arm,
                // describing a handler thirty lines away. Moving code
                // leaves its explanation behind unless somebody checks,
                // which is the second time this session.)
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
                // The store exists now. These two are no longer the
                // odd ones out: the app records the mark itself and emits
                // `SaveSnapState`, so all this arm owes is silence — a log
                // line per viewed snap would be noise about the normal case.
                //
                // They stay as distinct commands rather than being folded
                // into the save, because a future host has different work
                // for each: a read receipt to send for Viewed, a file to
                // delete for Expired.
                HostCommand::MarkViewed(_) | HostCommand::MarkExpired(_) => {}
                HostCommand::SaveCodeSvg(svg) => {
                    // Next to the snaps, named by the id, so it is where the
                    // export already looks; the note says the path.
                    let dir = config_kit::data_dir("snapclone").join("snaps");
                    let _ = std::fs::create_dir_all(&dir);
                    let path = dir.join(format!("{}.svg", self.snap.local_id));
                    let r = std::fs::write(&path, svg)
                        .map(|_| path.display().to_string())
                        .map_err(|e| format!("could not write {}: {e}", path.display()));
                    if let Ok(p) = &r { eprintln!("snapclone: my code saved as SVG: {p}"); }
                    let cmds = self.snap.apply(HostEvent::CodeSvgSaved(r));
                    self.handle(cmds);
                }
                HostCommand::SaveSnapState(st) => {
                    if let Err(e) = snapclone_egui::snap_state::store(&st) {
                        // Loud, because the failure mode is a snap coming
                        // back after a restart and nobody knowing why.
                        eprintln!("snapclone: could not save snap state: {e} — \
                                   viewed snaps may reappear after a restart");
                    }
                }
                HostCommand::ExportSnaps { dest_dir } => self.start_export(dest_dir),
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

/// Write an already-encoded snap under the app's data dir, named by wall clock
/// so a burst of presses cannot overwrite each other within a second (the
/// millisecond field sees to that).
///
/// Takes the ENCODED bytes rather than the frame, which is the change that
/// makes the disk file and the sent payload the same object rather than two
/// results of the same function. The extension comes from the encoder, so a
/// format change moves the filename with it and cannot leave `.ppm` on a JPEG.
fn save_snap(enc: &snapclone_egui::media_encode::Encoded) -> Result<String, String> {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis()).unwrap_or(0);
    let dir = config_kit::data_dir("snapclone").join("snaps");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(format!("snap-{ms}.{}", enc.ext));
    std::fs::write(&path, &enc.bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path.display().to_string())
}

impl eframe::App for App {
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.ime_repair.repair(&mut raw.events);
    }
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // SNAPCLONE_INPUT_LOG=1: each frame's keys / text / IME / clicks /
        // window focus, and focus changes, on stderr (input_log). For the
        // one-character fields no harness reproduces.
        let log_input = std::env::var_os("SNAPCLONE_INPUT_LOG").is_some();
        let (log_events, focus_before) = if log_input {
            (ctx.input(|i| i.events.clone()), ctx.memory(|m| m.focused()))
        } else {
            (Vec::new(), None)
        };
        // The widget visuals follow this app's palette choice too, not only the custom
        // painting: `apply_settings` themes egui from `prefs.palette`.
        let mut prefs = settings_kit::Prefs::default();
        prefs.palette = self.snap.active_palette().to_string();
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
        self.pump_glove(ctx);
        self.pump_gossip(now_secs);
        self.pump_capture();
        self.pump_voice();
        self.pump_snaps();
        self.pump_sends();
        self.sweep_snaps(now_secs);
        self.pump_export();

        // Discovery is time-driven, not input-driven, so the frame has to
        // come round on its own or an idle window never polls the socket —
        // and a live viewfinder needs the frame loop at frame rate, or the
        // preview updates only when the mouse moves.
        //
        // A live CALL needs it at the audio frame rate: `pump_voice` takes
        // at most four mic frames and sixteen datagrams per update, which
        // at 500 ms wakeups is a fifth of what a 20 ms codec produces —
        // the mic queue backs up without bound and the far end hears
        // half-second bursts. Only the viewfinder used to raise the rate,
        // so a call with the camera off stuttered and one with it on did
        // not, which is the kind of correlation that sends people looking
        // at the wrong subsystem.
        let interval_ms: u64 = if self.voice.live() {
            u64::from(self.snap.sink_latency.frame_ms).clamp(10, 33)
        } else if self.rig.running() {
            33
        } else {
            500
        };
        ctx.request_repaint_after(std::time::Duration::from_millis(interval_ms));

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

        if log_input {
            if let Some(l) = snapclone_egui::input_log::line(&log_events, focus_before, ctx.memory(|m| m.focused())) {
                eprintln!("input: {l}");
            }
        }
    }
}

/// Must match `packaging/org.snapclone.SnapClone.desktop` exactly — no
/// extension, same case. A mismatch fails SILENTLY: the app runs perfectly and
/// has no icon, no dock grouping and the wrong name, and nobody connects those
/// symptoms to a string.
const APP_ID: &str = "org.snapclone.SnapClone";

fn main() -> eframe::Result<()> {
    // WHY THE WINDOW WILL NOT OPEN, said before the panic in a dependency does.
    //
    // A build that cannot reach a display server dies inside `wayland-sys`:
    // `wayland_client_handle()` tries libwayland-client.so.0, then .so, and `.expect()`s the
    // result. What reaches the user is a panic naming a line in a crate they have never
    // heard of — on a phone, in a terminal they cannot copy out of — and the message points
    // the wrong way, because the usual cause is that the library IS installed and this
    // binary cannot dlopen anything, having been linked statically.
    if let Some(why) = uikit_egui::backend_check::preflight() {
        eprintln!("{why}");
    }
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

fn wall_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
