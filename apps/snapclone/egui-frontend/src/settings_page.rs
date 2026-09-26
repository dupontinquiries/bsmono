//! settings_page — the five tab bodies, ported from `SettingsPage.cpp`.
//!
//! The shell already ports the sub-tab TABLE verbatim (General/Camera/Network/
//! Storage/About) and pins it with a test. What was behind the tabs was
//! nothing: the port had the names and no page bodies, which the parity audit
//! would happily have counted as "settings present".
//!
//! Tab contents follow the Qt comments exactly, because a settings page people
//! know is one where things are where they were. `buildUi` routes fifteen
//! section builders into the five tabs and this mirrors that grouping:
//!
//! * **0 General** — identity, appearance, privacy, sounds
//! * **1 Camera** — camera, accessibility (audio), face library
//! * **2 Network** — discovery, STUN, crypto, ports
//! * **3 Storage** — storage, export, security, ambient
//! * **4 About** — about, diagnostics, debug flags, beta testers
//!
//! ## One struct per tab, and that is load-bearing
//!
//! `Settings` used to be sixteen flat fields and `dirty_tabs` a hand-written
//! list of which field belongs to which tab. That is the shape of bug this
//! tree keeps finding: **add a field, forget the comparison, and the tab
//! stops reporting its own unsaved changes** — silently, with every test still
//! green, because nothing looks at the set of fields as a whole. Nesting the
//! fields under the tab that owns them turns that from a thing to remember
//! into a thing the compiler arranges: `dirty_tabs` is five struct
//! comparisons, and a new field is inside one of them by construction.
//!
//! The same argument does NOT extend to persistence — `settings_store` still
//! has to name each field to write it — which is why the round-trip test
//! there compares whole structs rather than field by field.
//!
//! ## Dirty tracking, which is the part with teeth
//!
//! The Qt page marks itself dirty on any edit (`markD` wired to every widget)
//! and that drives "unsaved changes". Getting it wrong in either direction is
//! bad: miss an edit and the user loses work silently; report dirty when
//! nothing changed and the prompt becomes noise people click through — which
//! then hides the real one. So dirtiness is computed by COMPARING to the last
//! saved state rather than set by a flag on every keystroke.
//!
//! ## What Qt applies immediately and this does not
//!
//! Qt is inconsistent here and it is worth knowing rather than reproducing by
//! accident: the Network tab batches through the sticky Save bar
//! (`loadSettings`/`saveSettings`), while most General/Camera/Storage toggles
//! write to `QSettings` inside their own `toggled` lambda the moment they are
//! clicked. This page batches EVERYTHING behind Save. That is a deliberate
//! divergence: half a page that saves as you type and half that waits for a
//! button is the state in which "unsaved changes" cannot be given an honest
//! answer, and the warning is the feature.

use crate::relay_preset;
use crate::stun_servers;

/// Which tab, matching the Qt indices so `setCurrentSubTab(index)` from the nav
/// bar routes correctly. The numbering is the contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    General,
    Camera,
    Network,
    Storage,
    About,
}

impl Tab {
    pub const ALL: [Tab; 5] = [Tab::General, Tab::Camera, Tab::Network, Tab::Storage, Tab::About];

    pub fn label(self) -> &'static str {
        match self {
            Tab::General => "General", Tab::Camera => "Camera", Tab::Network => "Network",
            Tab::Storage => "Storage", Tab::About => "About",
        }
    }

    /// `setCurrentSubTab(index)`. An out-of-range index falls back to General
    /// rather than showing a blank page, because a blank settings page looks
    /// like the app has lost its configuration.
    pub fn from_index(i: usize) -> Tab { *Tab::ALL.get(i).unwrap_or(&Tab::General) }
    pub fn index(self) -> usize { Tab::ALL.iter().position(|t| *t == self).unwrap_or(0) }
}

