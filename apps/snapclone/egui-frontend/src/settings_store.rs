//! settings_store — make the Save button do something.
//!
//! `settings_page.rs` modelled five tabs, dirty tracking per tab, validation and
//! a Save that set `saved = current` **in memory**. Restart the app and every
//! choice is gone. A settings page that forgets is arguably worse than none: the
//! user believes they have configured something.
//!
//! `shared/config-kit` is XDG resolution plus a flat `[section] key = value`
//! store, std-only. Nothing in the frontend needs to know where a config lives.
//!
//! ## Why sections match tabs
//!
//! The file's `[general]`, `[camera]`, `[network]`, `[storage]`, `[about]`
//! mirror the page's tabs, so a hand-edited file and the UI agree about where a
//! setting is. That matters more than it sounds: people edit these files, and a
//! key that appears under a different heading than the tab it came from is a key
//! nobody finds.
//!
//! Qt's own keys do NOT match its tabs (`posture/enabled` is on General,
//! `ui/ambient_enabled` is on Storage), because QSettings groups grew by
//! subsystem while the tabs were introduced later. The Qt key is recorded in a
//! comment beside each line here so the two files can be compared, but the
//! grouping follows the tab — the person hand-editing this file is looking at
//! the settings page, not at `SettingsPage.cpp`.
//!
//! ## Reading is where the care goes
//!
//! Writing is a serialisation. Reading has to survive a file that a human edited
//! badly, an older version wrote, or a newer version wrote: **an unknown key is
//! ignored and a missing key keeps its default**, so a config from any version
//! loads. Refusing to start over one bad line is the failure mode that makes
//! people delete their config.
//!
//! Two keys moved when the page grew to Qt parity — `network/stun_host` became
//! `network/stun_extra` beside the seven-server bitmask, and `network/dht_host`
//! became `about/micro_dht_host` where Qt files it. Both old spellings are still
//! READ. Dropping them would be a silent loss rather than an error, which is the
//! one thing this module exists to prevent.

use crate::settings_page::{
    About, Camera, CameraBackend, CryptoBackend, DiscoveryBackend, General, Network, Settings,
    SpeechBoost, Storage,
};

/// The file, under the app's XDG config dir.
pub const APP: &str = "snapclone";
pub const FILE: &str = "settings.conf";

