//! Progress + settings persistence: a tiny human-readable `blackstar.save`
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
    /// PLAN.md Phase 1c plumbing, added ahead of the character-creation
    /// wizard itself (no UI reaches these yet — same shape as `wep_mods`
    /// above: the catalogs and types are real and tested,
    /// `patrol::Faction`/`weapons::ELEMENTS`/`content::SIGNATURES`/
    /// `ship::HULLS`, but there was nowhere to remember a pick even once
    /// a wizard existed to make one). Kept as raw indices/discriminants
    /// rather than importing each type here, matching how `car`/`map`
    /// are already plain `usize` rather than `content::Chassis`/
    /// `world::MapStyle`.
    ///
    /// `faction`: `patrol::Faction` discriminant order (0=None,
    /// 1=Hijacks, 2=Corrupted, 3=BlackstarClans, 4=Coalition).
    pub faction: u8,
    /// index into `weapons::ELEMENTS` (7 entries).
    pub element: u8,
    /// index into `content::SIGNATURES`.
    pub signature: usize,
    /// index into `ship::HULLS`.
    pub hull: usize,
    /// which room of the chosen hull the player spawns in — an index
    /// into that hull's own room list, not validated against a specific
    /// hull here (the wizard picks hull first, so cross-checking belongs
    /// there, not in the save format).
    pub berth: usize,
    /// OVERLOAD/CIRCUIT progress — added ahead of any in-game resume UI,
    /// same "plumbing only" shape as `wep_mods` above: `circuit.rs`,
    /// `overload.rs` and `moons.rs` are real and fully tested, but there
    /// was nowhere to remember a run across a relaunch. Empty
    /// `circuit_last_stop` means "never entered the facility" —
    /// `overload.rs` starts a brand-new run from the Dock either way,
    /// this only lets a results screen or a continue prompt say where
    /// the player last got to.
    pub circuit_last_stop: String,
    /// highest wave number any run has reached, for a results screen.
    pub circuit_best_wave: i32,
    /// most laps completed in a single run.
    pub circuit_best_laps: u32,
    /// the Dock bank (`docs`'s design brief marks this OPEN — whether
    /// valence should persist across runs at all — so this is plumbing
    /// for that decision, not a claim it's been made; nothing writes to
    /// this yet).
    pub circuit_bank: i64,
    /// has this save ever finished the intro cutscene? `false` on a
    /// brand-new save (and on a mangled one, per the lenient-parse
    /// contract) — `flow.rs` reads this to decide whether a launch goes
    /// through the intro and straight into Campaign, or skips both and
    /// opens on the character hub.
    pub seen_intro: bool,
}

impl Default for Save {
    fn default() -> Save {
        // STARTER GEAR. A brand-new player had an empty armed rail, an
        // empty bag, and an empty hotbar — the Armory console (`stations.rs`,
        // Station::Armory) is fully built and wired, but there was nothing
        // in it to look at or arm. `gear::WEAPONS` is a real, tested
        // catalog with nowhere it was ever reachable from during play.
        // Seeding a few of its entries here is the one place that reaches
        // every new save (both "no save file yet" and "the save on disk
        // failed to parse") without touching an existing player's gear.
        let firstedge = crate::gear::Item::Weapon {
            verb: crate::weapons::Verb::Thrust,
            element: crate::weapons::Element::None,
            from: None,
        };
        let placeholder_blade = crate::gear::Item::Weapon {
            verb: crate::weapons::Verb::Thrust,
            element: crate::weapons::Element::Fire,
            from: None,
        };
        let ashpulse = crate::gear::Item::Weapon {
            verb: crate::weapons::Verb::Beam,
            element: crate::weapons::Element::Fire,
            from: None,
        };
        let longcoil = crate::gear::Item::Weapon {
            verb: crate::weapons::Verb::Chain,
            element: crate::weapons::Element::None,
            from: None,
        };
        let starter_bag =
            [&firstedge, &placeholder_blade, &ashpulse].iter().map(|it| it.code()).collect::<Vec<_>>().join(",");
        // RACKED ON THE BAR TOO, not just in the bag — so pressing 1/2/3
        // does something the moment a new save loads, rather than
        // requiring a trip to the Armory first to find out the hotbar
        // exists. Built through Hotbar's own assign()/line() rather than
        // hand-written as a string, so this can't drift from whatever
        // format from_line() actually expects.
        let mut starter = crate::hotbar::Hotbar::default();
        let _ = starter.assign(0, firstedge);
        let _ = starter.assign(1, placeholder_blade);
        let _ = starter.assign(2, ashpulse);
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
            // OBSERVATORY, not MEADOW. The front door decides whether the
            // game reads as a demo, and MEADOW measures 0.58 stops of value
            // separation — the flattest of the 36 styles (valuestruct.rs).
            // OBSERVATORY is 4.16: a night field under a near-black zenith,
            // wet enough (0.50) that the ground genuinely reflects, lit by
            // its own accents. Dark with somewhere to look, which is the
            // register this game is aiming at.
            // Alternative branch if that reads too cold on a real screen:
            // FOUNDRY (1.47 stops, industrial orange) or CAVERN (3.30).
            map: 2,
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
            armed: format!("{},,", longcoil.code()), // LONGCOIL on the kinetic rail
            worn: String::new(),
            wep_mods: String::new(),
            bag: starter_bag,
            vault: String::new(),
            finds: String::new(),
            case: String::new(),
            watch: String::new(),
            hotbar: starter.line(),
            look: String::new(),
            // Coalition ("allies, mostly") as the default identity, index 4
            // in patrol::Faction's own declared order -- see the field's doc
            // comment on Save for the full mapping. Not 0/None: that reads
            // as "no faction chosen" rather than a real starting pick.
            faction: 4,
            element: 0,
            signature: 0,
            hull: 0,
            berth: 0,
            circuit_last_stop: String::new(),
            circuit_best_wave: 0,
            circuit_best_laps: 0,
            circuit_bank: 0,
            seen_intro: false,
        }
    }
}

