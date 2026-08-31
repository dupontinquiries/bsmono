//! prefs — the handful of choices that must survive a restart.
//!
//! ## What was being lost
//!
//! Every per-operator choice in the chat tab lives in memory and dies with the process. For most of
//! them that is fine — a scroll position, a selected tab. For three it is not:
//!
//! * the FREECAM LAYOUT, because a Dvorak user re-selecting it on every launch is the accessibility
//!   bug re-appearing in a slower form;
//! * the SPACE, because the mind palace's whole premise is that a place becomes familiar, and a
//!   palace that resets to the default every morning cannot;
//! * the ENGINEERING PROFILES, because an operator who has deliberately overridden the autopilot
//!   means it for more than one session.
//!
//! ## Using the shared crate, not a fourth way to write a file
//!
//! `config-kit` already does XDG resolution, create-dir-read-fallback, and an INI parse, and is
//! used by hatch and settingsd. Adding a private settings file here would be the same mistake as a
//! second panel-frame primitive: a capability that exists, is tested, and gets reimplemented
//! because nobody looked.
//!
//! It has no serializer, so this supplies the render half. Both halves are pure functions over
//! strings — the filesystem is only touched by `load`/`save`, which are thin — so round-tripping,
//! unknown keys and malformed input are all testable without writing anything.
//!
//! ## Unknown keys survive
//!
//! A newer build's settings file opened by an older one must not silently lose the newer build's
//! keys on the next save. This keeps anything it does not recognise and writes it back.

use crate::engprofile::ProfileId;
use crate::navkeys::NavLayout;
use std::collections::BTreeMap;

const APP: &str = "logscope";
const FILE: &str = "ui.ini";

/// The persisted choices, plus whatever else was in the file.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Prefs {
    pub nav_layout: NavLayout,
    /// The selected space (palette or procedural room) by name. None = whatever the app defaults to.
    pub space: Option<String>,
    /// Manually chosen engineering profiles. Empty = autopilot.
    pub profiles: Vec<ProfileId>,
    /// Keys this build did not recognise, preserved so an older binary cannot eat a newer one's
    /// settings by round-tripping them.
    pub unknown: BTreeMap<String, String>,
}

/// Keys this build owns. Anything else in the file is passed through untouched.
const KNOWN: [&str; 3] = ["nav_layout", "space", "profiles"];

pub fn parse(text: &str) -> Prefs {
    let ini = config_kit::Ini::parse(text);
    let top = ini.sections.get("").cloned().unwrap_or_default();
    let get = |k: &str| top.get(k).map(|s| s.trim()).filter(|s| !s.is_empty());
    Prefs {
        nav_layout: get("nav_layout").and_then(NavLayout::from_key).unwrap_or_default(),
        space: get("space").map(|s| s.to_string()),
        profiles: get("profiles").map(|s| s.split(',').filter_map(ProfileId::from_key).collect())
            .unwrap_or_default(),
        unknown: top.into_iter().filter(|(k, _)| !KNOWN.contains(&k.as_str())).collect(),
    }
}

pub fn render(p: &Prefs) -> String {
    let mut s = String::from("# logscope UI preferences. Written by the app; safe to edit.\n");
    s.push_str(&format!("nav_layout = {}\n", p.nav_layout.key()));
    if let Some(sp) = &p.space {
        // Space names contain spaces and apostrophes ("Titan's Ribcage", "Zen Garden"); the parser
        // trims and unquotes, so quoting keeps a trailing-space name intact and costs nothing.
        s.push_str(&format!("space = \"{}\"\n", sp.replace('"', "")));
    }
    if !p.profiles.is_empty() {
        let keys: Vec<&str> = p.profiles.iter().map(|x| x.key()).collect();
        s.push_str(&format!("profiles = {}\n", keys.join(",")));
    }
    for (k, v) in &p.unknown {
        s.push_str(&format!("{k} = {v}\n"));
    }
    s
}

/// Read the file, or defaults when it is absent or unreadable.
pub fn load() -> Prefs {
    parse(&config_kit::load_or(APP, FILE, ""))
}

