//! Progress + settings persistence: a tiny human-readable `skydribble.save`
//! file of `key value` lines next to the binary's working directory. Parsed
//! leniently — a missing or mangled file just means defaults. Custom key
//! bindings ride along as `bind <action> <spec,spec,...>` lines.

use std::fmt::Write as _;
use std::path::PathBuf;

use crate::config::Config;
use crate::input::Bindings;

#[derive(Clone)]
pub struct Save {
    pub total_rings: u64,
    pub total_car_rings: u64,
    pub total_goals: u64,
    pub best_combo: u32,
    pub best_air: f32,      // longest air-dribble in seconds
    pub top_speed: f32,
    pub volume: f32,
    pub music: bool,
    pub minimap: bool,
    pub ball_cam: bool,
    pub fullscreen: bool,
    pub car: usize,
    pub map: usize,
    pub seed: u64,
    pub name: String,       // multiplayer display name / profile id
    /// which hardware profile these settings were auto-tuned for, so a save
    /// carried onto a phone (or off one) re-tunes exactly once
    pub tuned_profile: u8,
    /// best time per trial, 0 = unfinished
    pub trials: String,
    /// academy lessons passed, as a bitmask
    pub academy: String,
    /// garage loadout: skin, wheels, trail, explosion
    pub look: String,
    /// raid wardens cleared, comma-separated boss ids (see raidlog.rs)
    pub wardens: String,
    /// campaign missions cleared, comma-separated ids (see campaign.rs)
    pub missions: String,
    /// nightfall weeks cleared, comma-separated week numbers (see nightfall.rs)
    pub nightfalls: String,
    /// accumulated hull wear from LZ-pinned sorties (see sortie.rs /
    /// raid::lz_wear); cleared by yard time at a dock
    pub shipwear: f32,
    /// the watch-bearer's elemental attunement, `elem:points` pairs (see chi.rs)
    pub chi: String,
    /// the three weapon rails, item codes or `-` (see gear.rs)
    pub armed: String,
    /// worn armor mods, comma-separated names
    pub worn: String,
    /// weapon mods fitted per rail, `slot:name` pairs comma-separated —
    /// added ahead of any drop source or equip UI (`weapons::Mod` has
    /// neither yet): the effect logic and the three slots already exist
    /// and are fully tested, but there was nowhere to remember one
    /// across a save even if a future drop system granted it. This is
    /// the plumbing only; where a mod comes from and how it's equipped
    /// are real design decisions, not made here.
    pub wep_mods: String,
    /// the bag, comma-separated item codes
    pub bag: String,
    /// the ship's chest, positional item codes (see vault.rs)
    pub vault: String,
    /// the archaeological dig: recovered finding ids (see provenance.rs)
    pub finds: String,
    /// what is carried and NOT yet relayed (see tradecraft.rs)
    pub case: String,
    /// the Watch's current form, resolve, and whether Photon Man has
    /// ever been reached deliberately (see watch.rs)
    pub watch: String,
    /// the action bar: nine slots, each item's side and ammo type,
    /// and the active slot (see hotbar.rs)
    pub hotbar: String,
}

impl Default for Save {
    fn default() -> Save {
        Save {
            total_rings: 0,
            total_car_rings: 0,
            total_goals: 0,
            best_combo: 0,
            best_air: 0.0,
            top_speed: 0.0,
            volume: 0.8,
            music: true,
            minimap: true,
            ball_cam: true,
            fullscreen: false,
            car: 0,
            map: 0,
            seed: 20260726,
            name: String::new(),
            tuned_profile: 255,
            trials: String::new(),
            academy: String::new(),
            wardens: String::new(),
            missions: String::new(),
            nightfalls: String::new(),
            shipwear: 0.0,
            chi: String::new(),
            armed: String::new(),
            worn: String::new(),
            wep_mods: String::new(),
            bag: String::new(),
            vault: String::new(),
            finds: String::new(),
            case: String::new(),
            watch: String::new(),
            hotbar: String::new(),
            look: String::new(),
        }
    }
}

pub fn path() -> PathBuf {
    PathBuf::from("skydribble.save")
}

/// Does a save already exist? (first run gets platform defaults applied)
pub fn exists() -> bool {
    path().exists()
}