/// A fixed set of choices persisted as a string, with Qt's own rule for a
/// value it does not recognise: fall back to the first entry rather than show
/// an empty combo. Qt spells this `idx >= 0 ? idx : 0` at four call sites; this
/// makes it one implementation with one test.
macro_rules! string_choice {
    ($(#[$m:meta])* $name:ident { $( $variant:ident => $key:literal , $label:literal ),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name { $( $variant ),+ }
        impl $name {
            pub const ALL: &'static [$name] = &[ $( $name::$variant ),+ ];
            /// The persisted string.
            pub fn key(self) -> &'static str { match self { $( $name::$variant => $key ),+ } }
            /// What the combo shows.
            pub fn label(self) -> &'static str { match self { $( $name::$variant => $label ),+ } }
            /// Parse, falling back to the first entry for anything unknown —
            /// including a value written by a newer build.
            pub fn from_key(s: &str) -> Self {
                let s = s.trim().to_ascii_lowercase();
                Self::ALL.iter().copied().find(|v| v.key() == s).unwrap_or(Self::ALL[0])
            }
        }
        impl Default for $name { fn default() -> Self { Self::ALL[0] } }
    };
}

/// A fixed set of choices persisted as an INDEX. Qt uses `currentIndex()` for
/// discovery and crypto, so the number on disk is positional and reordering
/// these silently repoints existing configs — same hazard as the STUN bitmask,
/// same answer: a test pins the order.
macro_rules! index_choice {
    ($(#[$m:meta])* $name:ident { $( $variant:ident => $label:literal ),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name { $( $variant ),+ }
        impl $name {
            pub const ALL: &'static [$name] = &[ $( $name::$variant ),+ ];
            pub fn label(self) -> &'static str { match self { $( $name::$variant => $label ),+ } }
            pub fn index(self) -> u8 {
                Self::ALL.iter().position(|v| *v == self).unwrap_or(0) as u8
            }
            /// Out of range falls back to the first, matching Qt.
            pub fn from_index(i: u8) -> Self {
                Self::ALL.get(i as usize).copied().unwrap_or(Self::ALL[0])
            }
        }
        impl Default for $name { fn default() -> Self { Self::ALL[0] } }
    };
}

string_choice! {
    /// `camera/rearBackend`, `camera/frontBackend`. Qt defaults to libcamera.
    CameraBackend {
        Libcamera => "libcamera", "libcamera (direct)",
        Gstreamer => "gstreamer", "GStreamer (camera-bridge)",
        Qt        => "qt",        "Qt",
        Auto      => "auto",      "Automatic",
    }
}

string_choice! {
    /// `audio/speechBoost`. Off by default — a boost applied to everyone by
    /// default is a change to how every call sounds.
    SpeechBoost {
        Off    => "off",    "Off",
        Mild   => "mild",   "Mild",
        Strong => "strong", "Strong",
    }
}

string_choice! {
    /// `camera/rotation` on the policy. Empty means "as the sensor reports",
    /// which is why Off is spelled as the empty key rather than "none".
    Rotation {
        None      => "",    "None",
        Ccw90     => "90l", "90° left (CCW)",
        Cw90      => "90r", "90° right (CW)",
        Half      => "180", "180°",
    }
}

index_choice! {
    /// `network/discoveryBackend`.
    DiscoveryBackend {
        Gossip => "Gossip (LAN)",
        Jami   => "Jami DHT",
        Both   => "Both",
    }
}

index_choice! {
    /// `crypto/backend`. A QT-ERA CHOICE THAT NO LONGER EXISTS.
    ///
    /// Index 0 was the placeholder Qt shipped for development. This app
    /// has no such path: `host.rs` and `snapclone_egui.rs` call
    /// `gossip_wire::encode_signed` unconditionally, which is
    /// `crypto-kit` — ed25519 signatures, AES-256-GCM, X25519. There is
    /// no code here that could do HMAC-SHA256 if it were selected.
    ///
    /// The variant is KEPT rather than deleted, because
    /// `settings_store` persists this key by INDEX and a Qt-written
    /// prefs file containing `crypto_backend = 0` must still load. It
    /// now says what selecting it actually does, which is nothing.
    ///
    /// This is the opposite failure from the usual dead setting: not a
    /// control that does nothing while looking live, but one that
    /// offered to make the app LESS SAFE and could not. A person reading
    /// "production" and selecting it would conclude they had changed
    /// something, and a person leaving it on "dev only" would think they
    /// were unprotected. Both were wrong.
    CryptoBackend {
        Placeholder => "legacy Qt setting — ignored, this build always uses Rust crypto",
        Rust        => "Rust ed25519 + AES-256-GCM + X25519 (always on)",
    }
}

/// Tab 0 — identity, appearance, privacy, sounds.
#[derive(Debug, Clone, PartialEq)]
pub struct General {
    // Identity
    pub display_name: String,
    // Appearance  (`ui/palette`, `ui/unified_nav`, `ui/scroll_detent_invert*`)
    pub palette: String,
    pub unified_nav: bool,
    pub scroll_invert_trackpad: bool,
    pub scroll_invert_mouse: bool,
    pub scroll_invert_touch: bool,
    // Calls  (`calls/enableLongCalls`, `posture/enabled`)
    pub long_calls: bool,
    pub proximity_screensaver: bool,
    // Privacy  (`privacy/*`, `video/recordAudio`, `location/manual*`)
    pub read_receipts: bool,
    pub share_location: bool,
    pub share_photo_location: bool,
    pub record_audio: bool,
    pub manual_lat: String,
    pub manual_lon: String,
    // Sounds  (`sounds/*`)
    /// Percent, 0–200. Qt stores a double and clamps to that range on load.
    pub ringtone_gain: u32,
    pub ringtone_muted: bool,
    /// App-wide microphone kill. Distinct from muting a call: this stops the
    /// pipeline existing, so nothing can un-mute it by accident.
    pub mic_pipeline_off: bool,
}

impl Default for General {
    fn default() -> Self {
        General {
            display_name: String::new(),
            // `PaletteId::from_str` maps anything unknown to Dark, so the old
            // "midnight" default was Dark under a name no palette answers to.
            palette: snapclone_theme::PaletteId::default().as_str().to_string(),
            unified_nav: false,
            scroll_invert_trackpad: false,
            scroll_invert_mouse: false,
            scroll_invert_touch: false,
            long_calls: false,
            proximity_screensaver: true,
            read_receipts: true,
            share_location: false,
            share_photo_location: false,
            record_audio: true,
            manual_lat: String::new(),
            manual_lon: String::new(),
            ringtone_gain: 100,
            ringtone_muted: false,
            mic_pipeline_off: false,
        }
    }
}

/// Tab 1 — camera, accessibility, face library.
///
/// The capture TUNING (timeouts, resolutions, rotation, GPU convert,
/// simultaneous capture) lives in [`crate::camera_policy::CameraPolicy`] and is
/// edited through this page rather than copied into it — Qt does the same,
/// reading and writing `CameraPolicyStore::instance()` from the spin boxes. Two
/// structs holding a startup timeout would disagree the first time one of them
/// was saved and the other was not.
#[derive(Debug, Clone, PartialEq)]
pub struct Camera {
    pub device: String,
    pub mirror_front: bool,
    pub shutter_sound: bool,
    pub rear_backend: CameraBackend,
    pub front_backend: CameraBackend,
    // Accessibility  (`audio/speechBoost*`)
    pub speech_boost: SpeechBoost,
    /// Apply the boost only when the app decides speech is present, rather than
    /// to everything.
    pub speech_boost_auto_only: bool,
    // Face library  (`face/enabled`)
    pub face_library: bool,
    /// Paint the capture flash white instead of dark.
    ///
    /// Dark by default — see `camera::flash_color`. Here rather than as a
    /// field on the app because a preference the user sets and a restart
    /// discards is worse than no preference: they believe they configured it.
    /// This app already had a settings page, a store and a Save that writes
    /// the file; the flash toggle simply was not going through any of it.
    pub capture_flash_bright: bool,
    /// Face-detection consent, as a word — `notgiven`, `granted`, `revoked`.
    ///
    /// A String rather than the `Consent` enum so this module stays free of
    /// the face subsystem: `Settings` is a plain data bag that the store
    /// serialises, and pulling a domain type in here would make the settings
    /// file's grammar depend on that type's shape. `Consent::as_key` and
    /// `from_key` are the only translation, in one place.
    pub face_consent: String,
}

impl Default for Camera {
    fn default() -> Self {
        Camera {
            device: String::new(),
            mirror_front: true,
            shutter_sound: true,
            rear_backend: CameraBackend::Libcamera,
            front_backend: CameraBackend::Libcamera,
            speech_boost: SpeechBoost::Off,
            speech_boost_auto_only: true,
            face_library: false,
            capture_flash_bright: false,
            face_consent: "notgiven".to_string(),
        }
    }
}

/// Tab 2 — discovery, STUN, crypto, ports.
#[derive(Debug, Clone, PartialEq)]
pub struct Network {
    pub listen_port: u16,
    pub discovery_backend: DiscoveryBackend,
    pub jami_enabled: bool,
    pub upnp: bool,
    /// The seven-server bitmask. See [`crate::stun_servers`] — a single host
    /// string is not a smaller version of this list, it is a different
    /// reachability story when one server is down.
    pub stun_mask: u32,
    /// One extra STUN server, tried after the checked ones.
    ///
    /// NOT from Qt. Kept because the validation behind it (a host without a
    /// port is rejected here rather than at connect time) is worth having and
    /// because the seven-server list has no way to name a server on your own
    /// network, which is exactly what someone testing NAT traversal needs.
    pub stun_extra: String,
    /// The single string `network/relayUrl` holds. Derived from the preset
    /// dropdown and the custom field by [`crate::relay_preset::effective_url`];
    /// the dropdown index is never persisted.
    pub relay_url: String,
    pub crypto_backend: CryptoBackend,
    pub require_encryption: bool,
    pub ws_port: u16,
    pub gossip_port: u16,
}

impl Default for Network {
    fn default() -> Self {
        Network {
            listen_port: 41234,
            discovery_backend: DiscoveryBackend::Gossip,
            jami_enabled: false,
            upnp: true,
            stun_mask: stun_servers::DEFAULT_MASK,
            stun_extra: String::new(),
            relay_url: String::new(),
            crypto_backend: CryptoBackend::Placeholder,
            require_encryption: true,
            ws_port: 9876,
            gossip_port: 9877,
        }
    }
}

/// Tab 3 — storage, export, security, ambient.
#[derive(Debug, Clone, PartialEq)]
pub struct Storage {
    pub keep_days: u32,
    /// `storage/keepEverything`. Overrides `keep_days` rather than being
    /// checked alongside it, which is why the retention control greys out.
    pub keep_everything: bool,
    pub export_dir: String,
    /// Empty means the monthly prompt; a path or rsync URL means sync there
    /// without asking. Qt's placeholder says exactly that and it is the only
    /// thing that tells a user what an empty field DOES.
    pub auto_sync_path: String,
    pub ambient_enabled: bool,
}

impl Default for Storage {
    fn default() -> Self {
        Storage {
            keep_days: 30,
            keep_everything: false,
            export_dir: String::new(),
            auto_sync_path: String::new(),
            ambient_enabled: false,
        }
    }
}

/// Tab 4 — about, diagnostics, debug flags, beta testers.
#[derive(Debug, Clone, PartialEq)]
pub struct About {
    pub debug_trace: bool,
    // Debug & FSM tuning. Qt reads these back from the live DebugFlags
    // singleton rather than from disk, because env/CLI can turn one on above
    // what is stored; the equivalent here is that a host which raises a flag
    // must write it into this struct, not only into its own state.
    pub no_fsm_tuning: bool,
    pub no_monitoring_blocking: bool,
    pub verbose_fsm: bool,
    pub mobile_display_debug: bool,
    // Beta testers
    pub micro_dht_enabled: bool,
    pub micro_dht_host: String,
}

impl Default for About {
    fn default() -> Self {
        About {
            debug_trace: false,
            no_fsm_tuning: false,
            no_monitoring_blocking: false,
            verbose_fsm: false,
            mobile_display_debug: false,
            micro_dht_enabled: false,
            micro_dht_host: "127.0.0.1:4544".into(),
        }
    }
}

/// Everything the page edits, grouped by the tab that owns it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Settings {
    pub general: General,
    pub camera: Camera,
    pub network: Network,
    pub storage: Storage,
    pub about: About,
}

/// The page: current values, the last saved values, and which tab is showing.
#[derive(Debug, Clone, Default)]
pub struct SettingsPage {
    pub tab: Tab,
    pub current: Settings,
    saved: Settings,
}

impl SettingsPage {
    pub fn new(initial: Settings) -> SettingsPage {
        SettingsPage { tab: Tab::General, current: initial.clone(), saved: initial }
    }

    /// Are there unsaved changes?
    ///
    /// Computed rather than flagged: a flag set on every keystroke reports
    /// dirty when a user types a character and deletes it, and a prompt that
    /// cries wolf is one people click through — which then hides the real one.
    pub fn is_dirty(&self) -> bool { self.current != self.saved }

    /// Which tabs hold an unsaved change, so the UI can mark them.
    ///
    /// Five comparisons, one per tab, and no list of field names to keep in
    /// sync — see the module header. A field added to any of the five structs
    /// is covered the moment it exists.
    pub fn dirty_tabs(&self) -> Vec<Tab> {
        let (c, s) = (&self.current, &self.saved);
        let mut out = Vec::new();
        if c.general != s.general { out.push(Tab::General); }
        if c.camera != s.camera { out.push(Tab::Camera); }
        if c.network != s.network { out.push(Tab::Network); }
        if c.storage != s.storage { out.push(Tab::Storage); }
        if c.about != s.about { out.push(Tab::About); }
        out
    }

    /// Commit. Returns false when nothing changed, so a caller does not write a
    /// file and emit a "saved" toast for a no-op.
    pub fn save(&mut self) -> bool {
        if !self.is_dirty() { return false; }
        self.saved = self.current.clone();
        true
    }

    pub fn revert(&mut self) { self.current = self.saved.clone(); }

    /// The last saved values -- what is on disk, and what `settings_store::store` writes.
    pub fn saved(&self) -> &Settings { &self.saved }

    /// Change ONE setting and treat it as saved at once, leaving every other pending edit
    /// pending. For the controls that persist on change (the capture flash, the face
    /// consent) rather than through the page's Save.
    ///
    /// These used to set `current` and call `save()`, which committed EVERY half-edited
    /// value on the page along with them, unvalidated, and cleared the page's dirty marks
    /// so Revert no longer knew what to revert.
    pub fn persist_field(&mut self, f: impl Fn(&mut Settings)) {
        f(&mut self.saved);
        f(&mut self.current);
    }

    /// Validate before saving. Returns the problems, empty when fine.
    ///
    /// Reported as a list rather than refusing at the first fault, because
    /// fixing settings one error at a time is miserable.
    pub fn problems(&self) -> Vec<String> {
        let mut v = Vec::new();
        let c = &self.current;
        // Ports below 1024 need privileges most desktop apps do not have, so
        // accepting one produces a listener that fails at runtime with a
        // confusing error rather than here with a clear one. All three ports
        // are checked, and they are named, because "port is privileged" on a
        // page with three ports is not actionable.
        for (name, p) in [("listen", c.network.listen_port),
                          ("WebSocket", c.network.ws_port),
                          ("gossip", c.network.gossip_port)] {
            if p < 1024 {
                v.push(format!("{name} port {p} is privileged; use 1024 or above"));
            }
        }
        // Two listeners on one port is a bind failure at startup, reported
        // wherever the second one happens to be constructed.
        let ports = [("listen", c.network.listen_port), ("WebSocket", c.network.ws_port),
                     ("gossip", c.network.gossip_port)];
        for i in 0..ports.len() {
            for j in (i + 1)..ports.len() {
                if ports[i].1 == ports[j].1 {
                    v.push(format!("{} and {} ports are both {}",
                                   ports[i].0, ports[j].0, ports[i].1));
                }
            }
        }
        // keep_everything overrides the day count, so 0 days is only a problem
        // when the count is what decides.
        if c.storage.keep_days == 0 && !c.storage.keep_everything {
            v.push("keeping snaps for 0 days deletes them on arrival".into());
        }
        if !c.network.stun_extra.is_empty() && !c.network.stun_extra.contains(':') {
            v.push("STUN host needs a port, like stun.example.org:3478".into());
        }
        // A warning rather than a refusal: no STUN at all is a legitimate
        // LAN-only configuration, it just cannot reach across a NAT, and Qt
        // says so in the section's own note.
        if stun_servers::none_enabled(c.network.stun_mask) && c.network.stun_extra.is_empty() {
            v.push("no STUN server is checked; cross-network calls will not connect".into());
        }
        if c.about.micro_dht_enabled && !c.about.micro_dht_host.contains(':') {
            v.push("micro-DHT node needs a host:port, like 127.0.0.1:4544".into());
        }
        if c.general.ringtone_gain > 200 {
            v.push("ringtone gain is capped at 200%".into());
        }
        if c.general.display_name.chars().count() > 64 {
            v.push("display name is too long".into());
        }
        v
    }
}

/// Draw the page body for the current tab.
/// The settings page. `a11y` is the scene every toggle registers with.
///
/// The parity audit reported `a11y=0` for this app — the registry has
/// existed since bot-arena adopted it and nothing here fed it. A
/// settings page is the worst place for that: it is where someone
/// configuring assistive input has to be able to work, and its 25
/// toggles are the easiest case the registry handles.
///
/// Ids are derived from the FIELD NAME, not the position, because
/// `A11yScene` keeps focus across frames by id — a position-derived id
/// would move focus to a different control whenever a section above it
/// grew, and this page has tabs that change length.
/// What the user asked the page to do this frame. The page does not save itself: saving
/// means writing the file and applying the result to the running app, which is the
/// caller's job -- see `Snapclone::save_settings`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageAction {
    Save,
    Revert,
}

