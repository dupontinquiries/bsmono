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
//! The file's `[general]`, `[camera]`, `[network]`, `[storage]` mirror the page's
//! tabs, so a hand-edited file and the UI agree about where a setting is. That
//! matters more than it sounds: people edit these files, and a key that appears
//! under a different heading than the tab it came from is a key nobody finds.
//!
//! ## Reading is where the care goes
//!
//! Writing is a serialisation. Reading has to survive a file that a human edited
//! badly, an older version wrote, or a newer version wrote: **an unknown key is
//! ignored and a missing key keeps its default**, so a config from any version
//! loads. Refusing to start over one bad line is the failure mode that makes
//! people delete their config.

use crate::settings_page::Settings;

/// The file, under the app's XDG config dir.
pub const APP: &str = "snapclone";
pub const FILE: &str = "settings.conf";

/// Serialise. Sections mirror the page's tabs so a hand-edited file and the UI
/// agree about where a setting lives.
pub fn render(s: &Settings) -> String {
    let mut out = String::new();
    out.push_str("# snapclone settings — sections match the settings page's tabs\n");
    out.push_str("[general]\n");
    out.push_str(&format!("display_name = {}\n", s.display_name));
    out.push_str(&format!("palette = {}\n", s.palette));
    out.push_str(&format!("read_receipts = {}\n", s.read_receipts));
    out.push_str("\n[camera]\n");
    out.push_str(&format!("device = {}\n", s.camera_device));
    out.push_str(&format!("mirror_front = {}\n", s.mirror_front));
    out.push_str(&format!("shutter_sound = {}\n", s.shutter_sound));
    out.push_str(&format!("face_library = {}\n", s.face_library));
    out.push_str("\n[network]\n");
    out.push_str(&format!("listen_port = {}\n", s.listen_port));
    out.push_str(&format!("upnp = {}\n", s.upnp));
    out.push_str(&format!("stun_host = {}\n", s.stun_host));
    out.push_str(&format!("dht_host = {}\n", s.dht_host));
    out.push_str(&format!("require_encryption = {}\n", s.require_encryption));
    out.push_str("\n[storage]\n");
    out.push_str(&format!("keep_days = {}\n", s.keep_days));
    out.push_str(&format!("export_dir = {}\n", s.export_dir));
    out.push_str(&format!("ambient_enabled = {}\n", s.ambient_enabled));
    out.push_str("\n[about]\n");
    out.push_str(&format!("debug_trace = {}\n", s.debug_trace));
    out
}

/// Parse, keeping defaults for anything absent or unreadable.
///
/// A key that will not parse keeps its DEFAULT rather than failing the load: a
/// single bad line — a port typed as a word, a truncated write — must not cost
/// the user every other setting in the file.
pub fn parse(text: &str) -> Settings {
    let ini = config_kit::Ini::parse(text);
    let mut s = Settings::default();
    let get = |sec: &str, key: &str| ini.get(sec, key).map(|v| v.to_string());
    let b = |v: Option<String>, d: bool| v.and_then(|x| x.parse().ok()).unwrap_or(d);

    if let Some(v) = get("general", "display_name") { s.display_name = v; }
    if let Some(v) = get("general", "palette") { s.palette = v; }
    s.read_receipts = b(get("general", "read_receipts"), s.read_receipts);

    if let Some(v) = get("camera", "device") { s.camera_device = v; }
    s.mirror_front = b(get("camera", "mirror_front"), s.mirror_front);
    s.shutter_sound = b(get("camera", "shutter_sound"), s.shutter_sound);
    s.face_library = b(get("camera", "face_library"), s.face_library);

    if let Some(v) = get("network", "listen_port").and_then(|x| x.parse().ok()) { s.listen_port = v; }
    s.upnp = b(get("network", "upnp"), s.upnp);
    if let Some(v) = get("network", "stun_host") { s.stun_host = v; }
    if let Some(v) = get("network", "dht_host") { s.dht_host = v; }
    s.require_encryption = b(get("network", "require_encryption"), s.require_encryption);

    if let Some(v) = get("storage", "keep_days").and_then(|x| x.parse().ok()) { s.keep_days = v; }
    if let Some(v) = get("storage", "export_dir") { s.export_dir = v; }
    s.ambient_enabled = b(get("storage", "ambient_enabled"), s.ambient_enabled);

    s.debug_trace = b(get("about", "debug_trace"), s.debug_trace);
    s
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

/// Write to disk. The io::Result is returned rather than swallowed, because a
/// Save that silently fails is the same lie as one that never wrote.
pub fn store(s: &Settings) -> std::io::Result<()> {
    config_kit::save(APP, FILE, &render(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edited() -> Settings {
        Settings {
            display_name: "Ada".into(), palette: "ember".into(), read_receipts: false,
            listen_port: 5555, keep_days: 7, ambient_enabled: true,
            stun_host: "stun.example.org:3478".into(), ..Default::default()
        }
    }

    #[test]
    fn every_setting_survives_a_round_trip() {
        // The whole point: a Save that does not persist is a lie the user
        // believes.
        let a = edited();
        let b = parse(&render(&a));
        assert_eq!(a.display_name, b.display_name);
        assert_eq!(a.palette, b.palette);
        assert_eq!(a.read_receipts, b.read_receipts);
        assert_eq!(a.listen_port, b.listen_port);
        assert_eq!(a.keep_days, b.keep_days);
        assert_eq!(a.stun_host, b.stun_host);
        assert_eq!(a.ambient_enabled, b.ambient_enabled);
        assert_eq!(a.require_encryption, b.require_encryption);
    }

    #[test]
    fn sections_match_the_pages_tabs() {
        // People edit these files, and a key filed under a different heading
        // than the tab it came from is a key nobody finds.
        let t = render(&Settings::default());
        for sec in ["[general]", "[camera]", "[network]", "[storage]", "[about]"] {
            assert!(t.contains(sec), "missing {sec}");
        }
        // and the port is under network, where the Network tab put it
        let net = t.split("[network]").nth(1).unwrap();
        assert!(net.contains("listen_port"));
    }

    #[test]
    fn one_bad_line_does_not_cost_every_other_setting() {
        // A port typed as a word, or a truncated write. Failing the whole load
        // is what makes people delete their config.
        let text = "[general]\ndisplay_name = Ada\n[network]\nlisten_port = banana\nupnp = false\n";
        let s = parse(text);
        assert_eq!(s.display_name, "Ada", "the good lines still applied");
        assert_eq!(s.listen_port, Settings::default().listen_port, "the bad one kept its default");
        assert!(!s.upnp, "and parsing continued past it");
    }

    #[test]
    fn an_unknown_key_is_ignored_so_any_version_loads() {
        // A file written by a newer build must not stop an older one starting.
        let text = "[general]\ndisplay_name = Bo\nquantum_mode = true\n[future]\nx = 1\n";
        let s = parse(text);
        assert_eq!(s.display_name, "Bo");
    }

    #[test]
    fn a_missing_key_keeps_its_default_rather_than_emptying() {
        // A file written by an OLDER build lacks newer keys; they must come up
        // as the default, not as empty or false.
        let s = parse("[general]\ndisplay_name = Cy\n");
        assert_eq!(s.palette, Settings::default().palette);
        assert_eq!(s.read_receipts, Settings::default().read_receipts);
        assert_eq!(s.listen_port, Settings::default().listen_port);
    }

    #[test]
    fn an_empty_or_absent_file_is_first_run_not_an_error() {
        // Reporting it would put a warning in front of every new user.
        assert_eq!(parse(""), Settings::default());
        assert_eq!(parse("# only a comment\n"), Settings::default());
    }
}