/// Serialise. Sections mirror the page's tabs so a hand-edited file and the UI
/// agree about where a setting lives.
pub fn render(s: &Settings) -> String {
    let mut out = String::new();
    out.push_str("# snapclone settings — sections match the settings page's tabs\n");

    let g = &s.general;
    out.push_str("[general]\n");
    out.push_str(&format!("display_name = {}\n", quote(&g.display_name)));
    out.push_str(&format!("palette = {}\n", quote(&g.palette)));                            // ui/palette
    out.push_str(&format!("unified_nav = {}\n", g.unified_nav));                    // ui/unified_nav
    out.push_str(&format!("scroll_invert_trackpad = {}\n", g.scroll_invert_trackpad)); // ui/scroll_detent_invert
    out.push_str(&format!("scroll_invert_mouse = {}\n", g.scroll_invert_mouse));    // ui/scroll_detent_invert_mouse
    out.push_str(&format!("scroll_invert_touch = {}\n", g.scroll_invert_touch));    // ui/scroll_detent_invert_touch
    out.push_str(&format!("long_calls = {}\n", g.long_calls));                      // calls/enableLongCalls
    out.push_str(&format!("proximity_screensaver = {}\n", g.proximity_screensaver));// posture/enabled
    out.push_str(&format!("read_receipts = {}\n", g.read_receipts));
    out.push_str(&format!("share_location = {}\n", g.share_location));              // privacy/shareLocation
    out.push_str(&format!("share_photo_location = {}\n", g.share_photo_location));  // privacy/sharePhotoLocation
    out.push_str(&format!("record_audio = {}\n", g.record_audio));                  // video/recordAudio
    out.push_str(&format!("manual_lat = {}\n", quote(&g.manual_lat)));                      // location/manualLat
    out.push_str(&format!("manual_lon = {}\n", quote(&g.manual_lon)));                      // location/manualLon
    out.push_str(&format!("ringtone_gain = {}\n", g.ringtone_gain));                // sounds/ringtoneGain
    out.push_str(&format!("ringtone_muted = {}\n", g.ringtone_muted));              // sounds/ringtoneMuted
    out.push_str(&format!("mic_pipeline_off = {}\n", g.mic_pipeline_off));          // sounds/micPipelineOff

    let c = &s.camera;
    out.push_str("\n[camera]\n");
    out.push_str(&format!("device = {}\n", quote(&c.device)));
    out.push_str(&format!("mirror_front = {}\n", c.mirror_front));
    out.push_str(&format!("shutter_sound = {}\n", c.shutter_sound));
    out.push_str(&format!("rear_backend = {}\n", c.rear_backend.key()));            // camera/rearBackend
    out.push_str(&format!("front_backend = {}\n", c.front_backend.key()));          // camera/frontBackend
    out.push_str(&format!("speech_boost = {}\n", c.speech_boost.key()));            // audio/speechBoost
    out.push_str(&format!("speech_boost_auto_only = {}\n", c.speech_boost_auto_only)); // audio/speechBoostAutoOnly
    out.push_str(&format!("face_library = {}\n", c.face_library));                  // face/enabled
    out.push_str(&format!("capture_flash_bright = {}\n", c.capture_flash_bright));
    out.push_str(&format!("face_consent = {}\n", quote(&c.face_consent)));                  // no Qt twin — the Qt app has no consent gate   // no Qt twin — the Qt app flashes white unconditionally

    let n = &s.network;
    out.push_str("\n[network]\n");
    out.push_str(&format!("listen_port = {}\n", n.listen_port));
    out.push_str(&format!("discovery_backend = {}\n", n.discovery_backend.index())); // network/discoveryBackend
    out.push_str(&format!("jami_enabled = {}\n", n.jami_enabled));                  // network/jamiEnabled
    out.push_str(&format!("upnp = {}\n", n.upnp));                                  // network/upnpEnabled
    out.push_str(&format!("stun_servers = {}\n", n.stun_mask));                     // network/stunServers
    out.push_str(&format!("stun_extra = {}\n", quote(&n.stun_extra)));
    out.push_str(&format!("relay_url = {}\n", quote(&n.relay_url)));                        // network/relayUrl
    out.push_str(&format!("crypto_backend = {}\n", n.crypto_backend.index()));      // crypto/backend
    out.push_str(&format!("require_encryption = {}\n", n.require_encryption));
    out.push_str(&format!("ws_port = {}\n", n.ws_port));                            // network/wsPort
    out.push_str(&format!("gossip_port = {}\n", n.gossip_port));                    // network/gossipPort

    let st = &s.storage;
    out.push_str("\n[storage]\n");
    out.push_str(&format!("keep_days = {}\n", st.keep_days));
    out.push_str(&format!("keep_everything = {}\n", st.keep_everything));           // storage/keepEverything
    out.push_str(&format!("export_dir = {}\n", quote(&st.export_dir)));
    out.push_str(&format!("auto_sync_path = {}\n", quote(&st.auto_sync_path)));
    out.push_str(&format!("ambient_enabled = {}\n", st.ambient_enabled));           // ui/ambient_enabled

    let a = &s.about;
    out.push_str("\n[about]\n");
    out.push_str(&format!("debug_trace = {}\n", a.debug_trace));
    out.push_str(&format!("no_fsm_tuning = {}\n", a.no_fsm_tuning));
    out.push_str(&format!("no_monitoring_blocking = {}\n", a.no_monitoring_blocking));
    out.push_str(&format!("verbose_fsm = {}\n", a.verbose_fsm));
    out.push_str(&format!("mobile_display_debug = {}\n", a.mobile_display_debug));
    out.push_str(&format!("micro_dht_enabled = {}\n", a.micro_dht_enabled));        // beta/micro_dht_enabled
    out.push_str(&format!("micro_dht_host = {}\n", quote(&a.micro_dht_host)));              // beta/micro_dht_host
    out
}