pub fn settings_ui(ui: &mut egui::Ui, page: &mut SettingsPage,
                   policy: &mut crate::camera_policy::CameraPolicy,
                   colors: &uikit_egui::PaletteColors,
                   a11y: &mut uikit_egui::a11y::A11yScene) -> Option<PageAction> {
    let dirty = page.dirty_tabs();
    ui.horizontal(|ui| {
        for t in Tab::ALL {
            let mark = if dirty.contains(&t) { " •" } else { "" };
            if ui.selectable_label(page.tab == t, format!("{}{mark}", t.label())).clicked() {
                page.tab = t;
            }
        }
    });
    ui.separator();
    let tab = page.tab;
    let c = &mut page.current;
    match tab {
        Tab::General => general_ui(ui, &mut c.general, colors, a11y),
        Tab::Camera => camera_ui(ui, &mut c.camera, policy, colors, a11y),
        Tab::Network => network_ui(ui, &mut c.network, colors, a11y),
        Tab::Storage => storage_ui(ui, &mut c.storage, colors, a11y),
        Tab::About => about_ui(ui, &mut c.about, colors, a11y),
    }
    let problems = page.problems();
    if !problems.is_empty() {
        ui.separator();
        for p in &problems {
            ui.label(egui::RichText::new(format!("⚠ {p}")).color(colors.warning).small());
        }
    }
    ui.separator();
    // ONE Save. There used to be two: this one only committed the in-memory snapshot,
    // which cleared the dirty marks and greyed out the "Save to disk" button below it --
    // so the natural tap lost every change on restart. Now the page reports the request
    // and the caller writes the file.
    let mut action = None;
    ui.horizontal(|ui| {
        // Saving with problems is refused, but the button explains itself
        // rather than being mysteriously grey.
        let can = page.is_dirty() && problems.is_empty();
        if ui.add_enabled(can, egui::Button::new("Save")).clicked() { action = Some(PageAction::Save); }
        if ui.add_enabled(page.is_dirty(), egui::Button::new("Revert")).clicked() { action = Some(PageAction::Revert); }
        if page.is_dirty() {
            ui.label(egui::RichText::new("unsaved changes").color(colors.warning).small());
        }
    });
    action
}