pub fn load() -> (Save, Bindings, Config) {
    match std::fs::read_to_string(path()) {
        Ok(text) => parse(&text),
        Err(_) => (Save::default(), Bindings::default_scheme(), Config::default()),
    }
}

/// Parse a save's text. Split from the file read so a round trip can be
/// TESTED: `finds` was added to `Save` and to `load`, and left out of
/// `Shipboard::store` for several sessions, because nothing could check that
/// every field survives the trip without touching the working directory.
/// A file-bound serializer is an untestable one.
pub fn parse(text: &str) -> (Save, Bindings, Config) {
    let mut s = Save::default();
    let mut b = Bindings::default_scheme();
    let mut cfg = Config::default();
    let mut saw_bind = false;
    for line in text.lines() {
        let Some((k, v)) = line.split_once(' ') else { continue };
        let v = v.trim();
        match k {
            "tuned_profile" => s.tuned_profile = v.parse().unwrap_or(s.tuned_profile),
            "trials" => s.trials = v.to_string(),
            "academy" => s.academy = v.to_string(),
            "wardens" => s.wardens = v.to_string(),
            "missions" => s.missions = v.to_string(),
            "nightfalls" => s.nightfalls = v.to_string(),
            "shipwear" => s.shipwear = v.parse().unwrap_or(s.shipwear),
            "chi" => s.chi = v.to_string(),
            "armed" => s.armed = v.to_string(),
            "worn" => s.worn = v.to_string(),
            "wep_mods" => s.wep_mods = v.to_string(),
            "bag" => s.bag = v.to_string(),
            "vault" => s.vault = v.to_string(),
            "finds" => s.finds = v.to_string(),
            "case" => s.case = v.to_string(),
            "watch" => s.watch = v.to_string(),
            "hotbar" => s.hotbar = v.to_string(),
            "look" => s.look = v.to_string(),
            "total_rings" => s.total_rings = v.parse().unwrap_or(s.total_rings),
            "total_car_rings" => s.total_car_rings = v.parse().unwrap_or(s.total_car_rings),
            "total_goals" => s.total_goals = v.parse().unwrap_or(s.total_goals),
            "best_combo" => s.best_combo = v.parse().unwrap_or(s.best_combo),
            "best_air" => s.best_air = v.parse().unwrap_or(s.best_air),
            "top_speed" => s.top_speed = v.parse().unwrap_or(s.top_speed),
            "volume" => s.volume = v.parse::<f32>().unwrap_or(s.volume).clamp(0.0, 1.0),
            "music" => s.music = v == "1",
            "minimap" => s.minimap = v == "1",
            "ball_cam" => s.ball_cam = v == "1",
            "fullscreen" => s.fullscreen = v == "1",
            "car" => s.car = v.parse::<usize>().unwrap_or(s.car).min(8),
            "map" => s.map = v.parse::<usize>().unwrap_or(s.map) % crate::world::STYLE_COUNT,
            "seed" => s.seed = v.parse().unwrap_or(s.seed),
            "name" => s.name = v.to_string(),
            "cfg" => {
                if let Some((ck, cv)) = v.split_once(' ') {
                    cfg.load_line(ck, cv);
                }
            }
            "bind" => {
                if !saw_bind {
                    // first bind line: start from an empty table so cleared
                    // actions stay cleared instead of inheriting defaults
                    for (_, binds) in b.map.iter_mut() {
                        binds.clear();
                    }
                    saw_bind = true;
                }
                if let Some((action, specs)) = v.split_once(' ') {
                    b.load_line(action, specs.trim());
                } else {
                    b.load_line(v, "");
                }
            }
            _ => {}
        }
    }
    (s, b, cfg)
}

pub fn store(s: &Save, b: &Bindings, cfg: &Config) {
    let out = render(s, b, cfg);
    if let Err(e) = std::fs::write(path(), out) {
        eprintln!("save: couldn't write {:?}: {e}", path());
    }
}