/// A string value as the file carries it: in double quotes, with `\\`, `\"` and `\n`
/// escaped.
///
/// Written bare, a value was cut short on load wherever it held a character the flat
/// format gives a meaning: `#` and `;` start a comment (display name "Ada #1" came back
/// as "Ada"; a relay URL with a #fragment lost it), surrounding quotes were stripped, and
/// leading/trailing spaces were trimmed. Inside quotes `config_kit::Ini` keeps `#`, `;`
/// and spaces, and skips backslash-escaped quotes; `unquote_escapes` decodes the escapes.
pub fn quote(v: &str) -> String {
    let mut out = String::with_capacity(v.len() + 2);
    out.push('"');
    for ch in v.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The inverse of [`quote`]'s escapes (the quotes themselves are already gone: `Ini`
/// strips them). An unknown escape is kept as written, so a hand-edited or pre-quoting
/// value with a stray backslash loads as it was.
pub fn unquote_escapes(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('n') => out.push('\n'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Parse, keeping defaults for anything absent or unreadable.
///
/// A key that will not parse keeps its DEFAULT rather than failing the load: a
/// single bad line — a port typed as a word, a truncated write — must not cost
/// the user every other setting in the file.
pub fn parse(text: &str) -> Settings {
    let ini = config_kit::Ini::parse(text);
    // Every value goes through `unquote_escapes`: string fields are written quoted and
    // escaped (see `quote`), and for the numbers and booleans it is the identity.
    let get = |sec: &str, key: &str| ini.get(sec, key).map(unquote_escapes);
    let b = |v: Option<String>, d: bool| v.and_then(|x| x.parse().ok()).unwrap_or(d);

    let mut g = General::default();
    if let Some(v) = get("general", "display_name") { g.display_name = v; }
    if let Some(v) = get("general", "palette") { g.palette = v; }
    g.unified_nav = b(get("general", "unified_nav"), g.unified_nav);
    g.scroll_invert_trackpad = b(get("general", "scroll_invert_trackpad"), g.scroll_invert_trackpad);
    g.scroll_invert_mouse = b(get("general", "scroll_invert_mouse"), g.scroll_invert_mouse);
    g.scroll_invert_touch = b(get("general", "scroll_invert_touch"), g.scroll_invert_touch);
    g.long_calls = b(get("general", "long_calls"), g.long_calls);
    g.proximity_screensaver = b(get("general", "proximity_screensaver"), g.proximity_screensaver);
    g.read_receipts = b(get("general", "read_receipts"), g.read_receipts);
    g.share_location = b(get("general", "share_location"), g.share_location);
    g.share_photo_location = b(get("general", "share_photo_location"), g.share_photo_location);
    g.record_audio = b(get("general", "record_audio"), g.record_audio);
    if let Some(v) = get("general", "manual_lat") { g.manual_lat = v; }
    if let Some(v) = get("general", "manual_lon") { g.manual_lon = v; }
    // Clamped rather than rejected: Qt clamps on load too, and a gain of 900
    // from a hand edit should become the maximum rather than reset to 100 —
    // the intent was "loud".
    if let Some(v) = get("general", "ringtone_gain").and_then(|x| x.parse::<u32>().ok()) {
        g.ringtone_gain = v.min(200);
    }
    g.ringtone_muted = b(get("general", "ringtone_muted"), g.ringtone_muted);
    g.mic_pipeline_off = b(get("general", "mic_pipeline_off"), g.mic_pipeline_off);

    let mut c = Camera::default();
    if let Some(v) = get("camera", "device") { c.device = v; }
    c.mirror_front = b(get("camera", "mirror_front"), c.mirror_front);
    c.shutter_sound = b(get("camera", "shutter_sound"), c.shutter_sound);
    if let Some(v) = get("camera", "rear_backend") { c.rear_backend = CameraBackend::from_key(&v); }
    if let Some(v) = get("camera", "front_backend") { c.front_backend = CameraBackend::from_key(&v); }
    if let Some(v) = get("camera", "speech_boost") { c.speech_boost = SpeechBoost::from_key(&v); }
    c.speech_boost_auto_only =
        b(get("camera", "speech_boost_auto_only"), c.speech_boost_auto_only);
    c.face_library = b(get("camera", "face_library"), c.face_library);
    c.capture_flash_bright = b(get("camera", "capture_flash_bright"), c.capture_flash_bright);
    if let Some(v) = get("camera", "face_consent") {
        // Normalised THROUGH the enum on the way in, so a hand-edited file
        // containing `yes` becomes `notgiven` here rather than sitting in the
        // struct as a word nothing recognises and being written back out.
        c.face_consent = crate::face_consent::Consent::from_key(&v).as_key().to_string();
    }

    let mut n = Network::default();
    if let Some(v) = get("network", "listen_port").and_then(|x| x.parse().ok()) { n.listen_port = v; }
    if let Some(v) = get("network", "discovery_backend").and_then(|x| x.parse::<u8>().ok()) {
        n.discovery_backend = DiscoveryBackend::from_index(v);
    }
    n.jami_enabled = b(get("network", "jami_enabled"), n.jami_enabled);
    n.upnp = b(get("network", "upnp"), n.upnp);
    if let Some(v) = get("network", "stun_servers").and_then(|x| x.parse().ok()) { n.stun_mask = v; }
    // `stun_host` is the pre-parity spelling. Read for compatibility; never
    // written, so the first save migrates it.
    if let Some(v) = get("network", "stun_extra").or_else(|| get("network", "stun_host")) {
        n.stun_extra = v;
    }
    if let Some(v) = get("network", "relay_url") { n.relay_url = v; }
    if let Some(v) = get("network", "crypto_backend").and_then(|x| x.parse::<u8>().ok()) {
        n.crypto_backend = CryptoBackend::from_index(v);
    }
    n.require_encryption = b(get("network", "require_encryption"), n.require_encryption);
    if let Some(v) = get("network", "ws_port").and_then(|x| x.parse().ok()) { n.ws_port = v; }
    if let Some(v) = get("network", "gossip_port").and_then(|x| x.parse().ok()) { n.gossip_port = v; }

    let mut st = Storage::default();
    if let Some(v) = get("storage", "keep_days").and_then(|x| x.parse().ok()) { st.keep_days = v; }
    st.keep_everything = b(get("storage", "keep_everything"), st.keep_everything);
    if let Some(v) = get("storage", "export_dir") { st.export_dir = v; }
    if let Some(v) = get("storage", "auto_sync_path") { st.auto_sync_path = v; }
    st.ambient_enabled = b(get("storage", "ambient_enabled"), st.ambient_enabled);

    let mut a = About::default();
    a.debug_trace = b(get("about", "debug_trace"), a.debug_trace);
    a.no_fsm_tuning = b(get("about", "no_fsm_tuning"), a.no_fsm_tuning);
    a.no_monitoring_blocking = b(get("about", "no_monitoring_blocking"), a.no_monitoring_blocking);
    a.verbose_fsm = b(get("about", "verbose_fsm"), a.verbose_fsm);
    a.mobile_display_debug = b(get("about", "mobile_display_debug"), a.mobile_display_debug);
    a.micro_dht_enabled = b(get("about", "micro_dht_enabled"), a.micro_dht_enabled);
    // `network/dht_host` is where the pre-parity page filed this. Qt puts it
    // under beta testers, which is the About tab here.
    if let Some(v) = get("about", "micro_dht_host").or_else(|| get("network", "dht_host")) {
        if !v.is_empty() { a.micro_dht_host = v; }
    }

    Settings { general: g, camera: c, network: n, storage: st, about: a }
}

/// Load from disk, or defaults when there is no file yet.
///
/// A missing file is the FIRST RUN, not an error — reporting it would put a
/// warning in front of every new user.
pub fn load() -> Settings {
    match config_kit::load(APP, FILE) {
        Some(text) => parse(&text),
        None => Settings::default(),
    }
}

/// Write to disk -- atomically (`config_kit::write_atomic`), because a phone app is
/// killed rather than closed and a torn file loads as all-defaults. The io::Result is
/// returned rather than swallowed, because a Save that silently fails is the same lie as
/// one that never wrote.
pub fn store(s: &Settings) -> std::io::Result<()> {
    config_kit::save(APP, FILE, &render(s))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stun_servers;

    #[test]
    fn sections_match_the_pages_tabs() {
        // People edit these files, and a key filed under a different heading
        // than the tab it came from is a key nobody finds.
        let t = render(&Settings::default());
        for sec in ["[general]", "[camera]", "[network]", "[storage]", "[about]"] {
            assert!(t.contains(sec), "missing {sec}");
        }
        // and the ports are under network, where the Network tab put them
        let net = t.split("[network]").nth(1).unwrap();
        for k in ["listen_port", "ws_port", "gossip_port", "stun_servers"] {
            assert!(net.contains(k), "{k} is not under [network]");
        }
        // the micro-DHT host is under About, where Qt files it (beta testers),
        // NOT under network where this port used to keep it
        let about = t.split("[about]").nth(1).unwrap();
        assert!(about.contains("micro_dht_host"));
    }

    #[test]
    fn one_bad_line_does_not_cost_every_other_setting() {
        // A port typed as a word, or a truncated write. Failing the whole load
        // is what makes people delete their config.
        let text = "[general]\ndisplay_name = Ada\n[network]\nlisten_port = banana\nupnp = false\n";
        let s = parse(text);
        assert_eq!(s.general.display_name, "Ada", "the good lines still applied");
        assert_eq!(s.network.listen_port, Network::default().listen_port,
                   "the bad one kept its default");
        assert!(!s.network.upnp, "and parsing continued past it");
    }

    #[test]
    fn an_unknown_key_is_ignored_so_any_version_loads() {
        // A file written by a newer build must not stop an older one starting.
        let text = "[general]\ndisplay_name = Bo\nquantum_mode = true\n[future]\nx = 1\n";
        assert_eq!(parse(text).general.display_name, "Bo");
    }

    #[test]
    fn a_missing_key_keeps_its_default_rather_than_emptying() {
        // A file written by an OLDER build lacks newer keys; they must come up
        // as the default, not as empty or false.
        let s = parse("[general]\ndisplay_name = Cy\n");
        let d = Settings::default();
        assert_eq!(s.general.palette, d.general.palette);
        assert_eq!(s.general.read_receipts, d.general.read_receipts);
        assert_eq!(s.network.listen_port, d.network.listen_port);
        assert_eq!(s.network.stun_mask, stun_servers::DEFAULT_MASK,
                   "an old config must not silently disable every STUN server");
        assert_eq!(s.about.micro_dht_host, d.about.micro_dht_host);
    }

    #[test]
    fn an_empty_or_absent_file_is_first_run_not_an_error() {
        // Reporting it would put a warning in front of every new user.
        assert_eq!(parse(""), Settings::default());
        assert_eq!(parse("# only a comment\n"), Settings::default());
    }

    #[test]
    fn the_two_renamed_keys_are_still_read_from_their_old_spelling() {
        // Both moved when the page reached Qt parity. Dropping them would be a
        // silent loss rather than an error — the user's STUN host and DHT node
        // would simply be gone, with the page looking fine.
        let old = "[network]\nstun_host = stun.mine.lan:3478\ndht_host = 10.0.0.5:4544\n";
        let s = parse(old);
        assert_eq!(s.network.stun_extra, "stun.mine.lan:3478");
        assert_eq!(s.about.micro_dht_host, "10.0.0.5:4544");
        // and the new spelling wins when both are present, so a half-migrated
        // file does not flip back
        let both = "[network]\nstun_host = old:1\nstun_extra = new:2\n";
        assert_eq!(parse(both).network.stun_extra, "new:2");
    }

    #[test]
    fn a_hand_edited_ringtone_gain_is_clamped_rather_than_reset() {
        // The intent behind "900" was "loud". Resetting to 100 reads as the
        // edit being ignored.
        assert_eq!(parse("[general]\nringtone_gain = 900\n").general.ringtone_gain, 200);
        assert_eq!(parse("[general]\nringtone_gain = 0\n").general.ringtone_gain, 0);
        assert_eq!(parse("[general]\nringtone_gain = loud\n").general.ringtone_gain,
                   General::default().ringtone_gain);
    }
}

#[cfg(test)]
mod roundtrip_tests {

    /// Values holding the characters the flat format gives a meaning survive a round
    /// trip exactly: `#` and `;` (comments), quotes, backslashes, edge spaces, newlines.
    #[test]
    fn string_values_with_special_characters_round_trip() {
        let mut s = crate::settings_page::Settings::default();
        s.general.display_name = "  Ada #1; \"the\" one \\ ok  ".into();
        s.network.relay_url = "wss://relay.example/ws#frag;x=1".into();
        s.storage.export_dir = "/media/sd card/snaps;2026".into();
        s.storage.auto_sync_path = "line1\nline2".into();
        s.about.micro_dht_host = "\"quoted\"".into();
        let back = super::parse(&super::render(&s));
        assert_eq!(back.general.display_name, s.general.display_name);
        assert_eq!(back.network.relay_url, s.network.relay_url);
        assert_eq!(back.storage.export_dir, s.storage.export_dir);
        assert_eq!(back.storage.auto_sync_path, s.storage.auto_sync_path);
        assert_eq!(back.about.micro_dht_host, s.about.micro_dht_host);
        assert_eq!(back, s, "the whole struct round-trips");
        // A file written before quoting (bare values) still loads.
        let old = super::parse("[general]\ndisplay_name = Bob\n[storage]\nexport_dir = /tmp/x\n");
        assert_eq!(old.general.display_name, "Bob");
        assert_eq!(old.storage.export_dir, "/tmp/x");
    }
    use super::*;
    use crate::settings_page::{CameraBackend, CryptoBackend, DiscoveryBackend, Settings, SpeechBoost};

    /// Every field set to something distinguishable from its default.
    ///
    /// The point is that a field ADDED to `Settings` and forgotten in `render`
    /// silently stops persisting — the user changes it, saves, restarts, and it
    /// is back to the default with no error anywhere. This catches that by
    /// comparing the whole struct rather than field by field, so a new field is
    /// covered the moment it exists.
    fn populated() -> Settings {
        let mut s = Settings::default();

        s.general.display_name = "Ada Lovelace".into();
        s.general.palette = "pitch_black".into();
        s.general.unified_nav = true;
        s.general.scroll_invert_trackpad = true;
        s.general.scroll_invert_mouse = true;
        s.general.scroll_invert_touch = true;
        s.general.long_calls = true;
        s.general.proximity_screensaver = false;
        s.general.read_receipts = false;
        s.general.share_location = true;
        s.general.share_photo_location = true;
        s.general.record_audio = false;
        s.general.manual_lat = "45.5019".into();
        s.general.manual_lon = "-73.5674".into();
        s.general.ringtone_gain = 175;
        s.general.ringtone_muted = true;
        s.general.mic_pipeline_off = true;

        s.camera.device = "/dev/video2".into();
        s.camera.mirror_front = false;
        s.camera.shutter_sound = false;
        s.camera.rear_backend = CameraBackend::Gstreamer;
        s.camera.front_backend = CameraBackend::Auto;
        s.camera.speech_boost = SpeechBoost::Strong;
        s.camera.speech_boost_auto_only = false;
        s.camera.face_library = true;

        s.network.listen_port = 44_444;
        s.network.discovery_backend = DiscoveryBackend::Both;
        s.network.jami_enabled = true;
        s.network.upnp = false;
        s.network.stun_mask = 0b110_0001;
        s.network.stun_extra = "stun.example.org:3478".into();
        s.network.relay_url = "wss://relay.example.org:8443".into();
        s.network.crypto_backend = CryptoBackend::Rust;
        s.network.require_encryption = false;
        s.network.ws_port = 19_876;
        s.network.gossip_port = 19_877;

        s.storage.keep_days = 45;
        s.storage.keep_everything = true;
        s.storage.export_dir = "/home/kd/exports".into();
        s.storage.auto_sync_path = "rsync://backup.lan/snaps".into();
        s.storage.ambient_enabled = true;

        s.about.debug_trace = true;
        s.about.no_fsm_tuning = true;
        s.about.no_monitoring_blocking = true;
        s.about.verbose_fsm = true;
        s.about.mobile_display_debug = true;
        s.about.micro_dht_enabled = true;
        s.about.micro_dht_host = "10.0.0.5:4544".into();

        s
    }

    #[test]
    fn the_capture_flash_colour_survives_a_round_trip() {
        // The flash toggle used to be a field on the app struct, set by a
        // checkbox and thrown away on restart — while this module, a settings
        // page and a Save that writes the file all already existed and it went
        // through none of them. The round trip is what proves it does now.
        let mut s = Settings::default();
        assert!(!s.camera.capture_flash_bright, "the default is the dark flash");
        s.camera.capture_flash_bright = true;
        let back = parse(&render(&s));
        assert!(back.camera.capture_flash_bright, "the setting did not survive the file");
        // And the other direction, which a one-way test would miss: an
        // explicit false must come back false rather than falling to a default
        // that happens to agree.
        s.camera.capture_flash_bright = false;
        assert!(!parse(&render(&s)).camera.capture_flash_bright);
    }

    /// The constructor must READ consent back, and nothing else can catch it.
    ///
    /// Found by breaking it: deleting the seeding line from `new_with_login`
    /// failed no test at all, while every other part of this path had two
    /// suites on it. The store round-trips, the enum round-trips, and the
    /// setter writes — and consent still reset on every restart, because the
    /// one line that reads it back was untested.
    ///
    /// A behavioural test would need `Snapclone::new_with_login` to load a
    /// settings file, which reads `$HOME`. So this greps the source, which is
    /// the pattern this tree already uses for wiring that cannot be exercised
    /// — and like every scan here it asserts it FOUND something first, since a
    /// grep that matches nothing passes every assertion below it.
    #[test]
    fn the_constructor_seeds_consent_from_the_loaded_settings() {
        let src = include_str!("lib.rs");
        let ctor = src
            .split_once("pub fn new_with_login")
            .expect("new_with_login is gone — this test is measuring nothing")
            .1;
        // Bounded to the constructor: the setter also mentions both names, and
        // matching it would let the read be deleted while this still passed.
        let body = &ctor[..ctor.find("\n    fn friend_of").unwrap_or(ctor.len())];
        assert!(
            body.contains("face_consent") && body.contains("from_key"),
            "new_with_login does not read consent back from settings — it will \
             reset on every restart, and a forgotten revocation forgets the \
             erase it owed"
        );
    }

    #[test]
    fn consent_round_trips_as_three_states_not_two() {
        // The reason it is a word. A bool would carry Granted and turn Revoked
        // into NotGiven, and those two are not the same: both stop detection,
        // only one owes the user an erase of crops already taken. A settings
        // file that forgets a revocation forgets the debt with it.
        use crate::face_consent::Consent;
        for c in [Consent::NotGiven, Consent::Granted, Consent::Revoked] {
            let mut s = Settings::default();
            s.camera.face_consent = c.as_key().to_string();
            let back = parse(&render(&s));
            assert_eq!(Consent::from_key(&back.camera.face_consent), c, "{c:?} was lost");
        }
    }

    #[test]
    fn an_unreadable_consent_value_loads_as_no_consent() {
        // A hand-edited or newer-version file must not be read as permission,
        // and must not be written back out as a word nothing recognises.
        let s = parse("[camera]\nface_consent = yes\n");
        assert_eq!(s.camera.face_consent, "notgiven", "junk normalised to {:?}", s.camera.face_consent);
        assert!(!crate::face_consent::Consent::from_key(&s.camera.face_consent).may_detect());
    }

    #[test]
    fn a_file_written_before_the_flash_existed_still_loads() {
        // The compatibility rule this module is built on: a missing key keeps
        // its default rather than refusing the file. Every user upgrading has
        // a settings.conf with no `capture_flash_bright` line in it.
        let older = "[camera]\nshutter_sound = false\nface_library = true\n";
        let s = parse(older);
        assert!(!s.camera.shutter_sound, "the old file did not load at all");
        assert!(s.camera.face_library);
        assert!(!s.camera.capture_flash_bright, "a missing key did not take its default");
    }

    #[test]
    fn every_setting_survives_a_save_and_load() {
        let want = populated();
        let got = parse(&render(&want));
        assert_eq!(got, want, "a field is missing from render() or parse()");
    }

    #[test]
    fn populated_actually_differs_from_default_in_every_group() {
        // Guards the guard: if `populated` stopped changing one of the five
        // groups, the round-trip above would still pass while proving nothing
        // about that group. A structural test that has stopped touching its
        // subject passes every assertion below it.
        let (p, d) = (populated(), Settings::default());
        assert_ne!(p.general, d.general);
        assert_ne!(p.camera, d.camera);
        assert_ne!(p.network, d.network);
        assert_ne!(p.storage, d.storage);
        assert_ne!(p.about, d.about);
    }

    #[test]
    fn the_camera_device_specifically_survives() {
        // Called out because it is the one a user notices: a camera that opens
        // the wrong device after every restart looks like the camera is broken
        // rather than like a settings bug.
        let mut s = Settings::default();
        s.camera.device = "/dev/video4".into();
        assert_eq!(parse(&render(&s)).camera.device, "/dev/video4");
    }

    #[test]
    fn the_stun_bitmask_survives_exactly_rather_than_approximately() {
        // A mask that round-trips through a bool list would lose any bit this
        // build does not know about — silently unchecking a server chosen in a
        // newer one.
        let mut s = Settings::default();
        s.network.stun_mask = 0b1_0110_1001;
        assert_eq!(parse(&render(&s)).network.stun_mask, 0b1_0110_1001);
    }

    #[test]
    fn defaults_round_trip_to_defaults() {
        // A freshly-written file must read back as what was written, or the
        // first save after a fresh install changes settings nobody touched.
        let d = Settings::default();
        assert_eq!(parse(&render(&d)), d);
    }

    #[test]
    fn a_value_containing_spaces_is_not_truncated() {
        // Display names have spaces. Splitting a line on whitespace rather than
        // on the first `=` loses everything after the first word.
        let mut s = Settings::default();
        s.general.display_name = "Ada  Lovelace  the First".into();
        assert_eq!(parse(&render(&s)).general.display_name, "Ada  Lovelace  the First");
    }
}