thread_local! {
    static SANDBOX: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// TESTS: point this thread's save file at a scratch directory of its own.
///
/// The save lives in the working directory, and cargo runs a crate's tests
/// FROM the crate root -- so every test that drove a console through
/// `stations::apply` (which stores) wrote a real `blackstar.save` next to
/// `Cargo.toml`: a dirty tree after `cargo test` (CLAUDE.md §4), and, for
/// anyone who runs the game from the crate root, the player's own save
/// overwritten with test state. One directory per THREAD, since cargo runs
/// one test per thread, so parallel tests cannot read each other's
/// half-written file either. Returns the directory.
pub fn sandbox_this_thread() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("blackstar-save-{}-{:?}", std::process::id(), std::thread::current().id()));
    let _ = std::fs::create_dir_all(&dir);
    SANDBOX.with(|s| *s.borrow_mut() = Some(dir.clone()));
    dir
}

pub fn path() -> PathBuf {
    if let Some(dir) = SANDBOX.with(|s| s.borrow().clone()) {
        return dir.join("blackstar.save");
    }
    // blackstar.save is the name now. skydribble.save is still READ when the
    // new one is absent, because renaming a binary should not silently
    // discard a player's progress -- the first write lands in the new file
    // and the old one is simply left alone.
    let new = PathBuf::from("blackstar.save");
    if new.exists() || !PathBuf::from("skydribble.save").exists() {
        return new;
    }
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
            "faction" => s.faction = v.parse::<u8>().unwrap_or(s.faction) % 5,
            "element" => s.element = v.parse::<u8>().unwrap_or(s.element) % crate::weapons::ELEMENTS.len() as u8,
            "signature" => s.signature = v.parse::<usize>().unwrap_or(s.signature) % crate::content::SIGNATURES.len(),
            "hull" => s.hull = v.parse::<usize>().unwrap_or(s.hull) % crate::ship::HULLS.len(),
            "berth" => s.berth = v.parse::<usize>().unwrap_or(s.berth),
            "circuit_last_stop" => s.circuit_last_stop = v.to_string(),
            "circuit_best_wave" => s.circuit_best_wave = v.parse().unwrap_or(s.circuit_best_wave),
            "circuit_best_laps" => s.circuit_best_laps = v.parse().unwrap_or(s.circuit_best_laps),
            "circuit_bank" => s.circuit_bank = v.parse().unwrap_or(s.circuit_bank),
            "seen_intro" => s.seen_intro = v == "1",
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
    let _ = writeln!(out, "faction {}", s.faction);
    let _ = writeln!(out, "element {}", s.element);
    let _ = writeln!(out, "signature {}", s.signature);
    let _ = writeln!(out, "hull {}", s.hull);
    let _ = writeln!(out, "berth {}", s.berth);
    if !s.circuit_last_stop.is_empty() {
        let _ = writeln!(out, "circuit_last_stop {}", s.circuit_last_stop);
    }
    if s.circuit_best_wave > 0 {
        let _ = writeln!(out, "circuit_best_wave {}", s.circuit_best_wave);
    }
    if s.circuit_best_laps > 0 {
        let _ = writeln!(out, "circuit_best_laps {}", s.circuit_best_laps);
    }
    if s.circuit_bank != 0 {
        let _ = writeln!(out, "circuit_bank {}", s.circuit_bank);
    }
    if s.seen_intro {
        let _ = writeln!(out, "seen_intro 1");
    }
    b.save_lines(&mut out);
    cfg.save_lines(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sandboxed_thread_stores_in_its_own_directory_and_the_working_directory_stays_clean() {
        let dir = sandbox_this_thread();
        assert_eq!(path(), dir.join("blackstar.save"));
        let before = std::path::Path::new("blackstar.save").exists();
        store(&Save::default(), &Bindings::default_scheme(), &Config::default());
        assert!(dir.join("blackstar.save").exists(), "the store went to the sandbox");
        assert_eq!(std::path::Path::new("blackstar.save").exists(), before, "and not to the working directory");
        // another thread is not sandboxed by this one
        let other = std::thread::spawn(|| path()).join().unwrap();
        assert_eq!(other, PathBuf::from("blackstar.save"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_new_save_has_real_starter_gear_not_empty_strings() {
        // Before this, Save::default() left armed/bag/hotbar as
        // String::new() -- a brand-new player's Armory console had
        // nothing to look at. This proves the seeded content actually
        // round-trips through the REAL parsers (Gear::from_save,
        // Hotbar::from_line), not just that the strings look non-empty.
        let sv = Save::default();
        assert!(!sv.armed.is_empty(), "armed rail was left empty");
        assert!(!sv.bag.is_empty(), "starter bag was left empty");
        assert!(!sv.hotbar.is_empty(), "starter hotbar was left empty");

        let gear = crate::gear::Gear::from_save(&sv.armed, &sv.worn, &sv.bag);
        assert!(gear.kinetic.is_some(), "kinetic rail did not parse back to an item");
        assert_eq!(gear.bag.len(), 3, "expected 3 starter bag items, got {}", gear.bag.len());

        let hb = crate::hotbar::Hotbar::from_line(&sv.hotbar);
        assert!(hb.slot(0).is_some(), "hotbar slot 1 is empty on a new save");
        assert!(hb.slot(1).is_some(), "hotbar slot 2 is empty on a new save");
        assert!(hb.slot(2).is_some(), "hotbar slot 3 is empty on a new save");
        // and specifically: the placeholder_blade the person asked for is
        // actually reachable, not just "some item"
        let has_placeholder_blade = (0..crate::hotbar::SLOTS).any(|i| {
            matches!(
                hb.slot(i).map(|s| &s.item),
                Some(crate::gear::Item::Weapon { verb: crate::weapons::Verb::Thrust, element: crate::weapons::Element::Fire, .. })
            )
        });
        assert!(has_placeholder_blade, "the placeholder_blade isn't in the starter hotbar");
    }

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
            faction: 2,
            element: 5,
            signature: 7,
            hull: 3,
            berth: 4,
            circuit_last_stop: "tithe".into(),
            circuit_best_wave: 9,
            circuit_best_laps: 2,
            circuit_bank: 1250,
            seen_intro: true,
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
        assert_eq!(got.faction, want.faction, "faction");
        assert_eq!(got.element, want.element, "element");
        assert_eq!(got.signature, want.signature, "signature");
        assert_eq!(got.hull, want.hull, "hull");
        assert_eq!(got.berth, want.berth, "berth");
        assert_eq!(got.circuit_last_stop, want.circuit_last_stop, "circuit_last_stop");
        assert_eq!(got.circuit_best_wave, want.circuit_best_wave, "circuit_best_wave");
        assert_eq!(got.circuit_best_laps, want.circuit_best_laps, "circuit_best_laps");
        assert_eq!(got.circuit_bank, want.circuit_bank, "circuit_bank");
        assert_eq!(got.seen_intro, want.seen_intro, "seen_intro");
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

