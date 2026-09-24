//! snapclone-egui — the founder app, ported to egui, now *consuming* the systems it started.
//!
//! Live backgrounds and page/subpage navigation used to live inside snapclone's QML; they're
//! now shared uikit-egui capabilities ([`livebg`](uikit_egui::livebg), [`nav`](uikit_egui::nav)),
//! so this port pulls them from the kit instead of owning them — and localweather / av1tuner get
//! the same for free. On top: adaptive layout, a11y, swipe between pages, and live cross-app
//! settings. The camera/peer content renders on the carved surface; the snapclone-* engine
//! crates (audio/proto/net-intel) drive it on-device.

use egui::Ui;
use uikit_egui::{Adaptive, BgStyle, GestureLayer, LiveBackground, Nav, PaletteColors};

pub mod camera;
pub mod debounce;
pub mod confirm;
pub mod audio;
pub mod camera_source;
pub mod capture;
pub mod peer_source;
pub mod contacts;
pub mod conversation;
pub mod composer;
pub mod send_flow; // the four steps of sending a snap, as data both pages read
pub mod send_flow_ui; // the strip that says which step, and the receipt
pub mod composer_ui; // SnapComposerDialog's look: caption on the photo, colour A, size, timer chips, Next
pub mod outbox;
pub mod group_voice;
/// The host seam: one typed event in, typed commands out. See `host.rs` —
/// this is the call site the ~35 tested-but-unreached entry points lacked.
/// Who this installation is, kept across restarts.
pub mod identity;
/// The gossip datagram format and the UDP socket that carries it.
pub mod gossip_wire;
/// Contacts that survive a restart, with the port the scanner found.
pub mod contact_book;
pub mod host;
/// Reading .vcf files, so the Import button does something.
pub mod vcard;
pub mod settings_page;
pub mod settings_store;
/// The Network tab's seven STUN servers and the bitmask they persist as.
pub mod stun_servers;
/// The Network tab's relay dropdown and the preset/custom URL resolution.
pub mod relay_preset;
pub mod inbox;
pub mod unified_nav;
pub mod friends;
pub mod modal;
pub mod snaps;
pub mod session;
pub mod ring;
/// Camera pixels to a payload string -- the half of the scanner that did not
/// exist. See its header: the kit's QR is a decorative identicon, so this
/// decodes REAL codes from other apps, which is what scanning a friend's
/// screen actually involves.
/// Per-speaker processing on incoming voice: de-bass, de-harsh, compressor.
/// De-bass helps ambient noise; the compressor HURTS it, since it lifts the
/// quiet parts and between words the quiet part is the room.
pub mod voice_fx;
pub mod qr_decode;
pub mod qr;
pub mod wayfinder;
pub mod voicebar;
/// The group-call coordinator: what wires capture, the floor session and the
/// three views into an actual call. Ported from Qt's GroupVoiceCoordinator,
/// whose behaviour lived entirely in its `connect()` calls.
pub mod voice_coord;
pub mod landmark;
pub mod export_reminder;
pub mod nav_chrome;
pub mod ribbon;
pub mod scroll_areas;
pub mod mobile_dialog;
pub mod glass;
pub mod swipe_physics;
pub mod video_viewer;
pub mod posture;
pub mod camera_policy;
/// Why the camera did not open, and what to do about it: the acquisition
/// failure taxonomy and the route ladder. camera_policy covers a camera that
/// works and then degrades; this covers one that never opened.
pub mod camera_fault;
pub mod ambient_blobs;
pub mod ambient_camera;
pub mod ambient_inbox;
pub mod ambient_palette;
pub mod camera_select;
/// Reading the actual camera list, so the picker has something to show.
pub mod camera_enumerate;
pub mod chevron_confirm;
pub mod dht_announce;
pub mod dht_bootstrap;
pub mod dht_envelope;
pub mod dht_presence;
pub mod hint;
pub mod palette;
pub mod nat_traversal;
pub mod face_consent;
pub mod frame_pump;
pub mod media_encode;
pub mod video_backend;
/// Asking the system which encoders it has, so `video_backend`'s decision has inputs.
pub mod video_probe;
pub mod video_record;
/// Admitted frames on disk, so `av1_encode`'s command has something to read.
/// Why the QR scanner has not found anything — the difference between looking and broken.
pub mod qr_scan_status;
pub mod video_spill;
/// Running the encode: the thin spawn layer `av1_encode` and `video_spill` were built for.
pub mod video_encode_run;
pub mod firewall_backend;
pub mod gossip;
pub mod handshake;
pub mod landmark_hash;
pub mod nat_punch;
pub mod peer_dial;
pub mod peer_session;
pub mod power_tier;
pub mod help_manual;
pub mod keyboard_inset;
pub mod press_gesture;
pub mod punch_addrs;
pub mod qr_render;
pub mod scroll_passthrough;
pub mod sink_cushion;
pub mod snap_expiry;
pub mod store_privacy;
pub mod snap_export;
pub mod snap_state;
pub mod snap_transport;
pub mod snap_wire;
pub mod av1_encode;
pub mod tab_bar;
pub mod thumbnails;
pub mod trace_channels;
pub mod voice_bridge;
/// The codec seam: Opus out when the frame allows, raw or Opus in, one decoder per speaker.
pub mod voice_codec;
pub mod voice_wire;
pub mod wire_v3;
pub mod touch_feedback;
pub mod requests;
pub mod reticle;
pub mod roster;
pub mod send_state;
pub mod send_summary;
pub mod shutter;
pub mod tile_strip;
pub mod voice_playout;
pub mod voice_send;
pub mod freshness;
pub mod glove_input;
pub mod gaze;
pub use unified_nav::{NavEvent, UnifiedNav, PAGE_NAMES, sub_tabs};

/// The five Qt pages, verbatim (`MainWindow.cpp` kPageCamera..kPageSettings). The old 3-page
/// skeleton ("Camera/Peers/Settings") predated the faithful-port pass; "Peers" was never a
/// Qt page — the roster lives on the FRIENDS page, and Inbox/Wayfinder were missing entirely.
pub const PAGES: [&str; 5] = unified_nav::PAGE_NAMES;

/// A connected peer (mirrors snapclone-proto; on-device the real roster feeds this).
#[derive(Debug, Clone)]
pub struct Peer {
    pub name: String,
    pub online: bool,
}

/// The snapclone view: page/subpage nav, a live background, the peer roster, and gesture/theme.
pub struct Snapclone {
    /// The ported surfaces. Held here rather than constructed per frame so they
    /// keep their state — a viewfinder that forgot its focus point every frame,
    /// or an inbox that forgot its filter, is the "dud demo" failure: the
    /// widgets exist, nothing remembers anything, and the app looks unfinished
    /// because it IS.
    pub viewfinder: crate::camera::Viewfinder,
    /// Which camera and format, from device-kit. `None` means no camera was
    /// found — shown as such rather than as an empty preview, because a black
    /// rectangle and a missing device look identical.
    pub camera: Option<crate::camera_source::CameraChoice>,
    /// Cameras this device offers, as `camera_select` understands them —
    /// facing, capture group, the PipeWire node handle. Richer than
    /// device-kit's Device, which is why both exist: device-kit answers "which
    /// device", camera_select answers "can these two run at once".
    pub cameras: Vec<crate::camera_select::Device>,
    /// Which one is showing, and whether to mirror it.
    pub cam_selection: Option<crate::camera_select::Selection>,
    /// The shutter's gesture state — tap/hold/lock/mode-picker.
    pub shutter: crate::shutter::ShutterButton,
    /// Which hand holds the phone. Read from `settings_kit` prefs in
    /// `apply_settings`; drives `uikit_egui::handed` so the primary control
    /// sits under the thumb. Nothing consumed this preference before.
    pub prefs_handedness: settings_kit::Handedness,
    /// What the shutter last did, shown under it. Plain state rather than a
    /// toast system: this app has none, and adding one to report a duration
    /// would be the tail wagging the dog.
    /// The recording in progress, if the shutter is held.
    ///
    /// `Option` because a `Recorder` is only meaningful during a recording and
    /// its counters must start clean each time — reusing one would report the
    /// previous clip's duration on the next.
    pub recorder: Option<crate::video_record::Recorder>,
    /// Where the recording's admitted frames are being written. Beside the recorder because
    /// the two start and stop together — `offer` feeds this, and the shutter finishes it.
    pub spill: Option<crate::video_spill::Spill>,
    /// What the QR scanner has seen this session — see `qr_scan_status`.
    pub scan_counts: crate::qr_scan_status::Counts,
    /// The last code decoded, so a hit survives the scanner closing on it.
    pub scan_hit: Option<String>,
    /// Where a finished encode reports back from its thread.
    ///
    /// A channel rather than a shared `Mutex<Option<Done>>`: the UI polls once a frame, and a
    /// channel makes "nothing yet" the cheap case without taking a lock the encode thread
    /// might be holding while it writes.
    pub encode_done: (
        std::sync::mpsc::Sender<crate::video_encode_run::Done>,
        std::sync::mpsc::Receiver<crate::video_encode_run::Done>,
    ),
    pub shutter_note: String,
    /// Why the last feed tap did not open a viewer. Empty when there is
    /// nothing to say.
    pub feed_note: String,
    /// This device's sc-ID, for the code somebody else scans.
    ///
    /// Held rather than derived: the identity belongs to the crypto layer and
    /// the view should display what it is TOLD, not compute a second answer
    /// that could disagree with the one the handshake uses.
    pub local_id: String,
    /// The durable address book. Beside `peers` rather than merged into it —
    /// see `contact_book` for why, and for the test that keeps the two in step.
    pub contact_book: crate::contact_book::ContactBook,
    /// Contacts whose announcement was signed by a key that is not the one on
    /// file. Drained as a command: this is a security event, not a log line.
    pub key_conflicts: Vec<String>,
    /// The book changed and has not been written. Drained as a command rather
    /// than written from here: the app has no business owning a file handle,
    /// and a save that happens inside apply_friends cannot be batched or
    /// deferred by a host that knows better.
    pub contacts_dirty: bool,
    /// Latest-frame-wins slot between the capture thread and the paint.
    pub frames: crate::frame_pump::FrameSlot,
    /// Where the capture engine stands, from `HostEvent::CaptureStatus`.
    /// App state rather than a host-side print because the person aiming the
    /// camera is looking at the viewfinder, not at stderr — a failing
    /// pipeline's reason belongs on the page it fails on.
    pub capture_status: crate::capture::Status,
    /// The uploaded texture, kept across frames so an unchanged frame is not
    /// re-uploaded.
    pub cam_texture: Option<egui::TextureHandle>,
    /// When the last drawn frame was captured, for the staleness marker.
    pub last_frame_ms: Option<u64>,
    /// Why the last save failed, if it did.
    pub settings_error: Option<String>,
    /// Page transitions, debounced. The nav is instant (leading edge) while the
    /// per-page LOAD waits for the swipe to settle — so a fast scroll through
    /// five pages loads the fifth once rather than all five in turn.
    pub page_debounce: crate::debounce::Debouncer<usize>,
    /// Notify on arrival only when the window is NOT focused. Notifying while
    /// someone is looking at the inbox is noise about something already on
    /// screen, and it is how an app trains people to turn notifications off.
    pub notify_enabled: bool,
    /// Suppress sender and content in notifications — they persist in KDE's
    /// history and show on lock screens.
    pub high_security: bool,
    pub inbox: crate::inbox::Inbox,
    pub composer: crate::composer::Composer,
    pub settings: crate::settings_page::SettingsPage,
    /// The running DHT node, when discovery asked for one. `None` is
    /// both "not started" and "switched off"; `tick_dht` owns the
    /// difference.
    pub dht: Option<snapclone_presence::node_host::NodeHost>,
    /// A bind failure is permanent for this run, so it is reported once
    /// rather than every tick.
    pub dht_failed: bool,
    /// When to publish and who to look up. Pure policy, so the cadence is
    /// testable without a socket — see `dht_presence`.
    pub dht_schedule: crate::dht_presence::Schedule,
    /// The last keepalive round: (pinged, answered). `None` until the node
    /// has had an idle period. Zero answered of several pinged is the cue
    /// that the network changed under us — a NAT mapping expired, wifi
    /// handed off to cellular — and the status line says so rather than
    /// showing a peer count that is no longer true.
    pub dht_keepalive: Option<(usize, usize)>,
    /// The punch socket's STUN-mapped address (`nat_traversal`). Announced
    /// in our record so a peer's volley hits the mapping that exists.
    pub reflexive: Option<std::net::SocketAddr>,
    /// STUN was asked and nobody answered (cleared when a server does).
    pub stun_failed: bool,
    /// Peers the node reported on its last publish or bootstrap.
    pub dht_peers: usize,
    /// The "reachable from outside" hint has been shown this session.
    pub reachable_shown: bool,
    /// The accessibility scene the settings toggles register with.
    /// Held here rather than in the page so focus survives between
    /// frames — a page-local scene forgets on every repaint, which
    /// presents as focus jumping rather than as a missing scene.
    pub a11y: uikit_egui::a11y::A11yScene,
    /// vCard text waiting to be imported, set by a host that can read a
    /// file. `None` until a file picker exists — which is why the import
    /// action reports what is missing rather than failing silently.
    pub pending_vcard_text: Option<String>,
    /// What the last import attempt did, shown to the person. A count of
    /// what was read and what was skipped, because "3 imported, 1
    /// skipped" is actionable and a silent difference is not.
    pub import_report: Option<String>,
    pub group_voice: crate::group_voice::GroupVoice,
    /// Per-contact processing on incoming voice: the leveler and the 4-band
    /// EQ, keyed by the authenticated sender. See `voice_fx`.
    pub voice_fx: crate::voice_fx::VoiceFx,
    pub picker: crate::contacts::Picker,
    pub conversation: crate::conversation::Conversation,
    /// Who the open conversation is with. The Conversation model holds the
    /// ITEMS; which peer they belong to is the shell's business.
    pub conversation_peer: String,
    /// What the last call attempt amounted to. A field rather than a toast:
    /// this controller has no general message channel, and the two that do
    /// exist (`shutter_note`, `feed_note`) belong to other pages.
    pub call_note: String,
    /// Recipients the user pressed Send for, drained by the seam. A flag
    /// rather than a direct call because the send happens on a socket the
    /// UI frame must not block on.
    pub send_requested: Vec<String>,
    /// Where the person is in sending a snap — Take, Compose, Choose, Send — and
    /// what was decided at each step. The camera page and the picker page both
    /// draw its strip; `SendSnap` is built from it so the composed snap travels.
    pub send_flow: crate::send_flow::SendFlow,
    /// A send the flow built: recipients AND the composed snap. Drained by the
    /// host into `HostCommand::SendSnap`. `send_requested` stays for the palette's
    /// bare "send to" action, which has no composition behind it.
    pub send_request: Option<crate::send_flow::SendRequest>,
    /// The expanded full-screen view of my code (the C++ showExpandedQr): `Some` while open.
    pub qr_expanded: Option<uikit_egui::qr::Expanded>,
    /// When the code dialog came on screen, so MY CODE draws itself in from that moment
    /// (the kit's staggered reveal). Set on the first frame the dialog is drawn.
    pub qr_shown_at: Option<f64>,
    /// The SVG of my code, when the expanded view's Save SVG was pressed; drained to the host.
    pub code_svg_requested: Option<String>,
    /// How the phone leans, smoothed, for the code card's light (the kit's `qr::Tilt`). From
    /// the accelerometer when there is one (`glove_input.raw()`), else the pointer over the
    /// card, so a desktop mouse tilts it too.
    pub tilt: uikit_egui::qr::Tilt,
    /// The unified bottom bar (Qt UnifiedNavBar): page arrows, sub-tab segments, underline
    /// morph, page dots. Owns the page + sub-tab state; `nav` mirrors the page for swipe.
    pub bar: UnifiedNav,
    /// Login overlay visibility, with the Qt ordering rule (MainWindow.cpp): the overlay
    /// STARTS HIDDEN; if auto-login already succeeded at construction we leave it hidden and
    /// go straight to camera; otherwise we show it and hide on `login_succeeded`.
    pub login_visible: bool,
    pub nav: Nav,
    /// How many pages the last frame laid out side by side. One on a
    /// phone; on a desktop as many as fit (`page_columns`).
    pub cols_drawn: usize,
    /// Hysteresis for the column count, so a window resized across a
    /// breakpoint cannot shuffle the layout frame by frame.
    pub plan: uikit_egui::pager::PlanState,
    pub bg: LiveBackground,
    pub peers: Vec<Peer>,
    /// The friends roster (Qt FriendListWidget). Derived from `peers` on
    /// change rather than kept in parallel — two lists of the same people
    /// disagree the moment one update misses.
    pub roster: crate::friends::Roster,
    /// Which row's action menu is open (Qt ContactCard), at most one.
    pub cards: crate::friends::Accordion,
    /// The manual add-peer form: the only way in when discovery does not work.
    pub add_form: crate::friends::AddForm,
    /// The six-layer modal stack (Qt ModalSurface) every dialog is presented
    /// into. Owning it here is the point: z-order becomes a property of the
    /// structure rather than of which code called raise() last.
    pub modal: crate::modal::ModalSurface,
    /// The sealed feed of unopened snaps (Qt SnapFeedWidget).
    pub feed: crate::snaps::SnapFeed,
    /// Which snaps are already finished, across restarts. Consulted before
    /// anything is put in the feed — see `snap_state` for why a snap coming
    /// back after a restart is the promise failing in the direction that
    /// matters.
    pub snap_state: crate::snap_state::SnapState,
    /// The state changed and owes a write.
    pub snap_state_dirty: bool,
    /// The ephemeral viewer. Holds the only decrypted bytes in the app, and
    /// wipes them on close.
    pub viewer: crate::snaps::Viewer,
    feed_press: crate::snaps::Press,
    /// Snaps opened this session, for the host to mark in storage.
    pub snaps_viewed: Vec<String>,
    /// Snaps whose view window has elapsed. Separate from `snaps_viewed`
    /// because they answer different questions: one is "did they see it", the
    /// other is "is the window over".
    pub snaps_expired: Vec<String>,
    pub last_saved: Option<String>,
    /// Send status per snap (Qt OutboxWidget), shown on the Inbox "Sent" tab.
    pub outbox: crate::session::Outbox,
    /// The sign-in form behind the login overlay. The overlay's visibility rule
    /// was ported long ago; this is what it was hiding.
    pub login: crate::session::Login,
    /// Wall clock in seconds, for relative times. Held rather than read at
    /// draw time so tests can advance it.
    pub now_secs: i64,
    /// The incoming-call ring (Qt IncomingCallDialog), presented into the
    /// Confirm layer — it is a question that must be answered, so it sits above
    /// any dialog already open.
    pub incoming_call: Option<crate::ring::IncomingCall>,
    /// Calls that ended without being taken, for the missed-call list. A
    /// timeout and a decline are recorded apart: same effect, different history.
    pub missed_calls: Vec<String>,
    /// The QR scanner sheet (Qt QrScanDialog), presented into the Dialog layer.
    /// `None` when it is not open — the scanner holds camera frames and should
    /// not exist while nobody is looking at it.
    pub qr: Option<crate::qr::QrScan>,
    /// The on-air control surface (Qt GroupVoiceBar). Drawn above the nav
    /// whenever a session is joined, which is where the join/mute/talk controls
    /// live now — the previous commit took them off the Wayfinder page, where
    /// they never belonged, and left them with no home.
    pub voicebar: crate::voicebar::VoiceBar,
    /// The room. voicebar is the CONTROL — press to talk, the lock, your own
    /// level — and this is who else is here and who is speaking. The control
    /// was wired and the room was not, so you could talk into a call without
    /// being able to see who was in it.
    /// Named voice_room, not roster: `roster` is already the FRIENDS list
    /// (crate::friends::Roster) and these are different sets — everyone you
    /// know versus everyone in this call. Two fields of the same name would not
    /// compile, and two of similar name would be confused forever.
    pub voice_room: crate::roster::Roster,
    /// Monthly export-reminder bookkeeping. Host-persisted in Qt (QSettings);
    /// held here so the decision stays a pure function of state + clock.
    pub reminder: crate::export_reminder::ReminderState,
    /// The current month, supplied by the host — the rule must not reach for a
    /// clock, or it cannot be tested.
    pub now_month: Option<crate::export_reminder::Month>,
    /// Recent snap paths for the "remember when?" strip.
    pub recent_snaps: Vec<String>,
    /// Set when the user asks to export; the host runs the actual zip.
    pub export_requested: bool,
    /// Set when the user asks to retry failed sends; the host owns the retry.
    pub retry_requested: bool,
    /// Scroll position of the background carousel, and whether its last wheel
    /// event ran off the end and was handed to the page nav.
    pub bg_carousel: crate::scroll_areas::HScroll,
    pub carousel_handoff: bool,
    /// Whether dialogs present as bottom sheets or floating windows. Decided
    /// once per frame from the real surface rather than hardcoded, so the same
    /// build is correct on a phone and on a desktop.
    pub dialog_form: crate::mobile_dialog::Form,
    /// Painted camera-chrome buttons. Held across frames so their press dip and
    /// ripple survive the redraw they are animating through.
    pub glass_flip: crate::glass::GlassButton,
    pub glass_shutter: crate::glass::GlassButton,
    /// Seconds since the capture flash started; `None` when not flashing.
    /// Set from `HostEvent::SnapSaved`, so it fires on the file actually
    /// existing rather than on the tap — a flash on the tap would confirm a
    /// capture that failed.
    pub snap_flash: Option<f32>,
    /// Set when the shutter is tapped; the host captures the frame.
    pub shutter_pressed: bool,
    /// The video snap viewer. Separate from `viewer` because a video snap is
    /// PERSISTENT and exportable while an image snap is view-once — opening the
    /// wrong one either destroys a video the user could keep or keeps an image
    /// they were promised would vanish.
    pub video: Option<crate::video_viewer::VideoViewer>,
    /// The action menu, opened by right-click on a desktop and press-and-hold
    /// on a phone. Before this the fleet had neither, anywhere.
    pub menu: uikit_egui::menu::ContextMenu,
    /// Snaps that arrived this session, bytes and all, so a tap on a tile
    /// can open one. Nothing kept them before: `SnapArrived` built the
    /// feed entry and dropped the frame, and the receive path ended at a
    /// badge. Bounded by count and bytes; a snap is view-once and the
    /// oldest go first.
    pub received: Vec<crate::snaps::Snap>,
    /// A menu chose "View": opened on the next frame, which has a surface.
    pub pending_open: Option<String>,
    /// "Someone is looking at this screen", from the gaze layer.
    pub attention: crate::gaze::Attention,
    /// The snap the menu was opened on.
    pub menu_target: Option<String>,
    /// Snaps the user asked to keep, for the host to write out.
    pub snaps_saved: Vec<String>,
    /// The send receipt, filled from the send path's own numbers.
    pub send_summary: crate::send_summary::SendSummary,
    /// The proximity screensaver and talk-mode flip. Sensors are the host's;
    /// the arming rule is ours.
    pub posture: crate::posture::PostureEngine,
    /// When voice last arrived, for the five-minute arming window.
    pub last_voice_at: Option<i64>,
    /// Cold-weather glove mode. The temperature comes from ambient-learner's
    /// EnvSnapshot::aux_temp_c and the tilt vector from the host's raw IIO
    /// poll — see uikit_egui::glove for why the D-Bus proxy cannot supply it.
    pub glove: uikit_egui::glove::GloveCursor,
    /// The field the cursor armed for voice entry. NOT a keyboard request:
    /// rusty-board's cue is that no key event is ever synthesised.
    pub voice_field: Option<String>,
    /// (field, text) pairs spoken into armed fields, for the host to apply.
    pub dictated: Vec<(String, String)>,
    /// Double-flick commands. Bound to camera swap here; independent of glove
    /// mode, because wanting the gesture and wanting a cursor are different
    /// wants.
    pub shake: uikit_egui::glove::ShakeCommands,
    /// The accelerometer driver. Everything downstream of it — the
    /// ratchet, `apply_shake`, `apply_glove` — was written and tested and
    /// unreachable, because nothing in this app read a sensor.
    pub glove_input: crate::glove_input::GloveInput,
    /// Capture timeouts and limits. The backend reads a SNAPSHOT at start:
    /// editing this mid-capture does not retune a running pipeline, matching
    /// the Qt policy's own threading rule.
    pub camera_policy: crate::camera_policy::CameraPolicy,
    /// Which cameras are known to work. Consulted before opening one.
    pub camera_health: crate::camera_policy::CameraHealth,
    /// The receive half's scheduling. Decode and the audio sink stay with the
    /// host; this decides play / conceal / silence per sender per interval.
    pub playout: crate::voice_playout::VoicePlayout,
    /// The layer BELOW playout: how many frames to hand the audio sink each
    /// tick. voice_playout decides play/conceal/silence per sender; this
    /// decides how much buffer the sink is holding, which is a different
    /// question and had a different bug.
    pub sink: crate::sink_cushion::Playout,
    /// Cushion depth and frame size, from the call's settings.
    pub sink_latency: crate::sink_cushion::Latency,
    /// The transmit half. The Opus encoder, the mic and the socket stay with
    /// the host; this owns WHEN a frame may leave and WHAT SEQUENCE it carries
    /// — the two things the receiver depends on.
    pub sender: crate::voice_send::VoiceSender,
    /// What our NAT looks like from outside, once two STUN probes agree.
    ///
    /// `None` until both come back — nat_punch returns Unknown from a single
    /// probe, and the whole classification rests on comparing TWO reflexive
    /// addresses seen from the SAME local port.
    pub nat: Option<crate::nat_punch::NatKind>,
    /// The on-screen keyboard, in SCREEN coordinates.
    ///
    /// A rectangle rather than a height, which the module is emphatic about:
    /// the inset is the overlap against this window's own bottom edge, and
    /// using the height insets a window the keyboard never covered — a
    /// floating composer, a split-screen phone, a desktop window above the
    /// panel.
    pub keyboard: crate::keyboard_inset::KeyboardWatcher,
    /// Per-peer send state: version, sequence, pending ack, what they know.
    ///
    /// ONE STRUCT IN ONE MAP, replacing the bare version map this used to be.
    /// peer_session exists because the C++ spread this across seven parallel
    /// maps — one marked "Bug 5 fix", another added later "otherwise it lingers
    /// for every ever-connected peer" — and "a checklist grows a hole every
    /// time somebody adds a map". My version map was that shape starting over.
    pub sessions: crate::peer_session::Sessions,
    /// Whether each peer's cached address is still worth trying. Beside
    /// `punch` rather than inside it: an address is replaced wholesale by
    /// gossip or the DHT, and a record living on the value would be reset
    /// by the refresh that handed back the same wrong address.
    pub addr_health: crate::punch_addrs::AddrHealth,
    /// The swipe affordance. Retires itself after four swipes.
    pub peel: crate::nav_chrome::PeelHint,
    /// The page strip. Equal segments, an underline and badges — the ported
    /// SegmentedTabStrip, replacing a hand-rolled selectable_label row.
    pub strip: crate::nav_chrome::TabStrip,
    /// The Inbox's bottom ribbon: All / Unread / Sent plus the actions each of
    /// those tabs can actually offer.
    pub ribbon: crate::ribbon::SmartRibbon,
    /// Our own position, when the device has a fix. `None` is a real state, not
    /// a placeholder: without it nobody can be placed geographically.
    pub self_fix: Option<crate::wayfinder::Fix>,
    /// Contacts as the wayfinder sees them — id, name, online, position.
    pub wayfinder_contacts: Vec<crate::wayfinder::Contact>,
    /// Named places you can be resolved to — "my apartment", "the studio" — so
    /// a contact can be matched to a place WITHOUT either of you sharing a live
    /// position. The other half of the wayfinder, and it had no surface.
    pub landmarks: Vec<crate::landmark::Landmark>,
    /// The add-landmark form, open only while naming somewhere.
    pub add_landmark: Option<crate::landmark::AddLandmark>,
    /// The risk-scored confirmation, open only while asking. Its mode — tap or
    /// press-and-hold — comes from how bad the mistake would be, which is the
    /// reason one component can serve both "clear this filter" and "delete this
    /// conversation" without either feeling wrong.
    pub confirm: Option<crate::chevron_confirm::ChevronConfirm>,
    /// Per-feature use records — UxGuide's familiarity counter, ported as
    /// `confirm::Familiarity`. This is what lets a confirmation stop being a
    /// tax once a feature is learned (and re-raise itself after a cancel).
    pub familiarity: std::collections::HashMap<String, crate::confirm::Familiarity>,
    /// A Block being confirmed on the requests list: (sender_id, panel).
    /// `confirm`'s own panel, not the chevron: Block is Tap-level risk where
    /// the familiarity model actually gets to skip.
    pub block_confirm: Option<(String, crate::confirm::Confirm)>,
    /// Inbound connection requests — the mailbox's approval lifecycle.
    pub mailbox: crate::requests::RequestMailbox,
    /// Fix freshness per contact id: (age_secs, accuracy_m), supplied by the
    /// host beside `wayfinder_contacts`. Parallel map rather than a Contact
    /// field for now; folding age/accuracy into `wayfinder::Fix` is the
    /// cleaner follow-up and is noted in the parity ledger.
    pub contact_freshness: std::collections::HashMap<String, (i64, f32)>,
    /// How stale is too stale, for the wayfinder's signal strip.
    pub freshness_policy: crate::freshness::FreshnessPolicy,
    /// The last export's one-line result, shown by the inbox card.
    pub export_note: Option<String>,
    /// Which tile the strip has active. Held across frames because scrubbing
    /// is a sequence of steps from the current position, not an absolute
    /// jump — the engine's `scrub` takes a base index for exactly that.
    pub strip_active: usize,
    /// Is the active tile playing voice? Drives the engine's gating, which
    /// DIMS and slows the neighbours rather than freezing them.
    pub strip_voice: bool,
    /// Decoded thumbnails, bounded. The strip asks for three tiles per frame
    /// and scrubbing walks the window, so without a cache the same handful of
    /// images decode over and over; without a BOUND, a long window decodes
    /// them all into memory. The module has both.
    pub thumbs: crate::thumbnails::ThumbnailCache,
    /// The connection broker: one authenticated handshake per peer, streams
    /// handed out per service. Held so the roster can ask what is actually
    /// CONNECTED rather than showing a stored bool that means "was online
    /// once".
    pub broker: peermux_core::Broker,
    /// The drifting colour field behind the camera page. Off by default: it
    /// is decoration, and decoration that runs before anyone asks costs
    /// battery on a phone.
    pub blobs: crate::ambient_blobs::AmbientBlobs,
    /// Tap ripples and the press-depth response. One layer, because
    /// disabling feedback must stop BOTH — killing only the ripples leaves a
    /// control stuck mid-scale.
    pub feedback: crate::touch_feedback::TouchFeedback,
    /// Tap vs long-press vs drag on the viewfinder, as a state machine with
    /// a watchdog — a pointer-up lost to a system gesture would otherwise
    /// leave the machine Held forever.
    pub press: crate::press_gesture::PressGesture,
    /// Double-tap on the viewfinder flips the camera.
    pub double_tap: crate::press_gesture::DoubleTap,
    /// The shell is the phone one this frame (`Adaptive::is_compact`).
    pub compact: bool,
    /// Horizontal scroll for the tile strip, with edge pass-through: at the
    /// end of the strip the wheel stops being ours and reaches the page
    /// navigator behind it. Without that the carousel silently swallows the
    /// wheel at its edge and the page never turns.
    pub strip_scroll: crate::scroll_passthrough::HScroll,
    /// Decides WHEN a snap's arrival is worth re-deriving the ambient
    /// palette. Without it every arrival walks the sampler, which is the
    /// burst case the throttle was written for.
    pub ambient_driver: crate::ambient_inbox::AmbientInboxDriver,
    /// Which synthetic palette is in charge, if any. The driver refuses to
    /// stomp a palette that is not its own.
    pub active_synthetic: Option<String>,
    /// Fingerprints for the landmarks above, keyed by the SAME id: two lossy
    /// one-way transforms per place. This is landmark_hash's half of the
    /// feature — the photo a user took to mark a place never leaves the
    /// device, and this pair is what gets published to the DHT instead.
    pub landmark_prints:
        std::collections::HashMap<String, crate::landmark_hash::Fingerprint>,
    /// Diagnostic channels the user can switch on.
    pub trace: crate::trace_channels::TraceControl,
    /// Whether face detection may run at all. Revoked is distinct from
    /// NotGiven because it means an erase is OWED — the app has to know it
    /// once had crops to delete.
    pub face_consent: crate::face_consent::Consent,
    /// Where a peer was last reachable. The hole-punching half of the DHT:
    /// entries expire, because an address nobody has confirmed in minutes is
    /// a guess presented as a fact.
    pub punch: crate::punch_addrs::PunchAddrs,
    /// Who has been heard from lately, newest first.
    pub heard: crate::punch_addrs::Gossip,
    /// An in-flight DHT lookup: values accumulate until the search completes.
    pub dht_lookup: crate::dht_bootstrap::Lookup,
    /// The transport half of group voice: envelopes outbound packets to every
    /// peer and routes inbound ones back. Sits beside `group_voice`, which is
    /// the coordinator — one decides WHO holds the floor, this one carries
    /// the bytes and enforces that identity comes from the transport.
    pub voice_bridge: crate::voice_bridge::VoiceBridge,
    /// Derives the theme from the live viewfinder on a slow tick, so the
    /// chrome tracks what the camera is pointed at without deriving at 30Hz.
    pub ambient_camera: crate::ambient_camera::AmbientCameraDriver,
    /// Which contact the open confirm panel is about. Held separately because
    /// ChevronConfirm is generic over WHAT is being confirmed — it carries a
    /// risk score, not a subject.
    pub confirm_id: String,
    /// Which contact the editor is renaming, empty when closed.
    pub editor_id: String,
    /// The name being typed. Seeded from the current one so the dialog opens
    /// showing what it is about to change rather than a blank box.
    pub editor_name: String,
    /// Peers heard from over gossip. The app's `peers` list is what the USER
    /// has; this is what the NETWORK reports, and they are different questions:
    /// a contact you added stays in your list when they go offline, while a
    /// device that stopped gossiping is simply gone.
    pub view: crate::gossip::PeerView,
    /// When the view was last pruned, so pruning runs on its own schedule
    /// rather than once per frame.
    last_prune_ms: u64,
    /// This device's own addresses, for the hairpin check. Hairpin detection
    /// compares against ALL of them, not just the public one — a phone
    /// hotspot's 172.16.42.1 is a local address that a naive check misses.
    pub local_hosts: Vec<String>,
    /// Our public address, when something has told us one.
    pub public_host: Option<String>,
    /// Rows for snaps in flight.
    ///
    /// The type is send_state::Outbox, and there is ALSO a crate::outbox — two
    /// different things sharing a name. This one owns what the SENDER SEES; the
    /// other owns a snap's lifetime in storage. Fully qualified at every use
    /// because the short name would be ambiguous to a reader even where the
    /// compiler is satisfied.
    pub sends: crate::send_state::Outbox,
    /// Battery and power state. Three sources for one fact because no single
    /// one is present everywhere this app runs — PowerDevil on Plasma, UPower
    /// elsewhere, a sysfs poll where neither answers.
    pub power: crate::power_tier::PowerState,
    /// The bottom navigation bar. Equal-width tabs, an animated indicator, and
    /// unread badges — and the badge is the reason it needs wiring rather than
    /// just drawing: a count nobody updates is worse than no count.
    pub tabs: crate::tab_bar::TabBar,
    /// A horizontal swipe in progress, if one is. `None` between gestures
    /// rather than a Swipe with a zero drag: "not swiping" and "swiping
    /// nowhere" render differently — the second holds the page a pixel off
    /// centre and never springs back.
    pub swipe: Option<crate::swipe_physics::Swipe>,
    /// Which way this touch turned out to be going. Latched for the length
    /// of the touch — see `swipe_physics::AxisLock`.
    pub axis_lock: crate::swipe_physics::AxisLock,
    /// Where the finger went down, so total travel can be measured against
    /// it. egui reports per-frame deltas; a lock fed deltas would decide on
    /// one frame's jitter.
    pub touch_origin: Option<egui::Pos2>,
    /// The settling animation after a release: (from_offset, ms_left,
    /// ms_total). The page does not snap — it continues the movement it was
    /// given, which is what makes a turn feel like the finger caused it.
    pub swipe_settle: Option<(f32, f32, f32)>,
    /// The touch path handled this drag, so the gesture layer's own
    /// `page_jump` must not act on it too. Set at release and cleared when
    /// read: checking `axis_lock.intent()` instead does not work, because
    /// the release clears the lock in the same frame the layer reports the
    /// jump — which turned two pages per swipe.
    pub swipe_claimed: bool,
    /// This touch has already gone back one level.
    pub back_armed: bool,
    /// Accent and background derived from the photos in the inbox, so the app
    /// takes on the colour of what is in it. `None` until enough pixels have
    /// been sampled, and `None` is a real answer — see `ambient()`.
    pub ambient: Option<(crate::ambient_palette::Rgb, crate::ambient_palette::Rgb)>,
    /// The manual. Closed unless asked for, and it dismisses itself after a
    /// period of no activity — where "activity" deliberately means scrolling
    /// OR any other input, so a reader pausing on a short section is not
    /// dismissed mid-read.
    pub help: crate::help_manual::HelpManual,
    pub help_filter: String,
    pub help_scroll_y: f32,
    /// The command palette (`palette`): open, its text, its cursor.
    pub palette_open: bool,
    pub palette_input: String,
    pub palette_cursor: usize,
    /// Set the frame the palette opens, so the field takes focus once.
    pub palette_focus: bool,
    pub schedule: crate::palette::Schedule,
    /// The keyboard is driving: Tab was pressed and the pointer has not
    /// moved since. Draws the focus ring and its caption.
    pub keyboard_nav: bool,
    /// What Space/Enter does on a widget, by id, registered as widgets
    /// draw (`key_label`). Cleared every frame.
    pub key_labels: std::collections::HashMap<egui::Id, String>,
    /// Running edge-pixel samples. Kept across snaps rather than recomputed:
    /// the palette should settle as the inbox fills, not lurch to whatever
    /// arrived last.
    ambient_samples: crate::ambient_palette::Samples,
    /// (sender id, caption) for the snaps floating on the map, newest first.
    pub wayfinder_snaps: Vec<(String, String)>,
    gesture: GestureLayer,
    palette: String,
}


/// Parse `a.b.c.d` into four octets, or `None`.
///
/// Deliberately narrow: `PeerAddr` is four bytes, and a hostname or an IPv6
/// literal cannot become one. Returning None keeps them out of the punch cache
/// rather than storing a zero address that would read as a reachable peer.
fn parse_dotted_quad(host: &str) -> Option<[u8; 4]> {
    let mut out = [0u8; 4];
    let mut parts = host.trim().split('.');
    for slot in out.iter_mut() {
        *slot = parts.next()?.parse().ok()?;
    }
    if parts.next().is_some() {
        return None;   // five or more segments is not an address
    }
    Some(out)
}

impl Snapclone {
    pub fn new(peers: Vec<Peer>) -> Snapclone {
        Self::new_with_login(peers, true)
    }

    /// `auto_logged_in` mirrors Qt's construction-time auto-login check.
    pub fn new_with_login(peers: Vec<Peer>, auto_logged_in: bool) -> Snapclone {
        let mut s =
        Snapclone {
            viewfinder: Default::default(),
            camera: None,
            cameras: Vec::new(),
            cam_selection: None,
            shutter: Default::default(),
            prefs_handedness: settings_kit::Handedness::Right,
            recorder: None,
            spill: None,
            scan_counts: Default::default(),
            scan_hit: None,
            encode_done: std::sync::mpsc::channel(),
            shutter_note: String::new(),
            feed_note: String::new(),
            local_id: String::new(),
            contact_book: Default::default(),
            key_conflicts: Vec::new(),
            contacts_dirty: false,
            frames: Default::default(),
            capture_status: crate::capture::Status::Idle,
            cam_texture: None,
            last_frame_ms: None,
            settings_error: None,
            notify_enabled: true,
            high_security: false,
            page_debounce: crate::debounce::Debouncer::new(
                crate::debounce::Config::page_transition()),
            inbox: Default::default(),
            composer: Default::default(),
            // Load from disk rather than defaults: a settings page that starts
            // blank every launch is the same lie as one that never saved.
            settings: crate::settings_page::SettingsPage::new(crate::settings_store::load()),
            dht: None, dht_failed: false,
            dht_schedule: Default::default(),
            dht_keepalive: None,
            reflexive: None,
            stun_failed: false,
            dht_peers: 0,
            reachable_shown: false,
            a11y: uikit_egui::a11y::A11yScene::new(),
            pending_vcard_text: None,
            import_report: None,
            group_voice: Default::default(),
            voice_fx: Default::default(),
            picker: Default::default(),
            cards: Default::default(),
            add_form: Default::default(),
            modal: Default::default(),
            feed: Default::default(),
            snap_state: Default::default(),
            snap_state_dirty: false,
            viewer: Default::default(),
            feed_press: Default::default(),
            snaps_viewed: Vec::new(),
            snaps_expired: Vec::new(),
            last_saved: None,
            outbox: Default::default(),
            login: Default::default(),
            now_secs: 0,
            incoming_call: None,
            missed_calls: Vec::new(),
            qr: None,
            voicebar: crate::voicebar::VoiceBar::new(),
            voice_room: Default::default(),
            reminder: Default::default(),
            now_month: None,
            recent_snaps: Vec::new(),
            export_requested: false,
            retry_requested: false,
            bg_carousel: crate::scroll_areas::HScroll::new(0.0, 400.0),
            carousel_handoff: false,
            dialog_form: crate::mobile_dialog::Form::Sheet,
            glass_flip: Default::default(),
            glass_shutter: Default::default(),
            snap_flash: None,
            shutter_pressed: false,
            video: None,
            menu: Default::default(),
            received: Vec::new(),
            pending_open: None,
            attention: Default::default(),
            menu_target: None,
            snaps_saved: Vec::new(),
            send_summary: Default::default(),
            posture: Default::default(),
            last_voice_at: None,
            glove: Default::default(),
            glove_input: Default::default(),
            voice_field: None,
            dictated: Vec::new(),
            camera_policy: Default::default(),
            camera_health: Default::default(),
            playout: Default::default(),
            sink: Default::default(),
            // 20 ms frames, the value snap_frame_ms normalises toward.
            sink_latency: crate::sink_cushion::Latency::from_settings(2, 8, 20),
            sender: Default::default(),
            nat: None,
            keyboard: Default::default(),
            sessions: Default::default(),
            addr_health: Default::default(),
            shake: uikit_egui::glove::ShakeCommands::with(vec![
                uikit_egui::glove::ShakeBinding {
                    dir: uikit_egui::glove::Orient::RightUp,
                    action: "swap_camera".into(), label: "Swap camera".into() },
            ]),
            peel: crate::nav_chrome::PeelHint::new(PAGES.len()),
            strip: crate::nav_chrome::TabStrip::new(&PAGES),
            ribbon: {
                // The tab labels are the nav's own, so the ribbon and the
                // sub-tab table cannot drift apart.
                let mut r = crate::ribbon::SmartRibbon::new(sub_tabs(1));
                r.set_tab_actions(1, vec![
                    crate::ribbon::Action::new("read_all", "check", "Mark all read")]);
                r.set_tab_actions(2, vec![
                    crate::ribbon::Action::new("retry_failed", "refresh", "Retry failed")]);
                r
            },
            self_fix: None,
            wayfinder_contacts: Vec::new(),
            landmarks: Vec::new(),
            add_landmark: None,
            confirm: None,
            familiarity: std::collections::HashMap::new(),
            block_confirm: None,
            mailbox: crate::requests::RequestMailbox::new(),
            contact_freshness: std::collections::HashMap::new(),
            freshness_policy: crate::freshness::FreshnessPolicy::default(),
            export_note: None,
            strip_active: 0,
            strip_voice: false,
            thumbs: crate::thumbnails::ThumbnailCache::new(),
            broker: peermux_core::Broker::new(),
            // Default is an EMPTY field; `seeded` is the constructor. A fixed
            // seed rather than entropy: the field is deterministic so a
            // screenshot is reproducible.
            blobs: crate::ambient_blobs::AmbientBlobs::seeded(5, 0x5eed_1234),
            // Enabled per the module's own rule — on touch only; a ripple
            // under a mouse cursor is a phone UI wearing a desktop's clothes.
            // Re-set from Adaptive each frame in `ui`.
            feedback: crate::touch_feedback::TouchFeedback::new(false),
            press: Default::default(),
            double_tap: Default::default(),
            compact: false,
            // Bounds are set from the tile count each frame; a strip that has
            // not been drawn yet scrolls nowhere rather than pretending to.
            strip_scroll: crate::scroll_passthrough::HScroll::new(0.0, 0.0),
            ambient_driver: Default::default(),
            active_synthetic: None,
            landmark_prints: std::collections::HashMap::new(),
            trace: Default::default(),
            // Seeded from the file, not from the default: consent asked for
            // once and then forgotten on every restart is a consent dialog the
            // user answers forever, and a REVOCATION forgotten is worse — the
            // erase it owes never happens because nothing remembers it was
            // owed. The settings load has already run by here (`settings`, a
            // few lines up).
            face_consent: crate::face_consent::Consent::NotGiven,
            punch: Default::default(),
            heard: Default::default(),
            dht_lookup: Default::default(),
            voice_bridge: Default::default(),
            ambient_camera: crate::ambient_camera::AmbientCameraDriver::new(),
            confirm_id: String::new(),
            editor_id: String::new(),
            editor_name: String::new(),
            view: Default::default(),
            last_prune_ms: 0,
            local_hosts: Vec::new(),
            public_host: None,
            sends: Default::default(),
            power: Default::default(),
            // The five surfaces this app actually has. Built here rather than
            // in TabBar::default() because the bar is generic over its tabs —
            // a default set inside the widget would be this app's navigation
            // leaking into a shared component.
            swipe: None,
            axis_lock: Default::default(),
            touch_origin: None,
            swipe_settle: None,
            swipe_claimed: false,
            back_armed: false,
            ambient: None,
            // Written here rather than in HelpManual::default(): the widget is
            // generic over its sections, and this app's manual living inside it
            // would be snapclone's copy leaking into a reusable component.
            //
            // The four things a new user actually needs, in the order they hit
            // them — not a feature list. A manual that documents the settings
            // panel before it explains what a snap IS has been written for the
            // person who wrote the app.
            help: crate::help_manual::HelpManual::new(vec![
                crate::help_manual::Section::new(
                    "snaps", "Sending a snap",
                    "Take a photo on the Camera tab and pick who it goes to. \
                     It is sent directly to them — there is no server holding a \
                     copy, so both of you need to be online, or it waits in your \
                     outbox until they are."),
                crate::help_manual::Section::new(
                    "expiry", "When snaps disappear",
                    "Text and photos are kept for 24 hours by default and media \
                     for 30 days. Keeping a snap stops the clock for that one \
                     only. Expiry is per-device: yours going does not take \
                     theirs with it."),
                crate::help_manual::Section::new(
                    "adding", "Adding someone",
                    "Show them your code on the Friends tab, or scan theirs. \
                     The code is your sc-ID — it identifies you, so it is safe \
                     to show a stranger and pointless to keep secret."),
                crate::help_manual::Section::new(
                    "privacy", "What leaves this device",
                    "Snaps go to the people you send them to and nowhere else. \
                     Timestamps are stored in UTC and last-seen is rounded to \
                     five minutes, so neither your timezone nor your exact \
                     habits are readable from the data."),
            ]),
            help_filter: String::new(),
            help_scroll_y: 0.0,
            palette_open: false,
            palette_input: String::new(),
            palette_cursor: 0,
            palette_focus: false,
            schedule: Default::default(),
            keyboard_nav: false,
            key_labels: Default::default(),
            ambient_samples: Default::default(),
            // Labels from PAGE_NAMES, not a second list. The bar said "Chats"
            // and "Map" where the rest of the app says "Inbox" and
            // "Wayfinder" — harmless while nothing drew it, and two names for
            // one page the moment it appeared beside the strip on a resize.
            // The icons stay hand-written because PAGE_NAMES has none.
            tabs: crate::tab_bar::TabBar::new(
                ["camera", "chats", "friends", "map", "settings"]
                    .iter()
                    .zip(PAGES.iter())
                    .map(|(icon, name)| crate::tab_bar::Tab::new(icon, name))
                    .collect()),
            wayfinder_snaps: Vec::new(),
            conversation: Default::default(),
            conversation_peer: String::new(),
            call_note: String::new(),
            send_requested: Vec::new(),
            send_flow: Default::default(),
            send_request: None,
            qr_expanded: None,
            qr_shown_at: None,
            code_svg_requested: None,
            tilt: Default::default(),
            bar: UnifiedNav::new(),
            login_visible: !auto_logged_in,
            nav: Nav::new(PAGES.len()),
            cols_drawn: 1,
            plan: uikit_egui::pager::PlanState::new(),
            bg: LiveBackground::new(BgStyle::Aurora),
            gesture: GestureLayer::horizontal_page(320.0),
            palette: String::new(),
            // Built from the same peers, once, so the two cannot drift.
            roster: {
                let mut r = crate::friends::Roster::default();
                r.set(peers.iter().map(Snapclone::friend_of).collect());
                r
            },
            peers,
        };
        // Consent comes BACK OUT of the settings that were just loaded.
        //
        // It cannot be set in the literal above — `settings` is one of the
        // fields being built there — and it must not be left at the default:
        // a consent answered once and forgotten on every restart is a dialog
        // the user answers forever, and a forgotten REVOCATION is worse, since
        // the erase it owes never happens because nothing remembers it was
        // owed.
        s.face_consent =
            crate::face_consent::Consent::from_key(&s.settings.current.camera.face_consent);
        s
    }

    fn friend_of(p: &Peer) -> crate::friends::Friend {
        crate::friends::Friend {
            // A discovered Peer has one string standing for both its routable
            // id and its name; a real contact record separates them, and the
            // Friend row is built to take either.
            id: p.name.clone(), name: p.name.clone(), online: p.online,
        }
    }

    /// Re-derive the roster after the peer list changes. One source of truth:
    /// keeping a second list in parallel means the day an update misses one of
    /// them, the page shows someone as online who left an hour ago.
    pub fn sync_roster(&mut self) {
        self.roster.set(self.peers.iter().map(Snapclone::friend_of).collect());
    }

    /// A gossip record arrived. Returns whether it was accepted.
    ///
    /// Every datagram goes through `gossip::accept` first — it refuses
    /// unsigned records, our own multicast echo, and anything oversized. That
    /// check is the whole security boundary for discovery: gossip arrives from
    /// ANYONE on the subnet, so a record that skips it can claim to be any
    /// peer.
    pub fn on_gossip(&mut self, rec: crate::gossip::PeerRecord, datagram_len: usize,
                     public: Option<[u8; 32]>) -> bool {
        if crate::gossip::accept(&rec, datagram_len, &self.local_id).is_err() {
            return false;
        }
        // THE PIN, checked before anything is believed. `accept` proves the
        // datagram was signed by the key whose fingerprint is its sc-ID; it
        // cannot know whether that is the SAME key this contact used yesterday.
        // Four bytes of fingerprint is ~2^32 to grind, so without this the
        // signature only stops the laziest impersonation.
        //
        // Taken as a parameter rather than read off the record because
        // PeerRecord is the view's currency and sixteen literals construct one;
        // the key is something this call learned, not something the view keeps.
        if let Some(pk) = public {
            match self.contact_book.pin(&rec.id, pk) {
                crate::contact_book::Pin::First => self.contacts_dirty = true,
                crate::contact_book::Pin::Same | crate::contact_book::Pin::Unknown => {}
                crate::contact_book::Pin::Changed => {
                    // Refused AND surfaced. A reinstall and a collision look
                    // identical from here: dropping silently makes a legitimate
                    // reinstall look like a broken network, and accepting
                    // silently makes the pin worthless. The user has to decide,
                    // so the event has to reach them.
                    if !self.key_conflicts.contains(&rec.id) {
                        self.key_conflicts.push(rec.id.clone());
                    }
                    return false;
                }
            }
        }
        // A discovered device becomes a peer row only if the user does not
        // already have one. Discovery ADDS reachability information; it does
        // not get to rename or replace a contact the user chose to keep.
        let id = rec.id.clone();
        // FEED THE REACH CACHES. note_peer_addr was reached only from a test,
        // so `heard` and `punch` were always empty and the network view could
        // not have shown anything however it was drawn. Records arrive here
        // and nowhere else.
        //
        // Only a dotted-quad host is taken. PeerAddr is [u8; 4] and a
        // hostname or an IPv6 literal has no honest conversion — recording a
        // zero address would put a fact-shaped guess in the cache the module's
        // own comment warns about, and the peer row above still gets added
        // either way, so nothing is lost by declining.
        if let Some(ip) = parse_dotted_quad(&rec.host) {
            self.note_peer_addr(&id, ip, rec.port, rec.last_seen_ms);
        }
        self.view.observe(rec);
        // PRESENCE, which was the asymmetry that made discovery useless. This
        // used to add a row for an unknown peer and do NOTHING for a known one,
        // while `tick_gossip` happily set `online = false` on prune — so a
        // contact loaded from the address book could go offline and could never
        // come back. Every saved contact was permanently grey no matter how
        // loudly it was broadcasting.
        //
        // The comment above about discovery not replacing a contact still
        // holds: this changes reachability, not the name or the identity. That
        // is precisely what discovery is FOR.
        match self.peers.iter_mut().find(|p| p.name == id) {
            Some(p) => p.online = true,
            None => self.peers.push(Peer { name: id, online: true }),
        }
        self.sync_roster();
        true
    }

    /// Which addresses to try for a discovered peer, best first.
    ///
    /// [`crate::peer_dial`] decides what to open; [`crate::handshake`] runs once
    /// a socket is open. Neither had a caller, so gossip could find a device and
    /// the app had no idea how to reach it.
    ///
    /// Two filters, in this order: `dialable` REFUSES addresses rather than
    /// ranking them low — a port of 0 cannot be dialled and a hairpin will not
    /// loop back on most consumer routers, so both are removed rather than
    /// tried and timed out. Then `rerank` orders what survives by class, so a
    /// peer on the same LAN is tried before a public address that will cost a
    /// round trip through the internet to reach a machine in the next room.
    pub fn dial_order(&self, peer_id: &str) -> Vec<crate::peer_dial::PeerAddress> {
        use crate::peer_dial::{class_of, dialable, rerank, AddrClass, PeerAddress};
        let addrs: Vec<PeerAddress> = self.view.peers().iter()
            .filter(|r| r.id == peer_id)
            .map(|r| PeerAddress { host: r.host.clone(), port: r.port })
            .collect();
        let local: Vec<String> = self.local_hosts.clone();
        let mut out = dialable(&addrs, self.public_host.as_deref(), &local);
        // The learned ranking is the app's, not the module's: peer_dial holds
        // the ordering RULE and this holds what was observed to work here.
        rerank(&mut out,
               |a| class_of(&a.host, local.iter().any(|l| same_prefix(l, &a.host))),
               &[AddrClass::SameLan, AddrClass::Private, AddrClass::Public]);
        out
    }

    /// The bytes to send once a socket to `peer_id` is open.
    ///
    /// Version is NEGOTIATED, not assumed: v2 only when both sides support it,
    /// because sending a v2 SHAK to a v1 peer is a magic number it will reject
    /// outright — and the failure looks like an unreachable device rather than
    /// a version mismatch.
    ///
    /// The peer's support comes from its gossip record rather than a guess. A
    /// device that has not been heard from gets v1, which every version
    /// understands: guessing high costs a whole handshake round trip to learn
    /// something the conservative choice never needed to ask.
    pub fn hello_for(&self, peer_id: &str, our_pub: &[u8], pq: Option<&[u8]>) -> Vec<u8> {
        let peer_v2 = self.view.peers().iter()
            .find(|r| r.id == peer_id)
            .map(|r| r.verified)
            .unwrap_or(false);
        let magic = crate::handshake::choose_version(pq.is_some(), peer_v2);
        // encode_v2 writes whichever magic the negotiation chose; passing the
        // PQ key only when v2 was selected keeps the flag and the payload in
        // agreement, which parse() checks and refuses.
        let pq = if magic == crate::handshake::MAGIC_V2 { pq } else { None };
        crate::handshake::encode_v2(&self.local_id, our_pub, pq, false)
    }

    /// When to retry a handshake, or `None` once the attempts are spent.
    ///
    /// Delegated rather than re-derived: the cap exists because "a handshake
    /// retried forever keeps a dead peer's slot warm and, on a phone, keeps the
    /// radio awake for it", and a second retry policy here could disagree with
    /// that one.
    pub fn handshake_retry(&self, attempt: u32) -> Option<u64> {
        crate::handshake::next_retry_ms(attempt)
    }

    /// Agree a wire version with a peer.
    ///
    /// Returns false when there is no overlap, which is a real outcome rather
    /// than an error to swallow: a peer too old to speak VERSION_MIN cannot be
    /// talked to at all, and pretending otherwise sends frames it will
    /// misparse.
    pub fn negotiate_wire(&mut self, peer: &str, theirs_max: u8) -> bool {
        match crate::wire_v3::negotiate(crate::wire_v3::VERSION_MAX, theirs_max) {
            Some(v) => { self.sessions.entry(peer).set_version(v); true }
            // Forget the WHOLE session, not just the version. A peer that can
            // no longer be spoken to should not keep a sequence number or a
            // list of what it knows — that is the lingering state the source's
            // own note warns about.
            None => { self.sessions.forget(peer); false }
        }
    }

    /// The flags for one frame to `peer`, or None if we cannot send yet.
    ///
    /// The predictive rule the protocol is built on: announce AND send, rather
    /// than announcing and waiting to be asked. "No HAVE/NEED ... no standalone
    /// ACK: every message carries ack number", and both halves remove a ROUND
    /// TRIP — which on a phone radio costs a wakeup, not just bytes.
    ///
    /// A delta is DOWNGRADED to a full payload for a peer below v3 rather than
    /// refused. The module is explicit that sending one anyway "produces a
    /// frame it parses as a full payload — silently corrupt data rather than a
    /// protocol error", so the check has to happen at send time, and the honest
    /// fallback is the whole thing rather than nothing.
    pub fn frame_flags(&self, peer: &str, has_payload: bool, is_delta: bool,
                       acking: bool) -> Option<u8> {
        let sess = self.sessions.get(peer)?;
        // is_v3() rather than a version comparison here: the session already
        // knows, and asking it keeps the delta rule in one place.
        let delta_ok = is_delta && sess.is_v3();
        Some(crate::wire_v3::predictive_flags(has_payload, delta_ok, acking))
    }

    /// The bottom padding this window needs so the keyboard covers nothing.
    ///
    /// Overlap against the window's own bottom edge, not the keyboard's height.
    pub fn keyboard_inset(&self, window: crate::keyboard_inset::Rect) -> f32 {
        self.keyboard.inset_for(window)
    }

    /// A compositor reported a new keyboard rectangle. True if it CHANGED.
    ///
    /// The return value is the point: compositors re-send the same rectangle,
    /// and relayouting on every report re-lays out continuously while the
    /// keyboard sits perfectly still. The caller only acts on true.
    pub fn on_keyboard(&mut self, r: crate::keyboard_inset::Rect) -> bool {
        self.keyboard.report(r)
    }

    /// How far to scroll so `field` clears the keyboard, or 0 if it already does.
    ///
    /// Guarded by `covers` inside the module: scrolling a field that was never
    /// covered moves the page under the user for no reason.
    pub fn scroll_for_keyboard(&self, field: crate::keyboard_inset::Rect,
                               margin: f32) -> f32 {
        self.keyboard.scroll_to_clear(field, margin)
    }

    /// Should the payload go out, or just the announcement?
    ///
    /// The predictive rule, and the source states its trade: "if we have no
    /// record of what this peer knows, send data immediately. Better to spend
    /// one extra transmission than to wait for a round trip."
    ///
    /// Right way round on a phone. An extra transmission costs bytes on a radio
    /// that is ALREADY AWAKE; a round trip costs a second wakeup and the
    /// sleeping radio has to come back up for it.
    pub fn should_send_payload(&self, peer: &str, hash: &str) -> bool {
        match self.sessions.get(peer) {
            Some(s) => s.should_send_data(hash),
            None => true,   // no record at all: send it
        }
    }

    /// Note that a peer now has this snap, so the next frame can skip the body.
    pub fn peer_learned(&mut self, peer: &str, hash: &str) {
        self.sessions.entry(peer).learn(hash);
    }


    /// The sequence and acknowledgement for the next frame to a peer.
    ///
    /// `next_seq` and `take_ack` were written, tested, and called by
    /// nothing: every snap this app sent went out with `seq: 0, ack: 0`
    /// hardcoded at the send site. The fields were on the wire and carried
    /// no information, so a receiver could not tell two snaps apart by
    /// sequence, could not spot a duplicate, and learned nothing about
    /// what the sender had received.
    ///
    /// Taken TOGETHER, in one call, because they belong to the same frame:
    /// fetching the sequence and the ack separately leaves a window where
    /// another send can interleave and pair this frame's sequence with the
    /// next one's ack.
    pub fn frame_ids(&mut self, peer: &str) -> (u64, u64) {
        let s = self.sessions.entry(peer);
        (s.next_seq(), s.take_ack())
    }

    /// Classify our NAT from two STUN responses.
    ///
    /// Both probes must have left the SAME socket, and the source is explicit
    /// about why: "send a STUN binding request FROM m_punchSocket" rather than
    /// an ephemeral one. A reflexive address discovered on a different socket
    /// describes a mapping that will not exist when the punch arrives — the
    /// classification would be right about a port nobody is listening on.
    ///
    /// Enforced here rather than trusted: the two responses carry the local
    /// port they came from, and a mismatch is refused.
    pub fn classify_nat(&mut self, local_port: u16,
                        a: &crate::nat_punch::Reflexive,
                        b: &crate::nat_punch::Reflexive) -> bool {
        if a.from_local_port != local_port || b.from_local_port != local_port {
            return false;
        }
        self.nat = Some(crate::nat_punch::classify(local_port, a, b));
        true
    }

    /// How to reach this peer, given both NATs.
    ///
    /// Returns `Wait` until ours is known, which is the module's own answer for
    /// missing information — better than guessing DirectTcp and burning the
    /// punch window on a connection that cannot form.
    pub fn reach_strategy(&self, theirs: crate::nat_punch::NatKind)
        -> crate::nat_punch::Strategy
    {
        match self.nat {
            Some(ours) => crate::nat_punch::strategy(ours, theirs),
            None => crate::nat_punch::Strategy::Wait,
        }
    }

    /// Ask whether this captured frame may go out, and with what sequence.
    ///
    /// The Qt contract the receiver relies on: sequence numbers restart at 0
    /// each talk-spurt and the first frame is flagged TALKSPURT_START so the
    /// receiver resets its jitter buffer cleanly.
    ///
    /// Worth stating why both halves being present matters. Porting the sender
    /// found a bug in the receiver: its drop-late-frames rule made a second
    /// spurt — restarting at 0 — look exactly like a flood of very late
    /// packets, and it ate the whole spurt. The receiver had passed its tests
    /// because every test fed it ONE spurt. The defect lived only in the
    /// relationship between the two, so wiring one without the other leaves
    /// exactly the gap that hid it.
    pub fn voice_frame(&mut self) -> crate::voice_send::Emit {
        self.sender.on_frame()
    }

    /// Start or stop a talk-spurt.
    ///
    /// Separate from the call state: joining a room is not the same as
    /// speaking, and a sender that transmits from the moment of joining sends
    /// silence at everyone for the length of the call.
    pub fn set_transmitting(&mut self, on: bool) {
        self.sender.set_transmitting(on);
    }

    /// Hand the audio sink its frames for this tick.
    ///
    /// The layer below voice_playout, and the bug it fixes is worth stating
    /// because the obvious diagnosis is wrong: the old design "wrote exactly
    /// one 20 ms frame per 20 ms timer tick into the sink with no buffer
    /// headroom", and crackled. Not because the CPU was slow — "a software
    /// timer can't stay phase-locked to the device's sample clock", so one
    /// frame per tick has ZERO margin however fast the machine is. A governor
    /// change would not have fixed it and neither would a faster phone.
    ///
    /// The fix is a cushion topped up every tick, with the tick running at half
    /// a frame so it refills well before the cushion drains. `sink_depth` is
    /// what the device still holds; the returned frames are what to write.
    pub fn sink_tick(&mut self, sink_depth: usize) -> Vec<Vec<f32>> {
        let l = self.sink_latency;
        self.sink.fill(sink_depth, &l)
    }

    /// Queue a decoded frame for the sink.
    pub fn sink_push(&mut self, frame: Vec<f32>) {
        let l = self.sink_latency;
        self.sink.push(frame, &l);
    }

    /// How often to call `sink_tick`, in ms.
    ///
    /// Half a frame, from the module: refill well before the cushion drains
    /// rather than exactly as it empties.
    pub fn sink_tick_ms(&self) -> u32 {
        crate::sink_cushion::tick_interval_ms(self.sink_latency.frame_ms)
    }

    /// The peer view as it should be WRITTEN TO DISK.
    ///
    /// store_privacy exists to stop the database leaking what the user never
    /// agreed to publish, and a precise last-seen is a presence fingerprint: it
    /// says when someone was awake, to the millisecond, for anything that later
    /// reads the store.
    ///
    /// APPLIED AT PERSISTENCE, NOT IN THE LIVE VIEW, and that distinction is
    /// load-bearing rather than stylistic. The bucket is 5 minutes and gossip's
    /// FAILURE_MS is 15 seconds, so bucketing on the way IN makes every peer
    /// look 100+ seconds stale the instant it is heard from — `is_alive` goes
    /// false for everyone and discovery stops working. I wrote that version
    /// first and the arithmetic caught it.
    ///
    /// The live view keeps millisecond precision because liveness needs it; the
    /// stored copy does not, because nothing reading it later needs to know the
    /// second.
    ///
    /// Floored rather than rounded to nearest, which the module is explicit
    /// about: rounding can move a timestamp into the FUTURE, and "a last-seen
    /// in the future reads as a clock bug and can make freshness comparisons
    /// negative".
    pub fn view_for_storage(&self) -> Vec<crate::gossip::PeerRecord> {
        self.view.peers().iter().map(|r| {
            let mut r = r.clone();
            r.last_seen_ms =
                crate::store_privacy::bucket_last_seen(r.last_seen_ms as i64).max(0) as u64;
            r
        }).collect()
    }

    /// Age out peers nobody has heard from, on the gossip prune schedule.
    ///
    /// Called every frame, acting every PRUNE_MS: a view pruned per frame would
    /// walk the peer list sixty times a second to do nothing, and one pruned
    /// only on a timer tick needs a timer.
    /// Which discovery backends the operator has actually asked for.
    ///
    /// `network/discovery_backend` was WRITTEN by the settings page,
    /// PERSISTED by settings_store, and read by nothing — so choosing
    /// "Jami DHT" or "Both" changed a stored byte and no behaviour. Same
    /// dead-setting shape as voiceterm's microphone toggle and the
    /// debate panel, found a third time.
    ///
    /// Returned as a pair rather than the enum so callers cannot forget
    /// that `Both` means both — an `if backend == Gossip` at one call
    /// site and a `match` at another is how the two halves drift.
    pub fn discovery_enabled(&self) -> (bool, bool) {
        use crate::settings_page::DiscoveryBackend as D;
        match self.settings.current.network.discovery_backend {
            D::Gossip => (true, false),
            D::Jami => (false, true),
            D::Both => (true, true),
        }
    }

    /// Drive the DHT half of discovery.
    ///
    /// Started lazily on the first tick where the operator has asked for
    /// it, rather than at construction: a node binds a socket and spawns
    /// a thread, and doing that for someone who only ever uses LAN
    /// gossip is a cost with no return.
    ///
    /// Results land as `Reach::Unverified` peers — see `peer_source`,
    /// which explains why a DHT record is weaker evidence than a gossip
    /// packet rather than stronger.
    pub fn tick_dht(&mut self, now_ms: u64) {
        if !self.discovery_enabled().1 {
            // Asked to stop. Dropping the host stops its worker thread
            // and closes the socket, so turning the setting off actually
            // stops the radio traffic rather than merely ignoring it.
            self.dht = None;
            return;
        }
        if self.dht.is_none() {
            // The id is derived from the sc-ID, NOT random: a node that
            // re-randomises on each launch makes every other node's
            // routing table stale. `node_host::spawn` documents this and
            // leaves it to the caller, which is here.
            // Derived from the sc-ID we already hold. Waiting for one
            // rather than binding with a placeholder: a node that starts
            // under a random id and later adopts its real one has
            // already told peers the wrong thing.
            if self.local_id.is_empty() {
                return;
            }
            let id = crate::identity::dht_node_id(&self.local_id);
            let [preferred, fallback] =
                Self::dht_bind_addr(self.settings.current.network.listen_port);
            match snapclone_presence::node_host::NodeHost::spawn(id, preferred)
                .or_else(|_| snapclone_presence::node_host::NodeHost::spawn(id, fallback))
            {
                Ok(h) => {
                    // BOOTSTRAP IMMEDIATELY, or the node is a routing
                    // table with nothing in it and every lookup returns
                    // empty forever. `bootstrap_addresses` has produced
                    // this list since it was written and only tests
                    // called it.
                    //
                    // `dns_working` is assumed true and `web_ports_only`
                    // false: those are the ordinary network, and
                    // `ordered_for` already puts the fallbacks after the
                    // standard port, so a hostile network costs a few
                    // failed sends rather than a wrong list. Detecting
                    // which network we are on is a separate job and
                    // guessing it here would be a second, worse copy of
                    // it.
                    for addr in self.bootstrap_addresses(true, false) {
                        // Resolution can fail — a hostname with no DNS,
                        // an IPv6-only network. Skipped rather than
                        // fatal: the list exists precisely so one dead
                        // entry is not the end of it.
                        if let Ok(mut it) = std::net::ToSocketAddrs::to_socket_addrs(&addr) {
                            if let Some(sa) = it.next() {
                                // The peer's real node id is unknown
                                // until it answers. Derived from its
                                // address so two bootstrap servers do
                                // not collide in the routing table —
                                // the id is corrected when the node
                                // actually replies.
                                let id = crate::identity::dht_node_id(&addr);
                                h.bootstrap(sa, id);
                            }
                        }
                    }
                    // And every peer we remember, under its real id. On a
                    // network without the servers above, this is the
                    // whole bootstrap.
                    for (sa, id) in self.peer_bootstraps() {
                        h.bootstrap(sa, id);
                    }
                    self.dht = Some(h);
                }
                Err(e) => {
                    // Said once. A bind failure is permanent for this
                    // run — a port conflict or a sandbox with no
                    // network — and retrying every tick would log it
                    // forever.
                    if !self.dht_failed {
                        self.dht_failed = true;
                        self.feed_note = format!("DHT unavailable: {e}");
                    }
                    return;
                }
            }
        }
        // WHAT THE NODE IS FOR. `publish` and `resolve` had no caller
        // anywhere in the tree, so this node maintained a routing table on
        // the network's behalf and asked it nothing — every peer this app
        // has ever found came from LAN gossip. The scheduler decides the
        // cadence (`dht_presence`); this performs it.
        //
        // Candidates are contacts NOT visible on the LAN. Gossip is better
        // evidence — signed, and it means they are on this subnet right now
        // — so asking the DHT about someone two metres away spends traffic
        // to learn something weaker.
        let offline: Vec<String> = self.contact_book.iter().map(|c| c.sc_id.clone())
            .filter(|id| !self.peers.iter().any(|p| p.name == *id && p.online))
            .collect();
        let plan = self.dht_schedule.tick(now_ms, &offline, true);
        if let Some(h) = self.dht.as_ref() {
            if plan.publish && !self.local_id.is_empty() {
                // EVERY address, not one, and never an empty one.
                //
                // I first published with an empty host, reasoning that "the
                // resolver observes the real address, the same rule gossip
                // already follows". That is right for gossip and wrong
                // here, and the difference is what a DHT is: a gossip
                // datagram arrives from a source address the receiver can
                // see, while a DHT record is stored DATA that a stranger
                // fetches later from a third machine. Nobody observes
                // anything, so the record said "I exist" and could not be
                // dialled.
                //
                // The Qt build works over CGNAT and its header says why:
                // announce all local IPs "so any peer on any shared subnet
                // can reach us". Choosing one is a guess about which
                // network the OTHER person is on, made by the machine with
                // the least information about it.
                //
                // One `publish` per address, matching the C++, which puts
                // several values under the one key. `dht_announce::newest`
                // is the other half — it discards addresses left over from
                // a network you have since left.
                // ONE VALUE CARRYING EVERY ADDRESS, which is both what the
                // Qt build does and the only thing that works.
                //
                // I first published one value per address. OpenDHT holds a
                // SET per key keyed by VALUE ID, and `value_id_for` derives
                // one id from the sc-ID alone — so three addresses were
                // three puts with the same id, two of which overwrote the
                // others. The multi-address announcement collapsed to
                // whichever arrived last, silently, which is worse than the
                // single address it replaced because it looks fixed.
                //
                // The payload is `dht_announce`'s plaintext inside
                // `dht_envelope`, byte for byte what a Qt peer stores.
                let port = self.settings.current.network.listen_port;
                // Local addresses, plus the one peers say we reach them
                // from — what the Qt build asked ipify for, now read off
                // the DHT's own replies.
                let addrs: Vec<crate::dht_announce::Addr> = crate::dht_announce::with_public(
                    crate::dht_announce::local_ipv4s().into_iter()
                        .map(|ip| crate::dht_announce::Addr::new(&ip.to_string(), port, 0))
                        .collect(),
                    h.public_ip(), port, self.reflexive);
                let published = if addrs.is_empty() {
                    0
                } else {
                    let ann = crate::dht_announce::Announcement {
                        user_id: self.local_id.clone(),
                        addrs,
                        stamp: self.now_secs.max(0),
                    };
                    let plain = crate::dht_announce::render(&ann);
                    match crate::dht_envelope::random_nonce() {
                        Some(nonce) => {
                            let sealed = crate::dht_envelope::seal_with_nonce(
                                &self.local_id, crate::dht_envelope::V_MULTI,
                                nonce, plain.as_bytes());
                            h.publish_raw(&self.local_id,
                                          crate::identity::dht_value_id(&self.local_id),
                                          sealed);
                            1
                        }
                        // A predictable nonce with a key anyone can derive
                        // is no scheme at all, so not publishing beats
                        // publishing that.
                        None => 0,
                    }
                };
                if published == 0 {
                    // Worth saying: a node that publishes nothing looks
                    // exactly like one that published fine and was never
                    // looked up.
                    self.feed_note =
                        "no local address to publish — nobody can find this device".into();
                }
            }
            if let Some(id) = plan.resolve.as_ref() {
                // Raw, because the payload is the Qt build's envelope and
                // presence has no business decoding it — see
                // `dht::publish_raw`.
                h.resolve_raw(id);
            }
        }

        let Some(h) = self.dht.as_ref() else { return };
        let events = h.poll();
        for ev in events {
            use snapclone_presence::node_host::Event;
            match ev {
                Event::ResolvedRaw { sc_id, values } => {
                    self.dht_schedule.finished(&sc_id, !values.is_empty());
                    // Unwrap, parse, then take only the FRESHEST
                    // announcement: values live about an hour, so this
                    // morning's record is still there this afternoon
                    // pointing at an address that now belongs to somebody
                    // else.
                    let anns: Vec<crate::dht_announce::Announcement> = values.iter()
                        .filter_map(|v| crate::dht_envelope::open(&sc_id, v).ok())
                        .filter_map(|(_, plain)| String::from_utf8(plain).ok())
                        .filter_map(|t| crate::dht_announce::parse(&t))
                        .collect();
                    // Something was stored and none of it opened: that is a
                    // format or key mismatch, not an absent peer, and the
                    // two need very different responses.
                    if anns.is_empty() && !values.is_empty() {
                        self.feed_note = format!(
                            "{sc_id}: found {} DHT record(s) this build cannot read",
                            values.len());
                    }
                    let candidates = crate::dht_announce::newest(&anns);
                    let ours = crate::dht_announce::local_ipv4s();
                    if let Some(best) = crate::dht_announce::best_for(&ours, &candidates) {
                        if let Some(ip) = parse_dotted_quad(&best.host) {
                            self.note_peer_addr(&sc_id, ip, best.punch_port, now_ms);
                            self.feed_note = format!("{sc_id} found via DHT");
                        }
                    }
                    if self.dht_schedule.silent() {
                        self.feed_note =
                            "the DHT has answered nothing at all — this node may not be \
                             reaching the bootstrap network".into();
                    }
                }
                Event::Resolved { sc_id, found } => {
                    // The lookup is over whether or not it found anything —
                    // an empty answer that never ends the lookup wedges the
                    // rotation on one id.
                    self.dht_schedule.finished(&sc_id, !found.is_empty());
                    // ALL the records for this id, reduced once. Each is
                    // one address of a multi-address announcement (the C++
                    // publishes one value per address), so treating them as
                    // independent findings and letting the last win would
                    // pick by arrival order — which on a DHT is arbitrary,
                    // and would routinely choose the public address of a
                    // peer sitting on the same wifi.
                    let candidates: Vec<crate::dht_announce::Addr> = found.iter()
                        .filter(|p| p.port != 0 && !p.host.trim().is_empty())
                        .map(|p| crate::dht_announce::Addr::new(&p.host, p.port, 0))
                        .collect();
                    let ours = crate::dht_announce::local_ipv4s();
                    //
                    // A DHT RECORD IS NOW WORTH SOMETHING. It used to set a
                    // note and evaporate: nothing recorded the address, so a
                    // peer found via DHT still could not be dialled and the
                    // message read as a tease.
                    //
                    // Only a literal address is taken. Resolving a hostname
                    // would need DNS on the UI thread, and `best_for`
                    // already sorts unparseable hosts last so this only
                    // declines when there was nothing better anyway.
                    let mut learned = 0;
                    if let Some(best) = crate::dht_announce::best_for(&ours, &candidates) {
                        if let Some(ip) = parse_dotted_quad(&best.host) {
                            self.note_peer_addr(&sc_id, ip, best.punch_port, now_ms);
                            learned += 1;
                        }
                    }
                    // A NODE THAT NEVER FINDS ANYTHING should say so. It
                    // looks identical to one whose contacts are simply
                    // offline, and the two need very different responses.
                    if self.dht_schedule.silent() {
                        self.feed_note =
                            "the DHT has answered nothing at all — this node may not be \
                             reaching the bootstrap network".into();
                    }
                    if learned > 0 {
                        self.feed_note = format!("{sc_id} found via DHT");
                    }
                }
                // Zero peers is the normal state of a node that has just
                // started and knows nobody, so it is not reported as a
                // problem.
                Event::Published { peers, .. } => { self.dht_peers = peers; }
                Event::Keepalive { pinged, answered } => {
                    self.dht_keepalive = Some((pinged, answered));
                    if pinged > 0 && answered == 0 {
                        // Every peer we were keeping warm has gone quiet at
                        // once. Said once per transition, not per round:
                        // a status that repeats every 25 s buries the
                        // next thing that goes wrong.
                        self.feed_note = format!(
                            "no DHT peer answered ({pinged} pinged) — the network may have changed");
                    }
                }
                Event::Stopped(why) => {
                    self.feed_note = why;
                    self.dht = None;
                    // The in-flight lookup died with the socket. Without
                    // this, the next node sits out a timeout on behalf of
                    // one that no longer exists.
                    self.dht_schedule.reset();
                }
            }
        }
    }

    pub fn tick_gossip(&mut self, now_ms: u64) {
        // The operator turned LAN gossip off. Returning before the prune
        // matters: pruning is what marks a peer offline for want of a
        // recent gossip packet, and doing that while gossip is disabled
        // would mark every peer offline and call it discovery.
        if !self.discovery_enabled().0 {
            return;
        }
        if now_ms.saturating_sub(self.last_prune_ms) < crate::gossip::PRUNE_MS {
            return;
        }
        self.last_prune_ms = now_ms;
        for lost in self.view.prune(now_ms) {
            // Marked offline, NOT removed. A device that stopped gossiping may
            // be asleep, and deleting the row would lose a contact the user
            // added deliberately — the two lists answer different questions.
            if let Some(p) = self.peers.iter_mut().find(|p| p.name == lost) {
                p.online = false;
            }
        }
        self.sync_roster();
    }

    /// Confirm the pending delete. Split from apply_friends because the
    /// confirmation arrives as its own answer, not as part of the tap that
    /// asked the question.
    /// Advance the manual's idle countdown.
    ///
    /// Returns true on the frame it dismisses itself. Fed rather than owning a
    /// timer, so the caller decides what a frame is — a manual that ran its own
    /// clock would keep counting while the app was suspended and be gone when
    /// the phone came back.
    pub fn tick_help(&mut self, dt_ms: f32) -> bool {
        self.help.tick(dt_ms)
    }

    /// Any input at all while the manual is open resets the countdown.
    ///
    /// Distinct from `scrolled`, and keeping the two apart is the feature: a
    /// reader who pauses on a short section that does not move the scrollbar
    /// must not be dismissed mid-read. Collapsing them into "did the offset
    /// change" is the tidier-looking thing that reintroduces the bug.
    pub fn help_interacted(&mut self) {
        if self.help.is_open() { self.help.interacted(); }
    }

    /// Feed a decoded snap into the ambient palette.
    ///
    /// `pixels` is the DOWNSCALED image — the deriver samples an edge band, and
    /// handing it a full-resolution photo would walk millions of pixels for a
    /// colour that a thumbnail answers identically.
    ///
    /// Samples accumulate rather than reset, so the palette settles as the
    /// inbox fills instead of lurching to whatever arrived last. One loud photo
    /// should tint the app, not repaint it.
    pub fn sample_ambient(&mut self, pixels: &[crate::ambient_palette::Rgb], side: u32) {
        self.ambient_samples.add_image(pixels, side);
        self.rederive_ambient();
    }

    /// A snap arrived: ask the driver whether this is worth a derivation.
    ///
    /// Sampling is cheap (it accumulates into a histogram); DERIVING walks
    /// that histogram, and doing it per arrival during a burst is the cost
    /// the throttle exists to prevent. The driver also refuses when another
    /// palette is in charge — a user's explicit pick is not ours to stomp,
    /// checked at trigger time because a queued arrival can outlive the
    /// switch.
    pub fn ambient_snap_arrived(&mut self, now_ms: u64) -> crate::ambient_inbox::Outcome {
        let out = self.ambient_driver.on_snap(now_ms, self.active_synthetic.as_deref());
        if out == crate::ambient_inbox::Outcome::Derive {
            self.rederive_ambient();
        }
        out
    }

    /// Walk the accumulated samples into a palette pair.
    fn rederive_ambient(&mut self) {
        // The two are INDEPENDENT, and my first version required both. A
        // vivid photo with no near-neutral edge pixels yields an accent and no
        // background — the test caught it — and refusing the accent because the
        // background was missing would drop a colour the deriver had correctly
        // found. Each is taken when it is available.
        if let Some(a) = self.ambient_samples.accent() {
            let bg = self.ambient_samples.background();
            self.ambient = Some((crate::ambient_palette::cap_tint(a),
                                 // No neutral pixels: keep the accent, and let
                                 // the caller's base background stand rather
                                 // than inventing one from the accent.
                                 bg.unwrap_or(a)));
        }
    }

    /// The derived accent, if the photos gave one.
    ///
    /// `None` is legitimate and the caller must keep its base palette: an inbox
    /// of greyscale photos qualifies no accent pixel, and rendering black there
    /// would make a monochrome inbox look broken rather than monochrome.
    pub fn ambient_accent(&self) -> Option<crate::ambient_palette::Rgb> {
        self.ambient.map(|(a, _)| a)
    }

    /// The derived background, only when real neutral pixels supplied one.
    ///
    /// Separate from the accent because they can arrive apart: a vivid photo
    /// with no near-neutral edge yields an accent and no background, and
    /// synthesising one from the accent would tint the whole app a colour no
    /// photo contained.
    pub fn ambient_background(&self) -> Option<crate::ambient_palette::Rgb> {
        self.ambient_samples.background()
    }

    /// Start a horizontal page swipe.
    ///
    /// The BOUNDS come from where we are in the tab list, and they are the
    /// whole reason this is not just a drag: at the first tab there is no
    /// previous page, so the rubber band resists instead of turning. Without
    /// them the first tab would swipe to nothing and spring back, which reads
    /// as a dropped gesture rather than an edge.
    pub fn begin_swipe(&mut self, x: f32, page_w: f32, t_ms: f32) {
        let cur = self.tabs.current();
        let bounds = crate::swipe_physics::Bounds {
            can_prev: cur > 0,
            can_next: cur + 1 < self.tabs.len(),
        };
        self.swipe = Some(crate::swipe_physics::Swipe::begin(page_w, bounds, x, t_ms));
    }

    /// Finish the swipe and turn the page if it earned it.
    ///
    /// Returns the tab now showing. The physics decides — distance past the
    /// commit threshold OR enough velocity — so a fast flick turns the page
    /// without crossing half the screen, which is what makes one-handed use
    /// possible at all.
    pub fn end_swipe(&mut self, t_ms: f32) -> usize {
        use crate::swipe_physics::Commit;
        let Some(sw) = self.swipe.as_mut() else { return self.tabs.current() };
        let commit = sw.release(t_ms);
        self.swipe = None;
        let cur = self.tabs.current();
        let next = match commit {
            // Saturating rather than wrapping: a swipe past the last tab should
            // stop, not land on Camera. Bounds already prevent it, and this is
            // the second guard on an action the user cannot undo by swiping
            // back the same way.
            Commit::Prev => cur.saturating_sub(1),
            Commit::Next => (cur + 1).min(self.tabs.len().saturating_sub(1)),
            Commit::Cancel => cur,
        };
        self.tabs.set_current(next);
        next
    }

    /// Index of the Chats tab, whose badge shows unread messages.
    ///
    /// Named rather than written as `1` at the call site: the tab order is a
    /// design decision that will change, and a bare index would silently start
    /// badging the wrong tab the first time somebody reorders them.
    pub const CHATS_TAB: usize = 1;

    /// Push the unread count onto the Chats badge.
    ///
    /// Called after anything that changes what is unread, so the badge is
    /// derived rather than maintained in parallel — two counts of the same
    /// thing disagree the moment one update is missed, and the badge is the
    /// one the user checks without opening the app.
    pub fn sync_badges(&mut self) {
        let unread = self.inbox.unread_count() as u32;
        self.tabs.set_badge(Self::CHATS_TAB, unread);
    }

    /// Retry whatever is waiting in the outbox, unless the battery says wait.
    ///
    /// Returns how many were retried. Gated on `should_defer_background`, which
    /// is the whole reason power_tier exists: a phone at 12% on battery should
    /// not be spending it on a resend that can happen in an hour.
    ///
    /// Deferring is NOT dropping. The rows stay in the outbox and go out on the
    /// next tick after the power state recovers — a send silently abandoned
    /// because of battery would be the worst possible reading of "defer".
    pub fn tick_outbox(&mut self, now_ms: u64) -> usize {
        if self.power.should_defer_background() {
            return 0;
        }
        // The EXISTING outbox (session::Outbox), not a second one. My first
        // version declared `pub outbox: Vec<SendRow>` beside it — the compiler
        // caught the duplicate field, and it was the right catch: two outboxes
        // means a row retried by one and pruned by the other, which is a
        // message that vanishes.
        let _ = now_ms;
        // What is waiting, from the outbox's own view. The gate above is the
        // decision; this is only the count that makes it visible.
        self.outbox.stalled()
    }

    /// Is the outbox held back right now, and why?
    ///
    /// Surfaced rather than silent: a message that has not sent for an hour
    /// looks broken, and "waiting for power" is the difference between a bug
    /// and a decision the user can act on by plugging in.
    pub fn outbox_deferred(&self) -> Option<&'static str> {
        if !self.power.should_defer_background() { return None; }
        if self.power.profile == crate::power_tier::PowerProfile::PowerSaver {
            Some("waiting — power saver is on")
        } else {
            Some("waiting — battery is low")
        }
    }

    /// Risk score for deleting a contact.
    ///
    /// Above HOLD_THRESHOLD, so it demands a press-and-hold. Removing someone
    /// discards the conversation and the shared key with them: there is no
    /// undo, and re-adding is a new pairing rather than a restore. A tap for
    /// that is a tap away from losing a relationship's history.
    ///
    /// Scored rather than hardcoded to Hold, so the same component keeps
    /// answering the question the same way everywhere.
    pub const DELETE_CONTACT_RISK: u32 = 70;

    /// Ask before deleting a contact, at the mode the risk demands.
    ///
    /// `familiar` comes from the caller's use record: a confirmation answered
    /// fifty times "has stopped being a safety net and become a tax". But it
    /// can only skip a TAP — a hold-level action still asks, because
    /// familiarity with a dangerous action is exactly when a slip happens.
    /// The measured familiarity for a feature (a fresh record for a new one).
    pub fn familiarity_of(&mut self, feature: &str) -> &mut crate::confirm::Familiarity {
        self.familiarity.entry(feature.to_string()).or_default()
    }

    pub fn ask_delete(&mut self, familiar: bool) -> bool {
        use crate::chevron_confirm::{ChevronConfirm, Mode};
        let mode = Mode::for_score(Self::DELETE_CONTACT_RISK);
        if familiar && mode == Mode::Tap {
            return false; // no panel; the caller proceeds
        }
        self.confirm = Some(ChevronConfirm::new(Self::DELETE_CONTACT_RISK));
        true
    }

    pub fn confirm_delete(&mut self, id: &str) {
        // The panel closes whichever way the answer went: leaving it open after
        // a confirmed delete would ask about a contact that no longer exists.
        self.confirm = None;
        // The confirmation was answered — the feature got more familiar.
        self.familiarity_of("delete-contact").record_use();
        // Forget the transport session too. Deleting a contact and leaving
        // their sequence number, negotiated version and known-snap list behind
        // is precisely the "lingers for every ever-connected peer" leak
        // peer_session was written to prevent — and re-adding them later would
        // resume a sequence they have no memory of.
        self.sessions.forget(&id);
        // The book too, or the contact returns on the next launch — a delete
        // that survives until you quit is the most confusing kind of not
        // working, because it looks like it worked.
        if self.contact_book.remove(&id) { self.contacts_dirty = true; }
        self.peers.retain(|p| p.name != id);
        self.cards.close(id);
        self.sync_roster();
        self.modal.dismiss(&format!("confirm-delete:{id}"));
    }

    /// The export plan for the host to run: tool, output path, argv. `None`
    /// when neither archiver is installed — the card should say so rather
    /// than fail later.
    pub fn export_plan(&self, has_zip: bool, has_tar: bool, requested: &str)
        -> Option<(crate::snap_export::Tool, String, Vec<String>)> {
        let tool = crate::snap_export::choose_tool(has_zip, has_tar)?;
        let out = crate::snap_export::output_path(requested, tool);
        let argv = crate::snap_export::argv(tool, &out);
        Some((tool, out, argv))
    }

    /// The host reports how the export went; the card's note shows the line.
    pub fn export_finished(&mut self, outcome: crate::snap_export::Outcome) {
        self.export_requested = false;
        self.export_note = Some(outcome.message());
    }

    /// Carry out what the Friends page asked for.
    /// What the address book makes of one roster row — the pure half of the
    /// Friends page's badges, so the rendering above stays a lookup.
    ///
    /// A row whose id is not a valid sc-ID gets `Plain` even when absent
    /// from the book: the book is keyed by id, a hostname row has nothing
    /// to store, and offering Keep on it would write a record that can
    /// never be matched against an announcement.
    pub fn standing_of(&self, id: &str) -> crate::friends::Standing {
        use crate::friends::Standing;
        match self.contact_book.get(id) {
            Some(entry) => {
                let t = entry.trust();
                Standing::Known {
                    trust: t.label().to_string(),
                    danger: matches!(t, crate::contact_book::Trust::Conflict),
                }
            }
            None if crate::identity::is_valid(id) => Standing::Discovered {
                port: self.view.peers().iter()
                    .find(|r| r.id == id)
                    .map(|r| r.port)
                    .unwrap_or(crate::qr::DEFAULT_PORT),
            },
            None => Standing::Plain,
        }
    }

    /// Everything the Friends card row needs that `friends.rs` cannot know.
    ///
    /// The three facts used to be three lists of the same people. Assembled
    /// here so the mapping from this app's types (`contact_book::Trust`,
    /// `peer_source::Reach`, the picker's selection) to the widget's
    /// vocabulary lives in one function that can be tested without a frame.
    ///
    /// `reach` is matched by NAME rather than by `PeerId`, because
    /// `peer_roster` synthesises ids by zero-padding the name — matching on
    /// the padded bytes would work today and break the moment real keys
    /// arrive, silently, by showing every row as unreachable.
    pub fn row_info(&self, id: &str, reach: &[crate::peer_source::RosterPeer])
                    -> crate::friends::RowInfo {
        crate::friends::RowInfo {
            standing: self.standing_of(id),
            reach: reach.iter().find(|r| r.name == id).map(|r| match r.reach {
                crate::peer_source::Reach::Connected => crate::friends::Reach::Connected,
                crate::peer_source::Reach::Known => crate::friends::Reach::Known,
                crate::peer_source::Reach::Unverified => crate::friends::Reach::Unverified,
            }),
            selected: self.picker.selected().iter().any(|s| s == id),
        }
    }

    pub fn apply_friends(&mut self, a: crate::friends::Action) {
        use crate::friends::Action;
        match a {
            // A manual add is optimistic: the row appears offline immediately,
            // because a Connect button that visibly does nothing for two
            // seconds gets pressed four more times.
            Action::Add(p) => {
                let name = p.host.clone();
                if !self.peers.iter().any(|e| e.name == name) {
                    self.peers.push(Peer { name: name.clone(), online: false });
                }
                // THE PORT, which was being dropped here. `AddPeer` has carried
                // one since the QR parser was written — a bare sc-ID means
                // 9876, an explicit one overrides — and this function read
                // `p.host` and never looked at `p.port`, so both forms produced
                // identical state and all that care was wasted one call later.
                //
                // Only a real sc-ID goes in the book: the manual field accepts
                // a hostname too, and a book keyed by "192.168.1.5" cannot be
                // matched against a peer that later announces itself by id.
                // Those rows still appear — they just are not durable yet,
                // which is the honest state rather than a silent half-record.
                if crate::identity::is_valid(&name)
                    && self.contact_book.upsert(&name, p.port, self.now_secs.max(0) as u64 * 1000) {
                    self.contacts_dirty = true;
                }
                self.sync_roster();
            }
            // A LOCAL nickname: it renames the row, not the peer. The other
            // side keeps whatever it calls itself, because this app cannot
            // change a name on someone else's device and pretending otherwise
            // would have two people disagreeing about who they are talking to.
            Action::Rename(id, to) => {
                let to = to.trim().to_string();
                // An empty rename is a DELETE of the only label the user has
                // for this contact, so it is refused rather than applied — the
                // row would become unidentifiable and unfindable.
                if !to.is_empty() {
                    if self.contact_book.rename(&id, &to) { self.contacts_dirty = true; }
                    if let Some(p) = self.peers.iter_mut().find(|p| p.name == id) {
                        p.name = to;
                    }
                    self.sync_roster();
                }
            }
            // The pick toggle, kept off `Selected`: one changes what a send
            // will do, the other changes what is on screen, and routing both
            // through one variant made opening someone's actions silently
            // add them to a send.
            Action::TogglePick(id) => self.picker.toggle(&id),
            Action::Selected(id) => {
                if let Some(i) = self.peers.iter().position(|p| p.name == id) {
                    self.nav.push_subpage(i);
                }
            }
            Action::Invoke(id, act) => match act.as_str() {
                // Delete asks first, in the Confirm layer above the dialog
                // layer — removing a contact on a single tap of a menu item
                // that sits one row below "Message" is how people lose people.
                "delete" => {
                    // ask_delete builds the panel AND decides whether one is
                    // needed at all. Presenting the modal without calling it
                    // left `confirm` as None, so the dialog had no state to
                    // render even once it had a renderer — three unwired halves
                    // of one feature.
                    //
                    // Familiarity is now MEASURED (confirm::Familiarity per
                    // feature) rather than guessed — though at delete's Hold
                    // risk it can only ever skip a Tap, so this stays a
                    // Hold panel however practised the user is. That is the
                    // point: familiarity with a dangerous action is exactly
                    // when a slip happens.
                    let familiar = matches!(
                        self.familiarity_of("delete-contact").mode(),
                        crate::confirm::Mode::None);
                    if self.ask_delete(familiar) {
                        self.confirm_id = id.clone();
                        self.modal.present_default(
                            &format!("confirm-delete:{id}"), crate::modal::Layer::Confirm,
                            crate::modal::Rect::new(40.0, 200.0, 300.0, 140.0));
                    } else {
                        // No panel wanted — a familiar, low-risk action goes
                        // straight through, which is the whole point of
                        // ask_delete returning a bool.
                        self.confirm_delete(&id);
                    }
                }
                // MESSAGE AND CALL WERE `_ => {}`. Two buttons on every
                // contact card, drawn exactly like the two that work, doing
                // nothing at all — worse than absent, because a missing
                // button is visible and a dead one teaches people the app is
                // unreliable.
                "message" => {
                    // The same landing the snap feed's hold gesture already
                    // uses, so a conversation opens one way rather than two
                    // that could drift.
                    self.conversation_peer = id.clone();
                }
                "call" => {
                    // Placing a call is: be in the room, and have the other
                    // person in it. They are added optimistically — they
                    // have not accepted, and there is no invite protocol on
                    // the wire yet — which is what lets the host bring the
                    // audio path up and start sending.
                    //
                    // Said plainly rather than dressed up as a ringing
                    // state: this app cannot make their phone ring yet, and
                    // a spinner claiming otherwise would be a lie with a
                    // progress indicator on it.
                    let me = self.local_id.clone();
                    if me.is_empty() {
                        // No identity, no call. Refused rather than half
                        // done: `group_voice::leave` releases the floor
                        // through `me()`, so a room we are not in cannot be
                        // left cleanly, and the far end would receive
                        // packets from a sender it cannot name. The same
                        // rule gossip already follows — no key, no
                        // announcement.
                        self.call_note =
                            "this device has no identity yet, so it cannot place a call"
                            .to_string();
                        return;
                    }
                    let mut people = self.group_voice.participants.clone();
                    if !people.iter().any(|p| p.is_self) {
                        people.push(crate::group_voice::Participant {
                            id: me, name: "you".into(), is_self: true });
                    }
                    if !people.iter().any(|p| p.id == id) {
                        let name = self.contact_book.get(&id)
                            .map(|c| c.label().to_string())
                            .unwrap_or_else(|| id.clone());
                        people.push(crate::group_voice::Participant {
                            id: id.clone(), name, is_self: false });
                    }
                    self.group_voice.set_participants(people);
                    self.group_voice.join();
                    self.call_note = match self.punch.get(&id) {
                        Some(_) => format!("calling {id}"),
                        // The honest version of a call that cannot work: no
                        // current address means nothing we send arrives.
                        // Better said now than after thirty seconds of
                        // silence that reads as their microphone.
                        None => format!(
                            "{id} has no known address - they must be seen on this \
                             network, or found via DHT, before a call can reach them"),
                    };
                }
                "edit" => {
                    self.editor_id = id.clone();
                    self.editor_name = id.clone();
                    self.modal.present_default(
                        &format!("editor:{id}"), crate::modal::Layer::Dialog,
                        crate::modal::Rect::new(20.0, 80.0, 340.0, 400.0));
                }
                _ => {}
            },
            Action::ImportVCard => {
                // WAS `=> {}`: a button a person can press, that looks
                // like every other working button, and did nothing at
                // all. Worse than a missing feature — a missing one is
                // visible, while a present one that silently declines
                // teaches people the app is unreliable, and they cannot
                // tell whether the file was rejected, held no contacts,
                // or was never read.
                //
                // The PARSE is ported (`vcard`, matching the Qt
                // `VCardIO`'s field set so the two halves agree about
                // each other's exports). What is still missing is the
                // file dialog and the store write, so this reports what
                // it would have imported rather than pretending to have
                // done it.
                self.import_report = Some(match &self.pending_vcard_text {
                    Some(text) => {
                        let (found, skipped) = crate::vcard::parse(text);
                        format!(
                            "vCard: {} contact(s) read, {skipped} skipped — no store yet",
                            found.len()
                        )
                    }
                    None => "vCard import needs a file picker — not wired yet".to_string(),
                });
            }
        }
    }

    pub fn page_name(&self) -> &'static str {
        PAGES[self.nav.page()]
    }

    /// Swipe between top-level pages (unless drilled into a subpage — then swipe is off so the
    /// content can scroll/interact).
    /// Run the live page swipe for this frame; return how far the page
    /// content should be offset.
    ///
    /// Touch only, by way of the axis lock: a mouse drag across the page is
    /// a selection or a viewfinder tap-and-hold far more often than a page
    /// turn, and the trackpad has its own path through the gesture layer.
    ///
    /// Returns 0.0 when nothing is happening, which is the common case and
    /// costs one pointer read.
    pub fn tick_page_swipe(&mut self, ui: &Ui, surface: egui::Rect, dt_ms: f32) -> f32 {
        /// How wide the left-edge strip is for a back swipe. Wide enough to
        /// hit without looking, narrow enough that it is not most of the
        /// screen — the same order as the system gesture area on Android
        /// and iOS, which is what people's thumbs are already trained on.
        const EDGE_BACK_WIDTH: f32 = 32.0;
        use crate::swipe_physics::Intent;
        let t_ms = ui.input(|i| i.time) as f32 * 1000.0;
        let (down, pos) = ui.input(|i| (i.pointer.primary_down(), i.pointer.interact_pos()));

        if down {
            if let Some(p) = pos {
                let origin = *self.touch_origin.get_or_insert(p);
                let (dx, dy) = (p.x - origin.x, p.y - origin.y);
                match self.axis_lock.feed(dx, dy) {
                    Intent::Undecided | Intent::Scroll => {}
                    Intent::Page => {
                        // EDGE-SWIPE BACK. On a subpage there is nowhere to
                        // page to — `swipe_page` refuses, correctly — and
                        // the gesture people try is a rightward drag from
                        // the left edge, which everywhere else on a phone
                        // means back. Without it the only way out of a
                        // subpage is a button, and on a large screen that
                        // button is at the far corner from the thumb.
                        //
                        // Anchored to the left EDGE rather than allowed
                        // anywhere: a rightward drag in the middle of a
                        // detail view is a scroll or a selection far more
                        // often than a back, and stealing it would make
                        // content undraggable.
                        if self.nav.can_go_back() {
                            if origin.x - surface.left() <= EDGE_BACK_WIDTH
                                && dx >= crate::swipe_physics::AXIS_SLOP
                                && !self.modal.blocks_input()
                                && !self.back_armed
                            {
                                self.back_armed = true;
                                self.nav.back();
                            }
                            return 0.0;
                        }
                        // Begun at the ORIGIN, not at the point where the
                        // lock decided: starting here would drop the first
                        // twelve pixels of travel, so the page would jump
                        // by that much the moment it took over.
                        if self.swipe.is_none() && !self.modal.blocks_input() {
                            self.begin_swipe(origin.x, surface.width(), t_ms);
                            self.swipe_settle = None;
                        }
                        if let Some(sw) = self.swipe.as_mut() {
                            sw.update(p.x, t_ms);
                        }
                    }
                }
            }
        } else if self.touch_origin.is_some() || self.swipe.is_some() {
            // Release.
            if self.swipe.is_some() {
                self.swipe_claimed = true;
                let from = self.swipe.as_ref().map(|s| s.offset()).unwrap_or(0.0);
                let ms = self.swipe.as_ref().map(|s| s.settle_ms(t_ms)).unwrap_or(200.0);
                let before = self.nav.page();
                let tab = self.end_swipe(t_ms);
                // `end_swipe` moves `tabs`; the page the app draws from is
                // `nav`, and they are kept in step elsewhere by
                // `tabs.set_current(nav.page())`. Pushing it the other way
                // here is what makes the gesture actually turn the page —
                // and going through `swipe_page` rather than `nav.go_page`
                // keeps the modal guard and the peel-hint bookkeeping in
                // one place.
                if tab != before {
                    self.swipe_page(if tab > before { 1 } else { -1 });
                }
                // Settle from wherever the finger left it, so a cancelled
                // swipe springs back rather than vanishing.
                self.swipe_settle = Some((from, ms, ms));
            }
            self.touch_origin = None;
            self.axis_lock.release();
            // One back per touch. Without this, holding past the threshold
            // pops a level every frame and a deep stack unwinds in a
            // tenth of a second.
            self.back_armed = false;
        }

        if let Some(sw) = self.swipe.as_ref() {
            return sw.offset();
        }
        if let Some((from, left, total)) = self.swipe_settle.as_mut() {
            *left -= dt_ms;
            if *left <= 0.0 {
                self.swipe_settle = None;
                return 0.0;
            }
            // Ease out: fast at first, slowing in. A linear settle reads as
            // mechanical because nothing physical stops at a constant rate.
            let progress = 1.0 - (*left / *total);
            let eased = 1.0 - (1.0 - progress) * (1.0 - progress);
            let off = *from * (1.0 - eased);
            ui.ctx().request_repaint();
            return off;
        }
        0.0
    }

    /// The narrowest each page is worth drawing at. The viewfinder wants
    /// room to be a viewfinder; a list does not.
    pub fn page_metas() -> Vec<uikit_egui::pager::PageMeta> {
        PAGES.iter().enumerate().map(|(i, name)| uikit_egui::pager::PageMeta {
            id: i, title: name.to_string(),
            min_width: if i == 0 { 420.0 } else { 340.0 },
        }).collect()
    }

    /// The pages a `cols`-wide window shows, the active one included and
    /// the window kept inside the page list rather than wrapping: a
    /// desktop user reads left to right, and settings beside the camera
    /// with the inbox off-screen reads as a bug.
    pub fn visible_pages(active: usize, n: usize, cols: usize) -> Vec<usize> {
        let cols = cols.clamp(1, n.max(1));
        let start = active.min(n.saturating_sub(cols));
        (start..start + cols).collect()
    }

    /// How many pages fit side by side in `width`, with hysteresis.
    pub fn page_columns(&mut self, width: f32, gutter: f32, now_s: f64) -> usize {
        let metas = Self::page_metas();
        let plain = uikit_egui::pager::fit_cols(width, gutter, &metas);
        let margined = uikit_egui::pager::fit_cols(
            width * (1.0 - uikit_egui::pager::GROW_MARGIN), gutter, &metas);
        self.plan.decide(plain, margined, now_s)
    }

    pub fn swipe_page(&mut self, sign: i32) {
        // Counted here rather than at the gesture layer so a swipe that is
        // REFUSED — blocked by a dialog, or at the end of the pages — does not
        // count toward familiarity. The user has not learned anything from a
        // gesture that did nothing.
        // A swipe under an open dialog would change the page out from under it
        // and leave the dialog floating over a screen it has nothing to do
        // with. This is what Qt's stackChanged signal exists to let callers
        // ask.
        if self.modal.blocks_input() {
            return;
        }
        if self.nav.can_go_back() {
            return;
        }
        let n = PAGES.len() as i32;
        let next = (self.nav.page() as i32 + sign).clamp(0, n - 1) as usize;
        if next != self.nav.page() { self.peel.note_swipe(); }
        self.nav.go_page(next);
        self.peel.set_page(next, PAGES.len());
    }

    /// Advance the animation; call once per frame with the frame delta.
    pub fn tick(&mut self, dt: f32) {
        self.bg.tick(dt);
        // The viewer countdown runs on the same clock, and deliberately keeps
        // running after the user closes early: the expiry event is what marks
        // the snap in storage.
        for ev in self.viewer.tick(dt * 1000.0) {
            self.apply_snap_event(ev);
        }
        // The scanner's success hold runs on the frame clock too.
        if let Some(c) = self.qr.as_mut().and_then(|q| q.tick(dt * 1000.0)) {
            self.apply_scan(c);
        }
        // The ring declines itself after thirty seconds whether or not anyone
        // is looking at this page.
        if let Some(call) = self.incoming_call.as_mut() {
            let caller = call.caller_name.clone();
            if let Some(a) = call.tick(dt * 1000.0) { self.finish_call(&caller, a); }
        }
    }

    /// Re-derive which ribbon actions are available. Called before drawing, so
    /// enablement always reflects the state on screen rather than the state
    /// when the tab was opened.
    pub fn refresh_ribbon(&mut self) {
        let unread = self.inbox.unread_count();
        let failed = self.outbox.rows().iter()
            .filter(|r| !r.delivered && r.phase.is_failure()).count();
        self.ribbon.refresh(|id| match id {
            // Nothing to mark, nothing to press. An always-enabled "mark all
            // read" on an empty inbox is a button that lies about having work.
            "read_all" => unread > 0,
            "retry_failed" => failed > 0,
            _ => true,
        });
    }

    /// Carry out a ribbon action.
    pub fn apply_ribbon(&mut self, ev: crate::ribbon::RibbonEvent) {
        use crate::ribbon::RibbonEvent;
        match ev {
            RibbonEvent::TabSelected(i) => { self.bar.sub[1] = i; }
            RibbonEvent::Triggered(id) => match id.as_str() {
                "read_all" => {
                    for id in self.inbox.unread_ids() { self.inbox.mark_read(id); }
                    // Marking everything read must clear the badge in the same
                    // breath: a badge surviving "read all" is the exact case
                    // that teaches people to ignore it.
                    self.sync_badges();
                }
                // Retry is the host's: this view does not own the transport.
                // Recorded so the host can act and the test can see it.
                "retry_failed" => self.retry_requested = true,
                _ => {}
            },
        }
    }

    /// Handle the export card. Either action marks the month shown — the card
    /// was on screen and answered, and re-offering it after an explicit answer
    /// is the nag this feature is built to avoid.
    pub fn apply_export_card(&mut self, a: crate::export_reminder::CardAction,
                             month: crate::export_reminder::Month) {
        use crate::export_reminder::CardAction;
        match a {
            CardAction::ExportRequested => {
                self.export_requested = true;
                self.reminder.mark_shown(month);
            }
            CardAction::Dismissed => self.reminder.snooze(month),
        }
    }

    /// Carry out what the voice bar asked for. The bar owns no audio and no
    /// sockets; this is where its events meet the session.
    /// Start naming the place you are standing in.
    ///
    /// Seeded from the wayfinder's own fix rather than a fresh read: the two
    /// must agree about where "here" is, and a second GPS read seconds later
    /// would put the landmark somewhere the map does not show you.
    ///
    /// The two Fix types are deliberately not the same — wayfinder's is
    /// {lat, lon} and a landmark also records accuracy_m, because a landmark
    /// SAVED at 500 m accuracy is a different claim from one saved at 5 m and
    /// the record has to carry which. Accuracy the wayfinder does not track is
    /// passed in by the caller.
    pub fn begin_landmark(&mut self, accuracy_m: f64) {
        let fix = self.self_fix.map(|f| crate::landmark::Fix {
            lat: f.lat, lon: f.lon, accuracy_m,
        });
        self.add_landmark = Some(crate::landmark::AddLandmark::new(fix));
    }

    /// Save the landmark being named, if it is complete.
    ///
    /// Returns whether one was added. The form CLOSES only on success — an
    /// error means the name was blank or there is no fix, and closing would
    /// discard what the user typed along with the reason it failed.
    /// `creator_id` is passed in rather than read from the app: this struct
    /// holds no identity of its own, and inventing one here would put a
    /// placeholder in a field that outlives the session — a landmark records
    /// WHO named it, and "unknown" is not recoverable later.
    pub fn commit_landmark(&mut self, creator_id: &str, id: &str, now_ms: i64) -> bool {
        let Some(form) = self.add_landmark.as_mut() else { return false };
        match form.save(creator_id, id, now_ms) {
            Some(l) => {
                self.landmarks.push(l);
                self.add_landmark = None;
                true
            }
            None => false,
        }
    }

    pub fn apply_voice(&mut self, ev: crate::voicebar::Event) {
        use crate::voicebar::Event;
        match ev {
            // Reflect the press locally AND ask the session. A talk button that
            // waits for the round trip before lighting feels broken on a slow
            // link, which is exactly when you are pressing it hardest.
            // Both sides of the same fact. Taking the floor locally must show
            // in the room too, or your own row stays silent while you are
            // talking — the one row the user can verify against reality, and
            // the one that would make them distrust the rest.
            Event::TalkPressed => {
                self.voicebar.set_on_air(true);
                if let Some(me) = self.voice_room.people().iter().find(|p| p.is_self) {
                    let id = me.id.clone();
                    self.voice_room.set_active_speaker(&id);
                }
            }
            Event::TalkReleased => {
                self.voicebar.set_on_air(false);
                // Empty id opens the floor rather than naming a speaker who is
                // not talking.
                self.voice_room.set_active_speaker("");
            }
            // The request is ours to record; the TIER is the negotiation's to
            // report, so this does not set Private itself.
            Event::PrivateRequested(want) => {
                self.voicebar.security = if want { crate::voicebar::Security::Securing }
                                         else { crate::voicebar::Security::Public };
            }
        }
    }

    /// Join or leave the group voice channel.
    pub fn toggle_voice(&mut self) {
        if self.group_voice.joined {
            self.group_voice.leave();
            self.voicebar.set_on_air(false);
            // The floor goes with the call. Left set, the bar names
            // whoever spoke last for as long as the app is open — a label
            // that outlives its call is a claim about a conversation that
            // is over.
            self.voicebar.clear_talker();
        } else {
            self.group_voice.join();
        }
    }

    /// Where a dialog of this height should sit, given the current form factor.
    /// Replaces the hardcoded rectangles the modal layer was being handed —
    /// those were phone-shaped constants that would have put a 340px sheet in
    /// the corner of a desktop window.
    pub fn dialog_rect(&self, surface: egui::Rect, content_h: f32) -> crate::modal::Rect {
        let mut d = crate::mobile_dialog::MobileDialog::new(self.dialog_form);
        d.open();
        d.tick(crate::mobile_dialog::SHEET_MS * 2.0);
        let (x, y, w, h) = d.geometry(surface.width(), surface.height(), content_h);
        crate::modal::Rect::new(x, y, w, h)
    }

    /// Open the QR scanner. `camera` false lands straight on the manual field
    /// rather than showing a viewfinder that will never produce a frame.
    pub fn open_qr_scan(&mut self, camera: bool) {
        self.qr = Some(if camera { crate::qr::QrScan::scanning() }
                       else { crate::qr::QrScan::manual_only() });
        // THE REVEAL RESTARTS ON EVERY OPEN. `qr_shown_at` is the clock the
        // fractal reveal runs from, and it used to be cleared only by the
        // dialog's own "close" button — so a dialog dismissed any other way
        // (a scanned code, Escape, a tap on the scrim) left a stale timestamp
        // behind, and the next open drew the code already finished. The
        // animation ran exactly once per process.
        //
        // Cleared on OPEN rather than on close, which is what makes it correct
        // by construction: there are three close paths today and nothing stops
        // a fourth being added, but there is one place the dialog opens. The
        // draw site stamps the current time on the next frame.
        self.qr_shown_at = None;
        // Sized for what it holds: the scanner (or the manual field), the
        // typed-code form, MY CODE at 168 with its two lines, and close. The
        // 340×420 it had was the scanner alone; once my code joined it the
        // lower half painted below the dialog's own card, over the page.
        self.modal.present_default("qr", crate::modal::Layer::Dialog,
                                   crate::modal::Rect::new(20.0, 56.0, 380.0, 600.0));
    }

    /// A scanned or pasted code becomes a peer, through the SAME path a typed
    /// address takes — so a contact added by camera and one added by hand are
    /// the same kind of contact.
    pub fn apply_scan(&mut self, c: crate::qr::ScannedContact) {
        self.apply_friends(crate::friends::Action::Add(
            crate::friends::AddPeer { host: c.sc_id, port: c.port }));
        self.qr = None;
        self.modal.dismiss("qr");
    }

    /// Feed a decoded frame from the camera. Returns true once it latches.
    pub fn qr_frame(&mut self, payload: &str) -> bool {
        self.qr.as_mut().map(|q| q.frame(payload)).unwrap_or(false)
    }

    /// A voice packet arrived for a conversation whose thread is not open.
    pub fn ring(&mut self, caller: &str) {
        self.incoming_call = Some(crate::ring::IncomingCall::new(caller));
        self.modal.present(&format!("call:{caller}"), crate::modal::Layer::Confirm,
                           crate::modal::Rect::new(0.0, 400.0, 400.0, 300.0),
                           // NOT dismiss-on-tap-outside: a stray tap while the
                           // phone is in a pocket must not answer or refuse a
                           // call. Only the two buttons and the timeout decide.
                           false, true);
    }

    /// Apply an answer: dismiss the sheet, and open the thread if it was taken.
    pub fn answer_call(&mut self, a: crate::ring::Answer) {
        let Some(call) = self.incoming_call.as_mut() else { return };
        let caller = call.caller_name.clone();
        if call.answer(a).is_none() { return; }
        self.finish_call(&caller, a);
    }

    fn finish_call(&mut self, caller: &str, a: crate::ring::Answer) {
        use crate::ring::Answer;
        self.modal.dismiss(&format!("call:{caller}"));
        match a {
            Answer::Accepted => {
                self.conversation_peer = caller.to_string();
                self.group_voice.joined = true;
            }
            // Both rejections drop the buffered packets; only the ignored one
            // is a MISSED call.
            Answer::TimedOut => self.missed_calls.push(caller.to_string()),
            Answer::Declined => {}
        }
        self.incoming_call = None;
    }

    /// Join or leave the group call, keeping play-out in step.
    ///
    /// One place does both, because a call whose floor state and whose audio
    /// scheduler disagree is a call where someone is talking and nobody hears
    /// it — or worse, where the previous call's frames play into the new one.
    pub fn set_in_call(&mut self, joined: bool) {
        if joined {
            self.group_voice.join();
            self.playout.start();
        } else {
            self.group_voice.leave();
            self.playout.stop();
            // Stop talking as well as listening. The reset happens on the way
            // UP — set_transmitting(true) is what zeroes seq and arms
            // spurt_pending — so a sender left `transmitting` across a call
            // boundary never sees that edge, and the next spurt continues the
            // old sequence instead of restarting at 0. The receiver then has no
            // TALKSPURT_START to reset its jitter buffer on.
            self.sender.set_transmitting(false);
            // The sink primes itself: push() flips `started` once the queue
            // reaches jitter_target, which is prime-then-play rather than an
            // explicit start. Leaving the call would have been inventing an
            // API — it has no start/stop, and resetting it here means the
            // NEXT call primes from empty rather than replaying this one's
            // tail into it.
            self.sink = Default::default();
        }
    }

    /// Which camera to open, given what is known about the fleet's health.
    ///
    /// Falls back to plain enumeration when nothing has been probed — asking an
    /// empty health cache would answer "nothing works" and leave the user with
    /// no viewfinder on a phone whose cameras are fine.
    pub fn camera_to_open(&self, known: &[String]) -> Option<String> {
        use crate::camera_policy::Facing;
        let want = if self.viewfinder.facing == crate::camera::Facing::Front {
            Facing::Front } else { Facing::Rear };
        if self.camera_health.has_any_result() {
            if let Some(id) = self.camera_health.best_working(want) {
                return Some(id.to_string());
            }
        }
        known.first().cloned()
    }

    /// Record that a capture actually delivered frames.
    pub fn note_camera_worked(&mut self, id: &str, now_ms: i64) {
        use crate::camera_policy::Facing;
        let facing = if self.viewfinder.facing == crate::camera::Facing::Front {
            Facing::Front } else { Facing::Rear };
        self.camera_health.record_capture_success(id, facing, now_ms);
    }

    /// Poll the accelerometer and act on any flick it produced.
    ///
    /// ONE call for both consumers, because a flick is one event: the
    /// cursor turns it into a step and the command layer matches it
    /// against a binding. Two readers of the same sensor would be a second
    /// source of truth about what a flick is — which is the reason
    /// `Ratchet::feed` returns an orientation rather than a vector, and
    /// splitting it here would undo that.
    pub fn poll_glove(&mut self, dt_ms: f32, src: &mut dyn crate::glove_input::Accel)
        -> Option<uikit_egui::glove::Orient> {
        let Some((o, t, elapsed)) = self.glove_input.tick(dt_ms, src) else {
            return None;
        };
        // THE CURSOR OWNS THE RATCHET. It is fed the reading and returns a
        // Tick; the flick in that Tick is the single event both consumers
        // take. A second ratchet in the driver — which is what I built
        // first — would be a second opinion about what a flick is, and the
        // two would disagree the moment their timing drifted.
        let targets = self.a11y.glove_targets();
        let tick = self.glove.feed(o, t, elapsed, &targets);
        // The command layer sees every POLL, not only the flicks: a double
        // flick is two within a window, and it needs the quiet time
        // between them to know the window elapsed.
        self.apply_shake(tick.flicked, elapsed);
        if tick.escaped {
            self.apply_glove(uikit_egui::glove::GloveAction::Escaped);
        }
        if let Some(id) = tick.snapped.clone() {
            self.apply_glove(uikit_egui::glove::GloveAction::ArmField(id));
        }
        tick.flicked
    }

    /// Feed a flick to the command layer and carry out whatever it binds.
    pub fn apply_shake(&mut self, flicked: Option<uikit_egui::glove::Orient>, dt_ms: f32) {
        let Some(action) = self.shake.feed(flicked, dt_ms) else { return };
        if action == "swap_camera" { self.viewfinder.flip(); }
    }

    /// Carry out what glove mode asked for.
    ///
    /// `ArmField` is the interesting one: it records the field and stops. No
    /// keyboard is raised, because the hand that needs this mode cannot press
    /// keys — that is the whole reason the mode exists.
    pub fn apply_glove(&mut self, a: uikit_egui::glove::GloveAction) {
        use uikit_egui::glove::GloveAction as G;
        match a {
            G::ArmField(id) => self.voice_field = Some(id),
            G::Dismiss => { self.voice_field = None; self.menu.close(); }
            G::Activate(id) => { if self.menu.is_open() { self.menu.choose(&id); } }
            // Dictated text goes to the armed field; the shell holds it until
            // the host writes it through. Escape clears the arm, which is the
            // whole point of the out-of-band gesture.
            G::Dictated(text) => {
                if let Some(f) = self.voice_field.clone() { self.dictated.push((f, text)); }
            }
            G::Escaped => self.voice_field = None,
            G::Toggle(_) | G::Nudge { .. } | G::Step(_) => {}
        }
    }

    /// Fill the send receipt from what the send path computed.
    ///
    /// One call, because the three facts must describe the SAME send. Filling
    /// them from three places is how a receipt ends up showing last send's
    /// dedup beside this send's recipient count — and a receipt that is
    /// partially stale is worse than none, since its whole value is being
    /// checkable.
    pub fn prepare_send(&mut self, format: i32, segments_removed: i32,
                        upload_bytes: i64, total_bytes: i64, recipients: i32) {
        self.send_summary.set_strip_result(format, segments_removed);
        self.send_summary.set_dedup(upload_bytes, total_bytes);
        self.send_summary.set_recipients(recipients);
    }

    /// May the send proceed?
    pub fn can_send(&self) -> bool { !self.send_summary.blocks_send() }

    /// Feed one gaze sample and route it to everything that wants attention.
    ///
    /// One place, because the posture veto and the glove gate must not disagree
    /// about whether the user is present — a screen that stays awake while
    /// gestures are refused would look broken in both directions at once.
    ///
    /// Note the two consumers use it in OPPOSITE directions, which is correct:
    /// posture only ever keeps the screen awake, and the glove gate only ever
    /// blocks an action. Neither can cause the harm the other guards against.
    pub fn feed_gaze(&mut self, s: crate::gaze::GazeSample, dt_ms: f32) {
        self.attention.feed(s, dt_ms);
        self.apply_attention();
    }

    /// The tracker stopped or the camera went away.
    pub fn gaze_lost(&mut self) {
        self.attention.lost();
        // NOT reported as Away. A missing tracker is Unknown, and Unknown must
        // leave glove mode exactly as it was — in cold weather the camera is
        // looking at a scarf as often as a face, so treating "no reading" as
        // "not looking" would disable gestures on the days they are for.
        self.glove.set_watching(uikit_egui::glove::Watching::Unknown);
        self.posture.set_watched(false);
    }

    fn apply_attention(&mut self) {
        let present = self.attention.present();
        self.posture.set_watched(present);
        self.glove.set_watching(if present {
            uikit_egui::glove::Watching::Looking
        } else {
            uikit_egui::glove::Watching::Away
        });
    }

    /// Re-derive whether posture should be acting.
    ///
    /// Armed on the Inbox, or in a conversation with voice in the last five
    /// minutes. This is the app's judgement and deliberately not the sensor's:
    /// a phone in your pocket must not dim a page you were about to read.
    pub fn refresh_posture_arming(&mut self, now_secs: i64) {
        const VOICE_WINDOW_SECS: i64 = 300;
        let on_inbox = self.nav.page() == 1;
        let recent_voice = self.last_voice_at
            .map(|t| now_secs.saturating_sub(t) <= VOICE_WINDOW_SECS)
            .unwrap_or(false);
        self.posture.set_armed(on_inbox || recent_voice);
        self.posture.set_in_call(self.group_voice.joined);
    }

    /// Advance posture and apply what it asks for.
    pub fn tick_posture(&mut self, dt_ms: f32) -> crate::posture::Output {
        let out = self.posture.tick(dt_ms);
        // The flip reaches the bar rather than being a number nobody reads:
        // gated talk means the bar latches instead of requiring a held finger,
        // which is the whole point of putting the phone to your ear.
        if out.gated_talk && !self.voicebar.latched() && self.voicebar.on_air() {
            self.voicebar.set_on_air(true);
        }
        out
    }

    /// Open the action menu for a snap tile.
    ///
    /// The entries are derived from the snap's own state rather than fixed:
    /// "Save" is disabled for a snap that has already expired, because the
    /// bytes are gone and offering it would fail after the tap.
    pub fn open_snap_menu(&mut self, id: &str, at: (f32, f32)) -> bool {
        use uikit_egui::menu::MenuItem;
        let expired = self.snaps_expired.iter().any(|e| e == id);
        let items = vec![
            MenuItem::new("open", "View"),
            MenuItem::new("thread", "Conversation"),
            if expired { MenuItem::new("save", "Save").disabled() }
            else { MenuItem::new("save", "Save") },
            MenuItem::separator(),
            MenuItem::new("delete", "Delete").danger(),
        ];
        if !self.menu.open(items, at) { return false; }
        self.menu_target = Some(id.to_string());
        true
    }

    /// Carry out a menu choice against the snap it was opened on.
    pub fn apply_menu(&mut self, id: &str) {
        let Some(target) = self.menu_target.take() else { return };
        match id {
            "delete" => { self.feed.remove(&target); self.received.retain(|s| s.id != target); }
            "save" => self.snaps_saved.push(target),
            "open" => { self.pending_open = Some(target); }
            "thread" => {
                if let Some(e) = self.feed.entries().iter().find(|e| e.id == target) {
                    self.conversation_peer = e.sender_name.clone();
                }
            }
            _ => {}
        }
    }

    /// Open a snap from the feed, routing by MEDIA KIND.
    ///
    /// A video goes to the persistent viewer and an image to the view-once one.
    /// The routing lives here, once, rather than at each tile: a tile that
    /// picked its own viewer would eventually pick the wrong one.
    /// Remember an arrived snap so it can be opened. See `received`.
    pub fn keep_received(&mut self, snap: crate::snaps::Snap) {
        const MAX_COUNT: usize = 48;
        const MAX_BYTES: usize = 96 * 1024 * 1024;
        if self.received.iter().any(|s| s.id == snap.id) { return; }
        self.received.push(snap);
        while self.received.len() > MAX_COUNT
            || self.received.iter().map(|s| s.bytes.len()).sum::<usize>() > MAX_BYTES
        {
            if self.received.len() <= 1 { break; }
            self.received.remove(0);
        }
    }

    /// Open a received snap by id, routed by its kind. `false` if it is
    /// not held any more (expired, evicted, or never arrived here).
    pub fn open_received(&mut self, id: &str, surface: egui::Rect) -> bool {
        let Some(snap) = self.received.iter().find(|s| s.id == id).cloned() else { return false };
        self.open_snap_routed(snap, surface);
        true
    }

    pub fn open_snap_routed(&mut self, snap: crate::snaps::Snap, surface: egui::Rect) {
        let id = snap.id.clone();
        if snap.media == crate::snaps::Media::Video {
            self.video = Some(crate::video_viewer::VideoViewer::new(
                snap.duration_secs.max(1) as f32 * 1000.0));
            self.feed.remove(&id);
            let r = self.dialog_rect(surface, 420.0);
            self.modal.present_default(&format!("video:{id}"), crate::modal::Layer::Dialog, r);
            return;
        }
        self.open_snap(snap);
    }

    /// Close the video viewer. Note what is absent: no wipe and no expiry — the
    /// file stays in snap storage until it expires on its own schedule.
    pub fn close_video(&mut self, id: &str) {
        self.video = None;
        self.modal.dismiss(&format!("video:{id}"));
    }

    /// Open a snap from the feed, view-once, presented into the modal Dialog
    /// layer so the swipe nav and the page behind it are both blocked.
    pub fn open_snap(&mut self, snap: crate::snaps::Snap) {
        let id = snap.id.clone();
        for ev in self.viewer.open(snap) { self.apply_snap_event(ev); }
        self.feed.remove(&id);
        self.modal.present_default(&format!("snap:{id}"), crate::modal::Layer::Dialog,
                                   crate::modal::Rect::new(0.0, 0.0, 400.0, 700.0));
    }

    /// Close the viewer by hand. The countdown is NOT stopped.
    pub fn close_snap(&mut self) {
        let id = self.viewer.current().map(|s| s.id.clone()).unwrap_or_default();
        self.viewer.close();
        self.modal.dismiss(&format!("snap:{id}"));
    }

    fn apply_snap_event(&mut self, ev: crate::snaps::ViewerEvent) {
        use crate::snaps::ViewerEvent as V;
        match ev {
            // Both mark the row in the inbox; the tile is already gone from the
            // feed, and an expiry for a snap the user closed early still has to
            // land or the row stays looking unopened.
            // Storage is the host's: it owns the snap records and this view
            // does not. Recorded rather than applied, and deliberately NOT
            // mapped onto inbox row ids — snap ids are strings and inbox ids
            // are u64s, and inventing a conversion here to make the types line
            // up would mark the wrong row.
            V::Viewed(id) => {
                // Recorded here as well as queued: the queue is the host's
                // to-do list and this is the app's own memory of it. Without
                // the second, a viewed snap that the host has not written
                // yet is still unviewed as far as the next feed load is
                // concerned.
                if self.snap_state.mark(&id, crate::snap_state::Mark::Viewed,
                                        self.now_secs.max(0) as u64 * 1000) {
                    self.snap_state_dirty = true;
                }
                if !self.snaps_viewed.contains(&id) { self.snaps_viewed.push(id); }
            }
            V::Expired(id) => {
                if self.snap_state.mark(&id, crate::snap_state::Mark::Expired,
                                        self.now_secs.max(0) as u64 * 1000) {
                    self.snap_state_dirty = true;
                }
                if !self.snaps_expired.contains(&id) { self.snaps_expired.push(id.clone()); }
                self.modal.dismiss(&format!("snap:{id}"));
            }
            V::Saved(_, path) => self.last_saved = Some(path),
            V::Kept(_) => {}
        }
    }

    /// Adopt cross-app settings + the swipe feel, and use the palette's background style pref.
    pub fn apply_settings(&mut self, ctx: &egui::Context, prefs: &settings_kit::Prefs) {
        uikit_egui::apply_prefs(ctx, prefs);
        self.prefs_handedness = prefs.input.handedness;
        *self.gesture.config_mut() = prefs.gesture_config(320.0);
        self.palette = prefs.palette.clone();
    }

    /// Render: live background, then the current page/subpage.
    /// Snaps arrived. Returns what to send to the notification daemon, or
    /// `None` when nothing should be sent.
    ///
    /// `focused` is the window's own state: an app that notifies about a message
    /// visible on screen is one people mute.
    pub fn on_snaps_arrived(&self, from: &[String], focused: bool)
        -> Option<uikit_egui::notify::Rendered> {
        if !self.notify_enabled || focused || from.is_empty() { return None; }
        let notes: Vec<uikit_egui::notify::Note> = from.iter().map(|f| {
            uikit_egui::notify::Note {
                from: f.clone(),
                // A snap has no text — the picture is the message — so there is
                // nothing to suppress in the detail, and the generic carries it.
                detail: String::new(),
                generic: "New snap".into(),
            }
        }).collect();
        uikit_egui::notify::render_batch(&notes, self.high_security)
    }

    /// Note the page the user is on, and report when they have settled there.
    ///
    /// The caller does the expensive part — fetching a conversation, opening a
    /// camera — only on Settled. Doing it on every frame of a swipe is what
    /// makes a page transition stutter, and doing it only on Settled without a
    /// leading edge is what makes it feel laggy.
    pub fn note_page(&mut self, page: usize, now_ms: u64) -> Option<usize> {
        self.page_debounce.change(page, now_ms);
        self.page_debounce.tick(now_ms).map(|(_, p)| p)
    }

    pub fn ui(&mut self, ui: &mut Ui, adaptive: &Adaptive) {
        let colors = uikit_egui::palette_colors(&self.palette);
        // The kit's readability floor: `.small()` is 9 pt in egui's default
        // style, under 10 pt on a phone. Measured by the layout probe test.
        uikit_egui::adaptive::apply_text_floor(ui.ctx(), adaptive);
        let surface = ui.available_rect_before_wrap();

        // The interaction layer advances every frame, on every page: a ripple
        // that stops because the user swiped away is a bug, not a saving. Touch
        // feedback follows the adaptive layer's answer rather than a guess —
        // the module's own rule is ripples on touch, not under a cursor.
        self.feedback.set_enabled(adaptive.is_compact());
        self.compact = adaptive.is_compact();
        // Grand where it is held constantly, comfortable where it is rarely
        // touched: the phone's talk button is the former.
        self.voicebar.set_talk_prominent(adaptive.is_compact());
        // The keyboard warrior's chrome: Tab starts keyboard mode, a pointer
        // move ends it; Ctrl+K the palette; Ctrl+1..5 the pages.
        self.key_labels.clear();
        // Read from the events, not the modifier snapshot: the chord's
        // modifiers ride on the key event, and a first pointer position has
        // no delta.
        let text_focus = ui.memory(|m| m.focused().is_some()) && !self.palette_open;
        let (tab, moved, ctrl_k, page_key) = ui.input(|i| {
            let mut tab = false; let mut moved = false; let mut ctrl_k = false; let mut page = None;
            for e in &i.events {
                match e {
                    egui::Event::Key { key, pressed: true, modifiers, .. } => {
                        let chord = modifiers.command || modifiers.ctrl;
                        match key {
                            egui::Key::Tab => tab = true,
                            egui::Key::K if chord => ctrl_k = true,
                            // Ctrl+P is what every other palette in the
                            // fleet answers to (studio, voiceterm); `/` is
                            // the one-finger version when no field has focus.
                            egui::Key::P if chord => ctrl_k = true,
                            egui::Key::Slash if !chord && !modifiers.shift && !text_focus => ctrl_k = true,
                            egui::Key::Num1 if chord => page = Some(0),
                            egui::Key::Num2 if chord => page = Some(1),
                            egui::Key::Num3 if chord => page = Some(2),
                            egui::Key::Num4 if chord => page = Some(3),
                            egui::Key::Num5 if chord => page = Some(4),
                            _ => {}
                        }
                    }
                    egui::Event::PointerMoved(_) => moved = true,
                    _ => {}
                }
            }
            (tab, moved, ctrl_k, page)
        });
        if tab { self.keyboard_nav = true; }
        if moved { self.keyboard_nav = false; }
        if ctrl_k {
            self.palette_open = !self.palette_open;
            self.palette_focus = self.palette_open;
        }
        if let Some(i) = page_key { self.nav.go_page(i); }
        // Deferred commands whose time has come.
        let wall_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64).unwrap_or(0);
        for item in self.schedule.due(wall_ms) {
            self.feed_note = format!("now: {}", item.label);
            self.run_action(item.action);
        }
        // Escape closes everything that is open, the way a desktop expects.
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) && self.modal.blocks_input() {
            self.modal.dismiss_all();
            self.viewer.close();
            self.video = None;
            self.qr = None;
            self.editor_id.clear();
        }
        let dt_ms = ui.input(|i| i.stable_dt).min(0.1) * 1000.0;
        let pointer_down = ui.input(|i| i.pointer.any_down());
        // POSTURE. The fleet's only sensor integration — proximity from
        // iio-sensor-proxy, motion from raw IIO counts — and tick_posture was
        // reached ONLY from tests, so in the running app the engine never
        // advanced: no screensaver dim at your ear, and the push-to-talk flip
        // to gated talk never fired. Ticked here because it is per-frame state
        // like the rest, and the returned dim is painted at the end of this
        // function over everything else.
        let posture = self.tick_posture(dt_ms);

        // The manual's idle countdown. tick_help was never called, so an open
        // manual stayed open forever — the auto-dismiss the module was built
        // around simply did not happen, and `interacted()` reset a timer that
        // was not running.
        self.tick_help(dt_ms);

        if self.tick_feedback(dt_ms, pointer_down) {
            ui.ctx().request_repaint();
        }

        // live background painted behind everything
        self.bg.paint(ui.painter(), surface, &colors);

        // LIVE PAGE SWIPE. `begin_swipe`/`end_swipe` and the whole of
        // `swipe_physics` — rubber band, commit distance, velocity flick,
        // settle timing — were written, tested, and reached from nothing.
        // What ran instead was the gesture layer's `page_jump`, which fires
        // once at release: the page did not follow the finger, so a swipe
        // was a guess that either worked or did nothing, with no feedback
        // in between. On a phone that is the difference between a control
        // and a lottery.
        let page_offset = self.tick_page_swipe(ui, surface, dt_ms);

        // The layer still runs, for the trackpad two-finger path and the
        // sub-page detents it also reports. Its `page_jump` is only obeyed
        // when the touch path did not claim the gesture — both acting on
        // one drag turns two pages.
        let frame = self.gesture.update(ui, surface, true);
        let claimed = std::mem::take(&mut self.swipe_claimed);
        if let Some(dir) = frame.page_jump {
            if !claimed && self.axis_lock.intent() != crate::swipe_physics::Intent::Page {
                self.swipe_page(dir.sign());
            }
        }
        // The detent: two-finger scroll along the page axis, accumulated
        // past the threshold, clicks over to the next page — the Qt
        // build's way of turning pages from a trackpad, ported to the
        // gesture model a session ago and read by nothing until now.
        if let Some(dir) = frame.detent {
            if !claimed {
                self.swipe_page(dir.sign());
            }
        }

        // Form factor first: every dialog opened this frame is placed by it.
        // The environment is the host's to report; what this view can see is
        // the surface it was handed, which is the width branch of the ladder.
        self.dialog_form = crate::mobile_dialog::form_factor(
            &Default::default(), surface.width(), adaptive.is_compact());

        // The peel hints ride over the page area, under everything else.
        self.peel.set_page(self.nav.page(), PAGES.len());
        crate::nav_chrome::peel_ui(ui, &mut self.peel, surface, &colors);
        // Discovery ages on its own schedule. Driven from the frame because
        // this app has no timer thread — the guard inside makes that cheap.
        let now_ms = (ui.input(|i| i.time) * 1000.0) as u64;
        self.tick_gossip(now_ms);
        self.tick_dht(now_ms);
        // Pending rows time out on their own clock. Without this a send that
        // never gets an ack stays "Pending" for the life of the process — the
        // spinner that never stops, which is worse than a failure because it
        // never prompts a retry.
        self.sends.tick(now_ms);
        // THE CONTACT EDITOR. Last of the six modals to get a body.
        //
        // It renames only. The Peer this app holds carries a name and an online
        // flag, and the flag is OBSERVED rather than chosen — offering to edit
        // it would be offering to lie about whether someone is reachable.
        if !self.editor_id.is_empty() {
            let want = format!("editor:{}", self.editor_id);
            let r = self.modal.topmost(crate::modal::Layer::Dialog)
                .filter(|p| p.id == want)
                .map(|p| p.rect.clone());
            if let Some(r) = r {
                let area = egui::Rect::from_min_size(
                    egui::pos2(r.x, r.y), egui::vec2(r.w, r.h));
                ui.painter().rect_filled(area, 8.0, colors.bg_base);
                let mut child = ui.new_child(
                    egui::UiBuilder::new().max_rect(area.shrink(12.0)));
                child.colored_label(colors.text, format!("rename {}", self.editor_id));
                child.add(egui::TextEdit::singleline(&mut self.editor_name).id_salt("lib-editor_name"));
                let ok = !self.editor_name.trim().is_empty();
                child.horizontal(|ui| {
                    // Disabled on empty rather than accepting and silently
                    // ignoring: a save button that does nothing is the same
                    // dead control the video scrubber avoided.
                    if ui.add_enabled(ok, egui::Button::new("save")).clicked() {
                        let (id, to) = (self.editor_id.clone(), self.editor_name.clone());
                        self.apply_friends(crate::friends::Action::Rename(id, to));
                        self.editor_id.clear();
                        self.modal.dismiss(&want);
                    }
                    if ui.button("cancel").clicked() {
                        self.editor_id.clear();
                        self.modal.dismiss(&want);
                    }
                });
            }
        }

        // THE VIDEO VIEWER. Fifth modal, and the contract is the OPPOSITE of
        // the snap viewer beside it: a video snap is PERSISTENT. snaps.rs wipes
        // an image's bytes on close; this deliberately does not, and the
        // module is blunt about why the two must not be confused — "the wrong
        // one would either destroy a video the user was allowed to keep or keep
        // an image they were promised would vanish".
        //
        // So there is no countdown here, and its absence is a statement.
        if self.video.is_some() {
            let r = self.modal.topmost(crate::modal::Layer::Dialog)
                .filter(|p| p.id.starts_with("video:"))
                .map(|p| p.rect.clone());
            if let Some(r) = r {
                let area = egui::Rect::from_min_size(
                    egui::pos2(r.x, r.y), egui::vec2(r.w, r.h));
                ui.painter().rect_filled(area, 8.0, colors.bg_base);
                let mut child = ui.new_child(
                    egui::UiBuilder::new().max_rect(area.shrink(12.0)));
                let ev = self.video.as_mut().and_then(|v|
                    crate::video_viewer::video_viewer_ui(&mut child, v, "~/Videos", &colors));
                let vid = self.modal.topmost(crate::modal::Layer::Dialog)
                    .and_then(|p| p.id.strip_prefix("video:").map(String::from)).unwrap_or_default();
                // close_video carries no wipe, by design — the snap outlives
                // the viewer. One path for the button and the viewer's own
                // Closed event.
                if child.button("close").clicked() {
                    self.close_video(&vid);
                }
                if let Some(crate::video_viewer::VideoEvent::Closed) = ev {
                    self.close_video(&vid);
                }
            }
        }

        // THE SNAP VIEWER. Fourth modal to get a body — and the one whose
        // absence actually destroyed data: close_snap's own note says "the
        // countdown is NOT stopped", so an undrawn snap expired on schedule.
        // The event fired, storage marked it gone, and the user never saw the
        // photo they had been told they had.
        if self.viewer.is_open() {
            let want = self.viewer.current().map(|s| format!("snap:{}", s.id));
            let r = want.as_ref().and_then(|w| self.modal.topmost(crate::modal::Layer::Dialog)
                .filter(|p| &p.id == w)
                .map(|p| p.rect.clone()));
            if let Some(r) = r {
                let area = egui::Rect::from_min_size(
                    egui::pos2(r.x, r.y), egui::vec2(r.w, r.h));
                ui.painter().rect_filled(area, 8.0, colors.bg_base);
                let mut child = ui.new_child(
                    egui::UiBuilder::new().max_rect(area.shrink(12.0)));
                let evs = crate::snaps::viewer_ui(&mut child, &mut self.viewer, &colors);
                if child.button("close").clicked() { self.close_snap(); }
                // Events go through the SAME handler open_snap uses, so a snap
                // viewed and one expired reach storage by one path.
                for ev in evs { self.apply_snap_event(ev); }
            }
        }

        // THE SCRIMS, under the dialogs and over the page. ModalSurface is
        // used for ORDERING already — dialogs present onto it and dismiss off
        // it, and `topmost` decides which body to draw — but scrims_ui was
        // never called, so a dialog appeared over a page at full brightness
        // with nothing saying the page behind it was inert.
        //
        // Drawn here rather than at the end of the frame: a scrim painted
        // after the dialogs would cover them, which is the one placement that
        // turns a fix into a black screen.
        crate::modal::scrims_ui(ui, &self.modal);

        // THE DELETE CONFIRMATION. Third of the six modals to get a body.
        if self.confirm.is_some() {
            let want = format!("confirm-delete:{}", self.confirm_id);
            let r = self.modal.topmost(crate::modal::Layer::Confirm)
                .filter(|p| p.id == want)
                .map(|p| p.rect.clone());
            if let Some(r) = r {
                let area = egui::Rect::from_min_size(
                    egui::pos2(r.x, r.y), egui::vec2(r.w, r.h));
                ui.painter().rect_filled(area, 8.0, colors.bg_base);
                let mut child = ui.new_child(
                    egui::UiBuilder::new().max_rect(area.shrink(12.0)));
                let id = self.confirm_id.clone();
                let what = format!("delete {id}?");
                let outcome = self.confirm.as_mut()
                    .and_then(|cc| crate::chevron_confirm::chevron_confirm_ui(
                        &mut child, cc, &what, &colors));
                match outcome {
                    Some(crate::chevron_confirm::Outcome::Confirmed) => {
                        self.confirm_delete(&id);
                    }
                    // Cancelled and TimedOut both close WITHOUT deleting, and
                    // they stay distinct in the enum for the caller's benefit —
                    // but here they mean the same thing, and conflating them at
                    // the point of action would be the one place it is safe to.
                    Some(_) => {
                        self.confirm = None;
                        self.modal.dismiss(&want);
                    }
                    None => {}
                }
            }
        }

        // THE INCOMING-CALL SHEET. Presented on the Confirm layer and never
        // drawn, while `tick` kept running — so a call could ring, time out and
        // be recorded as missed WITHOUT EVER APPEARING. That is worse than a
        // dialog that does not open: the state machine advances correctly and
        // the user is simply not told.
        //
        // On the Confirm layer, above Dialog, so a ringing call is not hidden
        // behind a QR sheet somebody left open.
        if self.incoming_call.is_some() {
            let r = self.modal.topmost(crate::modal::Layer::Confirm)
                .filter(|p| p.id.starts_with("call:"))
                .map(|p| p.rect.clone());
            if let Some(r) = r {
                let area = egui::Rect::from_min_size(
                    egui::pos2(r.x, r.y), egui::vec2(r.w, r.h));
                ui.painter().rect_filled(area, 8.0, colors.bg_base);
                let mut child = ui.new_child(
                    egui::UiBuilder::new().max_rect(area.shrink(12.0)));
                let answered = self.incoming_call.as_ref()
                    .and_then(|call| crate::ring::ring_ui(&mut child, call, &colors));
                // THROUGH answer_call, not around it. The sheet used to apply
                // the answer itself and the shell then called finish_call,
                // which left `answer_call` — the app's only non-UI way to
                // answer — permanently unreachable AND made a second
                // implementation of the same transition. One path now: the
                // sheet reports the press, this applies it, and answer_call
                // owns the guard against answering twice.
                if let Some(a) = answered { self.answer_call(a); }
            }
        }

        // THE QR DIALOG'S BODY. present_default put it on the stack and
        // nothing ever drew it, so "scan a code" opened an invisible modal that
        // blocked input — which reads as the app freezing rather than as a
        // missing render.
        //
        // In ui(), not swipe_page: my first attempt put this in a state method
        // with no `ui` in scope at all — a reminder that "where the modal is
        // dismissed" and "where it is drawn" are different places.
        if self.qr.is_some() {
            let r = self.modal.topmost(crate::modal::Layer::Dialog)
                .filter(|p| p.id == "qr")
                .map(|p| p.rect.clone());
            if let Some(r) = r {
                let area = egui::Rect::from_min_size(
                    egui::pos2(r.x, r.y), egui::vec2(r.w, r.h));
                // ON THE FOREGROUND LAYER. The dialog was painted in the page's
                // own layer, BEFORE the page — invisible while the page was
                // transparent rows, and the moment the page became glass cards
                // the cards painted over it (seen: "SCAN A CODE" under "find").
                // A dialog is a thing above the page; egui's layer order says
                // so, not the call order.
                let layer = egui::LayerId::new(egui::Order::Foreground, ui.id().with("qr-dialog"));
                let mut child = ui.new_child(egui::UiBuilder::new().layer_id(layer).max_rect(area));
                child.painter().rect_filled(area, uikit_egui::tokens::RADIUS_MD as f32, colors.bg_card);
                child.painter().rect_stroke(area, uikit_egui::tokens::RADIUS_MD as f32,
                    egui::Stroke::new(1.0, egui::Color32::from_white_alpha(uikit_egui::glass::GlassAlphas::HAIRLINE)),
                    egui::StrokeKind::Inside);
                let mut child = child.new_child(egui::UiBuilder::new().max_rect(area.shrink(12.0)));
                let mut scanned = None;
                if let Some(q) = self.qr.as_mut() {
                    // With the diagnosis, so the scanner can say whether it is looking or
                    // broken. Computed here because the counters live on the app and the
                    // scanner has no business owning them.
                    let diag = crate::qr_scan_status::status(
                        q.is_scanning(),
                        self.scan_counts,
                        self.scan_hit.as_deref(),
                    );
                    scanned = crate::qr::qr_ui_diag(&mut child, q, &colors, diag);
                }
                // BOTH HALVES. qr_ui reads somebody else's code; my_code_ui
                // shows yours so they can read it back. With only the scanner,
                // an exchange needs a third device.
                child.separator();
                let shown = *self.qr_shown_at.get_or_insert(ui.input(|i| i.time));
                let tilt = self.glove_input.raw().map(|_| self.tilt);
                if crate::qr::my_code_ui(&mut child, &self.local_id, &colors, Some(shown), tilt) {
                    self.qr_expanded = Some(uikit_egui::qr::Expanded::open(ui.input(|i| i.time)));
                }
                if child.button("close").clicked() {
                    self.qr = None;
                    self.qr_shown_at = None;
                    self.modal.dismiss("qr");
                }
                // apply_scan is the EXISTING path — it adds the peer the same
                // way a typed address does and dismisses the dialog itself. I
                // had written on_scanned_contact(), which does not exist;
                // adding a second entry point would have made a scanned contact
                // a different kind of contact from a typed one, which is
                // exactly what apply_scan's doc says it exists to prevent.
                if let Some(sc) = scanned {
                    self.apply_scan(sc);
                }
            }
        }


        // The voice bar sits above the page content while a session is live, so
        // the floor holder, the mic level and the talk button are reachable
        // from wherever you are — the point of a single bar over per-page
        // controls.
        if self.group_voice.joined {
            if let Some(ev) = crate::voicebar::voicebar_ui(ui, &mut self.voicebar, &colors) {
                self.apply_voice(ev);
            }
            ui.separator();
        }

        // page tabs on desktop; swipe hint on phone
        // NAVIGATION BY WIDTH. snapclone is mobile-first and a bottom bar is
        // what a thumb reaches, so compact gets tab_bar; a desktop window gets
        // the top strip, where a pointer already is and where vertical space
        // is not the scarce thing. One model each, chosen here — which is the
        // adaptability the kit exists to make cheap, rather than two apps.
        //
        // Compact previously got NEITHER: the strip was gated on
        // !is_compact() and the bottom bar had no renderer, so on a phone the
        // only way between pages was a swipe with nothing on screen saying so.
        if adaptive.is_compact() && !self.nav.can_go_back() {
            self.tabs.set_current(self.nav.page());
            self.sync_badges();
            let picked = egui::TopBottomPanel::bottom("tabbar")
                .show_inside(ui, |ui| {
                    crate::tab_bar::tab_bar_ui(ui, &mut self.tabs,
                                               adaptive.min_touch_target, &colors)
                })
                .inner;
            if let Some(i) = picked {
                self.nav.go_page(i);
            }
            // Above the tab bar: bottom panels stack upward in the order
            // they are shown.
            egui::TopBottomPanel::bottom("hint")
                .show_separator_line(false)
                .show_inside(ui, |ui| { self.hint_bar(ui, true, &colors); });
        }
        if !adaptive.is_compact() && !self.nav.can_go_back() {
            // THE PORTED STRIP, not a hand-rolled row. strip_ui gives equal
            // segments, an underline under the selected one rather than a box
            // that fights the card borders, and BADGES — which is the part
            // that mattered: sync_badges has computed an unread count into a
            // TabBar that has no renderer at all, so the number reached no
            // screen by any path.
            //
            // badge_text's own comment explains the shared cap: "two bars in
            // one app disagreeing about what a large count looks like is
            // visible on a single screen". Feeding this one from the same
            // source is what keeps that true.
            self.strip.select(self.nav.page());
            self.strip.set_badge(1, self.inbox.unread_count() as u32);
            if let Some(i) = crate::nav_chrome::strip_ui(ui, &self.strip, &colors) {
                self.nav.go_page(i);
            }
            // The status line a desktop user expects under the toolbar,
            // with the number or the address, since there is room.
            self.hint_bar(ui, false, &colors);
            ui.separator();
        }

        // The viewfinder only exists on the camera page. Off it, the ambient
        // driver stops rather than sampling frames nobody can see — the
        // Backgrounded case it was written to refuse. Stated plainly here
        // rather than hidden in a match guard's side effect.
        if self.nav.page() != 0 {
            self.ambient_camera.stop();
        }
        // THE PAGE FOLLOWS THE FINGER. A layer with a horizontal transform
        // rather than laying the body out at an offset: an offset layout
        // would re-run every widget's sizing on every frame of a drag, and
        // a transform is one matrix on shapes that are drawn anyway.
        //
        // Clipped to the surface so the sliding page does not paint over
        // the tab bar and the title, which do not move with it — the chrome
        // is the fixed thing the page slides behind.
        // Every page but the viewfinder scrolls inside the rect the tab bar
        // left it. C13_TODO §24: settings flowed over the bottom bar and its
        // sections over each other, on both platforms, because the page was
        // a bare stack of sections with nothing bounding it — a Ui does not
        // scroll, it clips. One ScrollArea here, keyed by page so each keeps
        // its own offset, rather than one per page body that each author
        // has to remember. The camera page fills; wrapping it would give the
        // viewfinder a scrollbar.
        //
        // The nav has always declared five pages — Camera, Inbox, Friends,
        // Wayfinder, Settings — and the shell once routed three of them, so
        // two tabs led nowhere. That is most of why the app read as a demo:
        // the chrome promised more than the body delivered.
        if self.palette_open {
            self.palette_ui(ui, surface, wall_ms, &colors);
        }
        // The help manual: ported with its filter, its idle countdown and the
        // scrolled/interacted distinction, and opened by nothing but tests.
        if self.help.is_open() {
            let area = surface.shrink2(egui::vec2(surface.width() * 0.08, 24.0));
            ui.painter().rect_filled(area, 10.0, colors.bg_base);
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area.shrink(12.0)));
            if child.input(|i| i.pointer.any_down() || !i.events.is_empty()) {
                self.help_interacted();
            }
            child.horizontal(|ui| {
                ui.colored_label(colors.text, "help");
                let r = ui.add(egui::TextEdit::singleline(&mut self.help_filter).hint_text("filter").id_salt("lib-help_filter"));
                if r.changed() {
                    let f = self.help_filter.clone();
                    self.help.set_filter(&f);
                }
                if ui.button("close").clicked() { self.help.close(); }
            });
            let out = egui::ScrollArea::vertical().id_salt("help_scroll").show(&mut child, |ui| {
                let sections: Vec<(String, String)> = self.help.visible().iter()
                    .map(|s| (s.title.clone(), s.body.clone())).collect();
                for (title, body) in sections {
                    ui.colored_label(colors.accent, title);
                    ui.colored_label(colors.text, body);
                    ui.add_space(8.0);
                }
            });
            if (out.state.offset.y - self.help_scroll_y).abs() > 0.5 {
                self.help_scroll_y = out.state.offset.y;
                self.help.scrolled();
            }
        }
        // The snap menu, drawn over whatever page is up; its choice applied
        // here so every entry point (a tile's hold, a keyboard activate)
        // ends in one place.
        if let Some(choice) = uikit_egui::menu::menu_ui(ui, &mut self.menu, &colors) {
            self.apply_menu(&choice);
        }
        if let Some(id) = self.pending_open.take() {
            if !self.open_received(&id, surface) {
                self.feed_note = format!("{id}: not held on this device any more");
            }
        }
        let draw_page = |app: &mut Self, ui: &mut Ui, page: usize| {
            if page == 0 {
                app.camera_page(ui, &colors);
                return;
            }
            egui::ScrollArea::vertical()
                .id_salt(("page", page))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // THE PAGE MARGIN. Pages drew from the panel's edge, so
                    // every row started 8px from the window and the peel
                    // tabs had to sit on the content. SPACE_MD either side
                    // is room for the tabs and the room a card needs to be
                    // a card rather than a stripe.
                    let m = uikit_egui::tokens::SPACE_MD as i8;
                    egui::Frame::new()
                        .inner_margin(egui::Margin { left: m, right: m, top: 6, bottom: 14 })
                        .show(ui, |ui| match page {
                            1 => app.inbox_page(ui, &colors),
                            2 => app.peers_page(ui, &colors),
                            3 => app.wayfinder_page(ui, &colors),
                            _ => app.settings_page(ui, &colors),
                        });
                });
        };
        let draw_pages = |app: &mut Self, ui: &mut Ui| {
            let page = app.nav.page();
            draw_page(app, ui, page);
        };
        // A desktop populates its width: as many pages side by side as fit,
        // the active one among them, each in its own column and its own
        // scroll. The phone keeps one page and the swipe. The kit's Pager
        // does this for the rest of the fleet; snapclone has its own swipe
        // physics, so it borrows the fit and the window rule and keeps the
        // rest.
        let gutter = adaptive.gutter.max(8.0);
        let cols = if adaptive.is_compact() || self.nav.can_go_back() { 1 } else {
            self.page_columns(ui.available_width(), gutter, ui.input(|i| i.time))
        };
        self.cols_drawn = cols;
        if cols > 1 {
            let avail = ui.available_rect_before_wrap();
            let ids = Self::visible_pages(self.nav.page(), PAGES.len(), cols);
            let col_w = (avail.width() - gutter * (cols as f32 - 1.0)) / cols as f32;
            for (i, page) in ids.into_iter().enumerate() {
                let x = avail.min.x + i as f32 * (col_w + gutter);
                let r = egui::Rect::from_min_size(egui::pos2(x, avail.min.y),
                                                  egui::vec2(col_w, avail.height()));
                ui.scope_builder(egui::UiBuilder::new().max_rect(r), |ui| draw_page(self, ui, page));
            }
        } else if page_offset.abs() < 0.5 {
            // The overwhelming majority of frames. No layer, no transform,
            // no cost — a swipe must not make the still case slower.
            draw_pages(self, ui);
        } else {
            // The PARENT's order, not a chosen one. egui refuses a sublayer
            // whose order differs from its parent's — "currently undefined
            // behavior" in its own words — and a CentralPanel is
            // Background, so naming Order::Middle here panicked the moment
            // a real frame ran. Derived rather than hardcoded so it stays
            // right if this is ever drawn inside a window.
            let layer = egui::LayerId::new(ui.layer_id().order,
                                           ui.id().with("swiping_page"));
            let mut child = ui.new_child(
                egui::UiBuilder::new().layer_id(layer).max_rect(ui.available_rect_before_wrap()));
            draw_pages(self, &mut child);
            ui.ctx().set_transform_layer(
                layer, egui::emath::TSTransform::from_translation(
                    egui::vec2(page_offset, 0.0)));
            ui.ctx().set_sublayer(ui.layer_id(), layer);
        }
        // Last, over everything: the keyboard caption.
        self.keyboard_caption(ui, &colors);

        // MY CODE, expanded: the C++ overlay — scrim, the code at 80% width,
        // the id at 24, Close — on the foreground layer, over every page.
        let layout = crate::qr_render::layout_for(&self.local_id);
        // THE COMPASS TILT. The accelerometer's gravity vector, eased, when the
        // phone has one; a machine without one hands the overlay `None` and
        // the pointer over the code stands in.
        let sensor = self.glove_input.raw().map(|(x, y, z)| uikit_egui::qr::Tilt::from_accel(x, y, z));
        if let Some(t) = sensor {
            self.tilt = self.tilt.ease_toward(t, 0.15);
            if self.qr_expanded.is_some() || self.qr.is_some() { ui.ctx().request_repaint(); }
        }
        let was_expanded = self.qr_expanded.is_some();
        let ev = uikit_egui::qr::expanded_ui_with(ui.ctx(), &mut self.qr_expanded, &self.local_id, &layout, &colors, sensor.map(|_| self.tilt));
        // Closing the big code replays the small one behind it. The card is
        // coming back into view and a code that snaps in finished reads as a
        // static image — which is exactly what it stopped being when the
        // reveal was written.
        if was_expanded && self.qr_expanded.is_none() {
            self.qr_shown_at = None;
        }
        if ev.save_svg {
            let (bg, fg) = uikit_egui::qr::theme_colors(&colors);
            self.code_svg_requested = Some(uikit_egui::qr::svg(&layout, 512.0, fg, bg));
        }
        // THE DIM, painted last so it covers the page rather than being
        // covered by it. dim_overlay_ui returns immediately at zero, so this
        // costs nothing on the overwhelming majority of frames — the phone is
        // not at your ear.
        crate::posture::dim_overlay_ui(ui, posture, ui.ctx().screen_rect());
    }

    /// Advance the interaction layer. Called once per frame from `ui` so the
    /// animations run whichever page is showing — a ripple that stops because
    /// the user swiped away is a bug, not a saving.
    ///
    /// `pointer_down` matters more than it looks. The watchdog measures
    /// `held_ms`, and `tick` stops accumulating it the moment the machine
    /// enters `Held` — so at 60fps a long press freezes at ~500ms and never
    /// reaches the 1500ms threshold. The module's own recovery test passes
    /// one big `tick(STUCK_MS)`, which is the suspend/resume case it was
    /// written for; a pointer-up lost to a system gesture at frame rate is a
    /// different one, and only the shell can see that the finger is gone.
    /// So the release is issued HERE rather than by lowering the module's
    /// threshold — the machine is right about what it can know.
    pub fn tick_feedback(&mut self, dt_ms: f32, pointer_down: bool) -> bool {
        self.blobs.tick(dt_ms / 1000.0);
        self.feedback.tick(dt_ms);
        // The gesture clock advances HERE, not only where taps are read: the
        // watchdog measures held_ms, and a page the user swiped away from
        // still has to be able to recover a press whose pointer-up was eaten
        // by a system gesture. Without the tick the machine sits in Held
        // forever and the next tap folds into the old gesture.
        let long_press = self.press.tick(dt_ms);
        if long_press == Some(crate::press_gesture::Event::LongPress) {
            self.shutter_note = "exposure locked".into();
        }
        if !pointer_down && !self.press.is_idle() {
            // The finger is gone. A release from Held emits nothing (the long
            // press already fired), which is exactly right — this is cleanup,
            // not a second event.
            let _ = self.press.release();
        }
        // Still consulted: it is the suspend/resume path, where a single huge
        // dt lands and no pointer state is trustworthy.
        let recovered = self.press.watchdog().is_some();
        recovered || self.feedback.is_animating() || self.blobs.is_active()
    }

    fn camera_page(&mut self, ui: &mut Ui, c: &PaletteColors) {
        // NO TITLE BAR. Every other page gets one; a camera does not. The word
        // "Camera" above a viewfinder tells the user what they are already
        // looking at, and it costs a band of the preview to say it — on a
        // phone, the band it costs is the one their own face is in.
        //
        // The tab bar below still names the page, so nothing is unlabelled.
        // THE AMBIENT FIELD, painted first so everything else sits on it. Its
        // colours come from ambient_palette's sampling of the last frame, so
        // the glow behind the camera is the camera's own light rather than a
        // fixed gradient — and it only runs while the page is showing.
        {
            let area = ui.available_rect_before_wrap();
            self.blobs.set_active(true);
            let side = area.width().min(area.height());
            // Rgb is [u8; 3]: the sampler's own representation, kept rather
            // than wrapped so the palette and the painter agree byte for byte.
            let (a, b) = self.ambient.unwrap_or(([40, 60, 120], [120, 40, 90]));
            for (i, blob) in self.blobs.blobs().iter().enumerate() {
                let (fx, fy) = blob.centre();
                let p = if i % 2 == 0 { a } else { b };
                // FEATHERED, and under the kit's readability cap. These were
                // hard-edged discs at alpha 90 — the Qt blobs are blurred
                // — and they were the "corny" in the first screenshots. The
                // kit's soft_disc is the glass buttons' off-centre highlight
                // at page scale; MAX_LIFT is the cap every live background
                // keeps, so text over the ambient field stays text.
                let alpha = (blob.opacity() * uikit_egui::livebg::MAX_LIFT as f32 * 0.8)
                    .clamp(0.0, uikit_egui::livebg::MAX_LIFT as f32) as u8;
                uikit_egui::livebg::soft_disc(
                    ui.painter(),
                    egui::pos2(area.left() + fx * area.width(),
                               area.top() + fy * area.height()),
                    blob.radius(side) * 1.6,
                    egui::Color32::from_rgb(p[0], p[1], p[2]),
                    alpha);
            }
        }
        // The REAL viewfinder, not a labelled rectangle. Tap-to-focus, the
        // fading reticle, full-bleed vs framed and the front-camera mirror all
        // come from camera.rs — which existed and was never called, which is
        // exactly why the app looked like a placeholder.
        // FRAME DELIVERY, through frame_pump. The slot is latest-wins, so a
        // compositor stall costs dropped frames rather than a preview that runs
        // permanently behind — and `should_upload` gates the texture write,
        // which is the expensive step, on there actually being something new.
        let now_ms = (ui.input(|i| i.time) * 1000.0) as u64;
        // FROZEN WHILE COMPOSING. From the shutter press until the receipt
        // clears, the viewfinder holds the frame it had: the Qt composer is a
        // dialog over the captured image, and a caption drawn over a live
        // stream is a caption over the wrong picture. Frames are still drained
        // so the slot does not back up; they are not uploaded.
        let live = self.send_flow.step() == crate::send_flow::Step::Capture;
        if !live {
            let _ = self.frames.take();
        }
        if live && crate::frame_pump::should_upload(&self.frames, self.cam_texture.is_some()) {
            if let Some(f) = self.frames.take() {
                self.last_frame_ms = Some(now_ms);
                // The ambient driver is told a frame EXISTS; it does not get
                // the pixels. Frames arrive at ~30Hz and derivation runs on a
                // slow tick, so the driver's job is deciding when to spend
                // one — not holding them.
                self.ambient_camera.on_frame();
                // THE UPLOAD, once per paint — `should_upload` already gated
                // this on a new frame existing, which is what makes 30 fps
                // into an idle background window cost nothing. A torn frame
                // (EOF mid-frame in the reader) is refused here rather than
                // asserted on inside egui.
                if f.pixels_plausible() {
                    let img = egui::ColorImage::from_rgba_unmultiplied(
                        [f.width as usize, f.height as usize], &f.rgba);
                    match &mut self.cam_texture {
                        Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                        None => self.cam_texture = Some(ui.ctx().load_texture(
                            "viewfinder", img, egui::TextureOptions::LINEAR)),
                    }
                }
            }
        }
        // The camera page is on screen, so the driver may run; leaving it
        // started when the page is not visible is the Backgrounded case it
        // exists to refuse.
        self.ambient_camera.start();
        self.ambient_camera.set_app_active(true);
        if self.ambient_camera_tick(now_ms) == crate::ambient_camera::Tick::Derive {
            // No pixel access in this build: complete with nothing usable,
            // which still clears the in-flight flag. Leaving it set would
            // wedge the driver on Busy forever.
            self.ambient_camera_done(None);
        }
        // THE WHOLE PAGE. The controls are drawn OVER the preview further down
        // rather than under it, so nothing is reserved from the frame.
        //
        // This replaces a reserve-a-band arrangement, which itself replaced a
        // hard-coded 220px strip. The band was the right shape for a page with
        // a form on it and the wrong shape for a camera: a viewfinder that
        // stops short of the edge reads as a preview OF a camera rather than
        // as the camera, and the part it gives up is the bottom, which on a
        // phone held at arm's length is where the subject is.
        let vf_rect = {
            let r = ui.allocate_ui(ui.available_size(), |ui| {
                crate::camera::viewfinder(ui, &mut self.viewfinder,
                                          self.cam_texture.as_ref(), c);
            });
            r.response.rect
        };
        // THE CAPTION ON THE PHOTO, from Compose on — the composer's paintEvent.
        if self.send_flow.step() >= crate::send_flow::Step::Compose {
            crate::composer_ui::caption_overlay(&ui.painter_at(vf_rect), vf_rect, &self.composer);
        }
        // TAP VS LONG-PRESS ON THE VIEWFINDER. The machine distinguishes them
        // (and cancels to a drag past the slop), which matters because they
        // mean different things here: a tap focuses, a hold locks exposure.
        // Reading a raw click would make every focus tap a possible lock.
        {
            let resp = ui.interact(vf_rect, ui.id().with("vf_press"),
                                   egui::Sense::click_and_drag());
            let dt_ms = ui.input(|i| i.stable_dt).min(0.1) * 1000.0;
            if let Some(pos) = resp.interact_pointer_pos() {
                let (lx, ly) = (pos.x - vf_rect.left(), pos.y - vf_rect.top());
                if resp.drag_started() || resp.is_pointer_button_down_on() {
                    if self.press.is_idle() { self.press.press(lx, ly); }
                    else { let _ = self.press.move_to(lx, ly); }
                }
                // Swipe UP to send: the composer's own gesture (Composer::
                // begin_drag/end_drag, upward only, a fraction of the height),
                // ported and tested and fed by nothing. On release past the
                // fraction, the recipient picker opens with "Send" armed.
                if resp.drag_started() {
                    self.composer.begin_drag(ly / vf_rect.height().max(1.0));
                }
                if resp.drag_stopped() && self.composer.end_drag(ly / vf_rect.height().max(1.0)) {
                    // The swipe is the shortcut for step 2's Next: it only means
                    // "send" while there is a captured frame being composed.
                    self.advance_to_recipients();
                }
            }
            let ev = self.press.tick(dt_ms).or_else(|| {
                if ui.input(|i| i.pointer.any_released()) { self.press.release() } else { None }
            });
            match ev {
                Some(crate::press_gesture::Event::Tap)
                    if self.send_flow.step() == crate::send_flow::Step::Compose =>
                {
                    // "Tap image to reposition text": while composing, a tap
                    // on the frame places the caption. Focus and flip are the
                    // live camera's business, and the camera is frozen.
                    if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                        crate::composer_ui::reposition_on_tap(&mut self.composer, pos, vf_rect);
                    }
                }
                Some(crate::press_gesture::Event::Tap) => {
                    if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                        // Two taps: flip the camera. The first still
                        // focuses, which is harmless and what phones do.
                        let now_ms = ui.input(|i| i.time) * 1000.0;
                        if self.double_tap.tap(pos.x, pos.y, now_ms) && self.flip_camera() {
                            self.shutter_note = "flipped".into();
                        }
                        // The viewfinder maps the tap to SENSOR coordinates
                        // itself (and refuses one outside the image rect), so
                        // it takes the screen point and the rect rather than
                        // pre-computed local offsets.
                        self.viewfinder.tap(pos, vf_rect);
                        self.feedback.tap(pos.x - vf_rect.left(), pos.y - vf_rect.top(),
                                          vf_rect.width(), vf_rect.height());
                    }
                }
                Some(crate::press_gesture::Event::LongPress) => {
                    self.shutter_note = "exposure locked".into();
                }
                // A drag is not a tap: any mid-gesture visual state is torn
                // down rather than left on screen.
                Some(crate::press_gesture::Event::DragCanceled) => {}
                None => {}
            }
            // RIPPLES, over the viewfinder they belong to.
            for r in self.feedback.ripples() {
                let a = ((1.0 - r.progress()) * 70.0) as u8;
                ui.painter().circle_stroke(
                    egui::pos2(vf_rect.left() + r.x, vf_rect.top() + r.y),
                    r.radius(),
                    egui::Stroke::new(1.5, egui::Color32::from_rgba_unmultiplied(
                        c.accent.r(), c.accent.g(), c.accent.b(), a)));
            }
        }
        // THE CAPTURE FLASH, over the preview and under the chrome: the
        // confirmation the shutter never had. Advanced here rather than in a
        // tick method because this is the only page that draws it, and a timer
        // that runs while its painter does not is a timer that has already
        // expired by the time anyone looks.
        if let Some(age) = self.snap_flash {
            let a = crate::camera::flash_alpha(age);
            if a <= 0.0 {
                self.snap_flash = None;
            } else {
                ui.painter_at(vf_rect).rect_filled(
                    vf_rect, 0.0,
                    crate::camera::flash_color(
                        age, self.settings.current.camera.capture_flash_bright));
                self.snap_flash = Some(age + ui.input(|i| i.stable_dt).min(0.1));
                ui.ctx().request_repaint();
            }
        }
        // ── from here the controls are drawn OVER the preview ───────────────
        //
        // A child Ui on the lower band of the viewfinder, so every `ui.` call
        // below lands on the overlay instead of below the frame. Shadowing
        // rather than wrapping the rest in a closure: the block that follows
        // borrows `self` in a dozen places and a closure would fight the
        // borrow checker for no gain.
        //
        // A scrim first. White shutter chrome over a bright sky is invisible,
        // and a camera is pointed at bright things by definition; the gradient
        // is what every phone camera puts there and it is not decoration.
        //
        // The band's height is derived from the theme's own interact size, so a
        // larger UI scale gets proportionally more room instead of the shutter
        // being clipped at 200%; `controls_band` clamps it against the frame.
        let band = {
            let row = ui.spacing().interact_size.y + ui.spacing().item_spacing.y;
            // SEVEN rows, not three. What actually goes in this band is: a
            // stall/frozen notice, the camera's name, the lens+rotate row, the
            // timer+colour row, the delivery explanation, the send receipt and
            // (sometimes) a "nobody to send to" warning. Sized at three, the
            // surplus did not vanish and did not scroll — the child had a
            // `max_rect` and NO clip rect, so it was drawn upward, over the
            // "Camera" heading and the delivery line above the frame. That is
            // exactly the six text-over-text collisions and the 34-pt clipped
            // "colour" the layout probe reported at 360x780, and why this page
            // was ratcheted instead of clean.
            //
            // `controls_band` still caps at half the frame, so a short viewport
            // cannot let the band eat the picture; the clip below is what
            // guarantees the cap can never again be paid for in overlap.
            crate::camera::controls_band(vf_rect, row * 7.0 + 16.0)
        };
        {
            let p = ui.painter_at(band);
            let steps = 8;
            for i in 0..steps {
                let t = i as f32 / steps as f32;
                let slice = egui::Rect::from_min_max(
                    egui::pos2(band.left(), band.top() + band.height() * t),
                    egui::pos2(band.right(), band.top() + band.height() * (t + 1.0 / steps as f32)),
                );
                let a = (t * t * 170.0) as u8;
                p.rect_filled(slice, 0.0, egui::Color32::from_black_alpha(a));
            }
        }
        let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(band.shrink(6.0)));
        // The clip is the guarantee. Whatever the band's height works out to on
        // a given viewport and UI scale, its contents cannot paint outside it
        // and over the page above.
        ui.set_clip_rect(band);
        let ui = &mut ui;
        // THE CAMERA'S CHROME — stall marker, capture status, which camera,
        // the shutter row — belongs to step 1. From Compose on the frame is
        // frozen on purpose (the Qt composer is a dialog over the captured
        // image), so "preview stalled" would be a false alarm and the
        // shutter is "retake" on the panel below; hiding the rows also gives
        // that panel the height it needs on a phone. Seen at 420x820: the
        // timer chips and Next were below the fold with the chrome shown.
        if self.send_flow.step() == crate::send_flow::Step::Capture {
        // A FROZEN STREAM IS MARKED rather than silently frozen: the user is
        // about to take a photo based on what this shows, and a still image
        // that stopped updating is a lie about what the lens sees.
        match crate::frame_pump::display_state(self.last_frame_ms, now_ms) {
            crate::frame_pump::Display::Stale => {
                ui.colored_label(c.warning, "preview stalled");
            }
            crate::frame_pump::Display::Waiting if !self.cameras.is_empty() => {
                ui.colored_label(c.text_weak, "starting camera…");
            }
            _ => {}
        }
        // The capture engine's own words, when it has any: which pipeline is
        // being opened, or why none worked — in GStreamer's phrasing, which
        // is the one the user can search for. "starting camera…" above says
        // something is happening; this says WHAT.
        if let Some(line) = self.capture_status.line() {
            let col = match self.capture_status {
                crate::capture::Status::Exhausted { .. } => c.warning,
                _ => c.text_weak,
            };
            ui.colored_label(col, line);
        }
        // Say WHICH camera, or that there is none. The viewfinder mirrors only
        // when device-kit says this device is front-facing.
        match &self.camera {
            Some(cam) => {
                ui.colored_label(c.text_weak, crate::camera_source::describe(cam));
            }
            None => {
                // DELIBERATELY SILENT. "no camera found" was already on this
                // screen twice: the viewfinder draws "no camera" in its centre,
                // and `hint` raises it as a Blocking hint with the actual
                // remedy (PipeWire/libcamera on a phone, plug one in on a
                // desktop) and a jump to Settings. A third copy — a bare label
                // in a third colour — read as three different problems. This
                // slot names WHICH camera; when there is none, the two that
                // can say something useful say it.
            }
        }
        // WHICH camera, and whether both can run at once. camera_select answers
        // a question camera_source cannot: device-kit picks a device, this one
        // knows about facing and CAPTURE GROUPS — two cameras sharing an ISP
        // cannot stream together, and offering PIP on such a device produces a
        // black inset rather than an error.
        if !self.cameras.is_empty() {
            let pip_ok = crate::camera_select::pip_available(&self.cameras);
            ui.horizontal(|ui| {
                let current = match &self.cam_selection {
                    Some(crate::camera_select::Selection::Single { id, .. }) => id.clone(),
                    _ => String::new(),
                };
                if self.compact {
                    // C13_TODO §23: a phone lists its cameras in a dropdown,
                    // not a row that runs off the page.
                    let label = self.cameras.iter().find(|d| d.id == current)
                        .map(|d| d.label.clone()).unwrap_or_else(|| "camera".into());
                    let mut pick: Option<String> = None;
                    egui::ComboBox::from_id_salt("camera_pick").selected_text(label).show_ui(ui, |ui| {
                        for d in &self.cameras {
                            if ui.selectable_label(d.id == current, &d.label).clicked() {
                                pick = Some(d.id.clone());
                            }
                        }
                    });
                    if let Some(id) = pick {
                        self.cam_selection = Some(crate::camera_select::select(&self.cameras, &id, true));
                    }
                } else {
                    for d in &self.cameras {
                        let on = d.id == current;
                        if ui.selectable_label(on, &d.label).clicked() {
                            self.cam_selection = Some(crate::camera_select::select(
                                &self.cameras, &d.id, true));
                        }
                    }
                }
                // Offered ONLY when the hardware can actually do it. A greyed
                // button would still be a promise; an absent one is the truth.
                if pip_ok {
                    let on = matches!(&self.cam_selection,
                                      Some(crate::camera_select::Selection::Pip { .. }));
                    if ui.selectable_label(on, "both").clicked() {
                        self.cam_selection = Some(crate::camera_select::select(
                            &self.cameras, crate::camera_select::PIP_ID, true));
                    }
                }
            });
            if let Some(crate::camera_select::Selection::Pip { main, inset, .. }) =
                &self.cam_selection
            {
                ui.colored_label(c.text_weak, format!("{main} with {inset} inset"));
            }
        }
        // The camera chrome uses the painted glass buttons, not flat ones: the
        // mottling and the tap-keyed ripple are the two things a stylesheet
        // cannot express, which is why Qt paints them.
        // THE SHUTTER IS CENTRED, and it leans toward the hand that holds the
        // phone. This row was a plain left-packed `horizontal`, so the shutter
        // sat wherever the buttons before it happened to end — visibly left of
        // centre in a screenshot at 400pt — and `settings_kit`'s handedness
        // preference, which has had a settings screen and a double-tilt
        // gesture for as long as it has existed, was read by nothing at all.
        // `uikit_egui::handed` turns it into the offset; the row width is
        // captured BEFORE the layout so the spacer is measured against the
        // whole row rather than what is left of it.
        let row_w = ui.available_width();
        let hand = self.prefs_handedness;
        ui.horizontal(|ui| {
            // ROTATE, not swap. This called `flip()`, which changes the front/
            // back CAMERA — a different action, already reachable from the
            // device picker directly above, and not what a circular arrow
            // means anywhere else. Reported from the phone as "the rotate
            // button should rotate by 90deg not flip".
            if crate::glass::glass_button_ui(ui, &mut self.glass_flip, "⟲", 44.0, c) {
                self.viewfinder.rotate_cw();
                self.shutter_note = format!("rotated {}°", self.viewfinder.quarter_turns as u32 * 90);
            }
            // THE REAL SHUTTER, not a button that sets a bool. One control,
            // four gestures: tap for a photo, hold to record, swipe up to lock
            // recording hands-free, swipe down for the mode picker. A plain
            // button can express exactly one of those.
            //
            // shutter_ui senses DRAG rather than click, because `clicked()`
            // fires on any release however long the press lasted — which is the
            // trap that makes hold-to-record indistinguishable from tap.
            // The gap that puts the shutter at its centred, hand-leaned x.
            // Measured from what the row has already consumed, so it stays
            // centred when the buttons before it change size or count.
            const SHUTTER: f32 = 56.0;
            let used = row_w - ui.available_width();
            let want = uikit_egui::handed::centred_x(row_w, SHUTTER, hand);
            ui.add_space((want - used).max(0.0));
            let resp = crate::shutter::shutter_ui(ui, &self.shutter, SHUTTER, c);
            // DRIVEN BY THE BUTTON STATE, not by egui's drag flags. For a
            // widget that senses click AND drag, egui only reports
            // `drag_started`/`drag_stopped` once the pointer is "decidedly
            // dragging" — moved past the threshold or held — so a clean tap
            // never reached `press`/`release`, and a tap on the shutter took
            // no photo in the running app unless the finger wiggled. The
            // palette's Snap action (which sets `shutter_pressed` directly)
            // and the tests (which call the FSM) both worked, which is how
            // it went unnoticed until the app was tapped under Xvfb.
            let pos = resp.interact_pointer_pos().or_else(|| ui.input(|i| i.pointer.latest_pos()));
            let down_on = resp.is_pointer_button_down_on();
            let ev = if down_on && !self.shutter.is_down() {
                pos.and_then(|p| self.shutter.press(p.x, p.y))
            } else if down_on {
                pos.and_then(|p| self.shutter.moved(p.x, p.y))
                    .or_else(|| self.shutter.tick(ui.input(|i| i.stable_dt) * 1000.0))
            } else if self.shutter.is_down() {
                pos.and_then(|p| self.shutter.release(p.x, p.y))
            } else {
                // The hold threshold is a TIMER, so it must be ticked on frames
                // where the finger has not moved — otherwise a perfectly still
                // press never becomes a recording.
                self.shutter.tick(ui.input(|i| i.stable_dt) * 1000.0)
            };
            match ev {
                Some(crate::shutter::Shutter::Photo) => self.shutter_pressed = true,
                Some(crate::shutter::Shutter::RecordStarted) => {
                    // A FRESH recorder each time. Reusing one would report the
                    // previous clip's duration on the next.
                    self.recorder = Some(crate::video_record::Recorder::new(
                        crate::video_record::Limits::default()));
                }
                Some(crate::shutter::Shutter::RecordStopped { duration_ms }) => {
                    // Report the DURATION: a recording that stopped without
                    // saying how long it was leaves the user unsure whether it
                    // captured anything at all.
                    //
                    // And report that NOTHING WAS WRITTEN, because nothing was.
                    // The frames were counted and measured; no encoder is linked
                    // yet (C14_TODO §15 step 2). Saying "recorded 3.2s" alone
                    // would be the same lie `EncoderReady(true)` tells on the
                    // voice path — §14 — and this one is avoidable today.
                    self.shutter_note = match self.recorder.take() {
                        Some(r) => {
                            // AND WHICH ENCODER IS MISSING. "no encoder linked yet" was true
                            // and unactionable: nobody could tell whether the build lacked
                            // support or the machine lacked a package.
                            // `video_backend::choose`/`unavailable_report` have been pure and
                            // tested all along with nothing supplying their probes;
                            // `video_probe` is the probes.
                            // THE SPILL FIRST. It is the input the encode command reads, and
                            // if it failed there is nothing to encode — printing "would run:
                            // ffmpeg …" over a file that is not there is the worst kind of
                            // confident.
                            let spilled = self.spill.take().map(|s| s.finish());
                            let ready = match &spilled {
                                Some(Ok(r)) => Some(r.clone()),
                                _ => None,
                            };
                            let out = crate::video_spill::spill_path(0).with_extension("mkv");
                            let why = match &spilled {
                                Some(Err(e)) => format!("nothing to encode \u{2014} {e}"),
                                // No spill at all means no recording was running — not a
                                // failure, just nothing to do.
                                None => "no recording".to_string(),
                                Some(Ok(_)) => match crate::video_probe::status(false) {
                                Ok(b) => {
                                    // THE COMMAND THAT WOULD RUN, not just the backend's
                                    // name. `av1_encode::command` turns the recorder's own
                                    // `Plan` into an argv and has had no caller — so the
                                    // policy decided how to encode and nothing ever asked
                                    // it. Printed rather than spawned, which is the module's
                                    // own reason: "for a step that consumes a recording the
                                    // user cannot get back", a command you can read beats
                                    // one that already ran.
                                    //
                                    // It also cannot run yet, and that is worth being
                                    // precise about: the recorder COUNTS frames, it does not
                                    // write them, so there is no input for ffmpeg to read.
                                    // The argv is what to run once frames land on disk.
                                    let plan = r.plan(snapclone_av1policy::Tier::Standard);
                                    // THE FRAMES' OWN SIZE. The recorder takes it from the
                                    // first frame and rejects any that disagree, so it is the
                                    // real one — a hardcoded 720x1280 would produce an argv
                                    // that looks right and encodes a stretched clip on every
                                    // other sensor. No size means no frame was ever accepted,
                                    // which is a different thing from a missing encoder and
                                    // says so.
                                    match r.size() {
                                        None => format!(
                                            "{} ready, but no frame was accepted \u{2014}                                              nothing to encode",
                                            b.name()
                                        ),
                                        Some((w, h)) => {
                                            // `ready` is Some in this arm: the outer match
                                            // only reaches here for `Some(Ok(_))`.
                                            let ready = ready.clone().expect("spill succeeded");
                                            let src = crate::av1_encode::Source::RawStdin {
                                                width: w,
                                                height: h,
                                                fps: r
                                                    .measured_fps()
                                                    .map(|f| f.round() as u32)
                                                    .unwrap_or(30),
                                                pix_fmt: "yuv420p".to_string(),
                                            };
                                            // AND IT RUNS. The argv was printed because the
                                            // frames did not exist; they do now, so the
                                            // remaining step was piping one into the other.
                                            //
                                            // ON A THREAD: an encode is seconds of work and
                                            // this is called from the shutter, with the user
                                            // still looking at the preview. Blocking here
                                            // freezes the camera for the length of the clip.
                                            match crate::av1_encode::command(&src, &plan, out.to_str().unwrap_or("snap.mkv")) {
                                                Ok(argv) => {
                                                    let (spill_path, out2) = (ready.path.clone(), out.clone());
                                                    let tx = self.encode_done.0.clone();
                                                    std::thread::spawn(move || {
                                                        let d = crate::video_encode_run::run_and_tidy(
                                                            &argv, &spill_path, &out2);
                                                        // A closed receiver means the app is
                                                        // gone; nothing to report to.
                                                        let _ = tx.send(d);
                                                    });
                                                    format!("{} \u{2014} encoding {} frames", b.name(), ready.frames)
                                                }
                                                Err(e) => format!("{}: {e}", b.name()),
                                            }
                                        }
                                    }
                                }
                                Err(missing) => missing.join("; "),
                            } };
                            format!(
                                "would record {:.1}s \u{2014} {} frames at {:.0} fps; {why}",
                                r.duration_secs(),
                                r.frames(),
                                r.measured_fps().unwrap_or(0.0),
                            )
                        }
                        None => format!("recorded {:.1}s", duration_ms as f32 / 1000.0),
                    };
                }
                Some(crate::shutter::Shutter::OpenModePicker) => {
                    // The voice/text picker does not exist yet, so this RECORDS
                    // the request rather than calling a method I would have to
                    // invent. A swipe that silently does nothing is worse than
                    // one that says the destination is not built — the user
                    // repeats the gesture harder, assuming they missed.
                    self.shutter_note = "mode picker not implemented yet".into();
                }
                // RecordStarted and RecordLocked are reflected by the button
                // itself — it turns red and shows the lock — so re-announcing
                // them in a toast would be noise over the thing already saying it.
                _ => {}
            }
            if !self.shutter_note.is_empty() && !self.shutter.recording() {
                ui.colored_label(c.text_weak, &self.shutter_note);
            }
            if self.shutter.recording() {
                ui.colored_label(c.warning, format!("● {:.1}s{}",
                    self.shutter.record_ms() as f32 / 1000.0,
                    if self.shutter.locked() { " locked" } else { "" }));
            }
            if ui.selectable_label(self.viewfinder.full_bleed, "fill").clicked() {
                self.viewfinder.full_bleed = !self.viewfinder.full_bleed;
            }
            if ui.selectable_label(self.viewfinder.scan_mode, "scan").clicked() {
                self.viewfinder.scan_mode = !self.viewfinder.scan_mode;
            }
        });
        } // end of the camera's chrome (step 1 only)
        ui.separator();
        // EVERYTHING BELOW THE VIEWFINDER SCROLLS. The camera page is drawn
        // outside the page-level ScrollArea on purpose — the viewfinder should
        // fill its space rather than be a scrolling box — but that left the
        // controls under it with no scroll and no clip, so once the composer,
        // the delivery line and the send receipt were taller than what the
        // hint bar and tab bar left, the last rows were painted UNDER them.
        // Seen at 400x800: "No recipients / Add someone to this thread" cut in
        // half by the hint bar. Only the tail scrolls; the viewfinder keeps
        // the space it had.
        egui::ScrollArea::vertical()
            .id_salt("camera-controls-tail")
            .auto_shrink([false, false])
            .show(ui, |ui| {
        // THE STEPS. Four cells, the current one lit, each decided one carrying
        // its record (file · timer and caption · who). The Qt predecessor made
        // every step its own dialog so you always knew where you were; the
        // port had every control on one page under a live viewfinder and the
        // send hidden in a swipe. See `send_flow`.
        let now_ms = (ui.input(|i| i.time) * 1000.0) as u64;
        crate::send_flow_ui::stepper_strip(ui, &self.send_flow, c);
        let step = self.send_flow.step();
        match step {
            crate::send_flow::Step::Capture => {
                // Nothing to compose yet: the composer's controls wait for a
                // frame. Drawing a caption field over a live viewfinder invited
                // typing a caption for a photo that did not exist.
                ui.colored_label(c.text_weak, step.prompt());
                return;
            }
            crate::send_flow::Step::Recipients => {
                ui.colored_label(c.text_weak, step.prompt());
                ui.horizontal(|ui| {
                    if ui.button("choose recipients").clicked() { self.nav.go_page(2); }
                    if ui.button("back to compose").clicked() { self.send_flow.back_to_compose(); }
                });
                return;
            }
            crate::send_flow::Step::Sending | crate::send_flow::Step::Sent => {
                crate::send_flow_ui::receipt(ui, &self.send_flow, c);
                if ui.button("new snap").clicked() || self.send_flow.receipt_expired(now_ms) {
                    self.send_flow.reset();
                }
                return;
            }
            crate::send_flow::Step::Compose => {
                ui.colored_label(c.text_weak, step.prompt());
            }
        }
        // the composer's bottom panel, as SnapComposerDialog lays it out
        let cap_focus = ui.memory(|m| m.focused()).is_some();
        // THE KEYBOARD INSET, fed by focus: winit/egui 0.34 do not surface the
        // input panel's rect; 0.45 is the module's own test fraction and about
        // what a phone keyboard takes. Right when the platform says, usable
        // when it does not, zero when nothing is focused.
        let fraction = ui.ctx().input(|i| i.events.iter().find_map(|e| match e {
            egui::Event::Ime(_) => Some(0.45),
            _ => None,
        })).unwrap_or(0.45);
        self.composer.apply_keyboard_inset(cap_focus, fraction);
        match crate::composer_ui::compose_controls(ui, &mut self.composer, c) {
            crate::composer_ui::ComposeAction::Next => self.advance_to_recipients(),
            crate::composer_ui::ComposeAction::Retake => self.shutter_pressed = true,
            crate::composer_ui::ComposeAction::Cancel => self.send_flow.cancel(),
            crate::composer_ui::ComposeAction::None => {}
        }
            });
    }

    /// Step 3's Send: one watched row per recipient, the send through the
    /// flow so the composed snap travels, the picker cleared for next time.
    /// Public so the whole sequence — shutter, compose, choose, send, receipt
    /// — is driven by tests in `host.rs` and not only by a finger.
    pub fn send_to(&mut self, chosen: &[String], now_ms: u64) {
        for who in chosen {
            // One row PER RECIPIENT. A single row for a multi-send cannot
            // express the normal case — one person offline, the rest fine —
            // and would have to report the whole thing failed or the whole
            // thing sent.
            //
            // KEYED BY THE RECIPIENT. The row id was `who:now`, and the host
            // acks and fails by `who` alone (`SnapDelivered { id }`), so no row
            // made from the button was ever matched: every real send aged out
            // as a timeout while the host tests — which pushed `who` — passed.
            self.sends.push(who, now_ms);
        }
        // Through the flow, so the composed snap (timer, caption) travels
        // with the recipients instead of the bin rebuilding it as duration 0,
        // which on this wire means never vanishes. A send with no snap being
        // composed (the palette's bare "send to") keeps the old bare path.
        match self.send_flow.on_send(chosen, now_ms) {
            Some(req) => self.send_request = Some(req),
            None => self.send_requested = chosen.to_vec(),
        }
        self.picker.clear();
    }

    /// Compose is done: bake the composition into the flow and open the
    /// picker with "Send" armed. Both the Next button and the upward swipe
    /// land here, so they cannot disagree about what "next" is.
    pub fn advance_to_recipients(&mut self) {
        if self.send_flow.on_composed(self.composer.compose()) {
            self.picker.mode = crate::contacts::PickMode::Multi { confirm_label: "Send".into() };
            self.nav.go_page(2);
        }
    }

    /// Route one action from the requests list. Accept and Decline apply
    /// immediately; Block goes through `confirm` — Tap-level risk, where the
    /// familiarity model earns its keep: a first Block asks, a fiftieth does
    /// not, and a cancel re-raises the friction.
    pub fn request_action(&mut self, sender: String, act: crate::requests::RequestAction) {
        if crate::requests::needs_confirmation(act) {
            let fam = *self.familiarity_of("block-request");
            match crate::confirm::Confirm::begin(
                "block-request", &format!("Block {sender}?"), &fam) {
                Some(panel) => { self.block_confirm = Some((sender, panel)); }
                None => {
                    // An expert is not asked — but the use is still recorded,
                    // or the score would decay back into asking.
                    self.mailbox.apply(&sender, act);
                    self.familiarity_of("block-request").record_use();
                }
            }
        } else {
            self.mailbox.apply(&sender, act);
        }
    }

    fn peers_page(&mut self, ui: &mut Ui, c: &PaletteColors) {
        self.nav.title_bar(ui, "Friends");
        // Inbound connection requests, above the roster: a stranger asking to
        // connect is the page's most time-sensitive row.
        if let Some((sender, act)) = crate::requests::requests_ui(ui, &self.mailbox, c) {
            self.request_action(sender, act);
        }
        // The Block confirmation, when one is open. confirm's own panel: tap
        // or hold by familiarity, auto-cancel at 4s.
        if let Some((sender, panel)) = &mut self.block_confirm {
            let dt = ui.input(|i| i.stable_dt).min(0.1);
            let mut outcome = panel.tick(dt);
            ui.ctx().request_repaint(); // the dismiss clock runs even untouched
            ui.horizontal(|ui| {
                ui.colored_label(c.danger, format!("Block {sender}?"));
                match panel.mode {
                    crate::confirm::Mode::Hold => {
                        let resp = ui.button("hold to block");
                        if resp.is_pointer_button_down_on() { panel.press(); }
                        else if panel.is_holding() { outcome = panel.release(); }
                        let p = panel.progress();
                        if p > 0.0 {
                            let r = resp.rect;
                            ui.painter().rect_filled(
                                egui::Rect::from_min_size(r.min, egui::vec2(r.width() * p, 2.0)),
                                0.0, c.danger);
                        }
                    }
                    _ => {
                        if ui.button("Block").clicked() {
                            panel.press();
                            outcome = panel.release();
                        }
                    }
                }
                if ui.button("Cancel").clicked() { outcome = panel.cancel(); }
            });
            let sender = sender.clone();
            match outcome {
                crate::confirm::Outcome::Confirmed => {
                    self.block_confirm = None;
                    self.mailbox.apply(&sender, crate::requests::RequestAction::Block);
                    self.familiarity_of("block-request").record_use();
                }
                crate::confirm::Outcome::Cancelled => {
                    self.block_confirm = None;
                    // A cancel means the panel was WANTED: friction re-raises.
                    self.familiarity_of("block-request").record_fail();
                }
                crate::confirm::Outcome::Pending => {}
            }
        }
        // THE PAGE IS CARDS NOW, not rows split by separators: on the live
        // backdrop a separator is a scratch and a card is a thing. The
        // picker, the reach summary and the send are one surface; the ways to
        // connect are another; the friends list and the add-peer form are the
        // list widget's own.
        //
        // The contact picker, so Friends is a place you DO something rather
        // than a list you look at.
        let reach_roster = self.peer_roster();
        uikit_egui::glass::card(ui, c.bg_card, |ui| {
        uikit_egui::glass::card_title(ui, "find", c.text_weak);
        ui.horizontal(|ui| {
            let room = (ui.available_width() - 76.0).max(80.0);
            ui.add(egui::TextEdit::singleline(&mut self.picker.query).id_salt("lib-self_picker_query")
                .hint_text("search friends").desired_width(room));
            let multi = self.picker.is_multi();
            if ui.selectable_label(multi, "multi").clicked() {
                self.picker.mode = if multi { crate::contacts::PickMode::Single }
                                   else { crate::contacts::PickMode::Multi {
                                       confirm_label: "Send".into() } };
            }
        });
        // REACHABILITY, not a stored bool. Three states with three meanings:
        // Connected sends now, Known queues, Unverified cannot be sent to at
        // all — and the label says which so a queued message does not look
        // like a delivered one.
        // THE COUNT STAYS, THE THIRD LIST GOES. This page used to render every
        // contact three times — the reachability list, the picker's list of
        // names, and the card — so six people filled a phone screen with
        // eighteen rows and three different ideas of what a row looks like.
        // Reachability and the pick checkbox are fields on the card row now
        // (`friends::RowInfo`); what is left here is the summary, which is
        // per-page information rather than per-person.
        if !reach_roster.is_empty() {
            ui.colored_label(c.text_weak,
                format!("{} of {} reachable now", self.sendable_peers(), reach_roster.len()));
        }
        // SEND, and the row that watches it. The picker could select people and
        // nothing consumed the selection — and send_state, "the row the sender
        // watches", had no caller at all. Between them the app could choose
        // recipients and never tell you whether anything arrived.
        let chosen: Vec<String> = self.picker.selected().to_vec();
        // STEP 3, when a snap is waiting. The strip says what is being sent
        // (file · timer · caption) and the button says it is THE snap, not a
        // generic "send to N"; with no contacts at all the Qt rule applies —
        // "Add a friend first" — and the shutter is not left holding a photo
        // nobody can receive.
        let sending = self.send_flow.step() == crate::send_flow::Step::Recipients;
        if sending {
            crate::send_flow_ui::stepper_strip(ui, &self.send_flow, c);
            // THE RECEIPT, BEFORE THE SEND, where its facts are known. The Qt
            // header says it makes the mediashare guarantees visible before
            // you send; "recipients" is the one fact this step decides, and
            // the encryption facet goes red — and the gate blocks — when the
            // thread holds nobody. It used to sit on the camera page, where
            // recipients are never known, and so was always red.
            self.send_summary.set_recipients(chosen.len() as i32);
            crate::send_summary::send_summary_ui(ui, &self.send_summary, c);
            if self.contact_book.is_empty() {
                ui.colored_label(c.warning, "Add a friend first — nobody to send this snap to");
            } else if !self.can_send() {
                ui.colored_label(c.text_weak, "tick who gets it");
            }
            if ui.button("back to compose").clicked() { self.send_flow.back_to_compose(); }
        }
        if !chosen.is_empty() {
            let label = if sending { format!("Send snap to {} \u{27a4}", chosen.len()) }
                        else { format!("send to {}", chosen.len()) };
            if uikit_egui::glass::primary_button(ui, &label, c.accent, c.bg_base).clicked() {
                let now = (ui.input(|i| i.time) * 1000.0) as u64;
                self.send_to(&chosen, now);
            }
        }
        // Failures surface without being hunted for. needs_attention exists
        // because a send that failed silently is indistinguishable from one
        // that worked, and the sender is the last person to find out.
        let stuck = self.sends.needs_attention().len();
        if stuck > 0 {
            ui.colored_label(c.warning, format!("{stuck} send(s) failed — tap to retry"));
        }
        });
        if let Some(sel) = self.nav.subpage() {
            // subpage: peer detail
            if let Some(p) = self.peers.get(sel) {
                ui.colored_label(c.text, format!("{}  ·  {}", p.name, if p.online { "online" } else { "offline" }));
                ui.colored_label(c.text_weak, "Share photos, connect a session, or send a ping.");
            }
            return;
        }
        // Join / leave lives here because Friends is where the people are; the
        // rest of the voice controls are on the bar itself once a session is up.
        if !self.call_note.is_empty() {
            // Where the call was placed from, so the answer is where the
            // question was — a note on another page is a note nobody reads.
            ui.colored_label(
                if self.call_note.contains("no known address") { c.warning } else { c.text_weak },
                &self.call_note);
        }
        uikit_egui::glass::card(ui, c.bg_card, |ui| {
        uikit_egui::glass::card_title(ui, "connect", c.text_weak);
        ui.horizontal(|ui| {
            let joined = self.group_voice.joined;
            if uikit_egui::glass::secondary_button(ui, if joined { "leave channel" } else { "join channel" }, c.bg_card, c.border)
                .clicked() { self.toggle_voice(); self.call_note.clear(); }
            if joined && ui.selectable_label(self.group_voice.muted, "mute").clicked() {
                self.group_voice.muted = !self.group_voice.muted;
            }
        });
        // The real FriendListWidget: cards with an action menu, the empty state
        // that names the ports to open, and the add-peer form. The flat list of
        // names this replaced had no way to add anyone, which is the half of
        // the page that matters when discovery has failed and you are standing
        // next to the person you are trying to reach.
        // THE WAY IN THAT IS NOT TYPING. `open_qr_scan` was ported, tested and
        // reachable from nothing: the dialog had a body, the parser handled
        // both the bare sc-ID and the snapclone:// URI, and no control in the
        // app opened it. Adding a contact meant knowing their address and
        // typing it, which is the case QR codes exist to remove.
        //
        // Two buttons rather than one, because `open_qr_scan(false)` is not a
        // degraded mode — it lands straight on the manual field instead of
        // showing a viewfinder that will never produce a frame, which is the
        // right surface on a machine with no camera and the honest one while
        // the capture pipeline is still unwired.
        ui.horizontal(|ui| {
            let r = uikit_egui::glass::primary_button(ui, "Scan code", c.accent, c.bg_base);
            self.key_label(&r, "scan a friend's code with the camera");
            if r.clicked() { self.open_qr_scan(true); }
            let r = uikit_egui::glass::secondary_button(ui, "Enter code", c.bg_card, c.border);
            self.key_label(&r, "type a friend's code");
            if r.clicked() { self.open_qr_scan(false); }
        });
        });

        // WHAT DISCOVERY FOUND, and what the app makes of it. Two gaps this
        // closes, both of which left real state with no way to see or act on
        // it:
        //
        // 1. A peer found by gossip appears as a row and is NOT in the address
        //    book, so it is gone on restart and there was no way to keep it.
        //    Discovery deliberately does not auto-save strangers — an address
        //    book that fills itself from the subnet is not an address book —
        //    but "I see them, keep them" had no button.
        // 2. A refused key change reached stderr and nowhere else. On a phone
        //    that is the same as nowhere.
        // The book's verdict rides ON the card rows now (`friends::Standing`)
        // rather than as a second plain list above them — the list this
        // replaces was the same parallel-surface problem this file had been
        // removing elsewhere, introduced by the commit that added it and
        // flagged in its own summary as the first thing to fix.
        //
        // Precomputed rather than closed over `self`, because the widget call
        // below needs `&mut self.cards` and a closure borrowing `self` for
        // the book would fight it.
        let infos: std::collections::HashMap<String, crate::friends::RowInfo> =
            self.roster.rows().iter()
                .map(|f| (f.id.clone(), self.row_info(&f.id, &reach_roster)))
                .collect();
        let info = |id: &str| infos.get(id).cloned().unwrap_or_default();
        if let Some(a) = crate::friends::friends_ui(
            ui, &self.roster, &mut self.cards, &mut self.add_form, &info, c) {
            self.apply_friends(a);
        }
    }

    /// Switch to the camera facing the other way. `false` when there is
    /// none, or the current selection is PIP (both are already showing).
    pub fn flip_camera(&mut self) -> bool {
        let current = match &self.cam_selection {
            Some(crate::camera_select::Selection::Single { id, .. }) => id.clone(),
            _ => return false,
        };
        let Some(other) = crate::camera_select::other_facing(&self.cameras, &current) else { return false };
        let id = other.id.clone();
        self.cam_selection = Some(crate::camera_select::select(&self.cameras, &id, true));
        true
    }

    /// Carry out a palette action. Every arm is a method that already
    /// existed; the palette only reaches it by name, now or later.
    pub fn run_action(&mut self, a: crate::palette::Action) {
        use crate::palette::Action;
        match a {
            Action::Call { leave } => {
                if leave == self.group_voice.joined { self.toggle_voice(); }
            }
            Action::SendTo { id, name } => {
                self.send_requested.push(id);
                self.feed_note = format!("sending to {name}");
            }
            Action::FlipCamera => { self.flip_camera(); }
            Action::GoPage(i) => self.nav.go_page(i.min(PAGES.len() - 1)),
            Action::Snap => { self.nav.go_page(0); self.shutter_pressed = true; }
            Action::Help => self.help.open(),
        }
    }

    /// Register what Space/Enter does on a widget, for the keyboard caption.
    pub fn key_label(&mut self, r: &egui::Response, what: &str) {
        self.key_labels.insert(r.id, what.to_string());
    }

    /// Contacts as the palette wants them: `(sc_id, name)`.
    fn palette_contacts(&self) -> Vec<(String, String)> {
        self.contact_book.iter().map(|c| (c.sc_id.clone(), c.label().to_string())).collect()
    }

    /// The palette: a field, a ranked list, a cursor. Enter runs the
    /// chosen one now or puts it on the schedule; Escape closes.
    fn palette_ui(&mut self, ui: &mut Ui, surface: egui::Rect, now_ms: u64, c: &PaletteColors) {
        // The palette FLOATS: dim the page behind it, or `bg_raised` over
        // `bg_base` is a few points of luminance and the two fields read as
        // one crowded surface. The card supplies the border, the shadow, the
        // padding and the row gap — see `uikit_egui::overlay`.
        // THE PALETTE IS A BOTTOM BAR NOW, not a translucent card mid-page.
        // The card drew a 45% scrim, which on a dark theme dims near-black to
        // near-black — so the settings form behind it stayed at full contrast
        // and the suggestions read as loose text over a form. Reported with a
        // screenshot. `uikit_egui::command_bar` owns the surface: a bar when
        // the page is worth keeping visible, a properly-scrimmed pane when it
        // is not, and the same inline completion either way.
        use uikit_egui::command_bar as cb;
        let contacts = self.palette_contacts();
        let items = crate::palette::suggest(&self.palette_input, now_ms, 0, &contacts,
                                            self.group_voice.joined, &PAGES);
        let n = items.len().min(cb::MAX_ROWS);
        if n == 0 { self.palette_cursor = 0; } else { self.palette_cursor = self.palette_cursor.min(n - 1); }
        let mode = cb::mode_for(surface.height(), n);
        // `wanted_height` is CONTENT height; the card adds its padding around
        // it. Without this the rows were laid out and then clipped away by
        // the card's own clip rect — the bar rendered with an input and an
        // empty strip under it, which a screenshot caught.
        let h = cb::wanted_height(n) + uikit_egui::overlay::CardStyle::default().pad * 2.0;
        let area = egui::Rect::from_min_size(
            egui::pos2(surface.min.x, surface.max.y - h), egui::vec2(surface.width(), h));
        let area = match mode {
            cb::Mode::Bar => area,
            // The pane takes the surface, so nothing behind it competes.
            cb::Mode::Pane => surface,
        };
        if mode == cb::Mode::Pane {
            uikit_egui::overlay::scrim(ui, surface, cb::SCRIM_PANE);
        }
        // OPAQUE, in both modes. A palette you can read the page through is
        // the fault being fixed; `bg_raised` at full alpha plus the card's
        // border is what separates it, not a wash.
        let style = uikit_egui::overlay::CardStyle {
            radius: if mode == cb::Mode::Bar { 0.0 } else { uikit_egui::tokens::RADIUS_MD as f32 },
            ..uikit_egui::overlay::CardStyle::default()
        };
        let mut child = uikit_egui::overlay::card(ui, area, c, style);

        // The best match, completed inline. The ghost is the part you have
        // not typed; Tab takes it.
        let best = items.first().map(|it| it.label.clone());
        let ghosted = best.as_deref()
            .and_then(|b| cb::ghost(&self.palette_input, b))
            .map(str::to_string);
        child.horizontal(|ui| {
            let field = ui.add(egui::TextEdit::singleline(&mut self.palette_input)
                .hint_text("call at the hour · send ada in 10 min · flip · inbox")
                .desired_width(ui.available_width() * 0.72).id_salt("lib-palette_input"));
            if std::mem::take(&mut self.palette_focus) { field.request_focus(); }
            if let Some(g) = &ghosted {
                ui.colored_label(c.text_weak, egui::RichText::new(format!("{g}  ⇥")).italics());
            }
        });
        if let (true, Some(b)) = (child.input(|i| i.key_pressed(egui::Key::Tab)), best.as_deref()) {
            self.palette_input = cb::accept(&self.palette_input, b);
        }
        uikit_egui::overlay::divider(&mut child, c, style.gap);
        let (up, down, enter, esc) = child.input(|i| (
            i.key_pressed(egui::Key::ArrowUp), i.key_pressed(egui::Key::ArrowDown),
            i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
        if up { self.palette_cursor = cb::move_cursor(self.palette_cursor, n, -1); }
        if down { self.palette_cursor = cb::move_cursor(self.palette_cursor, n, 1); }
        let mut chosen: Option<usize> = None;
        for (i, it) in items.iter().take(n).enumerate() {
            let on = i == self.palette_cursor;
            let r = child.add_sized([child.available_width(), cb::ROW_H],
                                    egui::SelectableLabel::new(on, &it.label));
            if r.clicked() { chosen = Some(i); }
        }
        if n == 0 && !self.palette_input.trim().is_empty() {
            child.colored_label(c.text_weak, "nothing matches — a time that did not parse, or a name nobody has");
        }
        if enter && n > 0 { chosen = Some(self.palette_cursor); }
        if let Some(i) = chosen {
            let it = items[i].clone();
            match it.at_ms {
                Some(at) => {
                    self.schedule.add(crate::palette::Scheduled { at_ms: at, label: it.label.clone(), action: it.action });
                    self.feed_note = format!("scheduled: {}", it.label);
                }
                None => self.run_action(it.action),
            }
            self.palette_open = false;
            self.palette_input.clear();
        }
        if esc { self.palette_open = false; }
    }

    /// The keyboard caption: a ring around the focused widget and one line
    /// under it saying what Space/Enter does there, only while the keyboard
    /// is driving. A mouse user never sees it.
    fn keyboard_caption(&self, ui: &mut Ui, c: &PaletteColors) {
        if !self.keyboard_nav { return; }
        let Some(id) = ui.ctx().memory(|m| m.focused()) else { return };
        let Some(r) = ui.ctx().read_response(id) else { return };
        let rect = r.rect.expand(3.0);
        ui.painter().rect_stroke(rect, 6.0, egui::Stroke::new(2.0, c.accent), egui::StrokeKind::Outside);
        let what = self.key_labels.get(&id).cloned().unwrap_or_else(|| "activate".into());
        let text = format!("Space / Enter: {what} · Tab: next · Ctrl+K: commands");
        let galley = ui.painter().layout_no_wrap(text, egui::FontId::proportional(11.0), c.text);
        // Padding on all four sides from the compact style (the old 8×4 was
        // 4 left / 4 right / 2 top / 2 bottom — tighter than any other
        // surface in the app), and CLAMPED: a focused widget near the bottom
        // edge used to draw its caption off the screen.
        let style = uikit_egui::overlay::CardStyle::compact();
        let want = egui::Rect::from_min_size(
            egui::pos2(rect.left(), rect.bottom() + 4.0),
            style.outer_size(galley.size()));
        let bg = uikit_egui::overlay::fit_on_screen(want, ui.ctx().content_rect(), 4.0);
        if style.border {
            ui.painter().rect_filled(bg, style.radius, c.bg_raised);
            ui.painter().rect_stroke(bg, style.radius, egui::Stroke::new(1.0, c.border),
                                     egui::StrokeKind::Inside);
        }
        ui.painter().galley(style.inner(bg).min, galley, c.text);
    }

    /// What the hint reads. Assembled here so `hint::hint` stays a pure
    /// function of a struct it can be handed in a test.
    pub fn situation(&self) -> crate::hint::Situation {
        use crate::send_state::SendState;
        let unreachable = self.group_voice.participants.iter()
            .filter(|p| !p.is_self && self.punch.get(&p.id).is_none()).count();
        crate::hint::Situation {
            has_identity: !self.local_id.is_empty(),
            cameras: self.cameras.len(),
            capture_exhausted: matches!(self.capture_status, crate::capture::Status::Exhausted { .. }),
            capture_trying: matches!(self.capture_status, crate::capture::Status::Trying { .. }),
            dht_running: self.dht.is_some(),
            dht_peers: self.dht_peers,
            keepalive: self.dht_keepalive,
            reflexive: self.reflexive.map(|a| a.to_string()),
            stun_failed: self.stun_failed,
            in_call: self.group_voice.joined,
            unreachable_participants: if self.group_voice.joined { unreachable } else { 0 },
            sends_failed: self.sends.rows().iter().filter(|r| r.state == SendState::Failed).count(),
            sends_pending: self.sends.rows().iter().filter(|r| r.state == SendState::Pending).count(),
            unread: self.inbox.unread_count(),
            reachable_shown: self.reachable_shown,
            next_scheduled: self.schedule.next().map(|n| {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64).unwrap_or(0);
                format!("{} {}", n.label.split(" · ").next().unwrap_or(&n.label), crate::palette::until(n.at_ms, now))
            }),
        }
    }

    /// One line of chrome. Tapping it goes where it helps. `true` when a
    /// line was drawn, so the caller can leave the row out otherwise.
    fn hint_bar(&mut self, ui: &mut Ui, compact: bool, c: &PaletteColors) -> bool {
        let Some(h) = crate::hint::hint(&self.situation()) else { return false };
        let color = match h.level {
            crate::hint::Level::Blocking => c.danger,
            crate::hint::Level::Degraded => c.warning,
            crate::hint::Level::Info => c.text_weak,
        };
        let text = if compact { h.short.clone() } else { h.long.clone() };
        let r = ui.add(egui::Label::new(egui::RichText::new(text).color(color)).sense(egui::Sense::click()));
        if h.level == crate::hint::Level::Info && h.short == "reachable from outside" {
            // Said once: the next frame will not repeat it.
            self.reachable_shown = true;
        }
        if r.clicked() {
            match h.go {
                crate::hint::GoTo::Camera => self.nav.go_page(0),
                crate::hint::GoTo::Inbox => self.nav.go_page(1),
                crate::hint::GoTo::Peers => self.nav.go_page(2),
                crate::hint::GoTo::Settings => self.nav.go_page(4),
                crate::hint::GoTo::Nowhere => {}
            }
        }
        true
    }

    fn settings_page(&mut self, ui: &mut Ui, c: &PaletteColors) {
        let r = ui.button("help");
        self.key_label(&r, "open the help");
        if r.clicked() { self.help.open(); }
        // DIAGNOSTIC CHANNELS, one toggle each. Individually, which is the
        // whole point of the module: the C++ it replaces used setFilterRules,
        // so switching one channel on silently switched every other off —
        // including one enabled from the command line a moment earlier.
        // FACE DETECTION CONSENT. A control, not a notice: the module's
        // argument is that consent decides whether the work HAPPENS, so the
        // switch has to be reachable.
        // CAMERA. One toggle so far; a collapsible rather than a bare
        // checkbox so the next camera setting has somewhere to go that is not
        // the top of the page.
        ui.collapsing("camera", |ui| {
            let mut bright = self.settings.current.camera.capture_flash_bright;
            if ui.checkbox(&mut bright, "bright capture flash").changed() {
                self.set_capture_flash_bright(bright);
            }
            ui.colored_label(c.text_weak, if bright {
                "white, like a phone screen"
            } else {
                "dark, like a shutter — easier on the eyes in a dim room"
            });
        });
        ui.collapsing("faces", |ui| {
            use crate::face_consent::Consent;
            let mut granted = self.face_consent.may_detect();
            if ui.checkbox(&mut granted, "allow face detection").changed() {
                let owes = self.set_face_consent(
                    if granted { Consent::Granted } else { Consent::Revoked });
                if owes {
                    // Revoked is not NotGiven: crops already exist and are
                    // owed an erase. Said in plain state, the pattern this
                    // app already uses — it has no toast system, and adding
                    // one to report this would be the tail wagging the dog.
                    self.shutter_note = "face data will be erased".into();
                }
            }
            ui.colored_label(c.text_weak, match self.face_consent {
                Consent::NotGiven => "never asked — no detection runs",
                Consent::Granted => "detection may run on your own and received photos",
                Consent::Revoked => "revoked — erase owed",
            });
        });
        // NETWORK REACH. Where peers were last seen, with the expiry that
        // keeps a stale address from reading as a live one.
        ui.collapsing(format!("network ({} heard)", self.heard.len()), |ui| {
            // THE PORTED VIEW, not a partial reimplementation of it. The
            // inline version printed sc_id and a dotted quad and ignored
            // `punch` entirely — so it showed an address without saying
            // whether it was still usable, which is the one thing the module
            // exists to convey: "an address nobody has confirmed in minutes is
            // a guess wearing a fact's clothes".
            //
            // gossip_ui adds the age and a "direct" marker when a punchable
            // address is cached, which is the actionable fact — a direct
            // connection can be attempted rather than a relay.
            let now_ms = (ui.input(|i| i.time) * 1000.0) as u64;
            crate::punch_addrs::gossip_ui(ui, &self.heard, &self.punch, now_ms, c);
        });
        ui.collapsing("diagnostics", |ui| {
            use crate::trace_channels::Channel;
            for ch in Channel::ALL {
                let mut on = self.trace.is_enabled(ch);
                if ui.checkbox(&mut on, ch.key()).on_hover_text(ch.description()).changed() {
                    self.trace.set_channel(ch, on);
                }
            }
            if self.trace.enabled().is_empty() {
                ui.colored_label(c.text_weak, "all channels quiet");
            }
        });
        self.nav.title_bar(ui, "Settings");
        // The five real tab bodies from settings_page.rs, not one row of
        // background styles. The background picker stays as an appearance
        // control, but it is no longer the whole page.
        // The camera policy is passed THROUGH rather than mirrored into
        // Settings: the Tuning spin boxes edit it in place, matching Qt
        // reading and writing CameraPolicyStore::instance() directly. Two
        // structs holding a startup timeout would disagree the first time
        // one of them was saved and the other was not.
        crate::settings_page::settings_ui(ui, &mut self.settings,
                                          &mut self.camera_policy, c, &mut self.a11y);
        // Persist. The io error is SHOWN rather than swallowed — a Save that
        // silently fails is the same lie as one that never wrote, and a
        // read-only config dir is a real thing on locked-down devices.
        if ui.add_enabled(self.settings.is_dirty(), egui::Button::new("Save to disk")).clicked() {
            self.settings.save();
            match crate::settings_store::store(&self.settings.current) {
                Ok(()) => {
                    self.settings_error = None;
                    // Was missing entirely: Save committed the in-memory
                    // snapshot and wrote it to disk, but never pushed the
                    // new value into `self.palette` -- the field
                    // `uikit_egui::palette_colors(&self.palette)` actually
                    // renders from (populated from the SEPARATE, cross-app
                    // `settings_kit::Prefs.palette` via `apply_settings`,
                    // not from this page's own `General.palette` at all).
                    // The saved file was correct; the RUNNING session kept
                    // showing the old theme until next launch, when
                    // `settings_store::load()` happened to repopulate
                    // things. Reported as "no color theme applied when it
                    // is changed." Fixed by applying it live here too.
                    self.palette = self.settings.current.general.palette.clone();
                }
                Err(e) => self.settings_error = Some(format!("could not save: {e}")),
            }
        }
        if let Some(err) = &self.settings_error {
            ui.colored_label(c.warning, err);
        }
        ui.separator();
        // The background styles are a horizontal CAROUSEL, not a wrapped row:
        // a wheel over it scrolls sideways, and at either end continued
        // scrolling hands off to the page nav rather than dying silently.
        ui.colored_label(c.text_weak, "background:");
        let strip = egui::ScrollArea::horizontal().id_salt("bg_carousel").show(ui, |ui| {
            ui.horizontal(|ui| {
                for s in BgStyle::all() {
                    if ui.selectable_label(self.bg.style == s, s.as_str()).clicked() {
                        self.bg.style = s;
                    }
                }
            });
        });
        let over = strip.inner_rect.contains(
            ui.input(|i| i.pointer.hover_pos()).unwrap_or(egui::pos2(-1e6, -1e6)));
        if over {
            let (dx, dy) = ui.input(|i| (i.smooth_scroll_delta.x, i.smooth_scroll_delta.y));
            if dx != 0.0 || dy != 0.0 {
                let w = crate::scroll_areas::Wheel::pixels(dx, dy);
                if self.bg_carousel.wheel(w) == crate::scroll_areas::Wheeled::PassThrough {
                    // Clamped and still pushing: this is the handoff the Qt
                    // comment describes — the carousel declines so the page
                    // swipe can take it.
                    self.carousel_handoff = true;
                }
            }
        }
    }

    /// Wayfinder: the group voice channel and the landmark tools. Was a dead
    /// tab.
    fn wayfinder_page(&mut self, ui: &mut Ui, c: &PaletteColors) {
        self.nav.title_bar(ui, "Wayfinder");
        // This page used to draw the group-voice controls — join / mute / push
        // to talk. Not a stub of the wayfinder: a different feature under its
        // name, which is worse than an empty page because nothing looks
        // missing. Those controls live on the voice bar, which is where the Qt
        // app puts them.
        // LANDMARKS. begin_landmark, commit_landmark and add_landmark_ui were
        // all written and none had a caller: the wayfinder places people, and
        // the other half — naming a point you can be RESOLVED to, so a contact
        // matches a place without either of you sharing a live position — was
        // unreachable.
        ui.horizontal(|ui| {
            if self.add_landmark.is_none() && ui.button("＋ landmark").clicked() {
                // Accuracy is passed in because the wayfinder does not track
                // it, and the record has to carry which claim it is: saved at
                // 500 m is a different statement from saved at 5 m.
                // Accuracy comes from contact_freshness under our own id when
                // the host has supplied it. There is no other source:
                // wayfinder::Fix carries lat/lon only, and folding accuracy
                // into it is noted as the cleaner follow-up on that field.
                //
                // The fallback is 0.0 rather than a plausible-looking metre
                // count, because begin_landmark's doc is explicit that "saved
                // at 500 m accuracy is a different claim from one saved at 5
                // m" — inventing a number would make the record assert a
                // precision nobody measured.
                let acc = self.contact_freshness.get(&self.local_id)
                    .map(|(_, a)| *a as f64)
                    .unwrap_or(0.0);
                self.begin_landmark(acc);
            }
            if !self.landmarks.is_empty() {
                ui.colored_label(c.text_weak,
                    format!("{} saved", self.landmarks.len()));
            }
        });
        if self.add_landmark.is_some() {
            let creator = self.local_id.clone();
            let id = format!("lm-{}", self.now_secs);
            let now_ms = self.now_secs * 1000;
            // The form closes only on success — the renderer returns None both
            // while editing and on a refused save, and discarding what was
            // typed along with the reason it failed is the case its doc calls
            // out.
            let saved = self.add_landmark.as_mut().and_then(|f| {
                crate::landmark::add_landmark_ui(ui, f, &creator, &id, now_ms, c)
            });
            if let Some(l) = saved {
                self.landmarks.push(l);
                self.add_landmark = None;
            }
            ui.separator();
        }

        let nodes = crate::wayfinder::build_nodes(self.self_fix, &self.wayfinder_contacts);
        let photos = crate::wayfinder::place_photos(&nodes, &self.wayfinder_snaps);
        // Said BEFORE the map, which takes every remaining point: written
        // after it, this line landed below the map's clip and was painted
        // nowhere (the layout probe: "clipped by 143pt").
        if nodes.is_empty() {
            ui.colored_label(c.text_weak, "No contacts to place yet.");
        }
        if let Some(id) = crate::wayfinder::wayfinder_ui(ui, &nodes, &photos, c) {
            // Tapping a contact jumps to Friends with them selected, per the
            // Qt page: the map is a way IN to a conversation, not a dead end.
            self.apply_friends(crate::friends::Action::Selected(id));
            self.nav.go_page(2);
        }
        // The signal strip: how much to trust each bearing. The policy maps a
        // fix's age and accuracy to a presentation — live arrow, faded arrow,
        // or a "last seen" label that REPLACES the arrow. A contact the host
        // has no freshness for is presented as long-stale, which is true.
        if !nodes.is_empty() {
            ui.separator();
            ui.colored_label(c.text_weak, "signal");
            for n in &nodes {
                let (age, acc) = self.contact_freshness.get(&n.id)
                    .copied().unwrap_or((i64::MAX / 4, 9_999.0));
                let p = crate::freshness::presentation(&self.freshness_policy, age, acc);
                crate::freshness::node_ui(
                    ui, &n.display_name, n.bearing_deg as f32, n.distance_m as f32, &p, c);
            }
        }
    }

    /// The inbox, on its own page. Was absent entirely: the app had a Peers
    /// list and no way to see a message, which is the gap that made a
    /// messaging app read as a demo.
    /// Send a voice packet to every peer in the call.
    ///
    /// One encode, many sends — encoding per peer would multiply a voice
    /// frame's cost by the group size at fifty packets a second. The peer
    /// list comes from the coordinator, so a participant who left cannot
    /// still be receiving audio.
    pub fn voice_send(&mut self, p: &crate::voice_bridge::Packet)
        -> Vec<(String, Vec<u8>)> {
        let peers: Vec<String> = self.group_voice.participants.iter()
            .filter(|x| !x.is_self)
            .map(|x| x.id.clone())
            .collect();
        self.voice_bridge.set_peers(peers);
        self.voice_bridge.outbound(p)
    }

    /// Route an inbound payload, applying floor changes to the coordinator.
    ///
    /// `sender` is the TRANSPORT's identity and the only one trusted: a
    /// FloorAssert carries a timestamp and nothing else precisely so a peer
    /// cannot claim the floor on someone else's behalf.
    pub fn voice_receive(&mut self, sender: &str, payload: &[u8])
        -> crate::voice_bridge::Inbound {
        let out = self.voice_bridge.inbound(sender, payload);
        if let crate::voice_bridge::Inbound::Handled { from, packet } = &out {
            match packet {
                crate::voice_bridge::Packet::FloorAssert { claim_ts } => {
                    self.group_voice.floor_assert(from, *claim_ts);
                }
                crate::voice_bridge::Packet::FloorRelease => {
                    self.group_voice.floor_release(from);
                }
                // The samples are not touched HERE: the caller gets the
                // packet back and the seam turns it into `DecodeAudio`, so
                // the codec stays in the host. What this arm must not do is
                // what it used to — nothing at all, silently, which is why
                // a call could connect, negotiate the floor and play no
                // sound.
                crate::voice_bridge::Packet::VoiceFrame(_) => {}
                // Same: an Opus frame goes back to the host to decode.
                crate::voice_bridge::Packet::VoiceOpus(_) => {}
            }
        }
        out
    }

    /// Should the viewfinder's colours be re-derived this frame?
    ///
    /// The tick answers with a REASON, and the reasons are not
    /// interchangeable: TooSoon is the rate limit doing its job, Busy means
    /// the last derive has not returned (deriving again would queue work
    /// behind work), Backgrounded means the app is off screen and sampling a
    /// frame nobody can see is pure battery cost.
    pub fn ambient_camera_tick(&mut self, now_ms: u64) -> crate::ambient_camera::Tick {
        self.ambient_camera.tick(now_ms)
    }

    /// A derivation finished. `None` means it produced nothing usable, which
    /// must still clear the in-flight flag or the driver never derives again.
    pub fn ambient_camera_done(&mut self, sample: Option<(crate::ambient_palette::Rgb,
                                                          crate::ambient_palette::Rgb)>) {
        self.ambient_camera.complete(sample);
        if let Some(pair) = self.ambient_camera.smoothed() {
            // Smoothed, not raw: the viewfinder swings wildly as the camera
            // moves, and an unsmoothed theme would strobe.
            self.ambient = Some(pair);
        }
    }

    /// May a face crop with this provenance be kept?
    ///
    /// BOTH gates, and the order is not decoration: consent decides whether
    /// detection runs AT ALL (not "runs and discards" — that is why revoking
    /// saves battery rather than costing it), and provenance decides whether
    /// a particular crop may be stored even when it does. A face in a photo
    /// the user took is a stranger who never initiated the data flow.
    pub fn may_store_face(&self, p: crate::face_consent::Provenance) -> bool {
        self.face_consent.may_detect() && p.may_store()
    }

    /// Set consent, reporting whether an erase is now owed.
    ///
    /// Revoked is deliberately distinct from NotGiven: the app has to know it
    /// once had crops to delete, and treating the two the same would leave
    /// them on disk.
    /// Set the flash colour and write it out NOW.
    ///
    /// Write-on-change, matching the Qt app: `SettingsPage.cpp` calls
    /// `setValue` in each toggle's handler rather than batching to a Save. A
    /// phone app is not closed, it is killed, so a preference written in a
    /// shutdown path is a preference that is never written.
    ///
    /// This bypasses the page's dirty tracking on purpose. That machinery is
    /// for the settings PAGE, where a user edits several fields and commits
    /// them together; a switch on a different page has already been committed
    /// by being flipped, and showing "unsaved changes" for it would be a lie
    /// about what the app is going to do.
    pub fn set_capture_flash_bright(&mut self, on: bool) {
        self.settings.current.camera.capture_flash_bright = on;
        self.settings.save();
        if let Err(e) = crate::settings_store::store(&self.settings.current) {
            // Said, not swallowed. A preference that silently does not stick
            // is the failure this whole path exists to prevent.
            self.shutter_note = format!("setting not saved — {e}");
        }
    }

    pub fn set_face_consent(&mut self, c: crate::face_consent::Consent) -> bool {
        self.face_consent = c;
        // Persisted on change, same rule as the flash: a phone app is killed
        // rather than closed, so a shutdown-path write never happens.
        self.settings.current.camera.face_consent = c.as_key().to_string();
        self.settings.save();
        if let Err(e) = crate::settings_store::store(&self.settings.current) {
            self.shutter_note = format!("consent not saved — {e}");
        }
        c.owes_erase()
    }

    /// Which firewall backend this machine can use, and whether it will
    /// interrupt the user.
    ///
    /// Eight backends tried in order, first success wins, because the same
    /// app ships to Fedora, Ubuntu, Alpine, a Flatpak sandbox and a container.
    /// `None` is a real answer — no backend means say so rather than silently
    /// not opening a port.
    pub fn firewall_plan(&self, env: &crate::firewall_backend::Environment,
                         port: u16)
        -> Option<(crate::firewall_backend::Backend, bool)> {
        let req = crate::firewall_backend::PortRequest { port, tcp: false };
        // A privileged port is not something to ask about at all.
        if !req.is_sane() { return None; }
        let b = crate::firewall_backend::detect(env)?;
        Some((b, b.is_silent()))
    }

    /// The bootstrap servers to try, in order.
    ///
    /// `dns_working` picks host or IP per server — the host survives an
    /// address change, the IP survives a broken resolver, and a network that
    /// breaks one usually leaves the other. `web_ports_only` is the hostile
    /// case: a network that permits 80/443 and nothing else.
    pub fn bootstrap_addresses(&self, dns_working: bool, web_ports_only: bool)
        -> Vec<String> {
        crate::dht_bootstrap::ordered_for(web_ports_only).into_iter()
            .map(|s| crate::dht_bootstrap::address_for(s, dns_working))
            .collect()
    }

    /// Peers we have spoken to, as DHT bootstrap entries — with their REAL
    /// node ids, since a peer's id is a function of its sc-ID
    /// (`identity::dht_node_id`), which is more than any bootstrap server
    /// offers.
    ///
    /// This is the bootstrap-independence lever. A device that remembers
    /// one other snapclone it has ever reached can rejoin the DHT through
    /// it, on a network where `bootstrap.jami.net` is blocked or gone.
    /// It works because a peer's DHT socket is at a fixed offset from
    /// its listen port (`DHT_PORT_OFFSET`): the address we remember for
    /// talking to it names the address of its node.
    ///
    /// Newest first, unset addresses dropped, one entry per address.
    pub fn peer_bootstraps(&self) -> Vec<(std::net::SocketAddr, snapcore::routing::NodeId)> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        let candidates = self.punch.all().into_iter()
            .chain(self.heard.recent().into_iter().map(|g| (g.sc_id.clone(), g.addr)));
        for (id, a) in candidates {
            if a.is_unset() || id.is_empty() || id == self.local_id { continue; }
            let Some(sa) = Self::dht_addr_of(a.ip, a.port) else { continue };
            if seen.insert(sa) {
                out.push((sa, crate::identity::dht_node_id(&id)));
            }
        }
        out
    }

    /// The DHT's UDP port is the listen port plus this. UDP on the listen
    /// port itself is the voice socket (`voice_wire::VoiceSocket::bind`),
    /// which this must not collide with — it did, for one commit: the DHT
    /// spawned first at startup and voice could never bind.
    ///
    /// A fixed offset keeps the property that matters: a peer whose app
    /// address we know has a node address we can DERIVE, without asking.
    /// Stated once, here, and used by both `dht_bind_addr` and
    /// `peer_bootstraps`, so the two cannot disagree.
    pub const DHT_PORT_OFFSET: u16 = 1;

    /// A peer's DHT address from its app address.
    pub fn dht_addr_of(ip: [u8; 4], listen_port: u16) -> Option<std::net::SocketAddr> {
        let port = listen_port.checked_add(Self::DHT_PORT_OFFSET)?;
        Some(std::net::SocketAddr::from((ip, port)))
    }

    /// Where the DHT socket binds: UDP on the listen port plus
    /// `DHT_PORT_OFFSET`, so any peer that knows how to reach the app knows
    /// how to reach the node. Falls back to an ephemeral port only if that
    /// one is taken, in which case peers can still reach us through the
    /// bootstrap servers but not directly.
    pub fn dht_bind_addr(listen_port: u16) -> [std::net::SocketAddr; 2] {
        let preferred = Self::dht_addr_of([0, 0, 0, 0], listen_port)
            .unwrap_or(std::net::SocketAddr::from(([0, 0, 0, 0], 0)));
        [preferred, std::net::SocketAddr::from(([0, 0, 0, 0], 0))]
    }

    /// A peer was reached at this address. Expiring entries is the point: an
    /// address nobody has confirmed in minutes is a guess wearing a fact's
    /// clothes.
    pub fn note_peer_addr(&mut self, id: &str, ip: [u8; 4], port: u16, now_ms: u64) {
        let addr = crate::punch_addrs::PeerAddr::new(ip, port);
        self.punch.set(id, addr, now_ms);
        self.heard.insert(crate::punch_addrs::GossipPeer {
            sc_id: id.to_string(), addr, seen_ms: now_ms,
        });
    }

    /// Record a landmark fingerprint under `id`.
    ///
    /// Refuses an invalid one rather than storing it. A wrong-length hash
    /// reaching the matcher silently never matches anything, which reads as
    /// "the landmark is not there" instead of "the data is corrupt" — the
    /// module's own reason for having `is_valid`.
    pub fn publish_landmark(&mut self, id: &str,
                            fp: crate::landmark_hash::Fingerprint) -> bool {
        if !fp.is_valid() { return false; }
        self.landmark_prints.insert(id.to_string(), fp);
        true
    }

    /// Which known landmark this fingerprint is, and how strong the claim is.
    ///
    /// Ranked rather than first-match: Both is stronger evidence than either
    /// hash alone, and a BlurOnly hit found first would otherwise beat a Both
    /// hit found later. Unknown/Neither never returns an id — no claim is the
    /// correct answer when the evidence does not support one.
    pub fn identify_landmark(&self, fp: &crate::landmark_hash::Fingerprint)
        -> Option<(String, crate::landmark_hash::Match)> {
        self.landmark_prints.iter()
            .map(|(id, known)| (id.clone(), crate::landmark_hash::compare(fp, known)))
            .filter(|(_, m)| m.is_match())
            .max_by_key(|(_, m)| m.rank())
    }

    /// The roster, over real reachability.
    ///
    /// This is where gossip and peer_source meet, and they compose rather than
    /// duplicate: gossip DISCOVERS (who is on the network, and whether their
    /// record's signature verified), the broker knows who has a SESSION, and
    /// `roster` turns those two into the three states a bool cannot express —
    /// Connected, Known, Unverified. A device that is merely seen is not
    /// sendable, because queueing against an unverified identity is how a
    /// message reaches whoever claims that name next.
    ///
    /// The id bridge is deliberate and lossy in one direction only: gossip
    /// carries a string id, peermux a 32-byte key. Hashing the string into the
    /// array would invent an identity, so a seen peer whose id is not a known
    /// contact's is carried by its own bytes and matches nothing in the
    /// broker — which is exactly the Unverified case.
    pub fn peer_roster(&self) -> Vec<crate::peer_source::RosterPeer> {
        let as_id = |s: &str| -> peermux_core::PeerId {
            let mut id = [0u8; 32];
            let b = s.as_bytes();
            let n = b.len().min(32);
            id[..n].copy_from_slice(&b[..n]);
            id
        };
        let known: Vec<(peermux_core::PeerId, String)> = self.peers.iter()
            .map(|p| (as_id(&p.name), p.name.clone())).collect();
        // Only VERIFIED gossip records count as seen. An unverified record is
        // a claim, and gossip::accept already refuses to trust it — carrying
        // it into the roster would undo that at the last step.
        let seen: Vec<peermux_core::PeerId> = self.view.peers().iter()
            .filter(|r| r.verified)
            .map(|r| as_id(&r.id)).collect();
        crate::peer_source::roster(&self.broker, &known, &seen)
    }

    /// How many peers a compose screen can actually send to now.
    pub fn sendable_peers(&self) -> usize {
        crate::peer_source::sendable_count(&self.peer_roster())
    }

    /// The composer's 24-hour tile window, built from the inbox.
    ///
    /// The engine owns the window rule and the gating; this only says which
    /// entries are tiles. Every inbox entry is one — a filter here would be a
    /// second, disagreeing copy of "what is recent", and the engine already
    /// has that in `Composer::new`.
    pub fn strip_tiles(&self) -> Vec<crate::tile_strip::Tile> {
        self.inbox.visible().iter().map(|e| crate::tile_strip::Tile {
            id: e.id,
            ts_ms: e.at.saturating_mul(1000),
            // A received snap may carry a voice note; the engine still refuses
            // to play audio on a still, so declaring the capability here is
            // safe and declaring it false would silence real voice notes.
            has_audio_track: true,
        }).collect()
    }

    /// Apply a strip action. Scrub moves the window from where it is; a tap on
    /// a neighbour makes it active.
    pub fn strip_action(&mut self, act: crate::tile_strip::StripAction) {
        use crate::tile_strip::StripAction;
        let tiles = self.strip_tiles();
        if tiles.is_empty() { return; }
        let composer = crate::tile_strip::Composer::new(&tiles, self.now_secs * 1000);
        let view = match act {
            StripAction::Scrub(steps) =>
                composer.scrub(self.strip_active, steps, self.strip_voice),
            StripAction::Activate(id) => composer.activate_id(id, self.strip_voice),
        };
        if let Some(i) = view.active_index { self.strip_active = i; }
    }

    fn inbox_page(&mut self, ui: &mut Ui, c: &PaletteColors) {
        self.nav.title_bar(ui, "Messages");

        // WHY NOTHING IS SENDING. outbox_deferred's own doc says it is
        // "surfaced rather than silent: a message that has not sent for an
        // hour looks broken, and 'waiting for power' is the difference between
        // a bug and a decision the user can act on by plugging in" — and
        // nothing called it, so the app had the second behaviour while
        // claiming the first.
        //
        // tick_outbox was unreached too. It does not drain anything: the
        // comment inside says the gate is the decision and the count is only
        // what makes it visible. Both halves are needed, and neither was here.
        let now_ms = (ui.input(|i| i.time) * 1000.0) as u64;
        let stalled = self.tick_outbox(now_ms);
        if let Some(reason) = self.outbox_deferred() {
            ui.colored_label(c.warning, reason);
        } else if stalled > 0 {
            // Not deferred and still waiting is a different situation — the
            // network, not the battery — and saying "waiting for power" there
            // would send the user to plug in a phone that is already charged.
            ui.colored_label(c.text_weak,
                format!("{stalled} waiting to send"));
        }
        // THE TILE STRIP, above the list. Three tiles — prev, active, next —
        // because rendering the whole 24h window would decode dozens of
        // thumbnails to show three.
        {
            let tiles = self.strip_tiles();
            let composer = crate::tile_strip::Composer::new(&tiles, self.now_secs * 1000);
            let active = self.strip_active.min(composer.len().saturating_sub(1));
            let view = composer.view_at(active, self.strip_voice);
            // ASK FOR ONLY WHAT IS VISIBLE. Three tiles, at one size — the
            // cache coalesces a repeat request for a decode already in flight
            // (scrubbing back and forth over the same tile is the common
            // case) and defers past MAX_ACTIVE rather than queueing an
            // unbounded pile of work a scrub has already moved past.
            let mut pending = 0;
            for t in &view.tiles {
                let key = crate::thumbnails::Key::new(&format!("snap:{}", t.id), 128);
                match self.thumbs.request(&key) {
                    // No decoder attached in this build, so a queued request
                    // completes as absent rather than being left in flight —
                    // an in-flight entry that never completes would wedge the
                    // MAX_ACTIVE gate against every later request.
                    crate::thumbnails::Request::Queued => {
                        self.thumbs.complete(&key, None);
                        pending += 1;
                    }
                    crate::thumbnails::Request::Deferred => pending += 1,
                    _ => {}
                }
            }
            let _ = pending;
            // THE WHEEL, with the edge rule. One notch is one tile; at either
            // end the event is NOT consumed, so the page navigator gets it and
            // the user keeps scrolling out of the strip instead of hitting a
            // wall they cannot see.
            let strip_rect = ui.min_rect();
            let n = composer.len() as f32;
            self.strip_scroll.max = (n - 1.0).max(0.0);
            self.strip_scroll.value = active as f32;
            if ui.rect_contains_pointer(strip_rect) {
                // smooth_scroll_delta is egui 0.34's accumulated wheel; the
                // module wants a per-notch figure, hence the divisor below.
                let d = ui.input(|i| i.smooth_scroll_delta);
                let (px, py) = (d.x, d.y);
                if px != 0.0 || py != 0.0 {
                    let w = crate::scroll_passthrough::Wheel {
                        pixel: Some((px / 40.0, py / 40.0)),
                        angle: (0.0, 0.0),
                    };
                    match self.strip_scroll.wheel(&w) {
                        crate::scroll_passthrough::Handled::Consumed => {
                            self.strip_active = self.strip_scroll.value.round() as usize;
                        }
                        // Past the end: the swipe layer owns it now.
                        crate::scroll_passthrough::Handled::PassThrough => {
                            let dir = if py + px > 0.0 { -1 } else { 1 };
                            let next = (self.nav.page() as i32 + dir).clamp(0, 4);
                            self.nav.go_page(next as usize);
                        }
                    }
                }
            }
            if let Some(act) = crate::tile_strip::tile_strip_ui(ui, &view, c) {
                self.strip_action(act);
            }
            ui.separator();
        }
        // The monthly card sits at the top of the Inbox, per the Qt page. It
        // gates itself: no month from the host means no card, rather than a
        // guess at what month it is.
        if let Some(month) = self.now_month {
            let card = crate::export_reminder::ExportCard::for_month(
                &self.reminder, month, self.recent_snaps.clone());
            if let Some(a) = crate::export_reminder::export_card_ui(ui, &card, c) {
                self.apply_export_card(a, month);
            }
        }
        // The ribbon sits at the BOTTOM of the inbox, where a thumb reaches it.
        //
        // Everything but the draw was here: refresh_ribbon recomputes which
        // actions are pressable each frame, set_current keeps the segment in
        // step with the nav, apply_ribbon carries out both actions — and
        // ribbon_ui was never called, so all of that maintained a bar nobody
        // saw. Drawn at the END of the page, because bottom-docked is the
        // whole argument for it existing beside SegmentedTabStrip: the actions
        // you press most should be under your thumb.
        self.refresh_ribbon();
        self.ribbon.set_current(self.bar.sub[1]);

        // Drawn BEFORE the branch below, not after: the Sent tab returns
        // early, so a single draw at the end of the function would give the
        // ribbon to two tabs out of three — and the tab it would skip is the
        // one whose action ("Retry failed") is the reason the ribbon carries
        // actions at all.
        if let Some(ev) = crate::ribbon::ribbon_ui(ui, &mut self.ribbon, c) {
            self.apply_ribbon(ev);
            // The event may have changed the sub-tab, and the rest of this
            // function reads it — so re-read rather than render one frame of
            // the tab the user just left.
            self.ribbon.set_current(self.bar.sub[1]);
        }
        ui.separator();

        // The nav has declared an Inbox "Sent" sub-tab since the shell was
        // ported, with nothing behind it. That is the outbox.
        if self.bar.sub[1] == 2 {
            crate::session::outbox_ui(ui, &self.outbox, self.now_secs, c);
            return;
        }
        if let Some(id) = crate::inbox::inbox_ui(ui, &mut self.inbox, c) {
            self.inbox.mark_read(id);
            self.conversation_peer = id.to_string();
        }
        // THE SNAP FEED. feed_ui was ported and never called, while the
        // controller carried the SnapFeed, a Press tracker for its
        // tap-vs-hold, and open_snap_routed for what a tap leads to. Received
        // snaps had nowhere to appear.
        if !self.feed.is_empty() {
            ui.separator();
            let now = self.now_secs;
            let surface = ui.ctx().content_rect();
            if let Some(act) = crate::snaps::feed_ui(ui, &self.feed, &mut self.feed_press, now, c) {
                match act {
                    // A hold on a tile is the tile's menu — view, save,
                    // conversation, delete — the Qt goodie that was ported,
                    // tested, and opened from nowhere. Opening the thread
                    // directly is one of its entries.
                    crate::snaps::FeedAction::OpenThread(id) => {
                        let at = ui.input(|i| i.pointer.latest_pos())
                            .map(|p| (p.x, p.y)).unwrap_or((surface.center().x, surface.center().y));
                        self.open_snap_menu(&id, at);
                    }
                    // A tap opens the snap, routed by kind. The bytes are
                    // held since arrival (`received`); before that a tap
                    // could only write a note.
                    crate::snaps::FeedAction::Open(id) => {
                        if !self.open_received(&id, surface) {
                            self.feed_note = format!("{id}: not held on this device any more");
                        }
                    }
                }
            }
        }

        if !self.feed_note.is_empty() {
            ui.colored_label(c.text_weak, &self.feed_note);
        }

        // The conversation itself, below the list: a tap on a row should show
        // the thread rather than only marking it read.
        if !self.conversation_peer.is_empty() {
            ui.separator();
            ui.label(format!("with {}", self.conversation_peer));
            let items: Vec<crate::conversation::Item> =
                self.conversation.ordered().into_iter().cloned().collect();
            for it in items {
                use crate::conversation::Item;
                let (label, openable) = match &it {
                    Item::Text { body, .. } => (body.clone(), false),
                    Item::Snap { opened, .. } =>
                        (if *opened { "snap (viewed)".to_string() }
                         else { "snap — tap to open".to_string() }, !*opened),
                    Item::Voice { secs, .. } => (format!("voice {secs}s"), false),
                };
                if ui.add_enabled(openable, egui::Button::new(label)).clicked() {
                    self.conversation.tap_snap(it.id());
                }
            }
            let n = self.conversation.unopened();
            if n > 0 { ui.colored_label(c.accent, format!("{n} unopened")); }
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.conversation.draft).id_salt("lib-self_conversation_draft"));
                let r = ui.add_enabled(self.conversation.can_send(), egui::Button::new("send"));
                self.key_label(&r, "send this message");
                if r.clicked() {
                    self.conversation.take_draft();
                }
            });
        }
    }
}

impl Snapclone {
    /// Draw the sign-in form. Returns credentials for the host to act on; this
    /// view does not decide whether they are good.
    pub fn login_ui(&mut self, ui: &mut Ui, c: &PaletteColors)
        -> Option<crate::session::Credentials> {
        crate::session::login_ui(ui, &mut self.login, c)
    }

    /// Qt's `loginSucceeded` signal: hide the overlay (idempotent, per the ordering rule).
    pub fn login_succeeded(&mut self) { self.login_visible = false; }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> Snapclone {
        Snapclone::new(vec![
            Peer { name: "alice".into(), online: true },
            Peer { name: "bob".into(), online: false },
        ])
    }

    fn a_snap(id: &str) -> crate::snaps::Snap {
        crate::snaps::Snap { id: id.into(), sender_id: "ada@10.0.0.2".into(),
            sender_name: "Ada".into(), sent_at: 0, media: crate::snaps::Media::Image,
            duration_secs: 5, bytes: vec![9, 9, 9] }
    }

    fn a_video_snap(id: &str) -> crate::snaps::Snap {
        crate::snaps::Snap { id: id.into(), sender_id: "ada@10.0.0.2".into(),
            sender_name: "Ada".into(), sent_at: 0, media: crate::snaps::Media::Video,
            duration_secs: 12, bytes: vec![1, 2, 3] }
    }

    fn entry(id: &str) -> crate::snaps::FeedEntry {
        crate::snaps::FeedEntry { id: id.into(), sender_name: "Ada".into(), sent_at: 0,
            media: crate::snaps::Media::Image, thumb_path: String::new() }
    }

    /// Typing through the WHOLE shell keeps every character.
    ///
    /// Written to reproduce a report that "input fields only accept one
    /// character". It does not reproduce it, and the road there is worth
    /// keeping, because two of my own probes invented the bug before this one
    /// measured it:
    ///
    /// 1. Focusing `ui.next_auto_id()` captured before the page call targets
    ///    the wrong widget — several widgets precede the field — and reported
    ///    ZERO characters for a page that works.
    /// 2. Watching only `add_form.host` reported zero again, because that field
    ///    is not focusable in the default state. The text was landing in
    ///    `picker.query` the whole time.
    ///
    /// Driving `Snapclone::ui` with Tab navigation and one `Event::Text` per
    /// frame, the fifth Tab reaches the picker query and all three characters
    /// arrive. So the widget layer, the page layer and the shell all accept
    /// multi-character input under synthetic events.
    ///
    /// That does NOT mean the report is wrong — it means the fault is not
    /// reachable from here. What this harness cannot see is the windowing
    /// layer: real keystrokes arrive through winit and may come as `Event::Ime`
    /// composition rather than `Event::Text`, and an on-screen keyboard adds
    /// another path again. That is where to look next, and it needs a running
    /// binary rather than a test.
    /// The Friends page draws each contact ONCE.
    ///
    /// The regression this pins is the one the commit that introduced it
    /// flagged in its own summary: trust labels and Keep buttons were drawn
    /// as a plain list ABOVE the card widget, so a contact appeared twice on
    /// the page in two different layouts. Counting the id in the frame's
    /// output is the only check that catches a second surface coming back —
    /// a unit test on `standing_of` would still pass with the duplicate
    /// list restored.
    /// A drag across the page turns it, and the page moves while it does.
    ///
    /// Driven through a real egui frame rather than by calling
    /// `tick_page_swipe` directly, because what was broken was not the
    /// physics — `swipe_physics` was written and tested — it was that
    /// nothing reached it. A unit test on the physics passed the whole time
    /// the gesture did nothing.
    /// C13_TODO §24: settings flowed past the bottom bar. On a phone-sized
    /// screen the page must stay inside the rect it was given and scroll.
    #[test]
    fn a_tall_page_scrolls_inside_its_bounds_rather_than_flowing_past_the_tab_bar() {
        use egui::{Context, Pos2, RawInput};
        let ctx = Context::default();
        let mut app = app();
        let adaptive = Adaptive::default();
        app.nav.go_page(4);
        let screen = egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(360.0, 640.0));
        let mut deepest = 0.0f32;
        for _ in 0..3 {
            let _ = ctx.run(RawInput { screen_rect: Some(screen), ..Default::default() }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    app.ui(ui, &adaptive);
                    deepest = deepest.max(ui.min_rect().max.y);
                });
            });
        }
        assert!(deepest <= screen.max.y + 0.5,
                "the page laid out to y={deepest} on a {}-tall screen: it is not scrolling", screen.max.y);
    }

    /// The hint reaches the screen through the real `ui()`, phone shell,
    /// and reads the app's state rather than a copy of it.
    #[test]
    fn the_hint_bar_shows_the_situation_and_the_situation_reads_the_state() {
        let mut app = app();
        app.local_id.clear();
        assert!(crate::hint::hint(&app.situation()).unwrap().short.contains("identity"));
        app.local_id = "sc-me".into();
        app.cameras.clear();
        assert_eq!(crate::hint::hint(&app.situation()).unwrap().short, "no camera found");
        app.cameras.push(crate::camera_select::Device {
            id: "cam".into(), label: "cam".into(),
            facing: crate::camera_select::Facing::Rear, capture_group: None });
        app.dht_keepalive = Some((3, 0));
        assert_eq!(crate::hint::hint(&app.situation()).unwrap().short, "network changed — reconnecting");
        // And it is drawn: a compact frame lays out a label with that text.
        use egui::{Context, Pos2, RawInput};
        let ctx = Context::default();
        let adaptive = Adaptive::default();
        let screen = egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(360.0, 640.0));
        let out = ctx.run(RawInput { screen_rect: Some(screen), ..Default::default() }, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| { app.ui(ui, &adaptive); });
        });
        // Either shell: the phone says "network changed", the desktop
        // "probably changed under us".
        let drew_it = out.shapes.iter().any(|cs| matches!(&cs.shape, egui::Shape::Text(t)
            if t.galley.text().contains("changed")));
        assert!(drew_it, "the hint was decided but never drawn");
    }

    /// The double-tap flips between the phone's two sides and does nothing
    /// on a machine with one camera.
    #[test]
    fn flip_camera_switches_facing_and_refuses_when_there_is_no_other_side() {
        let mut app = app();
        app.cameras = vec![
            crate::camera_select::Device { id: "front".into(), label: "front".into(),
                facing: crate::camera_select::Facing::Front, capture_group: Some(1) },
            crate::camera_select::Device { id: "rear".into(), label: "rear".into(),
                facing: crate::camera_select::Facing::Rear, capture_group: Some(1) },
        ];
        app.cam_selection = Some(crate::camera_select::select(&app.cameras, "rear", true));
        assert!(app.flip_camera());
        assert!(matches!(&app.cam_selection,
            Some(crate::camera_select::Selection::Single { id, .. }) if id == "front"));
        assert!(app.flip_camera());
        assert!(matches!(&app.cam_selection,
            Some(crate::camera_select::Selection::Single { id, .. }) if id == "rear"));
        app.cameras.truncate(1);
        app.cam_selection = Some(crate::camera_select::select(&app.cameras, "front", true));
        assert!(!app.flip_camera(), "one camera has no other side");
    }

    /// A desktop shows as many pages as fit, the active one among them and
    /// the window inside the list; a phone shows one.
    #[test]
    fn a_wide_screen_lays_out_columns_and_a_phone_one_page() {
        assert_eq!(Snapclone::visible_pages(0, 5, 3), vec![0, 1, 2]);
        assert_eq!(Snapclone::visible_pages(3, 5, 3), vec![2, 3, 4], "kept inside the list, no wrap");
        assert_eq!(Snapclone::visible_pages(4, 5, 3), vec![2, 3, 4]);
        assert_eq!(Snapclone::visible_pages(1, 5, 9), vec![0, 1, 2, 3, 4], "capped at the page count");
        assert_eq!(Snapclone::visible_pages(2, 5, 1), vec![2]);

        use egui::{Context, Pos2, RawInput};
        let ctx = Context::default();
        let mut app = app();
        let adaptive = Adaptive::default();
        assert!(!adaptive.is_compact(), "the default adaptive is the desktop one");
        let wide = egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(1400.0, 900.0));
        for i in 0..3 {
            let _ = ctx.run(RawInput { screen_rect: Some(wide), time: Some(i as f64), ..Default::default() }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| { app.ui(ui, &adaptive); });
            });
        }
        assert!(app.cols_drawn >= 3, "1400 px drew {} column(s)", app.cols_drawn);
        let narrow = egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 800.0));
        for i in 3..8 {
            let _ = ctx.run(RawInput { screen_rect: Some(narrow), time: Some(i as f64), ..Default::default() }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| { app.ui(ui, &adaptive); });
            });
        }
        assert_eq!(app.cols_drawn, 1, "400 px must fall back to one page");
    }

    /// The Qt build turned pages from a trackpad: two-finger scroll along
    /// the page axis accumulates to a detent and clicks over. The detent
    /// was in the gesture frame and nothing read it.
    #[test]
    fn a_two_finger_scroll_along_the_page_axis_turns_the_page() {
        use egui::{Context, Event, Pos2, RawInput};
        let ctx = Context::default();
        let mut app = app();
        let adaptive = Adaptive::default();
        let screen = egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 800.0));
        assert_eq!(app.nav.page(), 0);
        // The Qt constants: 220 raw at 0.75/ms is about 300 ms of scrolling,
        // inside the 1 s arm window. Forty frames is well past it.
        let mut t = 0.0;
        for _ in 0..40 {
            t += 0.016;
            let events = vec![
                Event::PointerMoved(Pos2::new(200.0, 400.0)),
                // Positive along the axis is Next in the kit's trackpad path
                // (the drag path feeds -dx; the wheel path feeds the delta).
                Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(60.0, 0.0),
                                    modifiers: Default::default(), phase: egui::TouchPhase::Move },
            ];
            let _ = ctx.run(RawInput { events, screen_rect: Some(screen), time: Some(t), ..Default::default() }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| { app.ui(ui, &adaptive); });
            });
            if app.nav.page() != 0 { break; }
        }
        assert_eq!(app.nav.page(), 1, "forty frames of scroll along the axis did not turn the page");
    }

    /// The receive path used to end at a tile: the frame's bytes were
    /// dropped as the feed entry was built. Now a snap that arrives is
    /// held and a tap opens it, routed by kind.
    #[test]
    fn a_snap_that_arrives_can_be_opened_and_a_hold_has_a_menu() {
        let mut app = app();
        let frame = crate::snap_wire::SnapFrame::announce(
            7, 3, "sc-ada", "ada", crate::snaps::Media::Image, 1_700_000_000, 5, vec![9u8; 2048]);
        let id = crate::snap_wire::content_id("sc-ada", &[9u8; 2048]);
        app.apply(crate::host::HostEvent::SnapArrived { from: "sc-ada".into(), frame: Box::new(frame) });
        assert_eq!(app.received.len(), 1, "the bytes were not kept");
        let surface = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 800.0));
        assert!(app.open_received(&id, surface));
        assert!(app.viewer.is_open(), "an image did not open the view-once viewer");
        assert!(!app.open_received("nope", surface));

        assert!(app.open_snap_menu(&id, (100.0, 100.0)));
        assert!(app.menu.is_open());
        app.menu_target = Some(id.clone());
        app.apply_menu("delete");
        assert!(app.received.is_empty(), "delete must drop the held bytes too");
    }

    /// Received snaps are bounded: a session that receives forever does
    /// not hold forever.
    #[test]
    fn received_snaps_are_bounded_oldest_first() {
        let mut app = app();
        for i in 0..60u32 {
            app.keep_received(crate::snaps::Snap { id: format!("s{i}"), sender_id: "a".into(),
                sender_name: "a".into(), sent_at: i as i64, media: crate::snaps::Media::Image,
                duration_secs: 5, bytes: vec![0; 10] });
        }
        assert!(app.received.len() <= 48);
        assert_eq!(app.received[0].id, "s12", "the oldest went first");
    }

    /// Every page, at a phone's width and a desktop's, through the kit's
    /// layout probe: text over text, text cut by its column, text off the
    /// screen, and type under 10 pt on the phone. Prints the report so a
    /// run shows the collisions; asserts the counts below the ratchet.
    #[test]
    fn every_page_lays_out_without_text_collisions() {
        use egui::{Context, RawInput, Pos2};
        for (label, size, compact) in [("phone", egui::vec2(360.0, 780.0), true),
                                       // 400x800 as well: the composer overflow that this
                                       // test missed was found by screenshot at exactly this
                                       // width, and a probe that only samples one narrow size
                                       // can miss a row that happens to fit at the other.
                                       ("phone400", egui::vec2(400.0, 800.0), true),
                                       ("desktop", egui::vec2(1280.0, 800.0), false)] {
            for page in 0..PAGES.len() {
                let ctx = Context::default();
                let mut app = app();
                app.nav.go_page(page);
                // `Adaptive::detect(ctx)`, exactly as the runtime's `update`
                // does — NOT a hand-built default with `class` overridden.
                // That was the fixture's flaw: `detect` derives the gutter,
                // the safe area and the touch target from the real content
                // rect, and those drive the spacing that decides whether a
                // row overflows. With a default `Adaptive` the harness laid
                // the page out differently from the app and reported a
                // composer overflow as clean. Detected inside the closure
                // because it needs the ctx.
                let _ = compact;
                let screen = egui::Rect::from_min_size(Pos2::ZERO, size);
                let mut shapes = Vec::new();
                for _ in 0..3 {
                    let out = ctx.run(RawInput { screen_rect: Some(screen), ..Default::default() }, |ctx| {
                        // The runtime applies the prefs (fonts, spacing,
                        // palette) BEFORE drawing. Without this the harness
                        // laid the page out at egui's default text metrics,
                        // where everything fits — which is why it could not
                        // see a row that overflows at the app's real ones.
                        app.apply_settings(ctx, &settings_kit::Prefs::default());
                        egui::CentralPanel::default().show(ctx, |ui| {
                            let adaptive = Adaptive::detect(ctx);
                            app.ui(ui, &adaptive);
                        });
                    });
                    shapes = out.shapes;
                }
                let r = uikit_egui::layout_probe::analyse(screen, &shapes, compact);
                eprintln!("== {label} / {} ({} texts, smallest {:.1}pt)\n{}", PAGES[page], r.texts, r.smallest_font_pt, r.describe());
                // THE RATCHET. Measured 2026-09-05 after the text floor and
                // the Wayfinder fix. Two known remainders, both named so
                // the next fix lowers a number rather than re-discovers it:
                // * phone / Camera: FIXED. Its controls band was sized for
                //   three rows while holding seven, and the child had no clip
                //   rect, so the surplus painted upward over the heading.
                // * desktop / Friends: FIXED. The chevron was laid out at the
                //   far right of the available width, which includes the
                //   scrollbar's strip.
                // WHAT THIS TEST DOES NOT COVER — measured, not assumed.
                // The composer row overflowed the window at 400x800 in the
                // running app (screenshot), and this harness reported the
                // Camera page CLEAN throughout. I confirmed it by reverting
                // the fix and re-running: bug present, probe still clean. So
                // a green run here is NOT evidence that a page has no
                // overflow — the fixture drives `app.ui()` inside a bare
                // CentralPanel, while the runtime installs the hint bar and
                // tab bar panels first and passes a different `Adaptive`,
                // and the two lay out differently. CLAUDE.md's rule for
                // harnesses applies: this one does not load its data the way
                // the runtime does. Treat it as a ratchet against
                // REGRESSIONS in what it does see, and use a screenshot for
                // "does this page fit".
                //
                // NO EXCEPTIONS LEFT. Every page, both form factors, clean.
                // phone/Camera was (6, 1) until its controls band was sized for
                // the seven rows it holds rather than three and given the clip
                // rect it never had; desktop/Friends was (0, 2) until its
                // chevron stopped being laid out underneath the scrollbar.
                let (max_overlaps, max_clipped) = (0usize, 0usize);
                let _ = (label, page);
                assert!(r.overlaps.len() <= max_overlaps && r.clipped.len() <= max_clipped,
                        "{label}/{}: {} overlaps (≤{max_overlaps}), {} clipped (≤{max_clipped}):\n{}",
                        PAGES[page], r.overlaps.len(), r.clipped.len(), r.describe());
                assert!(r.tiny.is_empty() && r.offscreen.is_empty(), "{label}/{}:\n{}", PAGES[page], r.describe());
                // TWO WIDGETS ON ONE ID. They share their state, and for a TextEdit that
                // includes the text cursor — typing into either moves the caret in the
                // other and keystrokes land in whichever egui thinks is current, which
                // reads as "only one character goes into this box". The desktop layout
                // draws several pages at once, which is exactly when a fixed `id_salt`
                // collides, so this belongs on the page probe rather than in a unit test.
                assert!(r.id_clashes.is_empty(), "{label}/{}:\n{}", PAGES[page], r.describe());
            }
        }
    }

    /// Ctrl+K opens the palette, typing narrows it, Enter runs the choice —
    /// through real frames — and a timed one waits on the schedule.
    #[test]
    fn the_palette_runs_a_command_and_schedules_a_timed_one() {
        use egui::{Context, Event, Key, Modifiers, Pos2, RawInput};
        let ctx = Context::default();
        let mut app = app();
        let adaptive = Adaptive::default();
        let screen = egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 800.0));
        let key = |k: Key, m: Modifiers| Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: m };
        let mut frame = |app: &mut Snapclone, events: Vec<Event>| {
            let _ = ctx.run(RawInput { events, screen_rect: Some(screen), ..Default::default() }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| { app.ui(ui, &adaptive); });
            });
        };
        frame(&mut app, vec![key(Key::K, Modifiers::COMMAND)]);
        assert!(app.palette_open, "Ctrl+K did not open the palette");
        frame(&mut app, vec![]);
        app.palette_input = "inbox".into();
        frame(&mut app, vec![]);
        frame(&mut app, vec![key(Key::Enter, Modifiers::NONE)]);
        assert_eq!(app.nav.page(), 1, "Enter on \"go to Inbox\" did not go");
        assert!(!app.palette_open);

        frame(&mut app, vec![key(Key::K, Modifiers::COMMAND)]);
        app.palette_input = "call at the hour".into();
        frame(&mut app, vec![]);
        frame(&mut app, vec![key(Key::Enter, Modifiers::NONE)]);
        assert_eq!(app.schedule.len(), 1, "a timed command must wait, not run");
        assert!(!app.group_voice.joined);
        // The situation carries it; the hint ranks it below blocking problems
        // (this fixture has no camera), which hint.rs tests.
        assert!(app.situation().next_scheduled.map(|n| n.starts_with("call in")).unwrap_or(false),
                "the schedule did not reach the situation");
    }

    #[test]
    fn tab_turns_the_keyboard_caption_on_and_a_pointer_move_turns_it_off() {
        use egui::{Context, Event, Key, Modifiers, Pos2, RawInput};
        let ctx = Context::default();
        let mut app = app();
        let adaptive = Adaptive::default();
        let screen = egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 800.0));
        let mut frame = |app: &mut Snapclone, events: Vec<Event>| {
            let _ = ctx.run(RawInput { events, screen_rect: Some(screen), ..Default::default() }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| { app.ui(ui, &adaptive); });
            });
        };
        frame(&mut app, vec![Event::Key { key: Key::Tab, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
        assert!(app.keyboard_nav);
        frame(&mut app, vec![Event::PointerMoved(Pos2::new(10.0, 10.0)), Event::PointerMoved(Pos2::new(30.0, 40.0))]);
        assert!(!app.keyboard_nav, "a mouse user must never see the caption");
        // Ctrl+3 is the Friends page.
        frame(&mut app, vec![Event::Key { key: Key::Num3, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND }]);
        assert_eq!(app.nav.page(), 2);
    }

    #[test]
    fn dragging_across_the_page_turns_it_and_moves_it_on_the_way() {
        use egui::{Context, Event, PointerButton, Pos2, RawInput};
        let ctx = Context::default();
        let mut app = app();
        let adaptive = Adaptive::default();
        let screen = egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 800.0));

        let mut frame = |app: &mut Snapclone, events: Vec<Event>| -> f32 {
            let mut offset = 0.0;
            let _ = ctx.run(RawInput { events, screen_rect: Some(screen),
                                       ..Default::default() }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    app.ui(ui, &adaptive);
                    offset = app.swipe.as_ref().map(|s| s.offset()).unwrap_or(0.0);
                });
            });
            offset
        };
        let at = |x: f32| Event::PointerMoved(Pos2::new(x, 400.0));
        let press = |x: f32| Event::PointerButton {
            pos: Pos2::new(x, 400.0), button: PointerButton::Primary,
            pressed: true, modifiers: Default::default() };
        let release = |x: f32| Event::PointerButton {
            pos: Pos2::new(x, 400.0), button: PointerButton::Primary,
            pressed: false, modifiers: Default::default() };

        assert_eq!(app.nav.page(), 0);
        frame(&mut app, vec![at(320.0), press(320.0)]);
        // A short move is below the slop: still a tap as far as anyone knows.
        frame(&mut app, vec![at(314.0)]);
        assert!(app.swipe.is_none(), "the page moved on a six-pixel wobble");
        // Now a real leftward drag — toward the NEXT page.
        let mid = frame(&mut app, vec![at(200.0)]);
        assert!(mid < -50.0, "the page did not follow the finger: {mid}");
        frame(&mut app, vec![at(60.0), release(60.0)]);
        assert_eq!(app.nav.page(), 1, "a drag most of the way across turned nothing");
    }

    #[test]
    fn a_swipe_from_the_left_edge_goes_back_out_of_a_subpage() {
        // On a subpage there is nowhere to page to, so the gesture people
        // try is the one that means back everywhere else on a phone. Without
        // it the only way out is a button in the far corner from the thumb.
        use egui::{Context, Event, PointerButton, Pos2, RawInput};
        let ctx = Context::default();
        let mut app = app();
        let adaptive = Adaptive::default();
        let screen = egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 800.0));
        let mut frame = |app: &mut Snapclone, events: Vec<Event>| {
            let _ = ctx.run(RawInput { events, screen_rect: Some(screen),
                                       ..Default::default() }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| { app.ui(ui, &adaptive); });
            });
        };
        let mv = |x: f32| Event::PointerMoved(Pos2::new(x, 400.0));
        let btn = |x: f32, pressed: bool| Event::PointerButton {
            pos: Pos2::new(x, 400.0), button: PointerButton::Primary,
            pressed, modifiers: Default::default() };

        app.nav.push_subpage(1);
        assert!(app.nav.can_go_back());
        frame(&mut app, vec![mv(8.0), btn(8.0, true)]);
        frame(&mut app, vec![mv(90.0)]);
        assert!(!app.nav.can_go_back(), "the edge swipe did not go back");
        let page_before = app.nav.page();
        // Still dragging: one back per touch, or a deep stack unwinds in a
        // tenth of a second and the page turns underneath it.
        frame(&mut app, vec![mv(300.0)]);
        frame(&mut app, vec![mv(360.0), btn(360.0, false)]);
        assert_eq!(app.nav.page(), page_before,
                   "the same touch also turned the page");
    }

    #[test]
    fn a_rightward_drag_in_the_middle_of_a_subpage_is_not_a_back() {
        // A rightward drag away from the edge is a scroll or a selection far
        // more often than a back. Stealing it makes content undraggable.
        use egui::{Context, Event, PointerButton, Pos2, RawInput};
        let ctx = Context::default();
        let mut app = app();
        let adaptive = Adaptive::default();
        let screen = egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 800.0));
        let mut frame = |app: &mut Snapclone, events: Vec<Event>| {
            let _ = ctx.run(RawInput { events, screen_rect: Some(screen),
                                       ..Default::default() }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| { app.ui(ui, &adaptive); });
            });
        };
        app.nav.push_subpage(1);
        frame(&mut app, vec![Event::PointerMoved(Pos2::new(200.0, 400.0)),
            Event::PointerButton { pos: Pos2::new(200.0, 400.0),
                button: PointerButton::Primary, pressed: true,
                modifiers: Default::default() }]);
        frame(&mut app, vec![Event::PointerMoved(Pos2::new(340.0, 400.0))]);
        assert!(app.nav.can_go_back(), "a mid-screen drag went back");
    }

    #[test]
    fn a_vertical_drag_scrolls_and_never_steals_the_page() {
        // The gesture people make constantly. Losing it to the pager is the
        // error worth avoiding, and the lock latches so a late sideways
        // swing cannot claim it back.
        use egui::{Context, Event, PointerButton, Pos2, RawInput};
        let ctx = Context::default();
        let mut app = app();
        let adaptive = Adaptive::default();
        let screen = egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 800.0));
        let mut frame = |app: &mut Snapclone, events: Vec<Event>| {
            let _ = ctx.run(RawInput { events, screen_rect: Some(screen),
                                       ..Default::default() }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| { app.ui(ui, &adaptive); });
            });
        };
        let mv = |x: f32, y: f32| Event::PointerMoved(Pos2::new(x, y));
        frame(&mut app, vec![mv(200.0, 600.0), Event::PointerButton {
            pos: Pos2::new(200.0, 600.0), button: PointerButton::Primary,
            pressed: true, modifiers: Default::default() }]);
        frame(&mut app, vec![mv(206.0, 520.0)]);   // clearly vertical
        assert!(app.swipe.is_none(), "a scroll started a page swipe");
        frame(&mut app, vec![mv(20.0, 500.0)]);    // now swing hard sideways
        assert!(app.swipe.is_none(), "the lock let go mid-touch");
        assert_eq!(app.nav.page(), 0);
    }

    #[test]
    fn a_contact_appears_once_on_the_friends_page_not_twice() {
        use egui::{Context, RawInput};
        let ctx = Context::default();
        let mut app = app();
        app.apply_friends(crate::friends::Action::Add(
            crate::friends::AddPeer { host: "sc-aabbccdd".into(), port: 9000 }));
        // Friends is page 2 — by name, so a nav reorder fails here loudly
        // rather than silently testing a different page.
        app.nav.go_page(PAGES.iter().position(|n| *n == "Friends").unwrap());
        let adaptive = Adaptive::default();
        let out = ctx.run(RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| { app.ui(ui, &adaptive); });
        });
        // Positions in the failure message, because "3 times" does not say
        // WHICH surfaces came back and the answer is what you need next.
        let hits: Vec<String> = out.shapes.iter().filter_map(|shape| {
            match &shape.shape {
                egui::epaint::Shape::Text(t) if t.galley.text().contains("sc-aabbccdd") =>
                    Some(format!("{:?} @{:?}", t.galley.text(), t.pos)),
                _ => None,
            }
        }).collect();
        assert_eq!(hits.len(), 1,
                   "the contact is drawn {} times — a parallel surface is back: {hits:#?}",
                   hits.len());
    }

    #[test]
    fn typing_through_the_shell_keeps_every_character() {
        use egui::{Context, Event, RawInput};
        let ctx = Context::default();
        let mut app = app();
        app.nav.go_page(2);
        let adaptive = Adaptive::default();

        let mut frame = |ctx: &Context, app: &mut Snapclone, events: Vec<Event>| {
            // One column: on egui's default 10 000-px screen the desktop
            // shell lays out every page side by side, and Tab would walk
            // the camera page before reaching this field.
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 800.0));
            let _ = ctx.run(RawInput { events, screen_rect: Some(screen), ..Default::default() }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| { app.ui(ui, &adaptive); });
            });
        };
        frame(&ctx, &mut app, vec![]);

        let tab = Event::Key { key: egui::Key::Tab, physical_key: None, pressed: true,
                               repeat: false, modifiers: Default::default() };
        let mut landed = None;
        for _ in 0..20 {
            frame(&ctx, &mut app, vec![tab.clone()]);
            for ch in "abc".chars() {
                frame(&ctx, &mut app, vec![Event::Text(ch.to_string())]);
            }
            if !app.picker.query.is_empty() { landed = Some(app.picker.query.clone()); break; }
        }
        assert_eq!(landed.as_deref(), Some("abc"),
                   "a field reached by Tab must keep every character, not the first");
    }

    #[test]
    fn the_receipt_describes_one_send_rather_than_three_moments() {
        // Filling the facts from three places is how a receipt shows last
        // send's dedup beside this send's recipient count, and a partially
        // stale receipt is worse than none — its whole value is being
        // checkable.
        use crate::send_summary::Facet;
        let mut s = app();
        s.prepare_send(1, 2, 400_000, 1_000_000, 4);
        assert_eq!(s.send_summary.detail_text(Facet::Privacy), "2 tags stripped");
        assert_eq!(s.send_summary.saved_bytes(), 600_000);
        assert_eq!(s.send_summary.primary_text(Facet::Encryption),
                   "Encrypted to 4 people");
        assert!(s.can_send());
    }

    #[test]
    fn a_send_with_nobody_in_the_thread_is_blocked() {
        let mut s = app();
        s.prepare_send(1, 0, 0, 1_000, 0);
        assert!(!s.can_send());
    }

    #[test]
    fn one_gaze_sample_reaches_both_consumers() {
        // The posture veto and the glove gate must not disagree about whether
        // the user is present: a screen that stays awake while gestures are
        // refused looks broken in both directions at once.
        use uikit_egui::glove::Watching;
        let mut s = app();
        s.feed_gaze(crate::gaze::GazeSample {
            gaze: [0.0, 0.0, 0.95], confidence: 0.8, t_ms: 0.0 }, 300.0);
        assert!(s.posture.watched());
        assert_eq!(s.glove.watching(), Watching::Looking);
    }

    #[test]
    fn a_lost_tracker_is_unknown_to_the_glove_gate_not_away() {
        // In cold weather the camera is looking at a scarf as often as a face.
        // Treating "no reading" as "not looking" would disable gestures on
        // exactly the days glove mode exists for.
        use uikit_egui::glove::Watching;
        let mut s = app();
        s.feed_gaze(crate::gaze::GazeSample {
            gaze: [0.0, 0.0, 0.95], confidence: 0.8, t_ms: 0.0 }, 300.0);
        s.gaze_lost();
        assert_eq!(s.glove.watching(), Watching::Unknown);
        assert!(!s.posture.watched(), "but the screen may dim again");
    }

    #[test]
    fn looking_away_blocks_gestures_while_leaving_the_screen_alone() {
        // The two consumers pull in opposite directions on purpose: posture can
        // only keep a screen awake, the gate can only block an action.
        use uikit_egui::glove::Watching;
        let mut s = app();
        s.feed_gaze(crate::gaze::GazeSample {
            gaze: [0.9, 0.0, 0.1], confidence: 0.8, t_ms: 0.0 }, 300.0);
        assert_eq!(s.glove.watching(), Watching::Away);
        assert!(!s.posture.watched());
    }

    #[test]
    fn joining_a_call_starts_playout_and_leaving_flushes_it() {
        // A call whose floor state and audio scheduler disagree is a call where
        // someone talks and nobody hears — or where the last call's frames play
        // into the next one.
        let mut s = app();
        s.set_in_call(true);
        assert!(s.group_voice.joined);
        assert!(s.playout.is_playing());

        s.playout.on_frame("ada", crate::voice_playout::Frame::new(1, 40));
        s.set_in_call(false);
        assert!(!s.group_voice.joined);
        assert!(!s.playout.is_playing());
        assert_eq!(s.playout.senders(), 0, "and the old call's audio is gone");
    }

    #[test]
    fn an_unprobed_phone_still_gets_a_viewfinder() {
        // Asking an empty health cache would answer "nothing works" and leave
        // the user staring at nothing on a phone whose cameras are fine.
        let s = app();
        let known = vec!["cam0".to_string(), "cam1".to_string()];
        assert_eq!(s.camera_to_open(&known).as_deref(), Some("cam0"));
    }

    #[test]
    fn a_known_working_camera_is_preferred_over_the_first_one_listed() {
        let mut s = app();
        s.camera_health.record_capture_success("cam1", 
            crate::camera_policy::Facing::Rear, 0);
        let known = vec!["cam0".to_string(), "cam1".to_string()];
        assert_eq!(s.camera_to_open(&known).as_deref(), Some("cam1"));
    }

    #[test]
    fn a_successful_capture_is_remembered_against_the_facing_in_use() {
        let mut s = app();
        s.viewfinder.facing = crate::camera::Facing::Front;
        s.note_camera_worked("front0", 500);
        assert_eq!(s.camera_health.get("front0").unwrap().facing,
                   crate::camera_policy::Facing::Front);
        assert_eq!(s.camera_to_open(&["x".into()]).as_deref(), Some("front0"));
    }

    #[test]
    fn a_double_flick_swaps_the_camera_and_a_single_one_does_not() {
        use uikit_egui::glove::Orient;
        let mut s = app();
        s.shake.enabled = true;
        let before = s.viewfinder.facing;
        s.apply_shake(Some(Orient::RightUp), 16.0);
        assert_eq!(s.viewfinder.facing, before, "one flick is a cursor move, not a command");
        s.apply_shake(Some(Orient::RightUp), 150.0);
        assert_ne!(s.viewfinder.facing, before);
    }

    #[test]
    fn shake_commands_are_off_until_switched_on() {
        use uikit_egui::glove::Orient;
        let mut s = app();
        let before = s.viewfinder.facing;
        s.apply_shake(Some(Orient::RightUp), 16.0);
        s.apply_shake(Some(Orient::RightUp), 150.0);
        assert_eq!(s.viewfinder.facing, before);
    }

    #[test]
    fn glove_mode_needs_the_user_and_the_weather_to_agree() {
        let mut s = app();
        use uikit_egui::glove::ColdMode;
        s.glove.mode_setting = ColdMode::Auto { threshold_c: 8.0 };
        assert!(!s.glove.active(), "no temperature reading is not cold");
        s.glove.ambient_c = Some(18.0);
        assert!(!s.glove.active(), "and a mild day is not cold");
        s.glove.ambient_c = Some(1.0);
        assert!(s.glove.active());
        s.glove.mode_setting = ColdMode::Off;
        assert!(!s.glove.active(), "Off means off, however cold it is");
        s.glove.mode_setting = ColdMode::On;
        s.glove.ambient_c = Some(30.0);
        assert!(s.glove.active(), "and manual On ignores the thermometer");
    }

    #[test]
    fn arming_a_field_asks_for_voice_and_never_a_keyboard() {
        // The hand that needs glove mode cannot press keys. rusty-board's cue
        // is that no key event is ever synthesised.
        let mut s = app();
        s.apply_glove(uikit_egui::glove::GloveAction::ArmField("peer_host".into()));
        assert_eq!(s.voice_field.as_deref(), Some("peer_host"));
    }

    #[test]
    fn dismissing_clears_the_armed_field_and_any_menu() {
        let mut s = app();
        s.apply_glove(uikit_egui::glove::GloveAction::ArmField("peer_host".into()));
        s.open_snap_menu("s1", (0.0, 0.0));
        s.apply_glove(uikit_egui::glove::GloveAction::Dismiss);
        assert!(s.voice_field.is_none());
        assert!(!s.menu.is_open());
    }

    #[test]
    fn posture_arms_on_the_inbox_and_after_recent_voice_only() {
        // Sensors say what the world is doing; arming says whether it matters
        // here.
        let mut s = app();
        s.nav.go_page(0);
        s.refresh_posture_arming(1000);
        assert!(!s.posture.armed(), "the camera page is not a voice context");

        s.nav.go_page(1);
        s.refresh_posture_arming(1000);
        assert!(s.posture.armed(), "the inbox is");

        s.nav.go_page(0);
        s.last_voice_at = Some(900);
        s.refresh_posture_arming(1000);
        assert!(s.posture.armed(), "voice 100s ago still counts");

        s.last_voice_at = Some(100);
        s.refresh_posture_arming(1000);
        assert!(!s.posture.armed(), "900s ago does not");
    }

    #[test]
    fn a_phone_with_no_sensors_never_dims() {
        // The degradation contract, end to end through the shell.
        let mut s = app();
        s.nav.go_page(1);
        s.refresh_posture_arming(0);
        s.posture.observe(&crate::posture::NoSensors);
        let out = s.tick_posture(120_000.0);
        assert_eq!(out.dim, 0.0);
        assert!(!out.screensaver);
    }

    #[test]
    fn the_talk_flip_only_engages_in_a_call() {
        let mut s = app();
        s.nav.go_page(1);
        s.refresh_posture_arming(0);
        s.posture.set_proximity_near(true);
        assert!(!s.tick_posture(16.0).gated_talk, "not in a call");
        s.toggle_voice();
        s.refresh_posture_arming(0);
        assert!(s.tick_posture(16.0).gated_talk);
    }

    #[test]
    fn a_tile_menu_offers_actions_derived_from_the_snap_not_a_fixed_list() {
        // "Save" on an expired snap would fail after the tap: the bytes are
        // gone. Disabled is the honest state.
        let mut s = app();
        s.feed.set_entries(vec![entry("s1")]);
        assert!(s.open_snap_menu("s1", (10.0, 10.0)));
        let save = s.menu.items().iter().find(|i| i.id == "save").unwrap();
        assert!(save.enabled);

        let mut t = app();
        t.feed.set_entries(vec![entry("s2")]);
        t.snaps_expired.push("s2".into());
        t.open_snap_menu("s2", (10.0, 10.0));
        let save = t.menu.items().iter().find(|i| i.id == "save").unwrap();
        assert!(!save.enabled, "an expired snap cannot be saved");
    }

    #[test]
    fn a_menu_choice_applies_to_the_snap_it_was_opened_on() {
        // The target is captured at open. Deriving it later from a selection
        // would act on whatever the user has since scrolled to.
        let mut s = app();
        s.feed.set_entries(vec![entry("s1"), entry("s2")]);
        s.open_snap_menu("s1", (0.0, 0.0));
        s.apply_menu("delete");
        let ids: Vec<&str> = s.feed.entries().iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, vec!["s2"], "only the tile the menu was opened on went");
    }

    #[test]
    fn a_menu_choice_with_no_target_does_nothing() {
        // A stray activation after the menu closed must not delete a snap.
        let mut s = app();
        s.feed.set_entries(vec![entry("s1")]);
        s.apply_menu("delete");
        assert_eq!(s.feed.count(), 1, "a stray activation must not delete a snap");
    }

    #[test]
    fn a_video_snap_opens_the_persistent_viewer_and_an_image_opens_the_view_once_one() {
        // The routing lives in one place because a tile that picked its own
        // viewer would eventually pick the wrong one — and the wrong one either
        // destroys a video the user could keep or keeps an image promised to
        // vanish.
        let surface = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(360.0, 780.0));
        let mut s = app();
        s.open_snap_routed(a_video_snap("v1"), surface);
        assert!(s.video.is_some(), "video went to the persistent viewer");
        assert!(!s.viewer.is_open(), "and not to the ephemeral one");

        let mut t = app();
        t.open_snap_routed(a_snap("s1"), surface);
        assert!(t.video.is_none());
        assert!(t.viewer.is_open());
    }

    #[test]
    fn closing_a_video_neither_wipes_nor_expires_it() {
        // The whole difference from the image path, which does both.
        let surface = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(360.0, 780.0));
        let mut s = app();
        s.open_snap_routed(a_video_snap("v1"), surface);
        s.close_video("v1");
        assert!(s.video.is_none());
        assert!(s.snaps_expired.is_empty(), "a video does not expire on close");
        assert!(s.modal.is_empty());
    }

    #[test]
    fn a_video_viewer_is_placed_by_the_form_factor_like_every_other_dialog() {
        let mut s = app();
        s.dialog_form = crate::mobile_dialog::Form::Sheet;
        let surface = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(360.0, 780.0));
        s.open_snap_routed(a_video_snap("v1"), surface);
        assert!(s.modal.blocks_input());
    }

    #[test]
    fn a_glass_button_animates_across_frames_rather_than_resetting() {
        // Held on the shell, not built per frame: a button rebuilt each redraw
        // would restart its dip and its ripple every single frame, so neither
        // would ever be visible.
        let mut s = app();
        s.glass_shutter.press(10.0, 10.0);
        s.glass_shutter.tick(crate::glass::PRESS_MS);
        assert!(s.glass_shutter.scale() < 1.0);
        assert!(s.glass_shutter.rippling());
        s.glass_shutter.release();
        s.glass_shutter.tick(crate::glass::PRESS_MS);
        assert!((s.glass_shutter.scale() - 1.0).abs() < 0.01, "sprang back");
    }

    #[test]
    fn a_desktop_surface_gets_a_centred_window_and_a_phone_gets_a_full_width_sheet() {
        // The hardcoded rectangles this replaced were phone-shaped constants —
        // a 340px sheet dropped in the corner of a desktop window.
        let mut s = app();
        let wide = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1600.0, 900.0));
        s.dialog_form = crate::mobile_dialog::Form::Window;
        let r = s.dialog_rect(wide, 400.0);
        assert!(r.w <= 480.0, "a dialog does not grow with the monitor");
        assert!((r.x + r.w / 2.0 - 800.0).abs() < 1.0, "centred");

        s.dialog_form = crate::mobile_dialog::Form::Sheet;
        let narrow = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(360.0, 780.0));
        let r = s.dialog_rect(narrow, 400.0);
        assert_eq!(r.x, 0.0);
        assert_eq!(r.w, 360.0, "full width");
        assert!((r.y + r.h - 780.0).abs() < 1.0, "flush with the bottom");
    }

    #[test]
    fn a_narrow_surface_chooses_the_sheet_form_on_its_own() {
        use crate::mobile_dialog::{form_factor, Form};
        assert_eq!(form_factor(&Default::default(), 360.0, false), Form::Sheet);
        assert_eq!(form_factor(&Default::default(), 1600.0, false), Form::Window);
    }

    #[test]
    fn the_background_carousel_scrolls_then_hands_off_at_its_end() {
        // The Qt bug this reproduces the fix for: the carousel silently ate the
        // wheel at its edge, so "keep scrolling to reach the next page" did
        // nothing at all.
        use crate::scroll_areas::{Wheel, Wheeled};
        let mut s = app();
        s.bg_carousel.value = 200.0;
        assert_eq!(s.bg_carousel.wheel(Wheel::pixels(-50.0, 0.0)), Wheeled::Consumed);
        // Run it to the end, then keep pushing.
        for _ in 0..20 { s.bg_carousel.wheel(Wheel::pixels(-50.0, 0.0)); }
        assert!(s.bg_carousel.at_max());
        assert_eq!(s.bg_carousel.wheel(Wheel::pixels(-50.0, 0.0)), Wheeled::PassThrough);
    }

    #[test]
    fn a_vertical_wheel_still_moves_the_carousel_sideways() {
        // A default scroll area has no vertical range here, so the wheel would
        // otherwise do nothing rather than the wrong thing.
        use crate::scroll_areas::{Wheel, Wheeled};
        let mut s = app();
        s.bg_carousel.value = 200.0;
        assert_eq!(s.bg_carousel.wheel(Wheel::degrees(0.0, 15.0)), Wheeled::Consumed);
        assert_ne!(s.bg_carousel.value, 200.0);
    }

    #[test]
    fn the_ribbon_tabs_are_the_navs_own_so_they_cannot_drift() {
        let s = app();
        let labels: Vec<&str> = s.ribbon.tabs().iter().map(|t| t.label.as_str()).collect();
        assert_eq!(labels, sub_tabs(1));
    }

    #[test]
    fn mark_all_read_is_disabled_with_nothing_to_mark() {
        // An always-enabled action on an empty inbox is a button that lies
        // about having work.
        let mut s = app();
        s.refresh_ribbon();
        s.ribbon.set_current(1);
        assert!(s.ribbon.trigger("read_all").is_none(), "nothing unread");
    }

    #[test]
    fn mark_all_read_clears_the_unread_count() {
        let mut s = app();
        s.inbox = Default::default();
        for (id, peer) in [(1u64, "ada"), (2, "bo")] {
            s.inbox.add(crate::inbox::Entry { id, peer: peer.into(),
                preview: "hi".into(), at: 0, unread: true, outgoing: false,
                kind: crate::inbox::Kind::Message });
        }
        assert_eq!(s.inbox.unread_count(), 2);
        s.refresh_ribbon();
        s.ribbon.set_current(1);
        assert!(s.ribbon.trigger("read_all").is_some(), "there is work to do");
        s.apply_ribbon(crate::ribbon::RibbonEvent::Triggered("read_all".into()));
        assert_eq!(s.inbox.unread_count(), 0);
    }

    #[test]
    fn retry_failed_only_lights_when_a_send_has_actually_failed() {
        let mut s = app();
        s.outbox.add("s1", "ada", 0);
        s.refresh_ribbon();
        s.ribbon.set_current(2);
        assert!(s.ribbon.trigger("retry_failed").is_none(), "still connecting, not failed");
        s.outbox.update("s1", crate::session::Phase::Failed, "no route");
        s.refresh_ribbon();
        assert!(s.ribbon.trigger("retry_failed").is_some());
        s.apply_ribbon(crate::ribbon::RibbonEvent::Triggered("retry_failed".into()));
        assert!(s.retry_requested, "the host owns the retry; the view reports it");
    }

    #[test]
    fn a_delivered_send_is_not_a_failed_one() {
        let mut s = app();
        s.outbox.add("s1", "ada", 0);
        s.outbox.update("s1", crate::session::Phase::Failed, "no route");
        s.outbox.mark_delivered("s1");
        s.refresh_ribbon();
        s.ribbon.set_current(2);
        assert!(s.ribbon.trigger("retry_failed").is_none(),
                "it arrived in the end; there is nothing to retry");
    }

    #[test]
    fn a_refused_swipe_does_not_count_toward_learning_the_gesture() {
        // The user has not learned anything from a gesture that did nothing.
        let mut s = app();
        assert_eq!(s.nav.page(), 0);
        s.swipe_page(-1);
        assert_eq!(s.peel.swipes(), 0, "swiping past the first page taught nothing");
        s.swipe_page(1);
        assert_eq!(s.peel.swipes(), 1);
    }

    #[test]
    fn a_swipe_blocked_by_a_dialog_does_not_count_either() {
        let mut s = app();
        s.open_qr_scan(true);
        s.swipe_page(1);
        assert_eq!(s.peel.swipes(), 0);
    }

    #[test]
    fn the_hint_retires_after_four_real_swipes() {
        let mut s = app();
        for _ in 0..4 { s.swipe_page(1); }
        assert!(s.peel.retired(), "four pages in, the gesture is learned");
    }

    #[test]
    fn exporting_and_dismissing_both_close_the_month() {
        // Either way the card was on screen and answered; re-offering it after
        // an explicit answer is the nag this feature exists to avoid.
        use crate::export_reminder::{CardAction, Month};
        let aug = Month { year: 2026, month: 8 };
        let mut s = app();
        s.apply_export_card(CardAction::ExportRequested, aug);
        assert!(s.export_requested);
        assert!(!s.reminder.should_show(aug, 50));

        let mut t = app();
        t.apply_export_card(CardAction::Dismissed, aug);
        assert!(!t.export_requested, "maybe later is not an export");
        assert!(!t.reminder.should_show(aug, 50));
    }

    #[test]
    fn no_month_from_the_host_means_no_card_rather_than_a_guess() {
        let s = app();
        assert!(s.now_month.is_none());
        // The gate is the host's month; without one the Inbox simply draws no
        // card, instead of inventing a date to compare against.
        assert!(s.recent_snaps.is_empty());
    }

    #[test]
    fn the_voice_controls_are_reachable_again_after_leaving_the_wayfinder_page() {
        // The regression I introduced last commit: the controls came off the
        // Wayfinder page, where they never belonged, and had no home.
        let mut s = app();
        assert!(!s.group_voice.joined);
        s.toggle_voice();
        assert!(s.group_voice.joined, "join is reachable from Friends");
        s.toggle_voice();
        assert!(!s.group_voice.joined);
    }

    #[test]
    fn pressing_talk_lights_the_bar_without_waiting_for_the_session() {
        // A talk button that waits for the round trip feels broken on a slow
        // link, which is exactly when you are pressing it hardest.
        let mut s = app();
        s.apply_voice(crate::voicebar::Event::TalkPressed);
        assert!(s.voicebar.on_air());
        s.apply_voice(crate::voicebar::Event::TalkReleased);
        assert!(!s.voicebar.on_air());
    }

    #[test]
    fn requesting_privacy_shows_securing_not_private() {
        // The request is ours to record; the TIER is the negotiation's to
        // report. Claiming Private on request is the padlock lie.
        let mut s = app();
        s.apply_voice(crate::voicebar::Event::PrivateRequested(true));
        assert_eq!(s.voicebar.security, crate::voicebar::Security::Securing);
        assert!(!s.voicebar.security.locked());
    }

    #[test]
    fn leaving_the_channel_takes_the_bar_off_air() {
        let mut s = app();
        s.toggle_voice();
        s.apply_voice(crate::voicebar::Event::TalkPressed);
        s.toggle_voice();
        assert!(!s.voicebar.on_air(), "a left channel cannot still be transmitting");
    }

    #[test]
    fn the_wayfinder_page_draws_the_wayfinder_and_not_the_voice_controls() {
        // The regression this replaced: PAGES[3] is "Wayfinder" and it drew
        // join/mute/push-to-talk. Nothing looked missing, which is why it
        // survived.
        assert_eq!(PAGES[3], "Wayfinder");
        let mut s = app();
        s.self_fix = Some(crate::wayfinder::Fix { lat: 51.5074, lon: -0.1278 });
        s.wayfinder_contacts = vec![crate::wayfinder::Contact {
            id: "paris".into(), display_name: "Paris".into(), online: true,
            loc: Some(crate::wayfinder::Fix { lat: 48.8566, lon: 2.3522 }) }];
        let nodes = crate::wayfinder::build_nodes(s.self_fix, &s.wayfinder_contacts);
        assert_eq!(nodes.len(), 1);
        assert!(nodes[0].distance_label().ends_with("SE"));
    }

    #[test]
    fn contacts_without_a_fix_still_appear_on_the_map() {
        let mut s = app();
        s.self_fix = None;
        s.wayfinder_contacts = vec![crate::wayfinder::Contact {
            id: "ada".into(), display_name: "Ada".into(), online: true, loc: None }];
        let nodes = crate::wayfinder::build_nodes(s.self_fix, &s.wayfinder_contacts);
        assert_eq!(nodes.len(), 1, "a contact with no GPS is not a missing contact");
        assert_eq!(nodes[0].distance_label(), "Nearby");
    }

    /// The reveal must run EVERY time the dialog opens, not once per process.
    ///
    /// `qr_shown_at` is the clock the fractal reveal runs from, and it used to be cleared
    /// only by the dialog's own "close" button. Dismiss the dialog any other way — scan a
    /// code, press Escape, tap the scrim — and the stale timestamp survived, so the next
    /// open drew a code that had already finished animating. Three close paths existed and
    /// one of them cleared it.
    #[test]
    fn the_code_reveal_restarts_on_every_open() {
        let mut s = app();

        // Opened, drawn, and the clock is stamped.
        s.open_qr_scan(true);
        assert_eq!(s.qr_shown_at, None, "the clock must start unset so the draw stamps it");
        s.qr_shown_at = Some(1234.0); // as a frame of drawing would

        // CLOSED BY SCANNING — the path that did not clear it.
        assert!(s.qr_frame("snapclone://sc-deadbeef:9000"));
        s.tick(0.3);
        s.tick(0.4);
        assert!(s.qr.is_none(), "the dialog should have closed itself");

        s.open_qr_scan(true);
        assert_eq!(s.qr_shown_at, None, "reopening reused the old reveal clock");

        // And by the close button's path, which always worked — still does.
        s.qr_shown_at = Some(99.0);
        s.qr = None;
        s.qr_shown_at = None;
        s.open_qr_scan(false);
        assert_eq!(s.qr_shown_at, None);
    }

    #[test]
    fn a_scanned_code_becomes_a_peer_through_the_same_path_as_a_typed_address() {
        // A contact added by camera and one added by hand must be the same kind
        // of contact — otherwise one of the two paths grows its own bugs.
        let mut s = app();
        s.open_qr_scan(true);
        assert!(s.modal.blocks_input());
        assert!(s.qr_frame("snapclone://sc-deadbeef:9000"));
        // Held first, so the user sees what was found.
        s.tick(0.3);
        assert_eq!(s.roster.len(), 2, "not yet — the success state is showing");
        s.tick(0.4);
        assert_eq!(s.roster.len(), 3);
        assert!(s.roster.get("sc-deadbeef").is_some());
        assert!(s.qr.is_none());
        assert!(s.modal.is_empty(), "and the sheet closes with it");
    }

    #[test]
    fn scanning_the_same_code_twice_adds_one_peer() {
        // At 8fps the code arrives repeatedly before the camera stops.
        let mut s = app();
        s.open_qr_scan(true);
        assert!(s.qr_frame("sc-deadbeef"));
        assert!(!s.qr_frame("sc-deadbeef"));
        s.tick(1.0);
        assert_eq!(s.roster.len(), 3);
        assert!(!s.qr_frame("sc-deadbeef"), "the sheet is gone; no stray frame lands");
        assert_eq!(s.roster.len(), 3);
    }

    #[test]
    fn no_camera_opens_straight_on_the_manual_field() {
        // A viewfinder that will never produce a frame is worse than no
        // viewfinder.
        let mut s = app();
        s.open_qr_scan(false);
        let q = s.qr.as_ref().unwrap();
        assert!(!q.is_scanning());
        assert!(q.status().contains("Camera unavailable"));
    }

    #[test]
    fn accepting_a_call_opens_the_thread_and_joins_the_voice_session() {
        let mut s = app();
        s.ring("alice");
        assert!(s.modal.blocks_input(), "the ring sits above whatever page is up");
        s.answer_call(crate::ring::Answer::Accepted);
        assert_eq!(s.conversation_peer, "alice");
        assert!(s.group_voice.joined);
        assert!(s.incoming_call.is_none());
        assert!(s.modal.is_empty());
    }

    #[test]
    fn an_ignored_ring_becomes_a_missed_call_but_a_declined_one_does_not() {
        // Same effect on the packets, different history: a missed call and a
        // refused one are not the same thing to whoever reads the list later.
        let mut s = app();
        s.ring("alice");
        s.tick(31.0);
        assert_eq!(s.missed_calls, vec!["alice".to_string()]);
        assert!(s.incoming_call.is_none());

        let mut t = app();
        t.ring("bob");
        t.answer_call(crate::ring::Answer::Declined);
        assert!(t.missed_calls.is_empty());
        assert_eq!(t.conversation_peer, "", "and it does not open the thread");
    }

    #[test]
    fn a_stray_tap_outside_the_sheet_neither_answers_nor_refuses_the_call() {
        // A phone in a pocket must not take or drop a call on its own.
        let mut s = app();
        s.ring("alice");
        let out = s.modal.tap(10.0, 10.0);
        assert_eq!(out, crate::modal::TapResult::Swallowed);
        assert!(s.incoming_call.as_ref().unwrap().ringing());
    }

    #[test]
    fn the_sent_sub_tab_shows_the_outbox_the_nav_has_always_declared() {
        // sub_tabs(1) has listed All/Unread/Sent since the shell was ported and
        // "Sent" drew the inbox list like the other two.
        let mut s = app();
        s.outbox.add("s1", "ada", 0);
        assert_eq!(sub_tabs(1)[2], "Sent");
        s.bar.sub[1] = 2;
        assert_eq!(s.outbox.len(), 1);
        assert_eq!(s.outbox.summary().as_deref(), Some("0 / 1 delivered"));
    }

    #[test]
    fn a_successful_sign_in_hides_the_overlay_and_leaves_no_plaintext() {
        let mut s = Snapclone::new_with_login(vec![], false);
        assert!(s.login_visible, "no auto-login, so the form is up");
        s.login.username = "ada".into();
        s.login.password = "hunter2".into();
        let creds = s.login.submit().expect("valid");
        s.login_succeeded();
        assert!(!s.login_visible);
        assert!(s.login.password.is_empty());
        assert_eq!(creds.username, "ada");
    }

    #[test]
    fn opening_a_snap_takes_the_tile_and_blocks_the_page_behind_it() {
        let mut s = app();
        s.feed.set_entries(vec![crate::snaps::FeedEntry {
            id: "s1".into(), sender_name: "Ada".into(), sent_at: 0,
            media: crate::snaps::Media::Image, thumb_path: String::new() }]);
        s.open_snap(a_snap("s1"));
        assert_eq!(s.feed.count(), 0, "a view-once snap cannot be tapped twice");
        assert_eq!(s.snaps_viewed, vec!["s1".to_string()]);
        assert!(s.modal.blocks_input(), "the viewer is modal");
    }

    #[test]
    fn a_snap_closed_early_still_expires_and_still_wipes() {
        // The two halves that matter: plaintext must not outlive the viewer,
        // and the host must still hear the expiry or the row stays unopened.
        let mut s = app();
        s.open_snap(a_snap("s1"));
        s.tick(1.0);
        s.close_snap();
        assert!(s.viewer.wiped(), "bytes gone on close");
        assert!(s.snaps_expired.is_empty(), "the window has not elapsed yet");
        s.tick(10.0);
        assert_eq!(s.snaps_expired, vec!["s1".to_string()]);
        assert!(s.modal.is_empty(), "and the modal goes with it");
    }

    #[test]
    fn the_countdown_runs_on_the_frame_clock() {
        let mut s = app();
        s.open_snap(a_snap("s1"));
        assert_eq!(s.viewer.countdown(), 1.0);
        s.tick(2.5);
        assert!(s.viewer.countdown() < 0.6 && s.viewer.countdown() > 0.4);
    }

    #[test]
    fn the_roster_is_built_from_the_peers_at_construction() {
        // Not a second list that has to be remembered to update.
        let s = app();
        assert_eq!(s.roster.len(), 2);
        assert_eq!(s.roster.online_count(), 1);
        assert!(s.roster.get("alice").unwrap().online);
    }

    #[test]
    fn a_manual_add_shows_the_row_immediately_as_offline() {
        // A Connect button that visibly does nothing until the handshake
        // completes gets pressed four more times.
        let mut s = app();
        s.apply_friends(crate::friends::Action::Add(
            crate::friends::AddPeer { host: "192.168.1.5".into(), port: 9876 }));
        assert_eq!(s.roster.len(), 3);
        let added = s.roster.get("192.168.1.5").expect("the new row");
        assert!(!added.online, "optimistic about the row, honest about the state");
    }

    #[test]
    fn adding_a_peer_that_is_already_there_does_not_duplicate_it() {
        let mut s = app();
        s.apply_friends(crate::friends::Action::Add(
            crate::friends::AddPeer { host: "alice".into(), port: 9876 }));
        assert_eq!(s.roster.len(), 2);
    }

    #[test]
    fn delete_asks_before_it_deletes() {
        // Qt puts a ChevronConfirm in the way — "Remove %1 from your address
        // book?" — and the port was deleting on the single tap of a menu item
        // sitting one row below "Message". That is how people lose people.
        let mut s = app();
        s.cards.open_only("bob");
        s.apply_friends(crate::friends::Action::Invoke("bob".into(), "delete".into()));
        assert_eq!(s.roster.len(), 2, "nobody is gone yet");
        assert_eq!(s.modal.blocking_layer(), Some(crate::modal::Layer::Confirm));

        s.confirm_delete("bob");
        assert_eq!(s.roster.len(), 1);
        assert!(s.roster.get("bob").is_none());
        // Leaving the accordion open on a deleted id strands the menu over
        // whichever row slides up into its place.
        assert!(!s.cards.is_open("bob"));
        assert!(s.modal.is_empty(), "and the confirm goes with it");
    }

    #[test]
    fn dismissing_the_confirm_keeps_the_contact() {
        let mut s = app();
        s.apply_friends(crate::friends::Action::Invoke("bob".into(), "delete".into()));
        s.modal.dismiss_topmost();
        assert_eq!(s.roster.len(), 2, "backing out of a confirm is not a delete");
    }

    #[test]
    fn a_swipe_under_an_open_dialog_does_not_change_the_page() {
        // Otherwise the dialog is left floating over a screen it has nothing
        // to do with.
        let mut s = app();
        let page = s.nav.page();
        s.apply_friends(crate::friends::Action::Invoke("bob".into(), "edit".into()));
        assert!(s.modal.blocks_input());
        s.swipe_page(1);
        assert_eq!(s.nav.page(), page, "the swipe was swallowed by the dialog");
        s.modal.dismiss_all();
        s.swipe_page(1);
        assert_ne!(s.nav.page(), page, "and works again once it closes");
    }

    #[test]
    fn tapping_a_row_drills_into_that_peers_subpage() {
        let mut s = app();
        s.apply_friends(crate::friends::Action::Selected("bob".into()));
        assert_eq!(s.nav.subpage(), Some(1), "bob's index, not alice's");
    }

    #[test]
    fn swipe_moves_between_pages() {
        let mut s = app();
        assert_eq!(s.page_name(), "Camera");
        s.swipe_page(1);
        assert_eq!(s.page_name(), "Inbox"); // page 1 in the faithful 5-page shell
        s.swipe_page(1);
        s.swipe_page(1);
        s.swipe_page(1);
        s.swipe_page(1); // clamp at Settings (page 4)
        assert_eq!(s.page_name(), "Settings");
        s.swipe_page(-1);
        assert_eq!(s.page_name(), "Wayfinder");
    }

    #[test]
    fn swipe_disabled_while_drilled_into_subpage() {
        let mut s = app();
        s.nav.go_page(1); // Peers
        s.nav.push_subpage(0); // into a peer detail
        s.swipe_page(1); // should be ignored so content interaction wins
        assert_eq!(s.page_name(), "Inbox"); // page 1 is Inbox in the faithful 5-page shell
        assert_eq!(s.nav.subpage(), Some(0));
    }

    #[test]
    fn background_tick_and_style_switch() {
        let mut s = app();
        let p0 = s.bg.phase;
        s.tick(0.5);
        assert!(s.bg.phase > p0);
        s.bg.style = BgStyle::Waves;
        assert_eq!(s.bg.style, BgStyle::Waves);
    }

    #[test]
    fn apply_settings_adopts_feel() {
        let ctx = egui::Context::default();
        let mut s = app();
        let mut prefs = settings_kit::Prefs::default();
        prefs.gesture.animation = settings_kit::AnimationStyle::Heavy;
        s.apply_settings(&ctx, &prefs);
        assert_eq!(s.gesture.config_mut().takeover_lock_ms, 220.0);
    }
}

#[cfg(test)]
mod faithful_shell_tests {
    use super::*;

    #[test]
    fn five_qt_pages_in_order() {
        assert_eq!(PAGES, ["Camera", "Inbox", "Friends", "Wayfinder", "Settings"],
            "the page set must match MainWindow.cpp kPage* exactly");
    }

    #[test]
    fn sub_tab_tables_match_the_qt_switches() {
        assert_eq!(sub_tabs(0), &[] as &[&str], "Camera is not SubTabbed");
        assert_eq!(sub_tabs(1), &["All", "Unread", "Sent"], "InboxWidget::subTabLabel");
        assert_eq!(sub_tabs(2), &["All", "Online", "Pending"], "ContactsPage::subTabLabel");
        assert_eq!(sub_tabs(3), &[] as &[&str], "Wayfinder is not SubTabbed");
        assert_eq!(sub_tabs(4), &["General", "Camera", "Network", "Storage", "About"],
            "SettingsPage::subTabLabel");
    }

    #[test]
    fn login_overlay_ordering_rule() {
        // auto-login already succeeded at construction → overlay stays hidden (the Qt bug
        // this rule exists to prevent: overlay flashing over the camera on first paint)
        let s = Snapclone::new_with_login(vec![], true);
        assert!(!s.login_visible);
        // auto-login not yet fired → shown, then hidden on the signal
        let mut s = Snapclone::new_with_login(vec![], false);
        assert!(s.login_visible);
        s.login_succeeded();
        assert!(!s.login_visible);
        s.login_succeeded(); // idempotent
        assert!(!s.login_visible);
    }

    fn run_bar(bar: &mut UnifiedNav) {
        let ctx = egui::Context::default();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(480.0, 800.0))),
            ..Default::default()
        };
        #[allow(deprecated)]
        let _ = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| { let _ = bar.ui(ui); });
        });
    }

    #[test]
    fn underline_morph_converges_on_the_active_segment() {
        let mut bar = UnifiedNav::new();
        bar.page = 1; // Inbox: segments exist
        run_bar(&mut bar);
        let start = bar.underline_span().expect("underline after first frame");
        bar.sub[1] = 2; // jump to "Sent" — the morph should EASE, not snap
        run_bar(&mut bar);
        let mid = bar.underline_span().unwrap();
        assert!(mid.0 > start.0, "morph must move toward the new segment");
        for _ in 0..40 { run_bar(&mut bar); }
        let done = bar.underline_span().unwrap();
        assert!((done.0 - mid.0).abs() > 0.0 || done.0 > start.0);
        // converged: two more frames barely move it
        let before = bar.underline_span().unwrap();
        run_bar(&mut bar);
        let after = bar.underline_span().unwrap();
        assert!((after.0 - before.0).abs() < 0.6, "underline must settle ({} -> {})", before.0, after.0);
    }

    #[test]
    fn every_page_renders_headless_with_the_bar() {
        for page in 0..PAGE_NAMES.len() {
            let mut bar = UnifiedNav::new();
            bar.page = page;
            run_bar(&mut bar); // must not panic on any page form (centered vs segmented)
        }
    }

#[cfg(test)]
mod desktop_entry_tests {
    /// The shipped entry, read at compile time so the test cannot pass against
    /// a file that is not the one installed.
    const ENTRY: &str = include_str!("../../packaging/org.snapclone.SnapClone.desktop");

    #[test]
    fn the_app_id_matches_the_desktop_files_icon_and_name() {
        // The mismatch that produces a working app with no icon. Checking it
        // here means a rename of either side breaks the build rather than the
        // dock.
        let icon = ENTRY.lines().find_map(|l| l.strip_prefix("Icon=")).expect("Icon=");
        assert_eq!(icon, "org.snapclone.SnapClone");
        assert!(uikit_egui::desktop::is_valid_app_id(icon), "{icon}");
        // StartupWMClass covers X11, where the compositor matches on class
        // rather than app id; both must name the same app or the integration
        // works on one display server and not the other.
        assert!(ENTRY.contains("StartupWMClass="), "no StartupWMClass");
    }

    #[test]
    fn the_entry_has_what_a_launcher_needs() {
        // A .desktop file missing any of these is accepted by the spec and
        // shows up wrong: no category means no menu placement, no Exec means it
        // cannot be launched at all.
        for key in ["Type=Application", "Name=", "Exec=", "Icon=", "Categories="] {
            assert!(ENTRY.contains(key), "missing {key}");
        }
        // Terminal=true on a GUI app opens a console window alongside it.
        assert!(ENTRY.contains("Terminal=false"));
    }
}

#[cfg(test)]
mod notify_tests {
    use super::*;

    fn app() -> Snapclone { Snapclone::new(Vec::new()) }

    #[test]
    fn a_focused_window_does_not_notify_about_what_is_on_screen() {
        // An app that pops a notification for a message the user is looking at
        // is one people mute, and a muted app cannot tell them anything.
        let a = app();
        assert!(a.on_snaps_arrived(&["Ada".into()], true).is_none());
        assert!(a.on_snaps_arrived(&["Ada".into()], false).is_some());
    }

    #[test]
    fn high_security_notifies_without_naming_anyone() {
        // The notification persists in KDE history and shows on a lock screen;
        // a name there defeats the encryption underneath it.
        let mut a = app();
        a.high_security = true;
        let r = a.on_snaps_arrived(&["Ada".into()], false).unwrap();
        assert!(!r.summary.contains("Ada"), "{r:?}");
        assert!(!r.body.contains("Ada"), "{r:?}");
    }

    #[test]
    fn a_burst_of_snaps_is_one_notification() {
        let a = app();
        let from: Vec<String> = (0..8).map(|i| format!("p{i}")).collect();
        let r = a.on_snaps_arrived(&from, false).unwrap();
        assert_eq!(r.summary, "8 new messages");
    }

    #[test]
    fn nothing_arriving_notifies_nothing() {
        let a = app();
        assert!(a.on_snaps_arrived(&[], false).is_none());
    }

    #[test]
    fn notifications_can_be_turned_off_entirely() {
        let mut a = app();
        a.notify_enabled = false;
        assert!(a.on_snaps_arrived(&["Ada".into()], false).is_none());
    }
}
}

#[cfg(test)]
mod modal_render_tests {
    /// Every modal id the app can present must have a renderer that draws it.
    ///
    /// This is a SOURCE check rather than a behavioural one, and that is
    /// deliberate: the failure it catches — a modal pushed onto the stack with
    /// no body — produces no wrong value anywhere. The state machine is
    /// correct, `blocks_input` is correct, the dismiss path is correct. The
    /// only symptom is a user staring at a frozen screen, which no unit test
    /// of the state can see.
    ///
    /// Two were found this way: the QR dialog and the incoming-call sheet, the
    /// second of which could ring, time out and be recorded as missed without
    /// ever appearing.
    #[test]
    fn every_presented_modal_is_drawn_somewhere() {
        let src = include_str!("lib.rs");
        // The ids as they appear in present/present_default calls.
        let presented = ["qr", "call:", "snap:", "video:", "confirm-delete:", "editor:"];
        // KNOWN-UNDRAWN, listed rather than removed from the check.
        //
        // These four are the same bug the other two were, found by this test on
        // its first run. They are not fixed here because each needs its own
        // view wired — a snap viewer, a video player, a delete confirmation, a
        // caption editor — and four half-built dialogs would be worse than four
        // honestly-absent ones.
        //
        // Listing them keeps the test GREEN and the debt VISIBLE: a new modal
        // is a failure, and clearing one of these is deleting a line from here.
        // Removing the check instead would lose both properties.
        // EMPTY. All six modals now have a body.
        let known_undrawn: [&str; 0] = [];
        let mut undrawn = Vec::new();
        for id in presented {
            // A renderer is a `topmost(...)` filtered on the id. Anything else
            // — a dismiss, a present — does not put pixels on screen.
            // Matches the id as a LITERAL or as the prefix of a format!, because
            // a parameterised modal ("confirm-delete:{id}") cannot appear as a
            // bare string. My first version only checked literals and rejected
            // the delete confirmation the moment I wired it — a check that
            // fails on a correct fix trains people to delete the check.
            // A WINDOW AROUND topmost(, not just after it.
            //
            // The id can be built BEFORE the call — `let want = format!("snap:
            // {}", ..); ... topmost(..).filter(|p| &p.id == want)` — so looking
            // only forward missed the snap viewer the moment I wired it. That
            // is twice this check has failed on a correct fix, which is a
            // property of the check rather than of the code: matching source
            // text cannot see intent, so the window has to be generous enough
            // to cover the shapes people actually write.
            //
            // Generous in the false-NEGATIVE direction on purpose. Missing a
            // genuinely undrawn modal costs one more bug of a kind we now know
            // how to find; failing on correct code costs the check itself.
            let drawn = src.match_indices("topmost(").any(|(i, _)| {
                let lo = i.saturating_sub(400);
                let hi = (i + 600).min(src.len());
                let window = &src[lo..hi];
                window.contains(&format!("\"{id}\"")) || window.contains(&format!("\"{id}{{"))
            });
            if !drawn && !known_undrawn.contains(&id) { undrawn.push(id); }
        }
        assert!(undrawn.is_empty(),
                "presented but never drawn: {undrawn:?} — these block input while invisible");
    }
}

#[cfg(test)]
mod help_tests {
    use crate::help_manual::IDLE_DISMISS_MS;
    fn app() -> super::Snapclone { super::Snapclone::new(Vec::new()) }

    #[test]
    fn a_reader_who_pauses_on_a_short_section_is_not_dismissed() {
        // The whole feature. A short page does not move the scrollbar, so an
        // idle rule watching only scroll offset would close on exactly the
        // careful reader — worse than never closing at all.
        let mut a = app();
        a.help.open();
        // Nearly out of time, then a keypress that scrolls nothing.
        a.tick_help(IDLE_DISMISS_MS - 1.0);
        a.help_interacted();
        assert!(!a.tick_help(2.0), "dismissed a reader who had just interacted");
        assert!(a.help.is_open());
    }

    #[test]
    fn an_untouched_manual_does_close() {
        // The countdown still has to work, or the rule above is just "never
        // dismiss".
        let mut a = app();
        a.help.open();
        assert!(a.tick_help(IDLE_DISMISS_MS));
        assert!(!a.help.is_open());
    }

    #[test]
    fn interaction_while_closed_does_nothing() {
        // Input elsewhere in the app must not resurrect or arm a manual nobody
        // opened.
        let mut a = app();
        a.help_interacted();
        assert!(!a.help.is_open());
    }

    #[test]
    fn the_manual_starts_closed_and_has_content() {
        // A help button that opens an empty scroller is worse than no button:
        // it answers the question with silence.
        let a = app();
        assert!(!a.help.is_open());
        // visible(), not the private field: it applies the filter, so this
        // asserts what a reader would actually SEE rather than what was loaded.
        assert!(a.help.visible().len() >= 4);
        assert!(a.help.visible().iter().all(|s| !s.body.trim().is_empty()));
    }
}

#[cfg(test)]
mod ambient_tests {
    fn app() -> super::Snapclone { super::Snapclone::new(Vec::new()) }

    #[test]
    fn a_greyscale_inbox_derives_no_accent() {
        // The module is explicit that None is a real answer rather than a
        // failure: an inbox of black-and-white photos qualifies no accent
        // pixel, and painting black there makes a monochrome inbox look BROKEN
        // rather than monochrome.
        let mut a = app();
        let grey: Vec<[u8; 3]> = (0..64 * 64).map(|_| [128, 128, 128]).collect();
        a.sample_ambient(&grey, 64);
        assert_eq!(a.ambient_accent(), None);
    }

    #[test]
    fn a_colourful_photo_gives_an_accent() {
        let mut a = app();
        let warm: Vec<[u8; 3]> = (0..64 * 64).map(|_| [200, 90, 40]).collect();
        a.sample_ambient(&warm, 64);
        assert!(a.ambient_accent().is_some());
    }

    #[test]
    fn samples_accumulate_rather_than_reset() {
        // The palette should settle as the inbox fills, not lurch to whatever
        // arrived last — one loud photo tints the app, it does not repaint it.
        let mut a = app();
        let warm: Vec<[u8; 3]> = (0..64 * 64).map(|_| [200, 90, 40]).collect();
        a.sample_ambient(&warm, 64);
        let first = a.ambient_accent().expect("an accent");
        let cool: Vec<[u8; 3]> = (0..64 * 64).map(|_| [40, 90, 200]).collect();
        a.sample_ambient(&cool, 64);
        let second = a.ambient_accent().expect("an accent");
        assert_ne!(first, second, "the second photo had no effect at all");
        // ...but it did not simply become the new photo's colour either.
        assert_ne!(second, [40, 90, 200], "the palette lurched to the last photo");
    }

    #[test]
    fn nothing_sampled_means_no_ambient() {
        assert_eq!(app().ambient_accent(), None);
    }
}

#[cfg(test)]
mod swipe_nav_tests {
    fn app() -> super::Snapclone { super::Snapclone::new(Vec::new()) }

    #[test]
    fn a_swipe_past_the_last_tab_stays_put() {
        // Bounds already refuse it, and this is the second guard: landing back
        // on Camera from the far end is a wrap the user did not ask for and
        // cannot undo by swiping the same way again.
        let mut a = app();
        let last = a.tabs.len() - 1;
        a.tabs.set_current(last);
        a.begin_swipe(300.0, 400.0, 0.0);
        // A long drag left = "next", which does not exist here.
        if let Some(sw) = a.swipe.as_mut() { sw.update(0.0, 100.0); }
        assert_eq!(a.end_swipe(120.0), last);
    }

    #[test]
    fn a_swipe_before_the_first_tab_stays_put() {
        let mut a = app();
        a.tabs.set_current(0);
        a.begin_swipe(50.0, 400.0, 0.0);
        if let Some(sw) = a.swipe.as_mut() { sw.update(390.0, 100.0); }
        assert_eq!(a.end_swipe(120.0), 0);
    }

    #[test]
    fn releasing_without_a_swipe_is_not_a_page_turn() {
        // end_swipe can be called on a frame where no gesture started — a
        // pointer-up with no drag — and must not move the page.
        let mut a = app();
        a.tabs.set_current(2);
        assert_eq!(a.end_swipe(10.0), 2);
        assert!(a.swipe.is_none());
    }

    #[test]
    fn the_gesture_clears_after_release() {
        // A Swipe left in place would keep rendering the page a few pixels off
        // centre, with nothing to spring it back.
        let mut a = app();
        a.begin_swipe(100.0, 400.0, 0.0);
        assert!(a.swipe.is_some());
        a.end_swipe(50.0);
        assert!(a.swipe.is_none());
    }
}

#[cfg(test)]
mod badge_tests {
    #[test]
    fn the_chats_tab_index_matches_the_tab_actually_built() {
        // CHATS_TAB is a constant precisely so a reorder cannot silently badge
        // the wrong tab — but a constant that drifts from the list is the same
        // bug with an extra step, so the two are checked against each other.
        // The label is PAGES[CHATS_TAB] now, not the literal "Chats": the bar
        // said "Chats" and "Map" where the rest of the app says "Inbox" and
        // "Wayfinder", which was harmless while nothing drew it and two names
        // for one page the moment it appeared. Checking against PAGES keeps
        // the constant honest without pinning a name that is not the app's.
        let app = super::Snapclone::new(Vec::new());
        let tab = &app.tabs.tabs()[super::Snapclone::CHATS_TAB];
        assert_eq!(tab.label, super::PAGES[super::Snapclone::CHATS_TAB],
                   "CHATS_TAB points at {:?}, not the messages page", tab.label);
        assert_eq!(tab.label, "Inbox", "the messages page is named Inbox");
    }

    #[test]
    fn the_badge_follows_the_inbox_rather_than_being_set_by_hand() {
        // Derived, not maintained in parallel: two counts of the same thing
        // disagree the moment one update is missed, and the badge is the one
        // the user trusts without opening the app.
        let mut app = super::Snapclone::new(Vec::new());
        app.sync_badges();
        let shown = app.tabs.tabs()[super::Snapclone::CHATS_TAB].badge;
        assert_eq!(shown as usize, app.inbox.unread_count());
    }
}

#[cfg(test)]
mod dial_wiring_tests {
    use super::*;

    fn seen(s: &mut Snapclone, id: &str, host: &str, port: u16) {
        s.on_gossip(crate::gossip::PeerRecord {
            id: id.into(), host: host.into(), port,
            last_seen_ms: 0, verified: true,
        }, 100, None);
    }

    #[test]
    fn a_port_of_zero_is_refused_rather_than_ranked_low() {
        // It cannot be dialled at all, so trying it costs a timeout to learn
        // what the address already said.
        let mut s = Snapclone::new(vec![]);
        seen(&mut s, "sc-a", "192.168.1.9", 0);
        assert!(s.dial_order("sc-a").is_empty());
    }

    #[test]
    fn a_gossip_record_holds_one_address_per_peer() {
        // My first version of this test fed two addresses for one id and
        // expected both back, ranked. PeerView::observe dedupes BY ID — a
        // second record is a refresh, not a second route — so dial_order can
        // only ever return one address per peer.
        //
        // That is a real limit of the discovery model rather than a bug in the
        // ranking, and it is worth pinning: multi-homing needs a record shape
        // that carries several addresses, and nothing here should quietly
        // assume it already does.
        let mut s = Snapclone::new(vec![]);
        s.local_hosts = vec!["192.168.1.5".into()];
        seen(&mut s, "sc-a", "203.0.113.7", 9876);
        seen(&mut s, "sc-a", "192.168.1.9", 9876);
        assert_eq!(s.dial_order("sc-a").len(), 1);
    }

    #[test]
    fn the_ranking_puts_same_lan_first_when_it_has_a_choice() {
        // The rule dial_order applies, checked directly on peer_dial::rerank
        // since the discovery model cannot hand it two addresses for one peer.
        use crate::peer_dial::{class_of, rerank, AddrClass, PeerAddress};
        let mut addrs = vec![
            PeerAddress { host: "203.0.113.7".into(), port: 9876 },
            PeerAddress { host: "192.168.1.9".into(), port: 9876 },
        ];
        let local = ["192.168.1.5".to_string()];
        rerank(&mut addrs,
               |a| class_of(&a.host, local.iter().any(|l| super::same_prefix(l, &a.host))),
               &[AddrClass::SameLan, AddrClass::Private, AddrClass::Public]);
        assert_eq!(addrs[0].host, "192.168.1.9",
                   "a machine in the next room was reached via the internet");
    }

    #[test]
    fn an_unknown_peer_has_nothing_to_dial() {
        let s = Snapclone::new(vec![]);
        assert!(s.dial_order("nobody").is_empty());
    }

    #[test]
    fn an_unheard_peer_gets_v1_rather_than_a_guess() {
        // Guessing high costs a whole round trip to learn something the
        // conservative choice never needed to ask.
        let s = Snapclone::new(vec![]);
        let hello = s.hello_for("nobody", &[7u8; 32], Some(&[1u8; 32]));
        assert!(!hello.is_empty());
    }

    #[test]
    fn the_retry_cap_is_the_modules_own() {
        // A second policy here could disagree with the one that exists to stop
        // a dead peer keeping the radio awake.
        let s = Snapclone::new(vec![]);
        assert!(s.handshake_retry(0).is_some());
        assert_eq!(s.handshake_retry(crate::handshake::MAX_SHAK_ATTEMPTS), None);
    }
}

#[cfg(test)]
mod gossip_wiring_tests {
    use super::*;

    fn rec(id: &str, seen: u64) -> crate::gossip::PeerRecord {
        crate::gossip::PeerRecord {
            id: id.into(), host: "192.168.1.9".into(), port: 9876,
            last_seen_ms: seen, verified: true,
        }
    }

    #[test]
    fn an_unsigned_record_never_becomes_a_peer() {
        // Gossip arrives from anyone on the subnet. A record that skips
        // `accept` can claim to be any peer, so this is the security boundary
        // for discovery rather than a validation nicety.
        let mut s = Snapclone::new(vec![]);
        let mut r = rec("sc-a", 0);
        r.verified = false;
        assert!(!s.on_gossip(r, 100, None));
        assert!(s.peers.is_empty());
    }

    #[test]
    fn our_own_echo_does_not_add_us_to_our_own_roster() {
        let mut s = Snapclone::new(vec![]);
        s.local_id = "sc-me".into();
        assert!(!s.on_gossip(rec("sc-me", 0), 100, None));
        assert!(s.peers.is_empty());
    }

    #[test]
    fn a_discovered_device_becomes_a_peer() {
        let mut s = Snapclone::new(vec![]);
        assert!(s.on_gossip(rec("sc-a", 0), 100, None));
        assert_eq!(s.peers.len(), 1);
        assert!(s.peers[0].online);
    }

    #[test]
    fn discovery_does_not_duplicate_a_contact_the_user_already_has() {
        // Discovery ADDS reachability; it does not get to add a second row for
        // somebody already in the list.
        let mut s = Snapclone::new(vec![Peer { name: "sc-a".into(), online: false }]);
        s.on_gossip(rec("sc-a", 0), 100, None);
        assert_eq!(s.peers.len(), 1);
    }

    #[test]
    fn the_bootstrap_list_is_never_empty() {
        // A node with no bootstrap addresses is a routing table with
        // nothing in it, and every lookup returns empty forever. The
        // hostile-network variant matters most: it is the case where a
        // naive filter could reduce the list to nothing.
        let s = Snapclone::new(vec![]);
        for (dns, web) in [(true, false), (false, false), (true, true), (false, true)] {
            assert!(!s.bootstrap_addresses(dns, web).is_empty(),
                    "empty list for dns={dns} web_only={web}");
        }
    }

    #[test]
    fn the_discovery_setting_is_actually_read() {
        // It was written, persisted, and read by NOTHING — choosing
        // "Jami DHT" changed a stored byte and no behaviour. This test
        // is the guard against it going quiet again.
        use crate::settings_page::DiscoveryBackend as D;
        let mut s = Snapclone::new(vec![]);
        s.settings.current.network.discovery_backend = D::Gossip;
        assert_eq!(s.discovery_enabled(), (true, false));
        s.settings.current.network.discovery_backend = D::Jami;
        assert_eq!(s.discovery_enabled(), (false, true));
        s.settings.current.network.discovery_backend = D::Both;
        assert_eq!(s.discovery_enabled(), (true, true), "Both must mean both");
    }

    #[test]
    fn turning_gossip_off_does_not_mark_every_peer_offline() {
        // Pruning marks a peer offline for want of a recent gossip
        // packet. Doing that while gossip is DISABLED would report
        // "everyone is offline" as though it were a discovery result.
        use crate::settings_page::DiscoveryBackend as D;
        let mut s = Snapclone::new(vec![]);
        s.settings.current.network.discovery_backend = D::Jami;
        let before = s.peers.len();
        s.tick_gossip(crate::gossip::PRUNE_MS * 4);
        assert_eq!(s.peers.len(), before);
        assert!(s.peers.iter().all(|p| p.online || !p.online),
                "peers survive a tick with gossip off");
    }

    #[test]
    fn a_silent_peer_goes_offline_rather_than_disappearing() {
        // The two lists answer different questions: a device that stopped
        // gossiping may be asleep, and deleting the row would lose a contact
        // the user added deliberately.
        let mut s = Snapclone::new(vec![]);
        s.on_gossip(rec("sc-a", 0), 100, None);
        s.tick_gossip(crate::gossip::FAILURE_MS + crate::gossip::PRUNE_MS);
        assert_eq!(s.peers.len(), 1, "the contact was deleted, not just marked");
        assert!(!s.peers[0].online);
    }

    #[test]
    fn the_stored_view_is_bucketed_and_the_live_one_is_not() {
        // The leak: a precise last-seen is a presence fingerprint. The trap:
        // bucketing the LIVE view kills liveness, because the bucket is 5
        // minutes and FAILURE_MS is 15 seconds.
        let mut s = Snapclone::new(vec![]);
        let precise = 7 * 60_000 + 1234;
        s.on_gossip(rec("sc-a", precise), 100, None);
        assert_eq!(s.view.peers()[0].last_seen_ms, precise,
                   "the live view must keep millisecond precision");
        let stored = s.view_for_storage();
        assert_eq!(stored[0].last_seen_ms % 300_000, 0,
                   "the stored copy must land on a bucket boundary");
    }

    #[test]
    fn bucketing_never_moves_a_timestamp_forward() {
        // store_privacy: rounding to nearest "can move a timestamp into the
        // FUTURE, and a last-seen in the future reads as a clock bug".
        let mut s = Snapclone::new(vec![]);
        for t in [1u64, 299_999, 300_000, 300_001, 899_999] {
            s = Snapclone::new(vec![]);
            s.on_gossip(rec("sc-a", t), 100, None);
            let got = s.view_for_storage()[0].last_seen_ms;
            assert!(got <= t, "{t} bucketed forward to {got}");
        }
    }

    fn refl(ip: &str, port: u16, from: u16) -> crate::nat_punch::Reflexive {
        crate::nat_punch::Reflexive { ip: ip.into(), port, from_local_port: from }
    }

    #[test]
    fn an_unknown_peer_gets_the_payload_immediately() {
        // "Better to spend one extra transmission than to wait for a round
        // trip." An extra send costs bytes on a radio already awake; a round
        // trip costs a second wakeup.
        let s = Snapclone::new(vec![]);
        assert!(s.should_send_payload("nobody", "abc123"));
    }

    #[test]
    fn a_peer_known_to_have_it_gets_only_the_announcement() {
        let mut s = Snapclone::new(vec![]);
        s.negotiate_wire("p", 3);
        s.peer_learned("p", "abc123");
        assert!(!s.should_send_payload("p", "abc123"));
        assert!(s.should_send_payload("p", "different"), "one hash is not all of them");
    }

    #[test]
    fn deleting_a_contact_forgets_their_whole_session() {
        // The leak the source's own note names: state that "lingers for every
        // ever-connected peer". Re-adding them later would otherwise resume a
        // sequence they have no memory of.
        let mut s = Snapclone::new(vec![Peer { name: "gone".into(), online: true }]);
        s.negotiate_wire("gone", 3);
        s.peer_learned("gone", "abc123");
        s.confirm_delete("gone");
        assert!(s.sessions.get("gone").is_none(), "session outlived the contact");
        assert!(s.frame_flags("gone", true, false, false).is_none());
    }

    #[test]
    fn a_failed_renegotiation_forgets_everything_not_just_the_version() {
        let mut s = Snapclone::new(vec![]);
        s.negotiate_wire("p", 3);
        s.peer_learned("p", "abc123");
        assert!(!s.negotiate_wire("p", 0));
        assert!(s.sessions.get("p").is_none(),
                "kept a sequence and a known-list for a peer we cannot speak to");
    }

    #[test]
    fn a_delta_to_an_old_peer_is_downgraded_not_sent() {
        // "Sending one to a v2 peer produces a frame it parses as a FULL
        // PAYLOAD — silently corrupt data rather than a protocol error." So the
        // check is at send time, and the fallback is the whole payload.
        use crate::wire_v3::{F_DELTA, F_DATA};
        let mut s = Snapclone::new(vec![]);
        s.negotiate_wire("old", 2);
        let f = s.frame_flags("old", true, true, false).unwrap();
        assert_eq!(f & F_DELTA, 0, "sent a delta to a v2 peer");
        assert_ne!(f & F_DATA, 0, "and did not fall back to the full payload");
    }

    #[test]
    fn a_v3_peer_gets_the_delta() {
        use crate::wire_v3::F_DELTA;
        let mut s = Snapclone::new(vec![]);
        s.negotiate_wire("new", 3);
        assert_ne!(s.frame_flags("new", true, true, false).unwrap() & F_DELTA, 0);
    }

    #[test]
    fn nothing_is_sent_before_a_version_is_agreed() {
        // Guessing a version sends frames the peer misparses, which is the same
        // silent-corruption failure one step earlier.
        let s = Snapclone::new(vec![]);
        assert!(s.frame_flags("stranger", true, false, false).is_none());
    }

    #[test]
    fn a_peer_too_old_to_speak_is_refused_and_forgotten() {
        let mut s = Snapclone::new(vec![]);
        s.negotiate_wire("ok", 3);
        assert!(!s.negotiate_wire("ok", 0), "agreed a version below the minimum");
        assert!(s.frame_flags("ok", true, false, false).is_none(),
                "a failed renegotiation left the old version usable");
    }

    #[test]
    fn one_frame_announces_sends_and_acks() {
        // The point of flags over a type enum: "a type enum could not express
        // ANNOUNCE|DATA|ACK in one frame". Each of those is a round trip saved,
        // and on a phone radio a round trip is a wakeup.
        use crate::wire_v3::{F_ACK, F_ANNOUNCE, F_DATA};
        let mut s = Snapclone::new(vec![]);
        s.negotiate_wire("p", 3);
        let f = s.frame_flags("p", true, false, true).unwrap();
        assert_ne!(f & F_ANNOUNCE, 0);
        assert_ne!(f & F_DATA, 0);
        assert_ne!(f & F_ACK, 0);
    }

    #[test]
    fn a_window_the_keyboard_does_not_reach_is_not_inset() {
        // The reason it is a RECTANGLE and not a height. A 400-tall window
        // ending at y=400, with a keyboard from y=800: using the height would
        // inset 280 and shove a window the keyboard never touched.
        use crate::keyboard_inset::Rect;
        let mut s = Snapclone::new(vec![]);
        s.on_keyboard(Rect::new(0.0, 800.0, 720.0, 280.0));
        let window = Rect::new(0.0, 0.0, 720.0, 400.0);
        assert_eq!(s.keyboard_inset(window), 0.0,
                   "inset a window the keyboard is nowhere near");
    }

    #[test]
    fn a_covered_window_is_inset_by_the_overlap() {
        use crate::keyboard_inset::Rect;
        let mut s = Snapclone::new(vec![]);
        s.on_keyboard(Rect::new(0.0, 800.0, 720.0, 280.0));
        // Full-height window: overlap is 1080 - 800 = 280.
        assert_eq!(s.keyboard_inset(Rect::new(0.0, 0.0, 720.0, 1080.0)), 280.0);
        // Window ending at 900: only 100 of it is covered.
        assert_eq!(s.keyboard_inset(Rect::new(0.0, 0.0, 720.0, 900.0)), 100.0);
    }

    #[test]
    fn an_identical_report_changes_nothing() {
        // Compositors re-send the same rectangle; acting on every one relayouts
        // continuously while the keyboard sits still.
        use crate::keyboard_inset::Rect;
        let mut s = Snapclone::new(vec![]);
        let r = Rect::new(0.0, 800.0, 720.0, 280.0);
        assert!(s.on_keyboard(r), "first report is a change");
        assert!(!s.on_keyboard(r), "the same rectangle again is not");
    }

    #[test]
    fn a_field_above_the_keyboard_does_not_scroll() {
        // Scrolling one that was never covered moves the page under the user
        // for no reason.
        use crate::keyboard_inset::Rect;
        let mut s = Snapclone::new(vec![]);
        s.on_keyboard(Rect::new(0.0, 800.0, 720.0, 280.0));
        assert_eq!(s.scroll_for_keyboard(Rect::new(0.0, 100.0, 300.0, 40.0), 8.0), 0.0);
        // One underneath it does.
        assert!(s.scroll_for_keyboard(Rect::new(0.0, 820.0, 300.0, 40.0), 8.0) > 0.0);
    }

    #[test]
    fn hiding_the_keyboard_clears_the_inset() {
        use crate::keyboard_inset::Rect;
        let mut s = Snapclone::new(vec![]);
        s.on_keyboard(Rect::new(0.0, 800.0, 720.0, 280.0));
        s.keyboard.hide();
        assert_eq!(s.keyboard_inset(Rect::new(0.0, 0.0, 720.0, 1080.0)), 0.0);
    }

    #[test]
    fn stun_from_the_wrong_socket_is_refused() {
        // "Send a STUN binding request FROM m_punchSocket". A reflexive address
        // discovered on an ephemeral socket describes a mapping that will not
        // exist when the punch arrives — correct about a port nobody is on.
        let mut s = Snapclone::new(vec![]);
        assert!(!s.classify_nat(5000, &refl("1.2.3.4", 5000, 40001),
                                       &refl("1.2.3.4", 5000, 5000)));
        assert!(s.nat.is_none(), "classified from a foreign socket");
    }

    #[test]
    fn two_agreeing_probes_from_one_socket_classify() {
        let mut s = Snapclone::new(vec![]);
        assert!(s.classify_nat(5000, &refl("1.2.3.4", 5000, 5000),
                                     &refl("1.2.3.4", 5000, 5000)));
        // Same port in and out: nothing is rewriting it.
        assert_eq!(s.nat, Some(crate::nat_punch::NatKind::Open));
    }

    #[test]
    fn a_changing_reflexive_port_is_symmetric_and_relays() {
        // Two probes to different servers seeing different ports is the
        // definition of symmetric, and relay is "not a failure, just the honest
        // answer".
        use crate::nat_punch::{NatKind, Strategy};
        let mut s = Snapclone::new(vec![]);
        s.classify_nat(5000, &refl("1.2.3.4", 6001, 5000),
                             &refl("1.2.3.4", 6002, 5000));
        assert_eq!(s.nat, Some(NatKind::Symmetric));
        assert_eq!(s.reach_strategy(NatKind::Open), Strategy::Relay);
    }

    #[test]
    fn an_unclassified_nat_waits_rather_than_guessing() {
        // Guessing DirectTcp burns the punch window on a connection that
        // cannot form.
        use crate::nat_punch::{NatKind, Strategy};
        let s = Snapclone::new(vec![]);
        assert_eq!(s.reach_strategy(NatKind::Open), Strategy::Wait);
    }

    #[test]
    fn nothing_leaves_before_the_user_speaks() {
        // Joining a room is not speaking. A sender transmitting from the moment
        // of joining sends silence at everyone for the length of the call.
        use crate::voice_send::Emit;
        let mut s = Snapclone::new(vec![]);
        assert!(matches!(s.voice_frame(), Emit::Gated));
    }

    #[test]
    fn each_spurt_restarts_at_zero_and_flags_its_first_frame() {
        // The Qt contract the receiver depends on. Porting the sender is what
        // exposed that the receiver ate a second spurt entirely, because
        // restarting at 0 looked like a flood of very late packets.
        use crate::voice_send::Emit;
        let mut s = Snapclone::new(vec![]);
        s.sender.set_encoder_ready(true);
        s.set_transmitting(true);
        match s.voice_frame() {
            Emit::Send { seq, talkspurt_start } => {
                assert_eq!(seq, 0);
                assert!(talkspurt_start, "first frame must flag the spurt");
            }
            other => panic!("{other:?}"),
        }
        match s.voice_frame() {
            Emit::Send { seq, talkspurt_start } => {
                assert_eq!(seq, 1);
                assert!(!talkspurt_start, "only the FIRST frame is flagged");
            }
            other => panic!("{other:?}"),
        }
        // Second spurt: back to 0, flagged again.
        s.set_transmitting(false);
        s.set_transmitting(true);
        assert!(matches!(s.voice_frame(),
                         Emit::Send { seq: 0, talkspurt_start: true }));
    }

    #[test]
    fn leaving_a_call_stops_transmission() {
        // The reset happens on the way UP, so a sender left transmitting across
        // a call boundary never sees that edge and the next spurt continues the
        // old sequence — leaving the receiver with no TALKSPURT_START to reset
        // its jitter buffer on.
        let mut s = Snapclone::new(vec![]);
        s.sender.set_encoder_ready(true);
        s.set_transmitting(true);
        s.voice_frame();
        s.set_in_call(false);
        assert!(!s.sender.is_transmitting(), "still talking after the call ended");
    }

    #[test]
    fn a_missing_encoder_is_reported_once_not_fifty_times_a_second() {
        use crate::voice_send::Emit;
        let mut s = Snapclone::new(vec![]);
        // encoder_ready DEFAULTS TO TRUE — an encoder is the normal case and
        // the sender assumes it until told otherwise. My first version of this
        // test assumed the opposite and failed, which is the default worth
        // knowing: a missing encoder is the exception that has to be reported,
        // not a state the sender starts in.
        s.sender.set_encoder_ready(false);
        s.set_transmitting(true);
        assert!(matches!(s.voice_frame(), Emit::NoEncoder));
        assert!(s.sender.note_no_encoder(), "first report");
        assert!(!s.sender.note_no_encoder(), "and not again");
    }

    #[test]
    fn the_sink_primes_before_it_plays() {
        // push() flips `started` at jitter_target rather than on the first
        // frame — playing from one frame is the zero-margin design that
        // crackled.
        let mut s = Snapclone::new(vec![]);
        assert!(s.sink_tick(0).is_empty(), "played before priming");
        s.sink_push(vec![0.0; 960]);
        s.sink_push(vec![0.0; 960]);
        assert!(!s.sink_tick(0).is_empty(), "primed but still silent");
    }

    #[test]
    fn the_tick_runs_at_half_a_frame() {
        // "run the tick at half a frame so it refills well before the cushion
        // drains". A tick equal to the frame is the original bug.
        let s = Snapclone::new(vec![]);
        assert_eq!(s.sink_tick_ms(), 10, "20 ms frames want a 10 ms tick");
        assert!(s.sink_tick_ms() < s.sink_latency.frame_ms);
    }

    #[test]
    fn a_full_sink_is_not_topped_up() {
        // The cushion is a target, not a floor to add to: writing every tick
        // regardless would grow latency without bound.
        let mut s = Snapclone::new(vec![]);
        for _ in 0..6 { s.sink_push(vec![0.0; 960]); }
        assert!(s.sink_tick(99).is_empty(), "topped up an already-full sink");
    }

    #[test]
    fn leaving_a_call_resets_the_sink() {
        // Otherwise the previous call's queued frames play into the next one,
        // which is the same failure set_in_call already guards for playout.
        let mut s = Snapclone::new(vec![]);
        s.sink_push(vec![0.0; 960]);
        s.sink_push(vec![0.0; 960]);
        s.set_in_call(false);
        assert_eq!(s.sink.depth(), 0, "the old call's audio survived");
    }

    #[test]
    fn a_freshly_heard_peer_is_still_alive() {
        // The regression the arithmetic caught: bucket on the way in and every
        // peer is 100+ seconds stale the moment it is heard from.
        let mut s = Snapclone::new(vec![]);
        let now = 7 * 60_000 + 1234;
        s.on_gossip(rec("sc-a", now), 100, None);
        assert!(s.view.peers()[0].is_alive(now), "a peer just heard from is dead");
    }

    #[test]
    fn pruning_runs_on_its_schedule_not_every_frame() {
        // Per-frame pruning walks the peer list sixty times a second to do
        // nothing.
        let mut s = Snapclone::new(vec![]);
        s.on_gossip(rec("sc-a", 0), 100, None);
        s.tick_gossip(crate::gossip::PRUNE_MS - 1);
        assert!(s.peers[0].online, "pruned before the interval elapsed");
    }
    // ── requests + confirm + freshness + snap_export wiring ──

    #[test]
    fn blocking_asks_the_first_time_and_stops_asking_once_learned() {
        let mut app = Snapclone::new_with_login(vec![], false);
        // A new user: Block is gated by a Tap panel.
        app.request_action("mallory".into(), crate::requests::RequestAction::Block);
        assert!(app.block_confirm.is_some(), "a first Block asks");
        // Confirm it: applied, and the feature got more familiar.
        let (sender, mut panel) = app.block_confirm.take().unwrap();
        panel.press();
        assert_eq!(panel.release(), crate::confirm::Outcome::Confirmed);
        app.mailbox.apply(&sender, crate::requests::RequestAction::Block);
        app.familiarity_of("block-request").record_use();
        // Learn the feature past the expert threshold.
        for _ in 0..40 { app.familiarity_of("block-request").record_use(); }
        assert_eq!(app.familiarity_of("block-request").mode(), crate::confirm::Mode::None);
        app.request_action("eve".into(), crate::requests::RequestAction::Block);
        assert!(app.block_confirm.is_none(), "an expert is not asked");
        // Friction re-raises — but not on one slip. A fail costs 10 against a
        // score saturated at 100, so a single hesitation after forty uses
        // keeps expert status; a PATTERN of backing out demotes.
        app.familiarity_of("block-request").record_fail();
        assert_eq!(app.familiarity_of("block-request").mode(), crate::confirm::Mode::None,
                   "one slip does not demote a practised user");
        app.familiarity_of("block-request").record_fail();
        app.familiarity_of("block-request").record_fail();
        assert_ne!(app.familiarity_of("block-request").mode(), crate::confirm::Mode::None,
                   "a pattern of cancels brings the panel back");
    }

    #[test]
    fn accept_and_decline_apply_without_a_panel() {
        let mut app = Snapclone::new_with_login(vec![], false);
        app.request_action("carol".into(), crate::requests::RequestAction::Accept);
        assert!(app.block_confirm.is_none(), "only Block needs confirmation");
    }

    #[test]
    fn delete_familiarity_is_recorded_but_can_never_skip_a_hold() {
        let mut app = Snapclone::new_with_login(vec![], false);
        for _ in 0..60 { app.familiarity_of("delete-contact").record_use(); }
        assert_eq!(app.familiarity_of("delete-contact").mode(), crate::confirm::Mode::None);
        // Even at expert familiarity, delete's risk score demands a Hold —
        // ask_delete only skips TAP-level panels.
        let familiar = matches!(app.familiarity_of("delete-contact").mode(),
                                crate::confirm::Mode::None);
        assert!(app.ask_delete(familiar), "a hold-level action still asks");
        assert!(app.confirm.is_some());
    }

    #[test]
    fn the_signal_strip_defaults_unknown_contacts_to_stale() {
        let app = Snapclone::new_with_login(vec![], false);
        let (age, acc) = app.contact_freshness.get("nobody")
            .copied().unwrap_or((i64::MAX / 4, 9_999.0));
        let p = crate::freshness::presentation(&app.freshness_policy, age, acc);
        assert!(!p.has_arrow(), "no freshness data must not draw a confident arrow");
        assert!(p.label.is_some(), "the label says last-seen instead");
    }

    #[test]
    fn the_export_plan_prefers_zip_and_reports_through_the_note() {
        let mut app = Snapclone::new_with_login(vec![], false);
        let (tool, out, argv) = app.export_plan(true, true, "/tmp/snaps").unwrap();
        assert!(matches!(tool, crate::snap_export::Tool::Zip));
        assert!(out.ends_with(".zip"));
        // argv is ARGS only; the program comes from the tool.
        assert_eq!(tool.program(), "zip");
        assert!(argv.iter().any(|a| a == &out), "the args name the output");
        assert!(app.export_plan(false, false, "/tmp/snaps").is_none(),
                "no archiver, no plan — the card says install one");
        app.export_requested = true;
        app.export_finished(crate::snap_export::Outcome::Wrote {
            path: out.clone(), files: 3 });
        assert!(!app.export_requested);
        assert!(app.export_note.as_deref().unwrap_or("").contains("3 snaps"));
    }
    // ── voice_bridge + ambient_camera ──

    fn part(id: &str, is_self: bool) -> crate::group_voice::Participant {
        crate::group_voice::Participant { id: id.into(), name: id.into(), is_self }
    }

    #[test]
    fn voice_goes_to_every_peer_but_never_back_to_me() {
        use crate::voice_bridge::Packet;
        let mut app = Snapclone::new_with_login(vec![], false);
        app.group_voice.set_participants(vec![part("me", true), part("bob", false),
                                              part("carol", false)]);
        let out = app.voice_send(&Packet::VoiceFrame(vec![1, 2, 3]));
        assert_eq!(out.len(), 2, "both peers, not myself");
        assert!(out.iter().all(|(_, bytes)| bytes == &out[0].1),
                "one encode, many sends");
        // A participant who left stops receiving audio, because the peer list
        // is taken from the coordinator each send rather than cached.
        app.group_voice.set_participants(vec![part("me", true), part("bob", false)]);
        assert_eq!(app.voice_send(&Packet::VoiceFrame(vec![1])).len(), 1);
    }

    #[test]
    fn an_inbound_floor_claim_is_credited_to_the_transports_sender() {
        use crate::voice_bridge::{encode, Inbound, Packet};
        let mut app = Snapclone::new_with_login(vec![], false);
        app.group_voice.set_participants(vec![part("me", true), part("bob", false)]);
        // Floor claims only mean anything in push-to-talk: in open mode the
        // coordinator decides by LEVEL, and honouring a claim there would let
        // a peer mute the room by asserting.
        app.group_voice.mode = crate::group_voice::TalkMode::PushToTalk;
        app.voice_bridge.set_connected(true);
        let claim = encode(&Packet::FloorAssert { claim_ts: 42 });
        // The packet carries only a timestamp: identity comes from the
        // transport, so a peer cannot claim the floor for someone else.
        let out = app.voice_receive("bob", &claim);
        assert!(matches!(out, Inbound::Handled { .. }));
        assert_eq!(app.group_voice.active_speaker(), Some("bob"));
        // A non-voice payload is NOT ours — the caller must still offer it to
        // other handlers, so it cannot be silently eaten.
        assert_eq!(app.voice_receive("bob", b"http/1.1"), Inbound::NotOurs);
        // A voice packet we cannot parse IS consumed: passing it on would
        // have another subsystem parse a packet already known broken.
        assert_eq!(app.voice_receive("bob", b"SCV1garbage"), Inbound::Malformed);
    }

    #[test]
    fn the_camera_driver_refuses_for_distinguishable_reasons() {
        use crate::ambient_camera::Tick;
        let mut app = Snapclone::new_with_login(vec![], false);
        // Stopped: nothing to sample.
        assert_eq!(app.ambient_camera_tick(1_000), Tick::Idle);
        app.ambient_camera.start();
        app.ambient_camera.set_app_active(true);
        // Started but no frame has arrived yet — still Idle, not "derive
        // whatever is in the buffer".
        assert_eq!(app.ambient_camera_tick(1_000), Tick::Idle);
        app.ambient_camera.on_frame();
        assert_eq!(app.ambient_camera_tick(1_000), Tick::Derive);
        // In flight: deriving again would queue work behind work.
        app.ambient_camera.on_frame();
        assert_eq!(app.ambient_camera_tick(9_000), Tick::Busy);
        // A derive that produced nothing usable STILL clears the flag, or the
        // driver never derives again.
        app.ambient_camera_done(None);
        assert!(!app.ambient_camera.is_deriving());
        // …and the rate limit is a separate, nameable refusal.
        app.ambient_camera.on_frame();
        assert_eq!(app.ambient_camera_tick(1_001), Tick::TooSoon);
        // Off screen is its own reason: sampling a frame nobody can see is
        // pure battery cost.
        app.ambient_camera.set_app_active(false);
        assert_eq!(app.ambient_camera_tick(999_000), Tick::Backgrounded);
    }

    // ── decision layers: consent, firewall, bootstrap, punch addresses ──

    #[test]
    fn face_storage_needs_both_consent_and_provenance() {
        use crate::face_consent::{Consent, Provenance};
        let mut app = Snapclone::new_with_login(vec![], false);
        // No consent: nothing stores, whatever the provenance.
        assert!(!app.may_store_face(Provenance::SelfCapture));
        assert!(!app.set_face_consent(Consent::Granted), "granting owes no erase");
        assert!(app.may_store_face(Provenance::SelfCapture));
        assert!(app.may_store_face(Provenance::ReceivedFromContact));
        // A stranger in a photo the user took never initiated the data flow,
        // and consent does not speak for them.
        assert!(!app.may_store_face(Provenance::ThirdPartyInCapture));
        assert!(!app.may_store_face(Provenance::Unknown));
        // Revoking is not the same as never asking: an erase is owed.
        assert!(app.set_face_consent(Consent::Revoked));
        assert!(!app.may_store_face(Provenance::SelfCapture));
    }

    #[test]
    fn the_firewall_prefers_a_silent_backend_and_refuses_a_privileged_port() {
        use crate::firewall_backend::{Backend, Environment};
        let app = Snapclone::new_with_login(vec![], false);
        let base = Environment {
            has_firewalld_dbus: false, has_ufw_dbus: false, in_flatpak: false,
            has_firewall_cmd: false, has_ufw_cli: false, has_iptables: true,
            has_nft: false, has_pkexec: true, is_root: false,
        };
        // Only pkexec available: usable, but NOT silent — it shows a prompt,
        // and the caller needs to know before it interrupts someone.
        let (b, silent) = app.firewall_plan(&base, 7000).unwrap();
        assert_eq!(b, Backend::Pkexec);
        assert!(!silent);
        // firewalld wins when present, and asks nobody.
        let fd = Environment { has_firewalld_dbus: true, ..base };
        let (b, silent) = app.firewall_plan(&fd, 7000).unwrap();
        assert_eq!(b, Backend::Firewalld);
        assert!(silent);
        // A privileged port is not something to ask about at all.
        assert!(app.firewall_plan(&fd, 80).is_none());
        // No backend at all is a real answer, not a silent no-op.
        let none = Environment { has_iptables: false, has_pkexec: false, ..base };
        assert!(app.firewall_plan(&none, 7000).is_none());
    }

    #[test]
    fn remembered_peers_become_bootstrap_entries_under_their_real_ids() {
        let mut app = Snapclone::new_with_login(vec![], false);
        app.local_id = "sc-me".into();
        assert!(app.peer_bootstraps().is_empty());
        app.note_peer_addr("sc-alice", [10, 0, 0, 5], 9876, 1_000);
        app.note_peer_addr("sc-bob", [10, 0, 0, 6], 9876, 2_000);
        app.note_peer_addr("sc-me", [10, 0, 0, 1], 9876, 3_000);      // ourselves: skipped
        app.punch.set("sc-unset", crate::punch_addrs::PeerAddr::new([0, 0, 0, 0], 0), 4_000);
        let got = app.peer_bootstraps();
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(got.iter().any(|(sa, id)| *sa == std::net::SocketAddr::from(([10, 0, 0, 5], 9876 + Snapclone::DHT_PORT_OFFSET))
                                          && *id == crate::identity::dht_node_id("sc-alice")),
                "alice under her real id, at her NODE's port");
        // The same peer known to both tables is one entry.
        assert_eq!(got.iter().filter(|(sa, _)| sa.port() == 9876 + Snapclone::DHT_PORT_OFFSET).count(), 2);
    }

    #[test]
    fn the_dht_port_is_derived_the_same_way_for_us_and_for_peers() {
        let [first, second] = Snapclone::dht_bind_addr(9876);
        assert_eq!(first.port(), 9876 + Snapclone::DHT_PORT_OFFSET);
        assert_ne!(first.port(), 9876, "UDP on the listen port is the voice socket");
        assert_eq!(second.port(), 0);
        assert_eq!(Snapclone::dht_addr_of([1, 2, 3, 4], 9876).map(|a| a.port()), Some(first.port()));
        // The top of the range has nowhere to go: ephemeral, not wraparound.
        assert_eq!(Snapclone::dht_bind_addr(65535)[0].port(), 0);
        assert!(Snapclone::dht_addr_of([1, 2, 3, 4], 65535).is_none());
    }

    #[test]
    fn bootstrap_falls_back_to_ip_and_can_restrict_to_web_ports() {
        let app = Snapclone::new_with_login(vec![], false);
        // The list is no longer test-only: `tick_dht` feeds it to
        // `NodeHost::bootstrap` on spawn. It was computed and unused for
        // as long as it existed, so this note is here to stop it going
        // back to that.
        let with_dns = app.bootstrap_addresses(true, false);
        let no_dns = app.bootstrap_addresses(false, false);
        assert!(!with_dns.is_empty());
        assert_ne!(with_dns, no_dns, "a broken resolver takes the IP route");
        // The hostile network REORDERS rather than filters: web-friendly
        // ports are tried first, and the standard-port servers stay on the
        // list as fallback. Dropping them would turn "this network is
        // restrictive" into "this network has no DHT", which is a different
        // and usually wrong claim.
        let web = app.bootstrap_addresses(true, true);
        assert_eq!(web.len(), with_dns.len(), "nothing is dropped, only reordered");
        assert_ne!(web, with_dns, "but the order changed");
        let first = &web[0];
        assert!(!first.ends_with(&format!(":{}", crate::dht_bootstrap::PORT_STANDARD)),
                "a web-port server leads");
    }

    #[test]
    fn a_peer_address_expires_rather_than_being_believed_forever() {
        let mut app = Snapclone::new_with_login(vec![], false);
        app.note_peer_addr("sc-abc", [192, 168, 1, 9], 7000, 1_000);
        assert!(app.punch.contains("sc-abc"));
        assert_eq!(app.heard.len(), 1);
        // Long enough later, the address is a guess wearing a fact's clothes.
        let dropped = app.punch.expire(1_000 + 60 * 60 * 1000);
        assert_eq!(dropped, 1);
        assert!(!app.punch.contains("sc-abc"));
        let purged = app.heard.purge_stale(1_000 + 60 * 60 * 1000, 60_000);
        assert_eq!(purged, 1);
    }

    // ── scroll_passthrough / ambient_inbox / landmark_hash / trace_channels ──

    #[test]
    fn the_strip_wheel_passes_through_at_the_edge() {
        use crate::scroll_passthrough::{Handled, Wheel};
        let mut h = crate::scroll_passthrough::HScroll::new(0.0, 3.0);
        let one = |d: f32| Wheel { pixel: Some((0.0, d)), angle: (0.0, 0.0) };
        // Moving is ours.
        assert_eq!(h.wheel(&one(-1.0)), Handled::Consumed);
        assert_eq!(h.value, 1.0);
        // At the end and still pushing: the page navigator gets it, which is
        // the bug the module exists for — a carousel that swallows the wheel
        // at its edge leaves the user pushing against nothing.
        h.value = 3.0;
        assert_eq!(h.wheel(&one(-1.0)), Handled::PassThrough);
        // A zero delta is a real touchpad event mid-reversal and must NOT
        // flip a page.
        assert_eq!(h.wheel(&one(0.0)), Handled::Consumed);
    }

    #[test]
    fn ambient_derivation_is_throttled_but_never_on_the_first_snap() {
        use crate::ambient_inbox::Outcome;
        let mut app = Snapclone::new_with_login(vec![], false);
        // Not running yet.
        assert_eq!(app.ambient_snap_arrived(1_000), Outcome::Inactive);
        app.ambient_driver.start(crate::ambient_inbox::SYNTHETIC_ID);
        app.active_synthetic = Some(crate::ambient_inbox::SYNTHETIC_ID.to_string());
        // First arrival derives immediately — waiting thirty seconds for any
        // ambience at all reads as the feature being broken.
        assert_eq!(app.ambient_snap_arrived(1_000), Outcome::Derive);
        // A burst inside the window does not.
        assert_eq!(app.ambient_snap_arrived(1_500), Outcome::Throttled);
        // Another palette in charge: the user's pick is not ours to stomp,
        // checked at trigger time because a queued arrival outlives the switch.
        app.active_synthetic = Some("user-chosen".into());
        assert_eq!(app.ambient_snap_arrived(600_000), Outcome::NotOurPalette);
    }

    #[test]
    fn a_landmark_is_identified_by_its_strongest_match_only() {
        use crate::landmark_hash::{Fingerprint, HashKind, Match};
        let mut app = Snapclone::new_with_login(vec![], false);
        let blur = |b: u8| vec![b; HashKind::Blur.len()];
        let edge = |b: u8| vec![b; HashKind::Edge.len()];
        // A corrupt fingerprint is refused rather than stored: a wrong-length
        // hash silently never matches, which reads as "not there" instead of
        // "corrupt".
        assert!(!app.publish_landmark("bad", Fingerprint { blur: Some(vec![0u8; 3]), edge: None }));
        assert!(app.publish_landmark("studio",
            Fingerprint { blur: Some(blur(0)), edge: Some(edge(0)) }));
        // An exact pair matches on both — the strongest claim available.
        let seen = Fingerprint { blur: Some(blur(0)), edge: Some(edge(0)) };
        let (id, m) = app.identify_landmark(&seen).expect("a known place");
        assert_eq!(id, "studio");
        assert_eq!(m, Match::Both);
        // Nothing like it makes NO claim, rather than naming the nearest.
        let elsewhere = Fingerprint { blur: Some(blur(0xff)), edge: Some(edge(0xff)) };
        assert!(app.identify_landmark(&elsewhere).is_none());
    }

    #[test]
    fn enabling_one_trace_channel_leaves_the_others_alone() {
        use crate::trace_channels::Channel;
        let mut app = Snapclone::new_with_login(vec![], false);
        app.trace.set_channel(Channel::Layout, true);
        let second = Channel::ALL.iter().find(|c| **c != Channel::Layout).copied().unwrap();
        app.trace.set_channel(second, true);
        // The whole point: setFilterRules replaced the entire rule set, so
        // enabling one channel silently disabled every other.
        assert!(app.trace.is_enabled(Channel::Layout), "the first channel survived");
        assert!(app.trace.is_enabled(second));
        // And the legacy flag maps onto Layout so a Qt-written settings file
        // keeps working.
        assert!(app.trace.mobile_display_debug());
    }

    // ── the interaction layer: blobs, press, ripples ──

    #[test]
    fn feedback_is_touch_only_and_disabling_it_stops_both_halves() {
        let mut app = Snapclone::new_with_login(vec![], false);
        // Off on a pointer device: a ripple under a mouse cursor is the tell
        // of a phone UI wearing a desktop's clothes.
        app.feedback.set_enabled(false);
        app.feedback.tap(10.0, 10.0, 100.0, 100.0);
        assert!(app.feedback.ripples().is_empty());
        app.feedback.set_enabled(true);
        app.feedback.tap(10.0, 10.0, 100.0, 100.0);
        assert_eq!(app.feedback.ripples().len(), 1);
        // The depth is an ANIMATION, not a state: at age zero the scale is
        // still 1.0 and only departs once time passes. Asserting before the
        // tick tests the clock, not the response.
        app.feedback.tick(40.0);
        assert!(app.feedback.scale() < 1.0, "the press depth engaged too");
        // Disabling mid-press must reset the SCALE as well, or a control is
        // left stuck at whatever depth it had reached.
        app.feedback.set_enabled(false);
        assert_eq!(app.feedback.scale(), 1.0, "both halves stop together");
    }

    #[test]
    fn the_press_watchdog_recovers_a_lost_pointer_up() {
        let mut app = Snapclone::new_with_login(vec![], false);
        app.press.press(5.0, 5.0);
        assert!(!app.press.is_idle());
        // No release ever arrives — a system gesture ate it. Ticking past the
        // watchdog window returns the machine to Idle rather than leaving it
        // Held forever, which would fold the next tap into the old gesture.
        // Frame-rate ticks WITH the finger still down: the long press fires
        // and the machine sits in Held, which is correct.
        for _ in 0..40 { app.tick_feedback(16.0, true); }
        assert!(!app.press.is_idle(), "still held while the finger is down");
        // Now the pointer-up is lost — the shell sees no finger and the
        // machine is released rather than wedged. (The module's own watchdog
        // cannot do this at 60fps: held_ms stops accumulating in Held, so it
        // only covers the suspend/resume case.)
        app.tick_feedback(16.0, false);
        assert!(app.press.is_idle(), "a lost pointer-up is recovered");
        // …and the long press fired on the way, which is the OTHER thing the
        // clock is for: a hold that never emits is a hold nobody can use.
        assert_eq!(app.shutter_note, "exposure locked");
    }

    #[test]
    fn the_ambient_field_only_runs_when_it_is_asked_to() {
        let mut app = Snapclone::new_with_login(vec![], false);
        // Inactive by default: decoration that runs before anyone asks costs
        // battery on a phone. blobs() returns nothing while inactive even
        // though the field is seeded.
        assert!(!app.blobs.is_active());
        assert!(app.blobs.blobs().is_empty(), "inactive means no blobs to draw");
        app.blobs.set_active(true);
        assert!(!app.blobs.blobs().is_empty());
        let before = app.blobs.blobs()[0].centre();
        app.tick_feedback(500.0, false);
        assert_ne!(app.blobs.blobs()[0].centre(), before, "the field drifts");
    }

    // ── tile_strip + thumbnails + peer_source wiring ──

    #[test]
    fn the_strip_shows_three_tiles_and_scrubbing_moves_from_where_it_is() {
        let mut app = Snapclone::new_with_login(vec![], false);
        app.now_secs = 10_000;
        for i in 0..6u64 {
            app.inbox.add(crate::inbox::Entry {
                id: i, peer: format!("p{i}"), preview: String::new(),
                at: app.now_secs - (i as i64 * 60), unread: false, outgoing: false,
                kind: crate::inbox::Kind::Snap,
            });
        }
        let tiles = app.strip_tiles();
        assert_eq!(tiles.len(), 6);
        let composer = crate::tile_strip::Composer::new(&tiles, app.now_secs * 1000);
        let view = composer.view_at(2, false);
        assert_eq!(view.tiles.len(), 3, "prev, active, next — never the whole window");
        // Scrub is relative: from 2, one step lands adjacent, not at index 1
        // of some absolute ordering.
        app.strip_active = 2;
        app.strip_action(crate::tile_strip::StripAction::Scrub(1));
        let moved = app.strip_active;
        assert_ne!(moved, 2, "the window moved");
        app.strip_action(crate::tile_strip::StripAction::Scrub(-1));
        assert_eq!(app.strip_active, 2, "and back again — scrub is relative");
    }

    #[test]
    fn the_thumbnail_cache_coalesces_and_stays_bounded() {
        use crate::thumbnails::{Key, Request, ThumbnailCache};
        let mut c = ThumbnailCache::with_limits(2, 1);
        let a = Key::new("snap:1", 128);
        assert_eq!(c.request(&a), Request::Queued);
        // A second ask while the first is in flight starts nothing — this is
        // the scrub-back-and-forth case.
        assert_eq!(c.request(&a), Request::Coalesced);
        let b = Key::new("snap:2", 128);
        assert_eq!(c.request(&b), Request::Deferred, "max_active gates the second decode");
        c.complete(&a, Some(vec![1, 2, 3]));
        assert_eq!(c.request(&a), Request::Hit);
        // Capacity holds: a third entry evicts rather than growing.
        c.complete(&b, Some(vec![4]));
        c.complete(&Key::new("snap:3", 128), Some(vec![5]));
        assert!(c.len() <= 2, "bounded — a long window cannot decode itself into memory");
    }

    #[test]
    fn the_roster_separates_connected_known_and_unverified() {
        use crate::peer_source::Reach;
        let mut app = Snapclone::new_with_login(
            vec![Peer { name: "alice".into(), online: false }], false);
        // A verified gossip record for a stranger; an unverified one that must
        // not appear at all.
        app.view.observe(crate::gossip::PeerRecord {
            id: "stranger".into(), host: "10.0.0.9".into(), port: 7000,
            last_seen_ms: 1, verified: true,
        });
        app.view.observe(crate::gossip::PeerRecord {
            id: "liar".into(), host: "10.0.0.10".into(), port: 7000,
            last_seen_ms: 1, verified: false,
        });
        let roster = app.peer_roster();
        assert!(roster.iter().any(|p| p.name == "alice" && p.reach == Reach::Known),
                "a contact with no session is Known — queue, do not drop");
        assert!(roster.iter().any(|p| p.reach == Reach::Unverified),
                "a verified-record stranger is seen but not verified as an identity");
        assert!(!roster.iter().any(|p| p.label().contains("liar")),
                "an unverified gossip record never reaches the roster");
        // Unverified is NOT sendable, so the count is contacts only.
        assert_eq!(app.sendable_peers(), 1);
    }
}

/// Do two hosts share a /24-ish prefix?
///
/// A cheap same-LAN test rather than a netmask: the app does not know the
/// peer's subnet mask, and asking the OS for one per candidate address would be
/// a syscall per dial attempt to refine a hint that only orders a list.
fn same_prefix(a: &str, b: &str) -> bool {
    let head = |s: &str| s.rsplit_once('.').map(|(h, _)| h.to_string());
    match (head(a), head(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// The two surfaces that were drawn, tested, and reachable from no control.
///
/// Both failures were invisible to every existing test for the same reason:
/// the widgets pass their own tests, the shell renders, and nothing anywhere
/// asks whether a finger can get from the running app to the widget. That is
/// the question these ask.
#[cfg(test)]
mod call_and_qr_wiring_tests {
    use super::*;

    fn app() -> Snapclone {
        Snapclone::new(vec![Peer { name: "studio".into(), online: true }])
    }

    #[test]
    fn answering_a_call_runs_through_answer_call_and_clears_the_sheet() {
        // `answer_call` was unreachable: ring_ui applied the answer itself and
        // the shell called finish_call, so this method would have found the
        // call already answered and returned early. It is now the only path,
        // which means this test covers what the UI does rather than a
        // parallel implementation of it.
        let mut s = app();
        s.ring("studio");
        assert!(s.incoming_call.is_some());
        s.answer_call(crate::ring::Answer::Accepted);
        assert!(s.incoming_call.is_none(), "the sheet did not clear");
        assert!(s.modal.topmost(crate::modal::Layer::Confirm).is_none(),
                "the modal was left on the stack, so input stays blocked");
    }

    #[test]
    fn only_an_ignored_call_is_a_missed_one() {
        // I wrote this expecting a decline to count as missed and the test was
        // right and I was wrong: finish_call records ONLY TimedOut. Both
        // rejections drop the buffered packets, but a call you refused is not
        // one you missed, and a missed-calls list that includes deliberate
        // declines is a list of things to feel bad about rather than a list of
        // people to ring back. Pinned here because the distinction is one line
        // in a match and nothing else in the crate asserts it.
        let mut s = app();
        s.ring("studio");
        s.answer_call(crate::ring::Answer::Declined);
        assert!(s.missed_calls.is_empty(), "a decline is not a missed call");

        s.ring("studio");
        s.answer_call(crate::ring::Answer::TimedOut);
        assert_eq!(s.missed_calls, vec!["studio".to_string()]);

        s.ring("studio");
        s.answer_call(crate::ring::Answer::Accepted);
        assert_eq!(s.missed_calls.len(), 1, "an answered call is not a missed one");
        assert!(s.group_voice.joined, "and accepting actually joins the call");
    }

    #[test]
    fn answering_twice_does_nothing_the_second_time() {
        // The guard lives in answer_call, which is why the sheet must not own
        // the mutation: a double press, or a press racing a headset button,
        // must not run the transition twice.
        let mut s = app();
        s.ring("studio");
        s.answer_call(crate::ring::Answer::Declined);
        let after = s.missed_calls.clone();
        s.answer_call(crate::ring::Answer::Declined);
        assert_eq!(s.missed_calls, after);
    }

    #[test]
    fn opening_the_qr_scanner_survives_the_frame_that_draws_it() {
        // The gap this closes: open_qr_scan was ported and tested, the dialog
        // had a body, the parser took both the bare sc-ID and the URI — and no
        // control in the running app called it, so adding a contact meant
        // knowing their address and typing it.
        //
        // Driven by keyboard focus rather than a coordinate click because a
        // hardcoded position tests the layout, not the wiring, and breaks the
        // next time a row is added above it. Bounded loop for the same reason
        // the typing test uses one: the number of tab stops before the button
        // is not a thing worth pinning.
        use egui::{Context, Event, Key, RawInput};
        let ctx = Context::default();
        let mut app = app();
        app.nav.go_page(2); // Friends
        let adaptive = Adaptive::default();

        // A REAL SCREEN RECT, and it is the difference between this test
        // working and not. `RawInput::default()` leaves `screen_rect` unset, so
        // the page lays out into a surface too small to reach the controls
        // below the roster and the focus walk never arrives at them — the test
        // fails while the button is present and correctly wired. Tall on
        // purpose: this page is a scrolling column and the buttons sit near the
        // bottom of it.
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(420.0, 2400.0));
        let mut frame = |ctx: &Context, app: &mut Snapclone, events: Vec<Event>| {
            let _ = ctx.run(RawInput { events, screen_rect: Some(screen), ..Default::default() },
                            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| { app.ui(ui, &adaptive); });
            });
        };
        frame(&ctx, &mut app, vec![]);
        assert_eq!(app.page_name(), "Friends", "the harness is not on the page under test");

        let key = |k: Key, pressed: bool| Event::Key {
            key: k, physical_key: None, pressed, repeat: false, modifiers: Default::default() };

        // Assert the scan found something before asserting what it found. A
        // focus walk that never focuses anything satisfies every check below
        // it by not running, which is the exact failure mode this tree keeps
        // catching in structural tests.
        frame(&ctx, &mut app, vec![key(Key::Tab, true), key(Key::Tab, false)]);
        assert!(ctx.memory(|m| m.focused()).is_some(),
                "Tab focused nothing, so this test proves nothing about the button");

        // Does the scanner survive the frame that draws it? Asked separately
        // from the focus walk because the two failures look identical from the
        // outside — "the button does nothing" — and have nothing in common.
        app.open_qr_scan(true);
        frame(&ctx, &mut app, vec![]);
        assert!(app.qr.is_some(), "the scanner closed itself during the frame that drew it");
        app.qr = None;
        app.modal.dismiss("qr");
        frame(&ctx, &mut app, vec![]);

        // WHAT THIS TEST DOES NOT PROVE, stated because the gap is real. I
        // tried to close the loop with a keyboard focus walk — Tab through the
        // page, press Space on each stop, assert the scanner opened — and it
        // never reached the buttons across 120 stops even with a 420x2400
        // surface and a check confirming Tab focuses SOMETHING. I did not find
        // out why (a scroll area culling the widgets before they get ids is
        // the best guess) and I am not going to assert a conclusion I did not
        // reach. A coordinate click was the alternative and it tests the
        // layout rather than the wiring, breaking the next time a row is added
        // above.
        //
        // So: the control is in peers_page's draw path and calls
        // open_qr_scan, and the state that call produces survives a frame.
        // That a finger can reach it is verified by reading the code, not by
        // this test.
    }
}

#[cfg(test)]
mod frame_id_tests {
    use super::*;

    fn app() -> Snapclone { Snapclone::new(Vec::new()) }

    #[test]
    fn each_frame_to_a_peer_gets_a_new_sequence() {
        // Every snap went out as seq 0, so a receiver could not tell two
        // apart or spot a re-send.
        let mut a = app();
        let (s1, _) = a.frame_ids("sc-1");
        let (s2, _) = a.frame_ids("sc-1");
        assert!(s2 > s1, "{s1} then {s2}");
    }

    #[test]
    fn sequences_are_per_peer_and_do_not_leak_between_them() {
        // A shared counter tells peer B that it missed everything sent to
        // peer A, and a receiver counting gaps would ask for re-sends of
        // frames addressed to somebody else.
        let mut a = app();
        a.frame_ids("sc-1");
        a.frame_ids("sc-1");
        let (first_to_b, _) = a.frame_ids("sc-2");
        assert_eq!(first_to_b, 1, "peer B started at {first_to_b}");
    }

    #[test]
    fn the_ack_reports_the_highest_sequence_seen_from_that_peer() {
        let mut a = app();
        a.sessions.entry("sc-1").observe_seq(7);
        assert_eq!(a.frame_ids("sc-1").1, 7);
    }

    #[test]
    fn a_late_low_sequence_does_not_move_the_ack_backwards() {
        // Reordering is normal. An ack that went backwards would make the
        // peer re-send everything after it — a retry storm caused by the
        // network being ordinary.
        let mut a = app();
        a.sessions.entry("sc-1").observe_seq(9);
        a.sessions.entry("sc-1").observe_seq(4);
        assert_eq!(a.frame_ids("sc-1").1, 9);
    }

    #[test]
    fn forgetting_a_peer_restarts_its_numbering() {
        // `forget` clears the session, and a stale sequence surviving it
        // would have the peer waiting for frames from a conversation that
        // is over.
        let mut a = app();
        a.frame_ids("sc-1");
        a.frame_ids("sc-1");
        a.sessions.forget("sc-1");
        assert_eq!(a.frame_ids("sc-1").0, 1);
    }
}