/// A section heading, so the four sections in a tab are visibly four sections
/// rather than one run-on column of controls — which is what a long settings
/// tab turns into without them.
fn heading(ui: &mut egui::Ui, text: &str, colors: &uikit_egui::PaletteColors) {
    ui.add_space(6.0);
    ui.label(egui::RichText::new(text.to_uppercase()).small().strong().color(colors.text_weak));
}

fn general_ui(ui: &mut egui::Ui, g: &mut General, colors: &uikit_egui::PaletteColors, a11y: &mut uikit_egui::a11y::A11yScene) {
    heading(ui, "Identity", colors);
    ui.add(egui::TextEdit::singleline(&mut g.display_name).id_salt("settings_page-g_display_name"));

    heading(ui, "Appearance", colors);
    // The persisted value is the palette's `as_str` slug and the label is its
    // display_name. Showing the slug would put "pitch_black" in front of the
    // user; storing the display name would break `PaletteId::from_str`, which
    // silently returns Dark for anything it does not recognise — so a theme
    // stored under its pretty name comes back as Dark on the next launch and
    // nothing reports an error.
    {
        let current = snapclone_theme::PaletteId::from_str(&g.palette);
        egui::ComboBox::from_label("colour theme")
            .selected_text(current.display_name())
            .show_ui(ui, |ui| {
                for p in snapclone_theme::PaletteId::all() {
                    let mut sel = current;
                    if ui.selectable_value(&mut sel, *p, p.display_name()).clicked() {
                        g.palette = p.as_str().to_string();
                    }
                }
            });
    }
    uikit_egui::a11y::semantic_toggle(
        ui, a11y, egui::Id::new("snapclone.settings.unified_nav").value(),
        "unified bottom navigation", &mut g.unified_nav);
    uikit_egui::a11y::semantic_toggle(
        ui, a11y, egui::Id::new("snapclone.settings.scroll_invert_trackpad").value(),
        "reverse scroll-to-navigate (trackpad)", &mut g.scroll_invert_trackpad);
    uikit_egui::a11y::semantic_toggle(
        ui, a11y, egui::Id::new("snapclone.settings.scroll_invert_mouse").value(),
        "reverse scroll-to-navigate (mouse wheel)", &mut g.scroll_invert_mouse);
    uikit_egui::a11y::semantic_toggle(
        ui, a11y, egui::Id::new("snapclone.settings.scroll_invert_touch").value(),
        "reverse scroll-to-navigate (touch drag)", &mut g.scroll_invert_touch);

    heading(ui, "Calls", colors);
    uikit_egui::a11y::semantic_toggle(
        ui, a11y, egui::Id::new("snapclone.settings.long_calls").value(),
        "enable long calls", &mut g.long_calls);
    ui.label(egui::RichText::new(if g.long_calls {
        "calls stay open until you end them"
    } else {
        "calls auto-close after long silences"
    }).small().color(colors.text_weak));
    uikit_egui::a11y::semantic_toggle(
        ui, a11y, egui::Id::new("snapclone.settings.proximity_screensaver").value(),
        "proximity screensaver", &mut g.proximity_screensaver);

    heading(ui, "Privacy", colors);
    uikit_egui::a11y::semantic_toggle(
        ui, a11y, egui::Id::new("snapclone.settings.read_receipts").value(),
        "send read receipts", &mut g.read_receipts);
    ui.checkbox(&mut g.share_location,
                "share location with contacts when I tap the location button");
    uikit_egui::a11y::semantic_toggle(
        ui, a11y, egui::Id::new("snapclone.settings.share_photo_location").value(),
        "attach my location to every photo snap I send", &mut g.share_photo_location);
    uikit_egui::a11y::semantic_toggle(
        ui, a11y, egui::Id::new("snapclone.settings.record_audio").value(),
        "record audio with video snaps", &mut g.record_audio);
    ui.horizontal(|ui| {
        ui.label("manual position");
        ui.add(egui::TextEdit::singleline(&mut g.manual_lat).id_salt("settings_page-g_manual_lat").desired_width(80.0).hint_text("lat"));
        ui.add(egui::TextEdit::singleline(&mut g.manual_lon).id_salt("settings_page-g_manual_lon").desired_width(80.0).hint_text("lon"));
    });

    heading(ui, "Sounds", colors);
    ui.add(egui::Slider::new(&mut g.ringtone_gain, 0..=200).suffix("%").text("ringtone gain"));
    uikit_egui::a11y::semantic_toggle(
        ui, a11y, egui::Id::new("snapclone.settings.ringtone_muted").value(),
        "mute ringtone", &mut g.ringtone_muted);
    uikit_egui::a11y::semantic_toggle(
        ui, a11y, egui::Id::new("snapclone.settings.mic_pipeline_off").value(),
        "turn off microphone (app-wide)", &mut g.mic_pipeline_off);
}