/// The save file's text. The other half of the split; see `parse`.
pub fn render(s: &Save, b: &Bindings, cfg: &Config) -> String {
    let mut out = String::with_capacity(1024);
    let _ = writeln!(out, "tuned_profile {}", s.tuned_profile);
    let _ = writeln!(out, "trials {}", s.trials);
    let _ = writeln!(out, "academy {}", s.academy);
    if !s.wardens.is_empty() {
        let _ = writeln!(out, "wardens {}", s.wardens);
    }
    if !s.missions.is_empty() {
        let _ = writeln!(out, "missions {}", s.missions);
    }
    if !s.nightfalls.is_empty() {
        let _ = writeln!(out, "nightfalls {}", s.nightfalls);
    }
    if s.shipwear > 0.0 {
        let _ = writeln!(out, "shipwear {:.2}", s.shipwear);
    }
    for (k, v) in [("chi", &s.chi), ("armed", &s.armed), ("worn", &s.worn), ("wep_mods", &s.wep_mods), ("bag", &s.bag), ("vault", &s.vault), ("finds", &s.finds), ("case", &s.case), ("watch", &s.watch), ("hotbar", &s.hotbar)] {
        if !v.is_empty() {
            let _ = writeln!(out, "{k} {v}");
        }
    }
    let _ = writeln!(out, "look {}", s.look);
    let _ = writeln!(out, "total_rings {}", s.total_rings);
    let _ = writeln!(out, "total_car_rings {}", s.total_car_rings);
    let _ = writeln!(out, "total_goals {}", s.total_goals);
    let _ = writeln!(out, "best_combo {}", s.best_combo);
    let _ = writeln!(out, "best_air {:.2}", s.best_air);
    let _ = writeln!(out, "top_speed {:.2}", s.top_speed);
    let _ = writeln!(out, "volume {:.2}", s.volume);
    let _ = writeln!(out, "music {}", s.music as u8);
    let _ = writeln!(out, "minimap {}", s.minimap as u8);
    let _ = writeln!(out, "ball_cam {}", s.ball_cam as u8);
    let _ = writeln!(out, "fullscreen {}", s.fullscreen as u8);
    let _ = writeln!(out, "car {}", s.car);
    let _ = writeln!(out, "map {}", s.map);
    let _ = writeln!(out, "seed {}", s.seed);
    if !s.name.is_empty() {
        let _ = writeln!(out, "name {}", s.name);
    }
    b.save_lines(&mut out);
    cfg.save_lines(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `Save` with every field set to something distinctive, so a field
    /// dropped by either half of the trip shows up as a mismatch rather than
    /// as a default that happens to look plausible.
    fn distinctive() -> Save {
        Save {
            total_rings: 11,
            total_car_rings: 12,
            total_goals: 13,
            best_combo: 14,
            best_air: 15.25,
            top_speed: 16.5,
            volume: 0.33,
            music: false,
            minimap: false,
            ball_cam: false,
            fullscreen: true,
            car: 3,
            map: 5,
            seed: 987_654,
            name: "kessler".into(),
            tuned_profile: 1,
            trials: "t1:9.5".into(),
            academy: "0b1011".into(),
            look: "skin2,wheel3".into(),
            wardens: "judgementor".into(),
            missions: "the_beach_opens".into(),
            nightfalls: "7".into(),
            shipwear: 1.75,
            chi: "rock:15".into(),
            armed: "w:beam:ice".into(),
            worn: "plating".into(),
            wep_mods: "0:long_barrel".into(),
            bag: "w:flak:fire".into(),
            vault: "w:push:water".into(),
            finds: "bunker_alloy,museum_specimen".into(),
            case: "thermal_plate".into(),
            watch: "native:3:1".into(),
            hotbar: "1:2:3".into(),
        }
    }

    /// THE TEST THAT WAS MISSING. `finds` lived in `Save` and in `load` and
    /// was absent from `Shipboard::store` for several sessions; nothing could
    /// catch it, because the serializer only existed as a pair of functions
    /// that touched the filesystem. Every field, both directions, no I/O.
    #[test]
    fn every_save_field_survives_a_round_trip() {
        let want = distinctive();
        let b = Bindings::default_scheme();
        let cfg = Config::default();
        let (got, _, _) = parse(&render(&want, &b, &cfg));

        // compared field by field so a failure NAMES the field that was lost
        assert_eq!(got.total_rings, want.total_rings, "total_rings");
        assert_eq!(got.total_car_rings, want.total_car_rings, "total_car_rings");
        assert_eq!(got.total_goals, want.total_goals, "total_goals");
        assert_eq!(got.best_combo, want.best_combo, "best_combo");
        assert!((got.best_air - want.best_air).abs() < 0.01, "best_air");
        assert!((got.top_speed - want.top_speed).abs() < 0.01, "top_speed");
        assert!((got.volume - want.volume).abs() < 0.01, "volume");
        assert_eq!(got.music, want.music, "music");
        assert_eq!(got.minimap, want.minimap, "minimap");
        assert_eq!(got.ball_cam, want.ball_cam, "ball_cam");
        assert_eq!(got.fullscreen, want.fullscreen, "fullscreen");
        assert_eq!(got.car, want.car, "car");
        assert_eq!(got.map, want.map, "map");
        assert_eq!(got.seed, want.seed, "seed");
        assert_eq!(got.name, want.name, "name");
        assert_eq!(got.tuned_profile, want.tuned_profile, "tuned_profile");
        assert_eq!(got.trials, want.trials, "trials");
        assert_eq!(got.academy, want.academy, "academy");
        assert_eq!(got.look, want.look, "look");
        assert_eq!(got.wardens, want.wardens, "wardens");
        assert_eq!(got.missions, want.missions, "missions");
        assert_eq!(got.nightfalls, want.nightfalls, "nightfalls");
        assert!((got.shipwear - want.shipwear).abs() < 0.01, "shipwear");
        assert_eq!(got.chi, want.chi, "chi");
        assert_eq!(got.armed, want.armed, "armed");
        assert_eq!(got.worn, want.worn, "worn");
        assert_eq!(got.wep_mods, want.wep_mods, "wep_mods");
        assert_eq!(got.bag, want.bag, "bag");
        assert_eq!(got.vault, want.vault, "vault");
        assert_eq!(got.finds, want.finds, "finds");
        assert_eq!(got.case, want.case, "case");
        assert_eq!(got.watch, want.watch, "watch");
        assert_eq!(got.hotbar, want.hotbar, "hotbar");
    }

    /// The guard against the NEXT one: if somebody adds a field to `Save` and
    /// does not add it above, this fails and says so. Counting lines in the
    /// struct is crude, and it is the only thing that notices a field nobody
    /// wired up — which is exactly the failure this module has already had.
    #[test]
    fn the_round_trip_test_covers_every_field() {
        let src = include_str!("save.rs");
        let decl = src
            .split("pub struct Save {")
            .nth(1)
            .and_then(|s| s.split("\n}").next())
            .expect("the Save struct");
        let fields: Vec<&str> = decl
            .lines()
            .filter_map(|l| l.trim().strip_prefix("pub "))
            .filter_map(|l| l.split(':').next())
            .collect();
        let checks = src.split("fn every_save_field_survives_a_round_trip").nth(1).unwrap();
        for f in &fields {
            assert!(
                checks.contains(&format!("\"{f}\"")),
                "Save::{f} is not checked by every_save_field_survives_a_round_trip"
            );
        }
        assert!(fields.len() >= 30, "only found {} fields; the parser broke", fields.len());
    }

    #[test]
    fn a_mangled_file_loads_as_defaults_rather_than_failing() {
        // lenient in, canonical out — the contract the rest of the codebase
        // relies on when it hand-edits a save
        let (s, _, _) = parse("garbage\nmissions\nseed notanumber\n\nfinds a,b\n");
        assert_eq!(s.seed, Save::default().seed, "a bad number should not move the field");
        assert_eq!(s.finds, "a,b", "a good line beside a bad one should still land");
    }

    #[test]
    fn config_and_bindings_ride_along() {
        let mut cfg = Config::default();
        cfg.load_line("story_anchor", "0.8");
        cfg.load_line("case_cap", "9");
        let (_, _, back) = parse(&render(&Save::default(), &Bindings::default_scheme(), &cfg));
        assert!((back.story.anchor - 0.8).abs() < 1e-3, "anchor");
        assert!((back.story.case_cap - 9.0).abs() < 1e-3, "case_cap");
    }
}