/// Best-effort write. A failure to persist a UI preference must never interrupt the session, so the
/// error is reported to the log and swallowed.
pub fn save(p: &Prefs) {
    if let Err(e) = config_kit::save(APP, FILE, &render(p)) {
        crate::diag!("wgpu-engine: could not save UI preferences: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_gives_working_defaults() {
        let p = parse("");
        assert_eq!(p.nav_layout, NavLayout::Qwerty);
        assert_eq!(p.space, None);
        assert!(p.profiles.is_empty(), "empty means autopilot, not 'no discipline'");
    }

    #[test]
    fn everything_round_trips() {
        let p = Prefs {
            nav_layout: NavLayout::DvorakCentered,
            space: Some("Observatory".into()),
            profiles: vec![ProfileId::Gpu, ProfileId::Optimizer],
            unknown: BTreeMap::new(),
        };
        assert_eq!(parse(&render(&p)), p);
    }

    /// Profile ORDER is precedence — it decides which profile wins a shared rule — so a round trip
    /// that reorders them silently changes how the agent behaves.
    #[test]
    fn profile_order_survives_because_it_is_precedence() {
        let a = Prefs { profiles: vec![ProfileId::Gpu, ProfileId::Optimizer], ..Default::default() };
        let b = Prefs { profiles: vec![ProfileId::Optimizer, ProfileId::Gpu], ..Default::default() };
        assert_eq!(parse(&render(&a)).profiles, a.profiles);
        assert_eq!(parse(&render(&b)).profiles, b.profiles);
        assert_ne!(parse(&render(&a)).profiles, parse(&render(&b)).profiles);
    }

    /// A space name with a space and an apostrophe is the common case here, not an edge case.
    #[test]
    fn awkward_space_names_survive() {
        for name in ["Titan's Ribcage", "Zen Garden", "Yellow Brick Road", "Folding@home", "Hearth Hall"] {
            let p = Prefs { space: Some(name.into()), ..Default::default() };
            assert_eq!(parse(&render(&p)).space.as_deref(), Some(name), "lost '{name}'");
        }
    }

    /// An older binary must not eat a newer one's settings by loading and saving them.
    #[test]
    fn unrecognised_keys_are_preserved_across_a_round_trip() {
        let text = "nav_layout = dvorak\nfuture_setting = 42\nanother = hello\n";
        let p = parse(text);
        assert_eq!(p.nav_layout, NavLayout::DvorakCentered);
        assert_eq!(p.unknown.get("future_setting").map(|s| s.as_str()), Some("42"));
        let out = render(&p);
        assert!(out.contains("future_setting = 42"), "a newer build's key was dropped:\n{out}");
        assert!(out.contains("another = hello"));
        // and they do not leak into the known fields
        assert_eq!(parse(&out).space, None);
    }

    /// Garbage must degrade to defaults rather than panicking — this file is user-editable, and it
    /// is read before a window exists to show an error in.
    #[test]
    fn malformed_input_degrades_instead_of_panicking() {
        for text in ["nav_layout =\n", "nav_layout = bepo\n", "profiles = ,,,\n",
                     "profiles = gpu,nonsense,optimizer\n", "[weird]\nnav_layout = dvorak\n",
                     "not an ini at all\n\x00\n", "space =\n"] {
            let p = parse(text);
            let _ = render(&p);
            // Membership, not an enumeration. This listed the two layouts that existed when it was
            // written and failed the moment three more were added — correctly, but a test that has
            // to be edited every time the set grows is a test that will eventually be edited
            // carelessly.
            assert!(NavLayout::all().contains(&p.nav_layout), "{:?}", p.nav_layout);
        }
        // an unknown layout name falls back rather than being invented
        assert_eq!(parse("nav_layout = bepo\n").nav_layout, NavLayout::Qwerty);
        // …and every layout that IS offered round-trips through the file
        for l in NavLayout::all() {
            assert_eq!(parse(&format!("nav_layout = {}\n", l.key())).nav_layout, l);
        }
        // unknown profile keys are dropped, known ones kept, order preserved
        assert_eq!(parse("profiles = gpu,nonsense,optimizer\n").profiles,
                   vec![ProfileId::Gpu, ProfileId::Optimizer]);
        // a key in a SECTION is not a top-level key
        assert_eq!(parse("[weird]\nnav_layout = dvorak\n").nav_layout, NavLayout::Qwerty);
    }

    #[test]
    fn the_rendered_file_is_stable() {
        // Same prefs, same bytes — otherwise every save rewrites the file and a diff is noise.
        let p = Prefs { nav_layout: NavLayout::DvorakCentered, space: Some("Cavern".into()),
                        profiles: vec![ProfileId::Planner], unknown: BTreeMap::new() };
        assert_eq!(render(&p), render(&p));
        assert_eq!(render(&parse(&render(&p))), render(&p), "a round trip must be a fixed point");
    }
}

/// The filesystem half, which the pure tests above deliberately do not touch.
///
/// I nearly left this untested on the grounds that config-kit's XDG resolution is tested in its own
/// crate — which is true and is not the same claim. What is untested there is whether THIS module
/// reaches THAT path with the right app name and file name, and a preference file written to the
/// wrong directory is indistinguishable from one that was never written.
///
/// `XDG_CONFIG_HOME` is process-global, so these run under one lock and restore it afterwards. The
/// repo builds with `--test-threads=1` in any case, but a test that only passes because of a build
/// flag somewhere else is a trap.
#[cfg(test)]
mod filesystem {
    use super::*;
    use std::sync::Mutex;

    static ENV: Mutex<()> = Mutex::new(());

    fn with_temp_config<R>(tag: &str, f: impl FnOnce(&std::path::Path) -> R) -> R {
        let _guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("logscope-prefs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let old = std::env::var_os("XDG_CONFIG_HOME");
        // SAFETY: serialised by the lock above; no other thread in this test binary touches it.
        unsafe { std::env::set_var("XDG_CONFIG_HOME", &dir) };
        let out = f(&dir);
        unsafe {
            match old { Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                        None => std::env::remove_var("XDG_CONFIG_HOME") }
        }
        let _ = std::fs::remove_dir_all(&dir);
        out
    }

    #[test]
    fn save_writes_where_load_reads_and_the_path_is_the_xdg_one() {
        with_temp_config("roundtrip", |dir| {
            let p = Prefs { nav_layout: NavLayout::DvorakCentered,
                            space: Some("Titan's Ribcage".into()),
                            profiles: vec![ProfileId::Optimizer, ProfileId::Gpu],
                            unknown: Default::default() };
            save(&p);
            // The exact path, not merely "somewhere": <XDG_CONFIG_HOME>/logscope/ui.ini
            let expected = dir.join("logscope").join("ui.ini");
            assert!(expected.is_file(), "nothing written to {}", expected.display());
            // and it is the file `load` reads
            assert_eq!(load(), p);
            // …by content, so a coincidence of defaults cannot pass this
            let text = std::fs::read_to_string(&expected).unwrap();
            assert!(text.contains("nav_layout = dvorak"), "{text}");
            assert!(text.contains("Titan's Ribcage"), "{text}");
            assert!(text.contains("profiles = optimizer,gpu"), "order is precedence: {text}");
        });
    }

    #[test]
    fn loading_with_no_file_present_gives_defaults_rather_than_failing() {
        with_temp_config("absent", |dir| {
            assert!(!dir.join("logscope").join("ui.ini").exists());
            assert_eq!(load(), Prefs::default());
        });
    }

    /// A hand-edited file with a newer build's key must come back intact after this build saves.
    #[test]
    fn a_hand_edited_file_survives_a_save_from_this_build() {
        with_temp_config("handedit", |dir| {
            let d = dir.join("logscope");
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("ui.ini"),
                "# hand written\nnav_layout = dvorak\nfuture_thing = keep-me\n").unwrap();
            let mut p = load();
            assert_eq!(p.nav_layout, NavLayout::DvorakCentered);
            p.space = Some("Cavern".into());
            save(&p);
            let text = std::fs::read_to_string(d.join("ui.ini")).unwrap();
            assert!(text.contains("future_thing = keep-me"), "ate a key it did not own:\n{text}");
            assert!(text.contains("Cavern"));
        });
    }

    /// An unwritable config dir must not take the session down — a preference is not worth a panic.
    #[test]
    fn an_unwritable_location_is_survivable() {
        with_temp_config("readonly", |dir| {
            // point the app dir at a FILE, so create_dir_all cannot succeed
            std::fs::write(dir.join("logscope"), b"not a directory").unwrap();
            save(&Prefs::default());     // must not panic
            assert_eq!(load(), Prefs::default()); // and reading still degrades cleanly
        });
    }
}