fn camera_ui(ui: &mut egui::Ui, c: &mut Camera,
             policy: &mut crate::camera_policy::CameraPolicy,
             colors: &uikit_egui::PaletteColors, a11y: &mut uikit_egui::a11y::A11yScene) {
    heading(ui, "Camera", colors);
    ui.add(egui::TextEdit::singleline(&mut c.device).id_salt("settings_page-c_device"));
    uikit_egui::a11y::semantic_toggle(
        ui, a11y, egui::Id::new("snapclone.settings.mirror_front").value(),
        "mirror the front camera", &mut c.mirror_front);
    uikit_egui::a11y::semantic_toggle(
        ui, a11y, egui::Id::new("snapclone.settings.shutter_sound").value(),
        "shutter sound", &mut c.shutter_sound);
    combo(ui, "rear camera backend", &mut c.rear_backend, CameraBackend::ALL);
    combo(ui, "front camera backend", &mut c.front_backend, CameraBackend::ALL);

    heading(ui, "Tuning", colors);
    // Straight onto the policy — see the Camera struct's doc for why these are
    // not copied in here first.
    ui.add(egui::DragValue::new(&mut policy.startup_timeout_ms)
        .range(50..=60_000).speed(50).prefix("startup timeout "). suffix(" ms"));
    ui.add(egui::DragValue::new(&mut policy.first_frame_timeout_ms)
        .range(50..=60_000).speed(50).prefix("first frame ").suffix(" ms"));
    ui.add(egui::DragValue::new(&mut policy.early_exit_window_ms)
        .range(0..=60_000).speed(50).prefix("early-exit window ").suffix(" ms"));
    {
        let mut rot = Rotation::from_key(&policy.rotation);
        if combo(ui, "rotation", &mut rot, Rotation::ALL) {
            policy.rotation = rot.key().to_string();
        }
    }
    ui.checkbox(&mut policy.auto_retry_alternate,
                "auto-retry alternate camera on early failure");
    ui.checkbox(&mut policy.prefer_gpu_convert,
                "use GPU-accelerated video conversion (experimental)");
    ui.label(egui::RichText::new(
        "restart the camera (switch away and back) after changing this")
        .small().color(colors.text_weak));
    ui.checkbox(&mut policy.simultaneous_capture,
                "allow simultaneous cameras (picture-in-picture)");

    heading(ui, "Accessibility", colors);
    combo(ui, "speech boost", &mut c.speech_boost, SpeechBoost::ALL);
    ui.add_enabled(c.speech_boost != SpeechBoost::Off,
                   egui::Checkbox::new(&mut c.speech_boost_auto_only,
                                       "only when speech is detected"));

    heading(ui, "Face library", colors);
    uikit_egui::a11y::semantic_toggle(
        ui, a11y, egui::Id::new("snapclone.settings.face_library").value(),
        "recognise faces on this device", &mut c.face_library);
}

fn network_ui(ui: &mut egui::Ui, n: &mut Network, colors: &uikit_egui::PaletteColors, a11y: &mut uikit_egui::a11y::A11yScene) {
    heading(ui, "Peer discovery", colors);
    combo_indexed(ui, "discovery", &mut n.discovery_backend, DiscoveryBackend::ALL);
    uikit_egui::a11y::semantic_toggle(
        ui, a11y, egui::Id::new("snapclone.settings.jami_enabled").value(),
        "enable Jami backend", &mut n.jami_enabled);
    uikit_egui::a11y::semantic_toggle(
        ui, a11y, egui::Id::new("snapclone.settings.upnp").value(),
        "enable UPnP / NAT-PMP", &mut n.upnp);

    heading(ui, "STUN", colors);
    ui.label(egui::RichText::new(
        "every checked server is tried in order — first responder wins. \
         Changes take effect on restart.").small().color(colors.text_weak));
    for s in &stun_servers::ALL {
        let mut on = stun_servers::enabled(n.stun_mask, s);
        ui.horizontal(|ui| {
            if ui.checkbox(&mut on, s.host).changed() {
                n.stun_mask = stun_servers::set(n.stun_mask, s, on);
            }
            ui.label(egui::RichText::new(s.detail).small().color(colors.text_weak));
        });
    }
    ui.horizontal(|ui| {
        ui.label("additional");
        ui.add(egui::TextEdit::singleline(&mut n.stun_extra).id_salt("settings_page-n_stun_extra")
            .hint_text("stun.example.org:3478"));
    });

    heading(ui, "Relay", colors);
    {
        let (mut idx, mut custom) = relay_preset::resolve(&n.relay_url);
        let before = (idx, custom.clone());
        egui::ComboBox::from_label("relay")
            .selected_text(relay_preset::ALL[idx.min(relay_preset::ALL.len() - 1)].label)
            .show_ui(ui, |ui| {
                for (i, r) in relay_preset::ALL.iter().enumerate() {
                    ui.selectable_value(&mut idx, i, r.label);
                }
            });
        if idx == relay_preset::custom_index() {
            ui.add(egui::TextEdit::singleline(&mut custom).id_salt("settings_page-custom").hint_text("wss://relay.example.org"));
        }
        let note = relay_preset::ALL[idx.min(relay_preset::ALL.len() - 1)].note;
        if !note.is_empty() {
            ui.label(egui::RichText::new(note).small().color(colors.text_weak));
        }
        if (idx, custom.clone()) != before {
            n.relay_url = relay_preset::effective_url(idx, &custom);
        }
    }

    heading(ui, "Crypto", colors);
    combo_indexed(ui, "crypto", &mut n.crypto_backend, CryptoBackend::ALL);
    uikit_egui::a11y::semantic_toggle(
        ui, a11y, egui::Id::new("snapclone.settings.require_encryption").value(),
        "require encryption", &mut n.require_encryption);

    heading(ui, "Ports", colors);
    ui.add(egui::DragValue::new(&mut n.listen_port).range(1..=65535).prefix("listen "));
    ui.add(egui::DragValue::new(&mut n.ws_port).range(1..=65535).prefix("websocket "));
    ui.add(egui::DragValue::new(&mut n.gossip_port).range(1..=65535).prefix("gossip "));
}

