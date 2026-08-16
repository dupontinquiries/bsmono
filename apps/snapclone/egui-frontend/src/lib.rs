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
pub mod camera_source;
pub mod peer_source;
pub mod contacts;
pub mod conversation;
pub mod composer;
pub mod outbox;
pub mod group_voice;
pub mod settings_page;
pub mod settings_store;
pub mod inbox;
pub mod unified_nav;
pub mod friends;
pub mod modal;
pub mod snaps;
pub mod session;
pub mod ring;
pub mod qr;
pub mod wayfinder;
pub mod voicebar;
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
pub mod ambient_blobs;
pub mod ambient_camera;
pub mod ambient_inbox;
pub mod ambient_palette;
pub mod camera_select;
pub mod chevron_confirm;
pub mod dht_bootstrap;
pub mod face_consent;
pub mod frame_pump;
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
pub mod tab_bar;
pub mod thumbnails;
pub mod trace_channels;
pub mod voice_bridge;
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
    /// What the shutter last did, shown under it. Plain state rather than a
    /// toast system: this app has none, and adding one to report a duration
    /// would be the tail wagging the dog.
    pub shutter_note: String,
    /// This device's sc-ID, for the code somebody else scans.
    ///
    /// Held rather than derived: the identity belongs to the crypto layer and
    /// the view should display what it is TOLD, not compute a second answer
    /// that could disagree with the one the handshake uses.
    pub local_id: String,
    /// Latest-frame-wins slot between the capture thread and the paint.
    pub frames: crate::frame_pump::FrameSlot,
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
    pub group_voice: crate::group_voice::GroupVoice,
    pub picker: crate::contacts::Picker,
    pub conversation: crate::conversation::Conversation,
    /// Who the open conversation is with. The Conversation model holds the
    /// ITEMS; which peer they belong to is the shell's business.
    pub conversation_peer: String,
    /// The unified bottom bar (Qt UnifiedNavBar): page arrows, sub-tab segments, underline
    /// morph, page dots. Owns the page + sub-tab state; `nav` mirrors the page for swipe.
    pub bar: UnifiedNav,
    /// Login overlay visibility, with the Qt ordering rule (MainWindow.cpp): the overlay
    /// STARTS HIDDEN; if auto-login already succeeded at construction we leave it hidden and
    /// go straight to camera; otherwise we show it and hide on `login_succeeded`.
    pub login_visible: bool,
    pub nav: Nav,
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
    /// The swipe affordance. Retires itself after four swipes.
    pub peel: crate::nav_chrome::PeelHint,
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
    /// Accent and background derived from the photos in the inbox, so the app
    /// takes on the colour of what is in it. `None` until enough pixels have
    /// been sampled, and `None` is a real answer — see `ambient()`.
    pub ambient: Option<(crate::ambient_palette::Rgb, crate::ambient_palette::Rgb)>,
    /// The manual. Closed unless asked for, and it dismisses itself after a
    /// period of no activity — where "activity" deliberately means scrolling
    /// OR any other input, so a reader pausing on a short section is not
    /// dismissed mid-read.
    pub help: crate::help_manual::HelpManual,
    /// Running edge-pixel samples. Kept across snaps rather than recomputed:
    /// the palette should settle as the inbox fills, not lurch to whatever
    /// arrived last.
    ambient_samples: crate::ambient_palette::Samples,
    /// (sender id, caption) for the snaps floating on the map, newest first.
    pub wayfinder_snaps: Vec<(String, String)>,
    gesture: GestureLayer,
    palette: String,
}

impl Snapclone {
    pub fn new(peers: Vec<Peer>) -> Snapclone {
        Self::new_with_login(peers, true)
    }