fn storage_ui(ui: &mut egui::Ui, s: &mut Storage, colors: &uikit_egui::PaletteColors, a11y: &mut uikit_egui::a11y::A11yScene) {
    heading(ui, "Retention", colors);
    uikit_egui::a11y::semantic_toggle(
        ui, a11y, egui::Id::new("snapclone.settings.keep_everything").value(),
        "never auto-delete snaps (power user)", &mut s.keep_everything);
    // Greyed rather than hidden: a control that disappears looks like a bug,
    // and the point is to show that the day count is being overridden.
    ui.add_enabled(!s.keep_everything,
        egui::DragValue::new(&mut s.keep_days).range(0..=3650).suffix(" days"));

    heading(ui, "Export", colors);
    ui.add(egui::TextEdit::singleline(&mut s.export_dir).id_salt("settings_page-s_export_dir"));
    ui.horizontal(|ui| {
        ui.label("auto-sync");
        ui.add(egui::TextEdit::singleline(&mut s.auto_sync_path).id_salt("settings_page-s_auto_sync_path")
            .hint_text("empty = monthly prompt. Or path / rsync URL"));
    });

    heading(ui, "Ambient", colors);
    uikit_egui::a11y::semantic_toggle(
        ui, a11y, egui::Id::new("snapclone.settings.ambient_enabled").value(),
        "ambient background effects", &mut s.ambient_enabled);
}

fn about_ui(ui: &mut egui::Ui, a: &mut About, colors: &uikit_egui::PaletteColors, a11y: &mut uikit_egui::a11y::A11yScene) {
    ui.label(format!("snapclone {}", env!("CARGO_PKG_VERSION")));

    heading(ui, "Diagnostics", colors);
    uikit_egui::a11y::semantic_toggle(
        ui, a11y, egui::Id::new("snapclone.settings.debug_trace").value(),
        "verbose tracing", &mut a.debug_trace);

    heading(ui, "Debug & FSM tuning", colors);
    uikit_egui::a11y::semantic_toggle(
        ui, a11y, egui::Id::new("snapclone.settings.no_fsm_tuning").value(),
        "disable FSM / per-peer routing tuning", &mut a.no_fsm_tuning);
    uikit_egui::a11y::semantic_toggle(
        ui, a11y, egui::Id::new("snapclone.settings.no_monitoring_blocking").value(),
        "FSM validators log but never reject", &mut a.no_monitoring_blocking);
    uikit_egui::a11y::semantic_toggle(
        ui, a11y, egui::Id::new("snapclone.settings.verbose_fsm").value(),
        "verbose FSM transition logging", &mut a.verbose_fsm);
    uikit_egui::a11y::semantic_toggle(
        ui, a11y, egui::Id::new("snapclone.settings.mobile_display_debug").value(),
        "mobile display debug (vkb, modals, dialogs)", &mut a.mobile_display_debug);

    heading(ui, "Beta testers", colors);
    uikit_egui::a11y::semantic_toggle(
        ui, a11y, egui::Id::new("snapclone.settings.micro_dht_enabled").value(),
        "use local micro-DHT instead of the real DHT", &mut a.micro_dht_enabled);
    ui.add_enabled(a.micro_dht_enabled,
        egui::TextEdit::singleline(&mut a.micro_dht_host).id_salt("settings_page-a_micro_dht_host").hint_text("127.0.0.1:4544"));
    ui.label(egui::RichText::new(if a.micro_dht_enabled {
        "enabled — the real DHT is not contacted"
    } else {
        "disabled (using the real DHT)"
    }).small().color(colors.text_weak));
}

/// A combo over a `string_choice!` set. Returns whether the value changed, so a
/// caller writing through to somewhere else (the camera policy) does not have
/// to diff it itself.
fn combo<T>(ui: &mut egui::Ui, label: &str, value: &mut T, all: &'static [T]) -> bool
where T: Copy + PartialEq + Labelled {
    let before = *value;
    // `from_id_salt` + a manual label, NOT `from_label`.
    //
    // `ComboBox::from_label` derives its widget id FROM THE LABEL, so two
    // combos with the same label share one id. This page has exactly
    // that: discovery backend and crypto backend are both labelled
    // "backend". They collided, and the symptom is that neither could be
    // changed — the popup state belongs to whichever drew last, so a
    // click on the first opens and immediately closes.
    //
    // Salting by the VALUE TYPE's name alone fixed THAT collision but
    // opened an identical one the other way: front camera backend and
    // rear camera backend are both `CameraBackend`, so
    // `type_name::<CameraBackend>()` was the same id for both — same
    // symptom (tab/click affects whichever combo drew last, so the
    // setting reads as permanently stuck on one value), just triggered
    // by two DIFFERENT labels sharing a type instead of two different
    // types sharing a label. Salting by (type name, label) together
    // means either axis alone can repeat without colliding — a second
    // enum reusing a label, or a second control over the same enum with
    // its own label, both stay distinct.
    ui.horizontal(|ui| {
        ui.label(label);
        egui::ComboBox::from_id_salt(combo_id_salt::<T>(label))
            .selected_text(value.text())
            .show_ui(ui, |ui| {
                for v in all { ui.selectable_value(value, *v, v.text()); }
            });
    });
    before != *value
}

/// The id salt `combo` actually uses — pulled out so a test can call the
/// exact same function production code does, rather than reimplementing
/// the formula separately and drifting from it silently.
fn combo_id_salt<T>(label: &str) -> (&'static str, String) {
    (std::any::type_name::<T>(), label.to_owned())
}

fn combo_indexed<T>(ui: &mut egui::Ui, label: &str, value: &mut T, all: &'static [T]) -> bool
where T: Copy + PartialEq + Labelled {
    combo(ui, label, value, all)
}

/// One trait so the combo helper works for both macro families without either
/// macro knowing about the other.
pub trait Labelled { fn text(&self) -> &'static str; }
impl Labelled for CameraBackend { fn text(&self) -> &'static str { self.label() } }
impl Labelled for SpeechBoost { fn text(&self) -> &'static str { self.label() } }
impl Labelled for Rotation { fn text(&self) -> &'static str { self.label() } }
impl Labelled for DiscoveryBackend { fn text(&self) -> &'static str { self.label() } }
impl Labelled for CryptoBackend { fn text(&self) -> &'static str { self.label() } }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_branches_on_the_crypto_backend_setting() {
        // The setting is a Qt-era artifact: this build signs with
        // crypto-kit unconditionally and has no HMAC path to fall back
        // to. If a `match` on CryptoBackend ever appears outside this
        // file, either a weaker path was added or the labels went back
        // to lying — both worth stopping at review time.
        //
        // Checked by grep rather than by types because the thing being
        // asserted is an ABSENCE, and absence has no signature.
        for f in ["lib.rs", "host.rs", "gossip_wire.rs"] {
            let src = std::fs::read_to_string(format!("src/{f}")).unwrap_or_default();
            assert!(!src.contains("CryptoBackend"),
                    "{f} branches on CryptoBackend — see the enum's doc");
        }
    }

    #[test]
    fn no_two_combos_on_a_page_share_a_label() {
        // `ComboBox::from_label` derives its id from the label, and this
        // page had TWO controls labelled "backend" — discovery and
        // crypto. They shared an id, so neither could be changed: the
        // popup belongs to whichever drew last.
        //
        // `combo` now salts by (value TYPE, label) together, which makes
        // a label collision on its own harmless. This test guards the
        // label anyway, because a duplicate label is still a UI bug —
        // two rows saying "backend" do not tell a person which backend.
        //
        // Scans both `combo(` and `combo_indexed(` call sites — the
        // front/rear camera backend collision (see
        // `same_typed_combos_with_different_labels_get_different_ids`
        // below) used the plain `combo(` form, which an earlier version
        // of this test did not scan, so it passed while that bug shipped.
        let src = include_str!("settings_page.rs");
        let mut labels: Vec<&str> = src
            .lines()
            .filter_map(|l| {
                let l = l.trim();
                l.strip_prefix("combo_indexed(ui, \"").or_else(|| l.strip_prefix("combo(ui, \""))
            })
            .filter_map(|r| r.split('"').next())
            .collect();
        let before = labels.len();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), before, "two combos share a label: {labels:?}");
    }

    #[test]
    fn same_typed_combos_with_different_labels_get_different_ids() {
        // The bug this guards: `combo`'s id_salt used to be JUST
        // `type_name::<T>()`, so front camera backend and rear camera
        // backend — both `CameraBackend` — collided with each other
        // (same failure shape as the discovery/crypto label collision
        // above, just triggered by sharing a TYPE instead of a LABEL).
        // The symptom reported from a real device: the two dropdowns
        // could not be opened independently and Tab moved both at once,
        // because there was only ever one popup's worth of state for
        // both of them to share.
        //
        // Calls `combo_id_salt` directly — the exact function `combo`
        // itself uses — so a regression to the salt formula (e.g. back
        // to bare `type_name::<T>()`) fails this test instead of it
        // silently passing against a reimplemented copy of the formula.
        let rear = egui::Id::new(combo_id_salt::<CameraBackend>("rear camera backend"));
        let front = egui::Id::new(combo_id_salt::<CameraBackend>("front camera backend"));
        assert_ne!(rear, front, "front/rear camera backend combos would share an id");
    }

    #[test]
    fn the_tab_indices_match_the_qt_contract() {
        // setCurrentSubTab(index) comes from the nav bar, so the numbering IS
        // the contract between the shell and this page.
        assert_eq!(Tab::from_index(0), Tab::General);
        assert_eq!(Tab::from_index(1), Tab::Camera);
        assert_eq!(Tab::from_index(2), Tab::Network);
        assert_eq!(Tab::from_index(3), Tab::Storage);
        assert_eq!(Tab::from_index(4), Tab::About);
        for (i, t) in Tab::ALL.iter().enumerate() { assert_eq!(t.index(), i); }
        // out of range falls back rather than showing a blank page, which looks
        // like the app lost its configuration
        assert_eq!(Tab::from_index(99), Tab::General);
    }

    #[test]
    fn dirtiness_is_computed_not_flagged() {
        // A flag set on every keystroke reports dirty when a user types a
        // character and deletes it, and a prompt that cries wolf gets clicked
        // through — which hides the real one.
        let mut p = SettingsPage::new(Settings::default());
        assert!(!p.is_dirty());
        p.current.general.display_name = "ada".into();
        assert!(p.is_dirty());
        p.current.general.display_name = String::new();
        assert!(!p.is_dirty(), "typing and deleting is not a change");
    }

    #[test]
    fn the_dirty_tab_is_named_so_the_user_is_not_hunting() {
        let mut p = SettingsPage::new(Settings::default());
        assert!(p.dirty_tabs().is_empty());
        p.current.network.listen_port = 5000;
        assert_eq!(p.dirty_tabs(), vec![Tab::Network]);
        p.current.camera.mirror_front = false;
        let d = p.dirty_tabs();
        assert!(d.contains(&Tab::Camera) && d.contains(&Tab::Network));
        assert!(!d.contains(&Tab::General));
    }

    #[test]
    fn every_tabs_own_fields_reach_dirty_tabs() {
        // The regression the nesting exists to prevent: the old flat Settings
        // had a hand-written list of which field belonged to which tab, and a
        // field added without editing that list stopped reporting its tab —
        // silently, with every other test still green. Touching one field per
        // tab is now enough to prove the whole tab is covered, because the
        // comparison is the whole struct.
        for (tab, touch) in [
            (Tab::General, (|s: &mut Settings| s.general.mic_pipeline_off = true)
                as fn(&mut Settings)),
            (Tab::Camera, |s: &mut Settings| s.camera.speech_boost = SpeechBoost::Strong),
            (Tab::Network, |s: &mut Settings| s.network.gossip_port = 1),
            (Tab::Storage, |s: &mut Settings| s.storage.auto_sync_path = "/tmp".into()),
            (Tab::About, |s: &mut Settings| s.about.micro_dht_enabled = true),
        ] {
            let mut p = SettingsPage::new(Settings::default());
            touch(&mut p.current);
            assert_eq!(p.dirty_tabs(), vec![tab], "{tab:?} did not report its own change");
        }
    }

    #[test]
    fn saving_a_no_op_does_nothing_and_says_so() {
        // Otherwise the app writes a file and shows "saved" for a click that
        // changed nothing, training the user to distrust the message.
        let mut p = SettingsPage::new(Settings::default());
        assert!(!p.save(), "nothing to save");
        p.current.network.upnp = false;
        assert!(p.save());
        assert!(!p.is_dirty(), "and it is clean afterwards");
        assert!(!p.save(), "saving twice changes nothing");
    }

    #[test]
    fn revert_restores_the_last_saved_state_not_the_defaults() {
        let mut s = Settings::default();
        s.network.listen_port = 5000;
        let mut p = SettingsPage::new(s);
        p.current.network.listen_port = 6000;
        p.save();
        p.current.network.listen_port = 7000;
        p.revert();
        assert_eq!(p.current.network.listen_port, 6000, "back to SAVED, not to the default");
        assert!(!p.is_dirty());
    }

    #[test]
    fn validation_lists_every_problem_rather_than_the_first() {
        // Fixing settings one error at a time is miserable.
        let mut p = SettingsPage::new(Settings::default());
        p.current.network.listen_port = 80;
        p.current.storage.keep_days = 0;
        p.current.network.stun_extra = "stun.example.org".into();
        let v = p.problems();
        assert_eq!(v.len(), 3, "all three at once: {v:?}");
        assert!(v.iter().any(|s| s.contains("privileged")));
        assert!(v.iter().any(|s| s.contains("0 days")));
        assert!(v.iter().any(|s| s.contains("needs a port")));
        // and a valid config has none
        p.current = Settings::default();
        assert!(p.problems().is_empty(), "{:?}", p.problems());
        // a STUN host WITH a port is fine
        p.current.network.stun_extra = "stun.example.org:3478".into();
        assert!(p.problems().is_empty());
    }

    #[test]
    fn a_privileged_port_is_named_because_there_are_three_of_them() {
        // "port is privileged" on a page with three ports is not actionable.
        let mut p = SettingsPage::new(Settings::default());
        p.current.network.ws_port = 80;
        let v = p.problems();
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].contains("WebSocket"), "{v:?}");
    }

    #[test]
    fn two_services_on_one_port_is_caught_here_rather_than_at_bind_time() {
        let mut p = SettingsPage::new(Settings::default());
        p.current.network.gossip_port = p.current.network.ws_port;
        let v = p.problems();
        assert!(v.iter().any(|s| s.contains("both")), "{v:?}");
        // and the default configuration does not trip it
        p.current = Settings::default();
        assert!(p.problems().is_empty());
    }

    #[test]
    fn keep_everything_silences_the_zero_days_complaint() {
        // Because it overrides the count. Complaining about a number that no
        // longer decides anything is how a warning becomes noise.
        let mut p = SettingsPage::new(Settings::default());
        p.current.storage.keep_days = 0;
        assert!(!p.problems().is_empty());
        p.current.storage.keep_everything = true;
        assert!(p.problems().is_empty(), "{:?}", p.problems());
    }

    #[test]
    fn unchecking_every_stun_server_warns_that_nat_traversal_is_gone() {
        let mut p = SettingsPage::new(Settings::default());
        p.current.network.stun_mask = 0;
        assert!(p.problems().iter().any(|s| s.contains("no STUN")), "{:?}", p.problems());
        // ...unless the extra host covers it, which is the whole reason that
        // field survived the port.
        p.current.network.stun_extra = "stun.mine.lan:3478".into();
        assert!(p.problems().is_empty(), "{:?}", p.problems());
    }

    #[test]
    fn the_micro_dht_host_is_only_checked_when_it_is_going_to_be_used() {
        let mut p = SettingsPage::new(Settings::default());
        p.current.about.micro_dht_host = "localhost".into();
        assert!(p.problems().is_empty(), "off, so nothing dials it");
        p.current.about.micro_dht_enabled = true;
        assert!(p.problems().iter().any(|s| s.contains("micro-DHT")), "{:?}", p.problems());
    }

    #[test]
    fn an_unknown_persisted_choice_falls_back_to_the_first_entry() {
        // Qt's `idx >= 0 ? idx : 0`, in one place. A value written by a newer
        // build must not leave the combo blank.
        assert_eq!(CameraBackend::from_key("vulkan"), CameraBackend::Libcamera);
        assert_eq!(SpeechBoost::from_key(""), SpeechBoost::Off);
        assert_eq!(DiscoveryBackend::from_index(9), DiscoveryBackend::Gossip);
        assert_eq!(CryptoBackend::from_index(9), CryptoBackend::Placeholder);
        // and a known one is honoured, including odd casing from a hand edit
        assert_eq!(CameraBackend::from_key("  GStreamer "), CameraBackend::Gstreamer);
        assert_eq!(SpeechBoost::from_key("STRONG"), SpeechBoost::Strong);
    }

    #[test]
    fn the_index_persisted_choices_keep_their_qt_numbering() {
        // The number on disk is positional, so reordering these silently
        // repoints every existing config — same hazard as the STUN bitmask.
        assert_eq!(DiscoveryBackend::Gossip.index(), 0);
        assert_eq!(DiscoveryBackend::Jami.index(), 1);
        assert_eq!(DiscoveryBackend::Both.index(), 2);
        assert_eq!(CryptoBackend::Placeholder.index(), 0);
        assert_eq!(CryptoBackend::Rust.index(), 1);
        for v in DiscoveryBackend::ALL {
            assert_eq!(DiscoveryBackend::from_index(v.index()), *v);
        }
    }

    #[test]
    fn rotation_none_is_the_empty_string_the_policy_expects() {
        // camera_policy treats an empty rotation as "as the sensor reports";
        // a literal "none" there would be an override to an unknown method.
        assert_eq!(Rotation::None.key(), "");
        assert_eq!(Rotation::from_key(""), Rotation::None);
        assert_eq!(Rotation::from_key("90l"), Rotation::Ccw90);
        assert_eq!(Rotation::from_key("upside-down"), Rotation::None);
    }

    #[test]
    fn the_defaults_match_qt_where_the_choice_is_about_privacy_or_cost() {
        // Not a blanket "defaults are these values" test, which would just
        // restate Default. These four are the ones where the default IS the
        // policy: who learns your IP, whether your location leaves the device,
        // whether the camera talks to a GPU path known to fail on some phones.
        let d = Settings::default();
        assert!(!d.general.share_location, "location sharing is opt-in");
        assert!(!d.general.share_photo_location, "photo geotagging is opt-in");
        assert!(d.general.record_audio, "video snaps record audio unless turned off");
        assert_eq!(d.network.stun_mask, stun_servers::DEFAULT_MASK,
                   "FOSS STUN servers only on a fresh install");
    }
}