    /// `auto_logged_in` mirrors Qt's construction-time auto-login check.
    pub fn new_with_login(peers: Vec<Peer>, auto_logged_in: bool) -> Snapclone {
        Snapclone {
            viewfinder: Default::default(),
            camera: None,
            cameras: Vec::new(),
            cam_selection: None,
            shutter: Default::default(),
            shutter_note: String::new(),
            local_id: String::new(),
            frames: Default::default(),
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
            group_voice: Default::default(),
            picker: Default::default(),
            cards: Default::default(),
            add_form: Default::default(),
            modal: Default::default(),
            feed: Default::default(),
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
            shutter_pressed: false,
            video: None,
            menu: Default::default(),
            attention: Default::default(),
            menu_target: None,
            snaps_saved: Vec::new(),
            send_summary: Default::default(),
            posture: Default::default(),
            last_voice_at: None,
            glove: Default::default(),
            voice_field: None,
            dictated: Vec::new(),
            camera_policy: Default::default(),
            camera_health: Default::default(),
            playout: Default::default(),
            sink: Default::default(),
            // 20 ms frames, the value snap_frame_ms normalises toward.
            sink_latency: crate::sink_cushion::Latency::from_settings(2, 8, 20),
            sender: Default::default(),
            shake: uikit_egui::glove::ShakeCommands::with(vec![
                uikit_egui::glove::ShakeBinding {
                    dir: uikit_egui::glove::Orient::RightUp,
                    action: "swap_camera".into(), label: "Swap camera".into() },
            ]),
            peel: crate::nav_chrome::PeelHint::new(PAGES.len()),
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
            ambient_samples: Default::default(),
            tabs: crate::tab_bar::TabBar::new(vec![
                crate::tab_bar::Tab::new("camera", "Camera"),
                crate::tab_bar::Tab::new("chats", "Chats"),
                crate::tab_bar::Tab::new("friends", "Friends"),
                crate::tab_bar::Tab::new("map", "Map"),
                crate::tab_bar::Tab::new("settings", "Settings"),
            ]),
            wayfinder_snaps: Vec::new(),
            conversation: Default::default(),
            conversation_peer: String::new(),
            bar: UnifiedNav::new(),
            login_visible: !auto_logged_in,
            nav: Nav::new(PAGES.len()),
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
        }
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
    pub fn on_gossip(&mut self, rec: crate::gossip::PeerRecord, datagram_len: usize) -> bool {
        if crate::gossip::accept(&rec, datagram_len, &self.local_id).is_err() {
            return false;
        }
        // A discovered device becomes a peer row only if the user does not
        // already have one. Discovery ADDS reachability information; it does
        // not get to rename or replace a contact the user chose to keep.
        let id = rec.id.clone();
        self.view.observe(rec);
        if !self.peers.iter().any(|p| p.name == id) {
            self.peers.push(Peer { name: id, online: true });
            self.sync_roster();
        }
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
    pub fn tick_gossip(&mut self, now_ms: u64) {
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
        self.peers.retain(|p| p.name != id);
        self.cards.close(id);
        self.sync_roster();
        self.modal.dismiss(&format!("confirm-delete:{id}"));
    }

    /// Carry out what the Friends page asked for.
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
                    if let Some(p) = self.peers.iter_mut().find(|p| p.name == id) {
                        p.name = to;
                    }
                    self.sync_roster();
                }
            }
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
                    // familiar=false for now: UxGuide's use counter is not
                    // ported, and claiming familiarity we have not measured
                    // would skip a confirmation on the strength of a guess.
                    // Erring toward asking is the safe direction for a delete.
                    if self.ask_delete(false) {
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
                "edit" => {
                    self.editor_id = id.clone();
                    self.editor_name = id.clone();
                    self.modal.present_default(
                        &format!("editor:{id}"), crate::modal::Layer::Dialog,
                        crate::modal::Rect::new(20.0, 80.0, 340.0, 400.0));
                }
                _ => {}
            },
            Action::ImportVCard => {}
        }
    }

    pub fn page_name(&self) -> &'static str {
        PAGES[self.nav.page()]
    }

    /// Swipe between top-level pages (unless drilled into a subpage — then swipe is off so the
    /// content can scroll/interact).
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
        self.modal.present_default("qr", crate::modal::Layer::Dialog,
                                   crate::modal::Rect::new(20.0, 80.0, 340.0, 420.0));
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
            "delete" => { self.feed.remove(&target); }
            "save" => self.snaps_saved.push(target),
            _ => {}
        }
    }

    /// Open a snap from the feed, routing by MEDIA KIND.
    ///
    /// A video goes to the persistent viewer and an image to the view-once one.
    /// The routing lives here, once, rather than at each tile: a tile that
    /// picked its own viewer would eventually pick the wrong one.
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
                if !self.snaps_viewed.contains(&id) { self.snaps_viewed.push(id); }
            }
            V::Expired(id) => {
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
        let surface = ui.available_rect_before_wrap();

        // live background painted behind everything
        self.bg.paint(ui.painter(), surface, &colors);

        // swipe paging (top-level only)
        let frame = self.gesture.update(ui, surface, true);
        if let Some(dir) = frame.page_jump {
            self.swipe_page(dir.sign());
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
                child.text_edit_singleline(&mut self.editor_name);
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
                if child.button("close").clicked() {
                    // close() carries no wipe, by design — the snap outlives
                    // the viewer. Clearing the state here is all that closing
                    // means for a persistent video.
                    self.video = None;
                    self.modal.dismiss_topmost();
                }
                if let Some(crate::video_viewer::VideoEvent::Closed) = ev {
                    self.video = None;
                    self.modal.dismiss_topmost();
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
                let mut answered = None;
                let mut who = String::new();
                if let Some(call) = self.incoming_call.as_mut() {
                    who = call.caller_name.clone();
                    answered = crate::ring::ring_ui(&mut child, call, &colors);
                }
                // finish_call owns dismissing and clearing; calling it is the
                // whole handling, and duplicating either here would leave the
                // two paths able to disagree about whether a call ended.
                if let Some(a) = answered {
                    self.finish_call(&who, a);
                }
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
                let painter = ui.painter();
                painter.rect_filled(area, 8.0, colors.bg_base);
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area.shrink(12.0)));
                let mut scanned = None;
                if let Some(q) = self.qr.as_mut() {
                    scanned = crate::qr::qr_ui(&mut child, q, &colors);
                }
                // BOTH HALVES. qr_ui reads somebody else's code; my_code_ui
                // shows yours so they can read it back. With only the scanner,
                // an exchange needs a third device.
                child.separator();
                crate::qr::my_code_ui(&mut child, &self.local_id, &colors);
                if child.button("close").clicked() {
                    self.qr = None;
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
        if !adaptive.is_compact() && !self.nav.can_go_back() {
            ui.horizontal(|ui| {
                for (i, name) in PAGES.iter().enumerate() {
                    if ui.selectable_label(i == self.nav.page(), *name).clicked() {
                        self.nav.go_page(i);
                    }
                }
            });
            ui.separator();
        }

        match self.nav.page() {
            // The nav has always declared five pages — Camera, Inbox, Friends,
            // Wayfinder, Settings — and the shell routed three of them, so two
            // tabs led nowhere. That is most of why the app read as a demo: the
            // chrome promised more than the body delivered.
            0 => self.camera_page(ui, &colors),
            1 => self.inbox_page(ui, &colors),
            2 => self.peers_page(ui, &colors),
            3 => self.wayfinder_page(ui, &colors),
            _ => self.settings_page(ui, &colors),
        }
    }

    fn camera_page(&mut self, ui: &mut Ui, c: &PaletteColors) {
        self.nav.title_bar(ui, "Camera");
        // The REAL viewfinder, not a labelled rectangle. Tap-to-focus, the
        // fading reticle, full-bleed vs framed and the front-camera mirror all
        // come from camera.rs — which existed and was never called, which is
        // exactly why the app looked like a placeholder.
        // FRAME DELIVERY, through frame_pump. The slot is latest-wins, so a
        // compositor stall costs dropped frames rather than a preview that runs
        // permanently behind — and `should_upload` gates the texture write,
        // which is the expensive step, on there actually being something new.
        let now_ms = (ui.input(|i| i.time) * 1000.0) as u64;
        if crate::frame_pump::should_upload(&self.frames, self.cam_texture.is_some()) {
            if let Some(f) = self.frames.take() {
                self.last_frame_ms = Some(now_ms);
                let _ = f; // the pixel upload belongs to the capture backend
            }
        }
        ui.allocate_ui(egui::vec2(ui.available_width(), 220.0), |ui| {
            crate::camera::viewfinder(ui, &mut self.viewfinder,
                                      self.cam_texture.as_ref(), c);
        });
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
        // Say WHICH camera, or that there is none. The viewfinder mirrors only
        // when device-kit says this device is front-facing.
        match &self.camera {
            Some(cam) => {
                ui.colored_label(c.text_weak, crate::camera_source::describe(cam));
            }
            None => {
                ui.colored_label(c.warning, "no camera found");
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
                for d in &self.cameras {
                    let on = matches!(&self.cam_selection,
                        Some(crate::camera_select::Selection::Single { id, .. }) if *id == d.id);
                    if ui.selectable_label(on, &d.label).clicked() {
                        self.cam_selection = Some(crate::camera_select::select(
                            &self.cameras, &d.id, true));
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
        ui.horizontal(|ui| {
            if crate::glass::glass_button_ui(ui, &mut self.glass_flip, "⟲", 44.0, c) {
                self.viewfinder.flip();
            }
            // THE REAL SHUTTER, not a button that sets a bool. One control,
            // four gestures: tap for a photo, hold to record, swipe up to lock
            // recording hands-free, swipe down for the mode picker. A plain
            // button can express exactly one of those.
            //
            // shutter_ui senses DRAG rather than click, because `clicked()`
            // fires on any release however long the press lasted — which is the
            // trap that makes hold-to-record indistinguishable from tap.
            let resp = crate::shutter::shutter_ui(ui, &self.shutter, 56.0, c);
            let ev = if resp.drag_started() {
                resp.interact_pointer_pos().and_then(|p| self.shutter.press(p.x, p.y))
            } else if resp.dragged() {
                resp.interact_pointer_pos().and_then(|p| self.shutter.moved(p.x, p.y))
            } else if resp.drag_stopped() {
                resp.interact_pointer_pos().and_then(|p| self.shutter.release(p.x, p.y))
            } else {
                // The hold threshold is a TIMER, so it must be ticked on frames
                // where the finger has not moved — otherwise a perfectly still
                // press never becomes a recording.
                self.shutter.tick(ui.input(|i| i.stable_dt) * 1000.0)
            };
            match ev {
                Some(crate::shutter::Shutter::Photo) => self.shutter_pressed = true,
                Some(crate::shutter::Shutter::RecordStopped { duration_ms }) => {
                    // Report the DURATION: a recording that stopped without
                    // saying how long it was leaves the user unsure whether it
                    // captured anything at all.
                    self.shutter_note = format!("recorded {:.1}s",
                                                duration_ms as f32 / 1000.0);
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
        ui.separator();
        // the composer, over the captured frame
        ui.horizontal(|ui| {
            ui.label("caption");
            ui.text_edit_singleline(&mut self.composer.caption);
            if ui.button("colour").clicked() { self.composer.cycle_color(); }
            let d = self.composer.duration;
            // Say what the timer MEANS before the snap is sent, not after. The
            // difference between a replicated durable snap and an ephemeral one
            // is invisible otherwise, and it is the difference that matters.
            let delivery = crate::outbox::Delivery::of(d);
            if ui.button(d.label()).clicked() {
                use crate::composer::Duration;
                self.composer.duration = match d {
                    Duration::Seconds(s) if s < 10 => Duration::Seconds(s + 5),
                    Duration::Seconds(_) => Duration::Unlimited,
                    Duration::Unlimited => Duration::Seconds(5),
                };
            }
        });
        // Recomputed here rather than captured: the button above may have just
        // changed the duration, and showing the PREVIOUS delivery would tell
        // the user the opposite of what pressing send will do.
        let delivery = crate::outbox::Delivery::of(self.composer.duration);
        ui.label(egui::RichText::new(delivery.explain())
            .color(if delivery.replicates() { c.warning } else { c.text_weak }).small());
    }

    fn peers_page(&mut self, ui: &mut Ui, c: &PaletteColors) {
        self.nav.title_bar(ui, "Friends");
        // The contact picker, so Friends is a place you DO something rather
        // than a list you look at.
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut self.picker.query);
            let multi = self.picker.is_multi();
            if ui.selectable_label(multi, "multi").clicked() {
                self.picker.mode = if multi { crate::contacts::PickMode::Single }
                                   else { crate::contacts::PickMode::Multi {
                                       confirm_label: "Send".into() } };
            }
        });
        let ids: Vec<String> = self.peers.iter().map(|p| p.name.clone()).collect();
        for id in ids {
            let sel = self.picker.selected().iter().any(|s| s == &id);
            if ui.selectable_label(sel, &id).clicked() { self.picker.toggle(&id); }
        }
        // SEND, and the row that watches it. The picker could select people and
        // nothing consumed the selection — and send_state, "the row the sender
        // watches", had no caller at all. Between them the app could choose
        // recipients and never tell you whether anything arrived.
        let chosen: Vec<String> = self.picker.selected().to_vec();
        if !chosen.is_empty() {
            let label = format!("send to {}", chosen.len());
            if ui.button(label).clicked() {
                let now = (ui.input(|i| i.time) * 1000.0) as u64;
                for who in &chosen {
                    // One row PER RECIPIENT. A single row for a multi-send
                    // cannot express the normal case — one person offline, the
                    // rest fine — and would have to report the whole thing
                    // failed or the whole thing sent.
                    self.sends.push(&format!("{who}:{now}"), now);
                }
            }
        }
        // Failures surface without being hunted for. needs_attention exists
        // because a send that failed silently is indistinguishable from one
        // that worked, and the sender is the last person to find out.
        let stuck = self.sends.needs_attention().len();
        if stuck > 0 {
            ui.colored_label(c.warning, format!("{stuck} send(s) failed — tap to retry"));
        }
        ui.separator();
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
        ui.horizontal(|ui| {
            let joined = self.group_voice.joined;
            if ui.selectable_label(joined, if joined { "leave channel" } else { "join channel" })
                .clicked() { self.toggle_voice(); }
            if joined && ui.selectable_label(self.group_voice.muted, "mute").clicked() {
                self.group_voice.muted = !self.group_voice.muted;
            }
        });
        // The real FriendListWidget: cards with an action menu, the empty state
        // that names the ports to open, and the add-peer form. The flat list of
        // names this replaced had no way to add anyone, which is the half of
        // the page that matters when discovery has failed and you are standing
        // next to the person you are trying to reach.
        if let Some(a) = crate::friends::friends_ui(
            ui, &self.roster, &mut self.cards, &mut self.add_form, c) {
            self.apply_friends(a);
        }
    }

    fn settings_page(&mut self, ui: &mut Ui, c: &PaletteColors) {
        self.nav.title_bar(ui, "Settings");
        // The five real tab bodies from settings_page.rs, not one row of
        // background styles. The background picker stays as an appearance
        // control, but it is no longer the whole page.
        crate::settings_page::settings_ui(ui, &mut self.settings, c);
        // Persist. The io error is SHOWN rather than swallowed — a Save that
        // silently fails is the same lie as one that never wrote, and a
        // read-only config dir is a real thing on locked-down devices.
        if ui.add_enabled(self.settings.is_dirty(), egui::Button::new("Save to disk")).clicked() {
            self.settings.save();
            match crate::settings_store::store(&self.settings.current) {
                Ok(()) => self.settings_error = None,
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
        let nodes = crate::wayfinder::build_nodes(self.self_fix, &self.wayfinder_contacts);
        let photos = crate::wayfinder::place_photos(&nodes, &self.wayfinder_snaps);
        if let Some(id) = crate::wayfinder::wayfinder_ui(ui, &nodes, &photos, c) {
            // Tapping a contact jumps to Friends with them selected, per the
            // Qt page: the map is a way IN to a conversation, not a dead end.
            self.apply_friends(crate::friends::Action::Selected(id));
            self.nav.go_page(2);
        }
        if nodes.is_empty() {
            ui.colored_label(c.text_weak, "No contacts to place yet.");
        }
    }

    /// The inbox, on its own page. Was absent entirely: the app had a Peers
    /// list and no way to see a message, which is the gap that made a
    /// messaging app read as a demo.
    fn inbox_page(&mut self, ui: &mut Ui, c: &PaletteColors) {
        self.nav.title_bar(ui, "Messages");
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
        self.refresh_ribbon();
        self.ribbon.set_current(self.bar.sub[1]);
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
                ui.text_edit_singleline(&mut self.conversation.draft);
                if ui.add_enabled(self.conversation.can_send(),
                                  egui::Button::new("send")).clicked() {
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
    #[test]
    fn typing_through_the_shell_keeps_every_character() {
        use egui::{Context, Event, RawInput};
        let ctx = Context::default();
        let mut app = app();
        app.nav.go_page(2);
        let adaptive = Adaptive::default();

        let mut frame = |ctx: &Context, app: &mut Snapclone, events: Vec<Event>| {
            let _ = ctx.run(RawInput { events, ..Default::default() }, |ctx| {
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
        let app = super::Snapclone::new(Vec::new());
        let tab = &app.tabs.tabs()[super::Snapclone::CHATS_TAB];
        assert_eq!(tab.label, "Chats",
                   "CHATS_TAB points at {:?}, not the chats tab", tab.label);
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
        }, 100);
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
        assert!(!s.on_gossip(r, 100));
        assert!(s.peers.is_empty());
    }

    #[test]
    fn our_own_echo_does_not_add_us_to_our_own_roster() {
        let mut s = Snapclone::new(vec![]);
        s.local_id = "sc-me".into();
        assert!(!s.on_gossip(rec("sc-me", 0), 100));
        assert!(s.peers.is_empty());
    }

    #[test]
    fn a_discovered_device_becomes_a_peer() {
        let mut s = Snapclone::new(vec![]);
        assert!(s.on_gossip(rec("sc-a", 0), 100));
        assert_eq!(s.peers.len(), 1);
        assert!(s.peers[0].online);
    }

    #[test]
    fn discovery_does_not_duplicate_a_contact_the_user_already_has() {
        // Discovery ADDS reachability; it does not get to add a second row for
        // somebody already in the list.
        let mut s = Snapclone::new(vec![Peer { name: "sc-a".into(), online: false }]);
        s.on_gossip(rec("sc-a", 0), 100);
        assert_eq!(s.peers.len(), 1);
    }

    #[test]
    fn a_silent_peer_goes_offline_rather_than_disappearing() {
        // The two lists answer different questions: a device that stopped
        // gossiping may be asleep, and deleting the row would lose a contact
        // the user added deliberately.
        let mut s = Snapclone::new(vec![]);
        s.on_gossip(rec("sc-a", 0), 100);
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
        s.on_gossip(rec("sc-a", precise), 100);
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
            s.on_gossip(rec("sc-a", t), 100);
            let got = s.view_for_storage()[0].last_seen_ms;
            assert!(got <= t, "{t} bucketed forward to {got}");
        }
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
        s.on_gossip(rec("sc-a", now), 100);
        assert!(s.view.peers()[0].is_alive(now), "a peer just heard from is dead");
    }

    #[test]
    fn pruning_runs_on_its_schedule_not_every_frame() {
        // Per-frame pruning walks the peer list sixty times a second to do
        // nothing.
        let mut s = Snapclone::new(vec![]);
        s.on_gossip(rec("sc-a", 0), 100);
        s.tick_gossip(crate::gossip::PRUNE_MS - 1);
        assert!(s.peers[0].online, "pruned before the interval elapsed");
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

