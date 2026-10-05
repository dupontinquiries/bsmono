//! STATIONS — the ship is the menu.
//!
//! `ship.rs` already refuses the costume: plotting is a PLACE, phases
//! are places, and the doc says so in italics. But the rest of the
//! game's surfaces never got places — gear, the vault, the clone vats,
//! the war ledger all live in command-line sims, and the in-game menus
//! that do exist (pause, tweaks, binds) float over the world belonging
//! to nowhere. That is the "separation" wrongness: the fiction built a
//! ship and the UI never boarded it.
//!
//! A STATION is a screen with an address. Each one:
//! * lives in a ROOM, and is reachable only when the ship's phase makes
//!   the interior walkable (`Phase::interior()`) — menus obey the hull;
//! * declares a DEPLOY STYLE (`menufx.rs`), so the armory snaps like a
//!   weapons rack while the bridge sweeps like a viewport — different
//!   animations, one system;
//! * declares an ACCENT, the room's light, so the draw layer can tint
//!   panel chrome per station without inventing per-screen draw code;
//! * BUILDS its menu from real state and APPLIES the events back to it.
//!   The armory equips through `gear.rs` with chi worthiness printed,
//!   the hold moves items through `vault.rs`, the bridge launches the
//!   campaign the sims have been running from the terminal.
//!
//! `Shipboard` is that real state, loaded once and stored once — the
//! same lenient save lines the sims use, so the terminal loops and the
//! in-game stations are two doors into one room.

use crate::campaign::CampaignLog;
use crate::chi::Chi;
use crate::content::MISSIONS;
use crate::valence;
use crate::gear::{Gear, Item, Rail};
use crate::menu::{Event, Menu, Row};
use crate::menufx::DeployStyle;
use crate::nightfall::{self, NightfallLog};
use crate::raidcon::BOSSES;
use crate::raidlog::RaidLog;
use crate::save;
use crate::ship::Phase;
use crate::vault::Vault;
use crate::archive;
use crate::curriculum;
use crate::provenance::{assay_with, Bearing, Dig, Ladder, Medium};
use crate::hotbar::Hotbar;
use crate::watch::{Watch, NATIVE, PHOTON};

/// The rooms with consoles in them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Station {
    /// sorties, the week's nightfall, the valence ledger
    Bridge,
    /// rails, worn armor, the bag — the gear sheet
    Armory,
    /// the chest: 27 slots of cargo grid
    Hold,
    /// the clone vats and the bearer's chi
    Medbay,
    /// the warden ladder and the seal
    Wardeck,
    /// THE STARMAP — the destination list, and the first place the campaign
    /// stack is reachable from inside the game.
    ///
    /// `docs/PRIORITIES.md` counted the problem: `patrol` has zero references
    /// in `main.rs`, so the twenty destinations exist, are tested, and cannot
    /// be looked at while playing. This is the smallest honest fix — the
    /// airlock is where you leave from, and it was the one hub room with no
    /// console on it.
    Starmap,
    /// THE CHARTROOM -- the archaeological map, and the only place the
    /// carried-back artifacts get looked at properly.
    ///
    /// This is the ship end of the split the archaeology layer is built
    /// around: `Medium::Artifact` evidence comes home to this table, while
    /// terminals and cut walls are read where they stand. The Destiny move
    /// is that the Tower is where you make sense of what you brought back,
    /// not where you find it.
    Chartroom,
}

pub const STATIONS: [Station; 7] = [
    Station::Bridge,
    Station::Armory,
    Station::Hold,
    Station::Medbay,
    Station::Wardeck,
    Station::Chartroom,
    Station::Starmap,
];

impl Station {
    pub fn id(self) -> &'static str {
        match self {
            Station::Bridge => "bridge",
            Station::Armory => "armory",
            Station::Hold => "hold",
            Station::Medbay => "medbay",
            Station::Wardeck => "wardeck",
            Station::Starmap => "starmap",
            Station::Chartroom => "chartroom",
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Station::Bridge => "THE BRIDGE",
            Station::Armory => "THE ARMORY",
            Station::Hold => "THE HOLD",
            Station::Medbay => "THE MEDBAY",
            Station::Wardeck => "THE WARDECK",
            Station::Starmap => "THE STARMAP",
            Station::Chartroom => "THE CHARTROOM",
        }
    }
    /// One line of place, drawn under the title — the room, not the UI.
    pub fn room(self) -> &'static str {
        match self {
            Station::Bridge => "forward viewport, the system laid out cold and slow",
            Station::Armory => "racked plating and rail housings, smelling of ozone",
            Station::Hold => "cargo lattice amidships, one chest bolted to the deck",
            Station::Medbay => "seven vats along the hull, one of them yours",
            Station::Wardeck => "the seal chart, nineteen names, some of them crossed",
            Station::Starmap => "the airlock plot: twenty places, and what each one will do to you",
            Station::Chartroom => "a long table under a hard lamp, and everything you carried home on it",
        }
    }
    /// How the room's console deploys. The character lives HERE, once.
    pub fn style(self) -> DeployStyle {
        match self {
            Station::Bridge => DeployStyle::sweep(),
            Station::Armory => DeployStyle::rack(),
            Station::Hold => DeployStyle::stack(),
            Station::Medbay => DeployStyle::glide(),
            Station::Wardeck => DeployStyle::glide(),
            // the plot unrolls: a chart is a thing you spread out
            Station::Starmap => DeployStyle::sweep(),
            Station::Chartroom => DeployStyle::sweep(),
        }
    }
    /// Which HUB room this console is IN.
    ///
    /// `room()` above is prose; this is an address. The two were unrelated
    /// until now: `interior.rs` has generated a real floor plan from
    /// `content::HUB` since it landed — eight rooms with positions, doors and
    /// reachability — and had no caller, while this module claimed in its own
    /// doc comment that "menus obey the hull" and enforced that with a phase
    /// check alone. A console you can open while standing on the other side of
    /// a sealed door is not obeying anything.
    ///
    /// Stations are placed by what the room IS, not by convenience: the
    /// armoury is the upgrade bay because that is where the chassis rigs and
    /// part racks are, and the hold is the hangar because that is the room
    /// with a deck to bolt a chest to.
    pub fn cell(self) -> &'static str {
        match self {
            Station::Bridge => "bridge",
            Station::Armory => "upgrade_bay",
            Station::Hold => "hangar",
            Station::Medbay => "quarters",
            Station::Wardeck => "corridor_aft",
            Station::Starmap => "airlock",
            Station::Chartroom => "market",
        }
    }

    /// The room's light, for panel chrome: r, g, b.
    pub fn accent(self) -> [f32; 3] {
        match self {
            Station::Bridge => [0.36, 0.62, 0.95],  // viewport starlight
            Station::Armory => [0.95, 0.55, 0.20],  // work lamps on steel
            Station::Hold => [0.55, 0.80, 0.40],    // cargo strip-lights
            Station::Medbay => [0.45, 0.90, 0.95],  // vat glow, ice-cold
            Station::Wardeck => [0.85, 0.30, 0.30], // the seal, unhappy
            Station::Starmap => [0.55, 0.62, 0.90], // airlock blue, cold outside
            Station::Chartroom => [0.88, 0.82, 0.62], // one hard lamp, paper-warm
        }
    }
    /// Whether the walk to this console is possible right now. Stations
    /// obey the hull: nobody browses the armory during reconfiguration.
    pub fn available(self, phase: Phase) -> Result<(), &'static str> {
        if !phase.interior() {
            return Err("the interior is not walkable right now");
        }
        // and the room has to be somewhere you can actually get to
        if !crate::content::HUB.iter().any(|r| r.id == self.cell()) {
            return Err("that room is not on the ship's plan");
        }
        match self {
            Station::Bridge => {
                if matches!(phase, Phase::Plotting | Phase::Cruise) {
                    Ok(())
                } else {
                    Err("the nav console lights up in orbit")
                }
            }
            _ => Ok(()),
        }
    }
}

/// Everything the consoles read and write: the save, opened as state.
/// One load, one store, same lines the terminal sims use.
pub struct Shipboard {
    pub camp: CampaignLog,
    pub raids: RaidLog,
    pub nf: NightfallLog,
    pub chi: Chi,
    pub gear: Gear,
    pub vault: Vault,
    pub watch: Watch,
    pub hotbar: Hotbar,
    /// what has been dug up (see provenance.rs)
    pub dig: Dig,
    /// the mystery anchor, 0..1 (see archive::RECEIVED)
    pub anchor: f32,
    /// how fast the map firms up (see provenance::Ladder, cfg arch_*)
    pub ladder: Ladder,
    /// THE MASTER SHIP — cfg `story_debug_ship`. Every progression gate
    /// defers to it, so a tester reaches the far end of the map without
    /// playing to it.
    pub debug_ship: bool,
}

impl Shipboard {
    /// Does the master ship apply? ONE method, consulted by every gate, so
    /// there is exactly one place the override lives — a second way to bypass
    /// a gate is a second thing to keep in sync with the gates, and this
    /// codebase already learned that lesson from having two fold durations.
    pub fn unrestricted(&self) -> bool {
        self.debug_ship
    }

    pub fn load() -> Shipboard {
        let (sv, _, cfg) = save::load();
        // the unlock is applied to the LOADED log, never written back — see
        // `CampaignLog::apply_playtest_unlock`
        let mut camp = CampaignLog::from_line(&sv.missions);
        if let Some(note) = camp.apply_playtest_unlock() {
            eprintln!("{note}");
        }
        Shipboard {
            anchor: cfg.story.anchor,
            debug_ship: cfg.story.debug_ship,
            ladder: Ladder::from_cfg(&cfg),
            camp,
            raids: RaidLog::from_line(&sv.wardens),
            nf: NightfallLog::from_line(&sv.nightfalls),
            chi: Chi::from_line(&sv.chi),
            gear: Gear::from_save(&sv.armed, &sv.worn, &sv.bag),
            vault: Vault::from_line(&sv.vault),
            dig: Dig::from_line(&sv.finds, archive::FINDINGS),
            watch: Watch::from_line(&sv.watch),
            hotbar: Hotbar::from_line(&sv.hotbar),
        }
    }
    pub fn store(&self) {
        let (mut sv, b, cfg) = save::load();
        sv.missions = self.camp.line();
        sv.wardens = self.raids.line();
        sv.nightfalls = self.nf.line();
        sv.chi = self.chi.line();
        let (armed, worn, bag) = self.gear.save_lines();
        sv.armed = armed;
        sv.worn = worn;
        sv.bag = bag;
        sv.vault = self.vault.line();
        sv.watch = self.watch.line();
        sv.hotbar = self.hotbar.line();
        // the dig was loaded into Shipboard and never written back, so
        // anything recovered at a console vanished on the next load. Silent,
        // because nothing at a console could recover anything yet.
        sv.finds = self.dig.line();
        save::store(&sv, &b, &cfg);
    }
    pub fn valence(&self) -> u32 {
        valence::valence(&self.camp, &self.raids, self.nf.count())
    }
}

/// What pressing things at a console can mean beyond the menu's own
/// events. The shell decides how to launch a sortie; the stations
/// decide everything that is pure state.
#[derive(Clone, Debug, PartialEq)]
pub enum Order {
    /// begin this campaign mission (id)
    Sortie(&'static str),
    /// run this week's nightfall
    Nightfall(u64),
    /// walk to another station
    Goto(Station),
    /// a state change happened and was stored; the line is feedback
    Did(String),
    /// a refusal, printed where the player is looking
    Refused(String),
    /// FLY THERE. The console cannot load a map — that needs the shell, the
    /// gpu and a fresh `World` — so it names the destination and the shell
    /// does it, the same way `Sortie` names a mission rather than running it.
    ///
    /// The line is the arrival report, already computed, so the shell does not
    /// have to fly the drop a second time to know what to say.
    Depart { dest: &'static str, line: String },
    /// FLY THE WARDEN RAID (boss id). Passes through to the shell for the
    /// same reason Sortie does — and more so: a raid is a run of phases,
    /// each with its own arena, its own sky, and its own drop.
    Raid(&'static str),
    /// LIGHT THE FORGE: ask the model for a new echo signal on this theme.
    /// Passes through because the call runs on a shell worker thread.
    Forge(String),
    /// FLY A FORGED ECHO (its `echo:<hash>` id from `raidforge::catalog`).
    Echo(&'static str),
}

/// Evidence recoverable but not yet recorded: sited where a mission has been
/// cleared, gate open, and not already in the dig.
///
/// The site gate is the whole design. Without it the Chartroom would be a
/// button that hands over the entire history, and the archaeology layer would
/// be an inventory screen.
fn unexamined(sb: &Shipboard) -> Vec<&'static crate::provenance::Finding> {
    archive::FINDINGS
        .iter()
        .filter(|f| !sb.dig.has(f.id))
        .filter(|f| f.gate.map(|g| archive::gate_open(g, &sb.camp, &sb.raids)).unwrap_or(true))
        .filter(|f| {
            // BY PLACE where there is one, by destination otherwise.
            //
            // This was destination-only, so clearing any Earth mission opened
            // all seventeen Earth findings at once — an observatory and a
            // museum the mission never went near. "You can only examine
            // evidence from somewhere you have actually been" was the stated
            // rule and "been to Earth" is not the same as "been everywhere on
            // Earth". `groundside::Place::reached_by` is the join that makes
            // the finer answer available.
            match crate::groundside::place_of(f.id) {
                Some(p) => crate::groundside::visited(p, &sb.camp),
                None => MISSIONS.iter().any(|m| m.dest == f.site && sb.camp.is_cleared(m.id)),
            }
        })
        .collect()
}

/// What happened on the way in.
pub struct Arrival {
    /// a phrase for the report line
    pub line: String,
    /// how much the landing cost the squad, 0..1 off their effective strength
    pub penalty: f32,
}

/// Fly a mission's descent before it resolves.
///
/// The join the campaign has never had: a mission names a `dest`, `patrol`
/// knows that world's gravity and surface, and `drop.rs` flies the fall. A
/// scattered landing is the first thing in this game where arriving badly
/// costs you at the far end rather than being scenery.
/// Miss distance -> engagement penalty. A clean drop is free; a scattered
/// one hurts, capped so a bad landing is a setback rather than a loss
/// before the fight starts.
///
/// ONE formula, used by the autopiloted arrival below AND by the flown
/// sortie (`sortie.rs`) — the flown drop must be worth exactly what the
/// autopilot's competence used to be worth, and a penalty computed twice
/// is the two-answers-to-one-question mistake `--parity` hunts.
pub fn landing_penalty(l: &crate::drop::Landing) -> f32 {
    ((l.miss_distance - 20.0) / 300.0).clamp(0.0, 0.35)
}

pub fn arrive_at(dest: &str) -> Arrival {
    let Some(d) = crate::patrol::ROUTE.iter().find(|d| d.id == dest) else {
        return Arrival { line: String::new(), penalty: 0.0 };
    };
    if is_dock(d) {
        return Arrival { line: "docked —".into(), penalty: 0.0 };
    }
    let g = crate::mapsim::ground_for(d);
    let l = fly_the_drop(d, &g);
    let penalty = landing_penalty(&l);
    let line = if l.miss_distance < 20.0 {
        format!("clean drop on {} —", d.name)
    } else if penalty < 0.2 {
        format!("loose drop on {} ({:.0} m) —", d.name, l.miss_distance)
    } else {
        format!("SCATTERED on {} ({:.0} m) —", d.name, l.miss_distance)
    };
    Arrival { line, penalty }
}

/// Fly the descent for a destination, in simulation.
///
/// The first time `patrol`, `mapsim` and `drop` run in sequence rather than
/// in three separate headless drivers. Gravity comes from the destination,
/// the ground from `ground_for`, and the fall from `drop.rs` — so a low-g
/// world really does give you longer to correct, and a heavy one really does
/// hurt on arrival.
///
/// Deterministic: the same destination flies the same way every time, because
/// a console that reports a different outcome each press is telling you about
/// its RNG rather than about the world.
/// You do not land on deep space; you dock with something in it.
///
/// Three of the eighteen destinations have gravity at or near zero —
/// `outer_rim` and `shield_edge` are `Terrain::DeepSpace`, `carrion` is an
/// asteroid field at 0.02g. A drop needs something to fall toward, and
/// forcing one produces a descent that never terminates: my first version
/// looped ninety seconds and reported an infinite miss distance, which a test
/// caught as "outer_rim never landed at all".
///
/// That is correct DATA and a wrong assumption on my part, not a bug in the
/// route table.
pub const DOCK_BELOW_G: f32 = 0.05;

pub fn is_dock(d: &crate::patrol::Destination) -> bool {
    d.gravity < DOCK_BELOW_G
}

fn fly_the_drop(
    d: &'static crate::patrol::Destination,
    g: &crate::terrain::Ground,
) -> crate::drop::Landing {
    use crate::drop::{Drop, Mode};
    use glam::Vec3;
    let mut dr = Drop::new(Vec3::ZERO, d.hostiles != crate::patrol::Faction::None);
    dr.jump(Vec3::new(0.0, 600.0, 0.0), Vec3::new(18.0, 0.0, 0.0));
    // a competent drop: fold to the fall gait at a sensible altitude and
    // steer back toward the mark
    let dt = 1.0 / 60.0;
    for step in 0..(60 * 90) {
        let alt = dr.pos.y;
        if alt < 120.0 && dr.mode == Mode::Vehicle {
            dr.transform(alt);
        }
        // steer at the landing zone, hardest early when there is time
        let to = (dr.lz - dr.pos) * Vec3::new(1.0, 0.0, 1.0);
        let steer = to.normalize_or_zero();
        if let Some(l) = dr.update(dt, steer, g.g, 0.0) {
            return l;
        }
        let _ = step;
    }
    crate::drop::Landing {
        impact_speed: 0.0,
        self_dmg: 0.0,
        shock: 0.0,
        shock_radius: 0.0,
        miss_distance: f32::INFINITY,
        aa_taken: 0.0,
        hero: false,
    }
}

/// Is there a route to this destination yet?
///
/// Uses the same gate vocabulary `archive::gate_open` does — a bare id is a
/// mission, `warden:<id>` a raid clear — so the starmap and the archaeology
/// map cannot disagree about where you have been.
fn gate_open_for(dest: &str, sb: &Shipboard) -> bool {
    if sb.unrestricted() {
        return true;
    }
    // the first two are always open; the rest follow the missions that reach
    // them, which is what `content::MISSIONS` already encodes
    if matches!(dest, "earth" | "outer_rim") {
        return true;
    }
    // THE BRIDGE GATES EVERYTHING OUTSIDE SOL. Reaching a world on a mission
    // is not the same as being able to GO there: leaving the system needs the
    // space bridge, and the bridge stays dark until Assay is dead. Before
    // this, an extra-solar destination opened purely because a mission had
    // touched it, so the reason the bridge was shut was nowhere in the game.
    if crate::patrol::beyond_sol(dest) && !crate::patrol::bridge_powered(&sb.camp) {
        return false;
    }
    crate::content::MISSIONS
        .iter()
        .filter(|m| m.dest == dest)
        .any(|m| sb.camp.is_cleared(m.id))
}

/// Build a station's console from the state.
pub fn build(st: Station, sb: &Shipboard, week: u64) -> Menu {
    let mut rows: Vec<Row> = Vec::new();
    match st {
        Station::Starmap => {
            use crate::patrol::ROUTE;
            // THE FIRST TIME `patrol` IS READABLE FROM INSIDE THE GAME.
            // Twenty destinations with real gravity, terrain and weather,
            // tested for years under `--terrain`, and until now invisible
            // while playing.
            let reachable = ROUTE.iter().filter(|d| gate_open_for(d.id, sb)).count();
            rows.push(Row::heading(&format!(
                "{reachable} of {} plotted",
                ROUTE.len()
            )));
            rows.push(Row::gap());
            for d in ROUTE.iter() {
                let open = gate_open_for(d.id, sb);
                let g = crate::mapsim::ground_for(d);
                let label = format!(
                    "{:<14} {:>4.2}g {:?}",
                    d.name.chars().take(14).collect::<String>(),
                    d.gravity,
                    g.material
                );
                if open {
                    rows.push(
                        Row::action(d.id, &label).hint(leak(format!(
                            "{:?} under a {:?} sky, {}, {:?} held",
                            d.terrain,
                            d.sky,
                            if d.weather { "weather" } else { "no air movement" },
                            d.hostiles
                        ))),
                    );
                } else {
                    // greyed rather than hidden: the menu's own note says "why can't
                    // I press this" beats a row that is not there, and a
                    // starmap with holes in it is a worse map
                    rows.push(
                        Row::action(d.id, &format!("{label}   — no route yet")).disabled(),
                    );
                }
            }
            rows.push(Row::gap());
            rows.push(Row::heading("plotting is not flying — see PRIORITIES.md"));
        }
        Station::Chartroom => {
            let a = assay_with(
                archive::CLAIMS,
                archive::FINDINGS,
                sb.dig.ids(),
                &archive::priors(sb.anchor),
            );
            let ladder = sb.ladder;
            let mut nodes: Vec<_> =
                archive::CLAIMS.iter().filter(|c| a[c.id].standing_with(ladder).drawn()).collect();
            // artifacts come HOME; terminals and cut walls were read where
            // they stood and never reach this table
            let shelved = archive::FINDINGS
                .iter()
                .filter(|f| sb.dig.has(f.id) && f.medium == Medium::Artifact)
                .count();
            let crated = unexamined(sb);
            rows.push(Row::heading(&format!(
                "{} home / {} nodes / anchor {}",
                shelved,
                nodes.len(),
                archive::anchor_name(sb.anchor)
            )));
            // The one thing this table DOES rather than shows. You can only
            // go through evidence from somewhere you have actually been, so
            // the gate is a cleared mission at that site: "everything you
            // carried home" is the room's own description of itself.
            if !crated.is_empty() {
                rows.push(Row::gap());
                rows.push(Row::heading(&format!("UNEXAMINED ({})", crated.len())));
                for f in &crated {
                    rows.push(
                        Row::action(f.id, &format!("{} ({})", f.id, f.site))
                            .hint(leak(format!("{:?} — recovered at {}", f.medium, f.site))),
                    );
                }
            }
            if nodes.is_empty() {
                rows.push(Row::heading(
                    "the table is bare. the map is built out of what you carry home.",
                ));
            }
            nodes.sort_by_key(|c| (std::cmp::Reverse(a[c.id].standing_with(ladder)), c.layer, c.id));
            for c in nodes {
                let asy = &a[c.id];
                // Lead with the claim. The first version put the STANDING
                // first and left the id out of the label entirely, so the
                // console showed three rows reading "L2 UNKNOWN TECHNOLOGICAL
                // RELATION — 1 for / 0 against" and nothing else: identical,
                // and truncated at the console's width besides. Found by
                // driving `--menu chartroom`, not by a test.
                rows.push(Row::action(
                    c.id,
                    &format!(
                        "L{} {:<26} {} {}/{}",
                        c.layer,
                        c.id,
                        asy.standing_with(ladder).short(),
                        asy.support(),
                        asy.opposition()
                    ),
                ));
            }
        }
        Station::Bridge => {
            rows.push(Row::heading(&format!(
                "VALENCE {}  ({}m {}w {}n)",
                sb.valence(),
                sb.camp.done(),
                sb.raids.shards(),
                sb.nf.count()
            )));
            let card = nightfall::card(week);
            let nf_label = format!(
                "NIGHTFALL {} / {}{}",
                card.mission.id,
                card.burn.name(),
                if card.exposure { " / exposed" } else { "" }
            );
            if sb.nf.cleared(week) {
                rows.push(Row::action("nf", &nf_label).hint("banked this week; it returns at the reset").disabled());
            } else {
                rows.push(Row::action("nf", &nf_label).hint("the burn runs hot for both sides"));
            }
            rows.push(Row::gap());
            rows.push(Row::heading("SORTIES"));
            match sb.camp.next() {
                Some(m) => {
                    let rec = valence::rec_valence(m);
                    let have = sb.valence();
                    // A campaign built on introducing one mechanic per mission
                    // should SAY so on the row that offers it -- that is the
                    // whole shape of the thing (see curriculum.rs), and a
                    // sortie list that hides it is a list of names.
                    let teaches = curriculum::taught_by(m.id);
                    // terse: the console gives ~46 characters and the tail is
                    // the informative part, so the first version lost exactly
                    // the words worth reading ("— teaches tra~")
                    // WHICH FILM YOU ARE IN, not just which number. The row
                    // said "f5", which is a fact the player has no way to
                    // resolve; `saga.rs` knows it is SOLAR. Short title
                    // because the prefix is identical on all nine and the
                    // console has about 46 characters — the overflow test
                    // below this module is what keeps that honest.
                    let film = crate::saga::film(m.film as u32)
                        .map(|f| f.short_title())
                        .unwrap_or("");
                    let label = match teaches.first() {
                        Some(k) => format!("{} ({film}, rec {rec}) +{}", m.id, k.id),
                        None => format!("{} ({film}, rec {rec})", m.id),
                    };
                    // WHO IS IN IT. The row said what the mission is and who
                    // opposes you, never who is present with you — so a beat
                    // that turns on playing as Keelwright and watching Cleave go
                    // read as an anonymous errand until you were in it.
                    let cast: Vec<String> = m
                        .cast
                        .iter()
                        .filter(|(_, p)| p.on_screen())
                        .map(|(id, p)| format!("{id} {}", p.label()))
                        .collect();
                    let r = Row::action(m.id, &label);
                    let who = if cast.is_empty() { String::new() } else { format!("  |  {}", cast.join(", ")) };
                    rows.push(if have < rec {
                        r.hint("LOW VALENCE: this will hurt")
                    } else if let Some(k) = teaches.first() {
                        r.hint(leak(format!("{}  |  {}{who}", m.beat, k.note)))
                    } else {
                        r.hint(leak(format!("{}{who}", m.beat)))
                    });
                    // what the player can already do, so the funnel is visible
                    let known = curriculum::known_after(m).len();
                    rows.push(Row::heading(&format!(
                        "KIT {known}/{} — nothing is taken away",
                        curriculum::CURRICULUM.len()
                    )));
                }
                None => rows.push(Row::heading("the campaign is finished; the wardens hold the rest")),
            }
            for m in MISSIONS.iter().filter(|m| sb.camp.is_cleared(m.id)).take(3) {
                rows.push(Row::action(m.id, &format!("{}  (cleared)", m.id)).hint("fly it again; the save keeps its mark"));
            }
        }
        Station::Armory => {
            rows.push(Row::heading("RAILS"));
            for (r, slot) in
                [(Rail::Kinetic, &sb.gear.kinetic), (Rail::Energy, &sb.gear.energy), (Rail::Power, &sb.gear.power)]
            {
                match slot {
                    Some(it) => rows.push(
                        Row::action(rail_row_id(r), &format!("{:8} {}", r.name(), it.label()))
                            .hint("press to stow back into the bag"),
                    ),
                    None => rows.push(Row::action(rail_row_id(r), &format!("{:8} --", r.name())).disabled()),
                }
            }
            rows.push(Row::gap());
            rows.push(Row::heading(&format!("WORN ({} slots spent)", sb.gear.worn.iter().map(|m| m.slots).sum::<u8>())));
            for m in &sb.gear.worn {
                rows.push(Row::action(m.name, &Item::Armor(m).label()).hint("press to unbolt"));
            }
            rows.push(Row::gap());
            rows.push(Row::heading(&format!("BAG ({} of {})", sb.gear.bag.len(), crate::gear::BAG_CAP)));
            for (i, it) in sb.gear.bag.iter().enumerate() {
                let id = bag_row_id(i);
                let row = Row::action(id, &it.label());
                rows.push(match it {
                    Item::Armor(m) => match crate::chi::worthy(&sb.chi, m) {
                        Err(e) => row.hint(leak(e)),
                        Ok(()) => row.hint("press to wear"),
                    },
                    _ => row.hint("press to arm"),
                });
            }
            // THE HOTBAR, from the room where the weapons live. Nine slots
            // (`hotbar.rs`), pressed to clear; the armed rails send with
            // one press each — arm at the rail, rack it on the bar, and
            // the shell's number keys reach it in the field. The shell
            // refreshes its own copy when the console closes (`close_deck`),
            // so what the armory arranges is what the field sees.
            rows.push(Row::gap());
            rows.push(Row::heading("HOTBAR — press a slot to clear it"));
            for i in 0..crate::hotbar::SLOTS {
                match sb.hotbar.slot(i) {
                    Some(hs) => {
                        let name: String = hs.item.label().chars().take(34).collect();
                        rows.push(Row::action(hb_row_id(i), &format!("[{}] {}", i + 1, name)));
                    }
                    None => rows.push(
                        Row::action(hb_row_id(i), &format!("[{}] --", i + 1)).disabled(),
                    ),
                }
            }
            for (rail, slot) in [
                (Rail::Kinetic, &sb.gear.kinetic),
                (Rail::Energy, &sb.gear.energy),
                (Rail::Power, &sb.gear.power),
            ] {
                if slot.is_some() {
                    rows.push(
                        Row::action(hb_send_id(rail), &format!("{} -> first open slot", rail.name()))
                            .hint("racks the armed item on the bar"),
                    );
                }
            }
        }
        Station::Hold => {
            // MASTER ACCESS OPENS DOORS; this puts something behind them.
            // Weapons and customisation are not gated by the campaign — they
            // are gated by what you OWN — so a tester with every gate open
            // still walks into an empty hold. Offered as a PRESS rather than
            // applied automatically, because a tester with real progress must
            // not have their bag overwritten by turning a debug flag on.
            if sb.unrestricted() {
                rows.push(Row::action("dbg:kit", "STOCK THE BAG (debug kit)"));
                rows.push(Row::gap());
            }
            rows.push(Row::heading("THE CHEST — press a slot to withdraw"));
            for (i, slot) in (0..crate::vault::SLOTS).map(|i| (i, sb.vault.peek(i))) {
                if let Some(it) = slot {
                    rows.push(Row::action(vault_row_id(i), &format!("[{i:2}] {}", it.label())));
                }
            }
            if sb.vault.count() == 0 {
                rows.push(Row::heading("empty; the bag deposits here"));
            }
            rows.push(Row::gap());
            rows.push(Row::heading("BAG — press to deposit"));
            for (i, it) in sb.gear.bag.iter().enumerate() {
                rows.push(Row::action(deposit_row_id(i), &it.label()));
            }
            rows.push(Row::gap());
            rows.push(Row::action("sort", "TIDY THE CHEST").hint("weapons first, then armor, alphabetical"));
        }
        Station::Medbay => {
            // 46 total on the console, minus this prefix
            rows.push(Row::heading(&format!(
                "THE BEARER — chi: {}",
                crate::chi::sheet_short(&sb.chi, 28)
            )));
            rows.push(Row::gap());
            // THE WATCH — every native form is already the Morpher's
            // (`watch.rs`, against Clones.md: "he has mapped them"), so
            // every row below is pressable; a refusal comes back from
            // apply() with the resolve numbers rather than a locked row.
            rows.push(Row::heading(&format!(
                "THE WATCH — {} form, {:.0}% resolve",
                sb.watch.current_name(),
                sb.watch.resolve() * 100.0
            )));
            for i in NATIVE {
                let name = crate::weapons::TABLE[i].name;
                let row = Row::action(watch_row_id(i), name);
                rows.push(if Some(i) == sb.watch.current() {
                    row.hint("held now").disabled()
                } else if i == crate::watch::slot_named("ICE").unwrap() {
                    row.hint("native state; the only form that opens the quantum leap")
                } else {
                    row.hint("press to shift")
                });
            }
            let photon_label = if sb.watch.photon_reached() {
                "PHOTON MAN — reached once, remembered dimly"
            } else {
                "PHOTON MAN — the Watch's ceiling"
            };
            rows.push(
                Row::action("watch:photon", photon_label)
                    .hint("no other form is sufficient; costs nearly everything the Watch has"),
            );
            rows.push(Row::gap());
            rows.push(Row::heading("THE SEVEN VATS"));
            for c in &crate::clones::CLONES {
                rows.push(Row::action(c.name, &format!("{:12} {:8} {:.1}m", c.name, c.element.name(), c.height_m)));
            }
        }
        Station::Wardeck => {
            rows.push(Row::heading(&format!(
                "{}/{} SHARDS — {}",
                sb.raids.shards(),
                BOSSES.len(),
                sb.raids.seal().label()
            )));
            for b in BOSSES {
                let cleared = sb.raids.is_cleared(b.id);
                let label = format!("{:14} t{} {:?}  {}", b.id, b.tier, b.mech, if cleared { "x" } else { " " });
                let row = Row::action(b.id, &label);
                rows.push(if cleared { row.hint("the shard is banked") } else { row.hint(b.arena) });
            }
            // THE ECHO BROWSER: signals the forge wrote (`raidforge`), on
            // file beside the save, flyable through the same flow as the
            // ladder's own and worth nothing to the seal. Ids leak once per
            // rebuild through the same `leak` the hints use — bounded by
            // menu size, per its own comment.
            rows.push(Row::gap());
            rows.push(Row::heading("ECHO SIGNALS — forged, not found"));
            // READY AND HELD, both shown. An echo naming someone this save has
            // not met is not waste to be discarded — generation is the
            // expensive part and eligibility is a cheap function of the
            // campaign log, so it is banked and becomes flyable when the
            // campaign catches up. Listing it greyed says that; dropping it
            // silently would look like the forge losing work.
            let (ready, held) = crate::raidforge::catalog_split(&sb.camp);
            for e in ready {
                rows.push(
                    Row::action(leak(e.id()), &format!("{:24} {} phases", e.name, e.phases.len()))
                        .hint("an echo: flyable, banks nothing"),
                );
            }
            for (e, why) in held {
                let mut r =
                    Row::action(leak(e.id()), &format!("{:24} {} phases", e.name, e.phases.len()))
                        .hint(leak(format!("{why} — it keeps until you have")));
                // dim rather than hidden: "why can I not press this" is better
                // answered than avoided, which is the rule ui.rs already uses
                // for disabled menu rows
                r.enabled = false;
                rows.push(r);
            }
            rows.push(Row::action("forge:storm", "FORGE: a siege under closing weather"));
            rows.push(Row::action("forge:sky", "FORGE: a fight that lives in the air"));
            rows.push(Row::action("forge:deep", "FORGE: an excavation that woke something"));
        }
    }
    let mut m = Menu::new(st.title(), rows);
    m.view = 11;
    m
}

fn hb_row_id(i: usize) -> &'static str {
    const IDS: [&str; 9] = ["hb:0", "hb:1", "hb:2", "hb:3", "hb:4", "hb:5", "hb:6", "hb:7", "hb:8"];
    IDS[i.min(8)]
}
fn hb_send_id(r: Rail) -> &'static str {
    match r {
        Rail::Kinetic => "hbsend:kinetic",
        Rail::Energy => "hbsend:energy",
        Rail::Power => "hbsend:power",
    }
}

fn rail_row_id(r: Rail) -> &'static str {
    match r {
        Rail::Kinetic => "rail:kinetic",
        Rail::Energy => "rail:energy",
        Rail::Power => "rail:power",
    }
}
fn bag_row_id(i: usize) -> &'static str {
    const IDS: [&str; 9] = ["bag:0", "bag:1", "bag:2", "bag:3", "bag:4", "bag:5", "bag:6", "bag:7", "bag:8"];
    IDS[i.min(8)]
}
fn deposit_row_id(i: usize) -> &'static str {
    const IDS: [&str; 9] = ["dep:0", "dep:1", "dep:2", "dep:3", "dep:4", "dep:5", "dep:6", "dep:7", "dep:8"];
    IDS[i.min(8)]
}
fn watch_row_id(i: usize) -> &'static str {
    const IDS: [&str; 11] = [
        "watch:0", "watch:1", "watch:2", "watch:3", "watch:4", "watch:5", "watch:6", "watch:7",
        "watch:8", "watch:9", "watch:10",
    ];
    IDS[i.min(10)]
}
fn vault_row_id(i: usize) -> &'static str {
    // 27 stable ids, generated once — positional, because a chest slot IS
    // its position
    const IDS: [&str; 27] = [
        "v:0", "v:1", "v:2", "v:3", "v:4", "v:5", "v:6", "v:7", "v:8", "v:9", "v:10", "v:11", "v:12",
        "v:13", "v:14", "v:15", "v:16", "v:17", "v:18", "v:19", "v:20", "v:21", "v:22", "v:23", "v:24",
        "v:25", "v:26",
    ];
    IDS[i.min(26)]
}

/// Hints sometimes carry computed text; the menu wants `&'static str`.
/// Leaking a handful of short strings per rebuild is the honest cost of
/// not redesigning `Row` today, and it is bounded by menu size.
fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// Apply a console event to the state. Pure state changes store and
/// report; sorties come back as Orders for the shell to fly.
pub fn apply(st: Station, ev: &Event, sb: &mut Shipboard, week: u64) -> Option<Order> {
    let Event::Pressed(id) = ev else { return None };
    let id = *id;
    match st {
        Station::Wardeck => {
            // the forge and the echoes sit outside the ladder's gate: an
            // echo is not a signal, so nothing about the seal applies
            if let Some(theme) = id.strip_prefix("forge:") {
                let theme = match theme {
                    "storm" => "a siege under closing weather",
                    "sky" => "a fight that lives in the air, nodes and grapples",
                    "deep" => "an excavation that woke something old",
                    other => other,
                };
                return Some(Order::Forge(theme.to_string()));
            }
            if id.starts_with("echo:") {
                return Some(Order::Echo(id));
            }
            // THE CAMPAIGN GATES FIRST. The ladder below decides WHICH
            // warden; this decides whether the Wardeck is open at all. A save
            // that has not finished Film 1 could previously walk in and start
            // the cooperative endgame as its second experience — the ladder
            // has never had an opinion about that, because its job is
            // ordering signals, not readiness.
            if !sb.unrestricted() && !crate::raidcon::unlocked(&sb.camp) {
                return Some(Order::Did(crate::raidcon::locked_note()));
            }
            // THE LADDER GATES. `next_up` is the lowest-tier unrecovered
            // warden; everything above it is a signal you can read and not
            // yet answer, and a banked shard is not fought twice. Pressing
            // the eligible one is the whole point of the console — it now
            // emits the order instead of being a list.
            let b = BOSSES.iter().find(|b| b.id == id)?;
            if sb.raids.is_cleared(b.id) {
                return Some(Order::Did(format!("{} — the shard is banked", b.name)));
            }
            return match sb.raids.next_up() {
                Some(next) if next.id == b.id => Some(Order::Raid(b.id)),
                _ => Some(Order::Did(format!(
                    "{} — the ladder holds; answer the lower signal first",
                    b.name
                ))),
            };
        }
        Station::Starmap => {
            use crate::patrol::ROUTE;
            let Some(d) = ROUTE.iter().find(|d| d.id == id) else { return None };
            if !gate_open_for(d.id, sb) {
                return Some(Order::Did(format!("{} — no route yet.", d.name)));
            }
            // AND NOW IT FLIES IT. The chain the campaign has always had in
            // pieces and never in sequence:
            //
            //     patrol::Destination -> mapsim::ground_for -> drop::Drop
            //
            // Resolved in simulation, not rendered — which is the same
            // boundary the missions already sit behind, and honest about it.
            let g = crate::mapsim::ground_for(d);
            if is_dock(d) {
                return Some(Order::Did(format!(
                    "{} — {:?} at {:.2}g. nothing to fall toward: you dock, you \
                     do not land.",
                    d.name, d.terrain, d.gravity
                )));
            }
            let land = fly_the_drop(d, &g);
            let verdict = if land.miss_distance < 40.0 {
                format!("down at {:.0} m from the mark", land.miss_distance)
            } else {
                format!("SCATTERED — {:.0} m wide", land.miss_distance)
            };
            let line = format!(
                "{} {:.2}g: {} at {:.0} m/s{}",
                d.name,
                d.gravity,
                verdict,
                land.impact_speed,
                if land.hero { ", clean fold" } else { "" }
            );
            // and actually GO. Plotting a course that changes nothing is a
            // line of text; the shell loads the map because the console
            // cannot.
            Some(Order::Depart { dest: d.id, line })
        }
        Station::Chartroom => {
            // Going through what you brought back IS this room's job, so a
            // press on an unexamined crate records it. A press on a NODE is
            // still read-only: the map is derived from the evidence and is
            // not somewhere you can write.
            if let Some(f) = unexamined(sb).into_iter().find(|f| f.id == id) {
                let before = assay_with(
                    archive::CLAIMS,
                    archive::FINDINGS,
                    sb.dig.ids(),
                    &archive::priors(sb.anchor),
                );
                sb.dig.record(f.id);
                let after = assay_with(
                    archive::CLAIMS,
                    archive::FINDINGS,
                    sb.dig.ids(),
                    &archive::priors(sb.anchor),
                );
                sb.store();
                let moved: Vec<&str> = archive::CLAIMS
                    .iter()
                    .filter(|c| {
                        before[c.id].standing_with(sb.ladder)
                            != after[c.id].standing_with(sb.ladder)
                    })
                    .map(|c| c.id)
                    .collect();
                return Some(Order::Did(if moved.is_empty() {
                    format!("{} — logged. nothing on the map moves.", f.id)
                } else {
                    format!("{} — logged. {} moves.", f.id, moved.join(", "))
                }));
            }
            // a reading surface for the map itself: pressing a node shows
            // its citations.
            let c = archive::claim(id)?;
            let a = assay_with(
                archive::CLAIMS,
                archive::FINDINGS,
                sb.dig.ids(),
                &archive::priors(sb.anchor),
            );
            let asy = a.get(c.id)?;
            let mut out = format!("{} — {}", asy.standing_with(sb.ladder).label(), c.text);
            for f in archive::FINDINGS.iter().filter(|f| sb.dig.has(f.id)) {
                for l in f.links.iter().filter(|l| l.claim == c.id) {
                    let verb = match l.bearing {
                        Bearing::Supports => "for",
                        Bearing::Contradicts => "against",
                        Bearing::Mentions => "names",
                    };
                    out.push_str(&format!("\n  {verb}: {} (via {})", f.id, f.source));
                }
            }
            if asy.support() > 0 && asy.opposition() > 0 {
                out.push_str("\n  these do not average. somebody is wrong, or lying.");
            }
            Some(Order::Did(out))
        }
        Station::Bridge => {
            if id == "nf" {
                return Some(Order::Nightfall(week));
            }
            if MISSIONS.iter().any(|m| m.id == id) {
                return Some(Order::Sortie(id));
            }
            None
        }
        Station::Armory => {
            // hotbar traffic first: it edits `sb.hotbar`, not the gear
            if let Some(rest) = id.strip_prefix("hb:") {
                let i = rest.parse::<usize>().ok()?;
                return Some(match sb.hotbar.clear(i) {
                    Some(it) => {
                        sb.store();
                        Order::Did(format!("slot {} cleared — {} stays armed", i + 1, it.label()))
                    }
                    None => Order::Did(format!("slot {} is already empty", i + 1)),
                });
            }
            if let Some(rail) = id.strip_prefix("hbsend:") {
                let item = match rail {
                    "kinetic" => sb.gear.kinetic.clone(),
                    "energy" => sb.gear.energy.clone(),
                    "power" => sb.gear.power.clone(),
                    _ => None,
                };
                let Some(item) = item else {
                    return Some(Order::Refused(format!("nothing armed on the {rail} rail")));
                };
                let open = (0..crate::hotbar::SLOTS).find(|i| sb.hotbar.slot(*i).is_none());
                return Some(match open {
                    Some(i) => match sb.hotbar.assign(i, item) {
                        Ok(()) => {
                            sb.store();
                            Order::Did(format!("racked on slot {}", i + 1))
                        }
                        Err(e) => Order::Refused(e),
                    },
                    None => Order::Refused("the bar is full — clear a slot first".into()),
                });
            }
            let out = if let Some(rest) = id.strip_prefix("bag:") {
                rest.parse::<usize>().ok().map(|i| sb.gear.equip(i, &sb.chi))
            } else if let Some(rail) = id.strip_prefix("rail:") {
                Some(sb.gear.stow(rail))
            } else if sb.gear.worn.iter().any(|m| m.name == id) {
                Some(sb.gear.stow(id))
            } else {
                None
            }?;
            Some(match out {
                Ok(msg) => {
                    sb.store();
                    Order::Did(msg)
                }
                Err(e) => Order::Refused(e),
            })
        }
        Station::Hold => {
            if id == "dbg:kit" {
                if !sb.unrestricted() {
                    return Some(Order::Did("no".into()));
                }
                let kit = crate::gear::debug_kit();
                let n = kit.len();
                sb.gear.bag = kit;
                return Some(Order::Did(format!("{n} items in the bag — debug kit")));
            }
            let out = if let Some(rest) = id.strip_prefix("v:") {
                rest.parse::<usize>().ok().map(|i| {
                    let mut v = sb.vault.clone();
                    let r = v.withdraw(&mut sb.gear, i);
                    sb.vault = v;
                    r
                })
            } else if let Some(rest) = id.strip_prefix("dep:") {
                rest.parse::<usize>().ok().map(|i| {
                    let mut v = sb.vault.clone();
                    let r = v.deposit(&mut sb.gear, i);
                    sb.vault = v;
                    r
                })
            } else if id == "sort" {
                let mut v = sb.vault.clone();
                v.sort();
                sb.vault = v;
                Some(Ok("the chest is tidied".to_string()))
            } else {
                None
            }?;
            Some(match out {
                Ok(msg) => {
                    sb.store();
                    Order::Did(msg)
                }
                Err(e) => Order::Refused(e),
            })
        }
        Station::Medbay => {
            // A console press is never the "no other form is enough"
            // moment the treatment describes for Photon Man, so this
            // path always declares desperate=false and PHOTON always
            // refuses through it — matching watch.rs's own rule rather
            // than special-casing the id. A true hold gesture is the
            // seam: menufx already has `Motion::hold`, unbound from any
            // shell key today; that binding is where "desperate" should
            // come from once it exists.
            let out = if id == "watch:photon" {
                Some(sb.watch.shift(PHOTON, false))
            } else {
                id.strip_prefix("watch:")
                    .and_then(|rest| rest.parse::<usize>().ok())
                    .map(|i| sb.watch.shift(i, false))
            }?;
            Some(match out {
                Ok(()) => {
                    sb.store();
                    Order::Did(format!("the Watch holds {} now", sb.watch.current_name()))
                }
                Err(e) => Order::Refused(e),
            })
        }
    }
}

/// Which mission an order means, and under what modifiers.
///
/// Split out from `run_order` because that one loads and stores the save,
/// which makes it untestable in a suite that runs in one working directory.
/// This half is a pure lookup and carries the whole of the mapping — an
/// order naming a mission that does not exist is the failure worth catching,
/// and it is catchable here.
fn order_target(
    order: &Order,
) -> Option<(&'static crate::content::Mission, Option<crate::nightfall::Nightfall>)> {
    let (id, card) = match order {
        Order::Sortie(id) => (*id, None),
        Order::Nightfall(week) => {
            let c = crate::nightfall::card(*week);
            (c.mission.id, Some(c))
        }
        // Did / Refused / Goto never reach the shell: `deck::on_event`
        // consumes them itself. Depart DOES reach the shell and is handled
        // before this — it loads a map, which needs the gpu. Reaching here
        // with one means the shell forgot to intercept it, and returning
        // None is the right answer: better a departure that does nothing
        // than a mission run for a destination.
        _ => return None,
    };
    let m = crate::content::MISSIONS.iter().find(|m| m.id == id)?;
    Some((m, card))
}

/// Execute a station order, and say what happened.
///
/// These were built and thrown away: `deck` handles `Did`, `Refused` and
/// `Goto` itself, and the two that escaped — a sortie and the week's
/// nightfall — were toasted with "fly it from the terminal sims for now".
/// The console offered a mission and pressing it did nothing, which is the
/// wiring debt `docs/PLAN.md` describes, in one line of the shell.
///
/// **What this does NOT claim.** The mission is RESOLVED by
/// `campaign::MissionRun` — the same deterministic resolver `--campaign`
/// runs — not played. That is a real outcome that persists, and it is not an
/// action mission; the toast says so rather than letting a resolved clear
/// look like a flown one. Flying them is the renderer's job and is still
/// ahead.
///
/// Returns the line to show and whether the save changed.
///
/// The DECISION half lives in `order_target` so it can be tested without
/// touching the save file; this function is the effect.
/// Record a cleared mission (and its nightfall week, when it was one) on
/// the save. THE ONE PERSISTENCE PATH: the resolved sortie below and the
/// flown one (`sortie.rs`) both land here, so the two cannot drift into
/// writing different lines. Its own load-modify-store rather than taking a
/// loaded save, because the flown path commits minutes after it last read
/// the file and must not clobber whatever else moved in between.
///
/// What a clear banked besides the board entry: the lines a debrief or a sim prints. Every
/// reward lives HERE, on the one persistence path, because until c21 it did not: `campsim`
/// and `raidsim` banked chi and the drops themselves, through their own load-modify-store,
/// and the flown sortie and raid -- the game -- recorded the mission or the shard and nothing
/// else. A nightfall flown and won trained no chi and paid no kit; a warden beaten on the
/// field dropped no arm. Only the CLI sims ever rewarded anyone.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Banked {
    /// The clear was not on the board before (a repeat banks nothing and drops nothing).
    pub fresh: bool,
    /// Where the drop went, if there was one: "drop: <label> -> bag" and the like.
    pub drop: Option<String>,
    /// Chi banked: element, points, level before, level after.
    pub chi: Option<(crate::weapons::Element, u32, u8, u8)>,
}

impl Banked {
    /// The reward lines, for a toast or a sim transcript (empty on a repeat).
    pub fn lines(&self) -> Vec<String> {
        let mut v = Vec::new();
        if let Some(d) = &self.drop {
            v.push(d.clone());
        }
        if let Some((e, pts, before, after)) = self.chi {
            if after > before {
                v.push(format!("chi: {} +{pts} -- LEVEL {after}", e.name()));
            } else {
                v.push(format!("chi: {} +{pts}", e.name()));
            }
        }
        v
    }
}

/// Put `drop` in the bag, else the ship's vault, else say it was left on the field.
fn place_drop(sv: &mut crate::save::Save, drop: crate::gear::Item) -> String {
    use crate::gear::Gear;
    use crate::vault::Vault;
    let mut gear = Gear::from_save(&sv.armed, &sv.worn, &sv.bag);
    let mut vault = Vault::from_line(&sv.vault);
    let label = drop.label();
    let placed = match gear.take(drop.clone()) {
        Ok(()) => format!("drop: {label} -> bag"),
        Err(_) => match vault.deposit(&mut { let mut g = Gear::default(); g.bag.push(drop); g }, 0) {
            Ok(_) => format!("drop: {label} -> the ship's vault (bag full)"),
            Err(_) => format!("drop: {label} left on the field (bag and vault full)"),
        },
    };
    let (armed, worn, bag) = gear.save_lines();
    sv.armed = armed;
    sv.worn = worn;
    sv.bag = bag;
    sv.vault = vault.line();
    placed
}

/// Bank `pts` of `e` on the save's chi log; (element, points, level before, level after).
fn bank_chi(sv: &mut crate::save::Save, e: crate::weapons::Element, pts: u32) -> (crate::weapons::Element, u32, u8, u8) {
    let mut chi = crate::chi::Chi::from_line(&sv.chi);
    let before = chi.level(e);
    chi.bank(e, pts);
    let after = chi.level(e);
    sv.chi = chi.line();
    (e, pts, before, after)
}

/// Returns what was banked (`fresh`, and on a fresh NIGHTFALL the burn's chi and its kit --
/// a plain mission's reward is the board itself: it opens the next one).
pub fn record_outcome(mission_id: &str, nightfall_week: Option<u64>) -> Banked {
    let (mut sv, binds, cfg) = crate::save::load();
    let mut camp = crate::campaign::CampaignLog::from_line(&sv.missions);
    let mut out = Banked { fresh: camp.record(mission_id), ..Banked::default() };
    sv.missions = camp.line();
    if let Some(w) = nightfall_week {
        let mut nf = crate::nightfall::NightfallLog::from_line(&sv.nightfalls);
        let fresh_week = nf.record(w);
        sv.nightfalls = nf.line();
        if fresh_week {
            // the burn trains the bearer, and some weeks it pays kit
            let card = crate::nightfall::card(w);
            out.chi = Some(bank_chi(&mut sv, card.burn, crate::chi::PER_NIGHTFALL));
            out.drop = crate::gear::nightfall_drop(card.burn).map(|d| place_drop(&mut sv, d));
            out.fresh = true;
        }
    }
    crate::save::store(&sv, &binds, &cfg);
    out
}

/// Record a recovered warden shard on the save. The raid twin of
/// `record_outcome` and under the same discipline: its own
/// load-modify-store, one path for both the flown raid and the sim.
/// A fresh shard drops the warden's arm into the bag (vault, field);
/// `suited` -- a human-suited team walked the arena -- banks the chi the
/// arena trains as well. Returns what was banked.
pub fn record_raid_outcome(boss_id: &str, suited: bool) -> Banked {
    let (mut sv, binds, cfg) = crate::save::load();
    let mut raids = crate::raidlog::RaidLog::from_line(&sv.wardens);
    let fresh = raids.record(boss_id);
    sv.wardens = raids.line();
    let mut out = Banked { fresh, ..Banked::default() };
    if fresh {
        if let Some(boss) = crate::raidcon::boss(boss_id) {
            out.drop = Some(place_drop(&mut sv, crate::gear::warden_drop(boss)));
            if suited {
                out.chi = Some(bank_chi(&mut sv, crate::chi::arena_element(boss.arena), crate::chi::PER_SUITED_RAID));
            }
        }
    }
    crate::save::store(&sv, &binds, &cfg);
    out
}

/// Accumulated hull wear, read for the next voyage's briefing.
pub fn ship_wear() -> f32 {
    crate::save::load().0.shipwear
}

/// Charge wear to the hull, capped. The cap is a GUESS sized so a run of
/// careless LZ sorties makes the pass genuinely ugly without becoming
/// unflyable — the ratchet needs a ceiling because the only release is
/// yard time, and a save that spirals past recoverable is a corrupted
/// save wearing a mechanic's clothes.
pub fn record_ship_wear(delta: f32) {
    if delta <= 0.0 {
        return;
    }
    let (mut sv, binds, cfg) = crate::save::load();
    sv.shipwear = (sv.shipwear + delta).clamp(0.0, 8.0);
    crate::save::store(&sv, &binds, &cfg);
}

/// Yard time: berthing at a dock squares the hull away. Returns what was
/// cleared, so the shell can say something only when there was something
/// to say.
pub fn clear_ship_wear() -> f32 {
    let (mut sv, binds, cfg) = crate::save::load();
    let was = sv.shipwear;
    if was > 0.0 {
        sv.shipwear = 0.0;
        crate::save::store(&sv, &binds, &cfg);
    }
    was
}

pub fn run_order(order: &Order) -> (String, bool) {
    use crate::campaign::{CampaignLog, MissionRun, DEFAULT_DOCTRINE, DEFAULT_SQUAD};
    let (sv, _binds, _cfg) = crate::save::load();
    let camp = CampaignLog::from_line(&sv.missions);
    let raids = crate::raidlog::RaidLog::from_line(&sv.wardens);
    let nf = crate::nightfall::NightfallLog::from_line(&sv.nightfalls);
    let have = crate::valence::valence(&camp, &raids, nf.count());

    let Some((m, card)) = order_target(order) else {
        return (format!("{order:?}"), false);
    };

    // THE DROP COMES FIRST. A sortie used to go straight from the Bridge to a
    // resolved result — "resolved, not flown" was the honest label and there
    // was nothing to fly. There is now: the mission has a `dest`, `patrol` has
    // its physics, and `drop.rs` flies the descent.
    //
    // A scattered landing costs you: the squad arrives spread and starts the
    // engagement worse. That is the first thing in this game where getting
    // somewhere badly has a consequence at the far end.
    let arrival = arrive_at(m.dest);

    let squad: Vec<&str> = DEFAULT_SQUAD.to_vec();
    let mut run = MissionRun::new(m, &squad, DEFAULT_DOCTRINE, have);
    // SPEND THE PENALTY. `power` is the multiplier the whole engagement runs
    // on, resolved once against the mission's recommended valence — which is
    // exactly where arriving badly belongs. A cost that costs nothing is
    // decoration, and this is the first consequence in the game for getting
    // somewhere badly.
    if arrival.penalty > 0.0 {
        run = run.arrived_badly(arrival.penalty);
    }
    if let Some(c) = &card {
        run = run.with_modifiers(Some(c.burn), c.exposure);
    }
    // cap the resolve so a Survive objective cannot hang the frame
    let won = run.resolve(600.0);

    if !won {
        return (
            format!("{} — {} lost. the squad did not hold", m.id, arrival.line),
            false,
        );
    }
    let banked = record_outcome(m.id, card.as_ref().map(|c| c.week));
    let what = if card.is_some() { "nightfall" } else { "sortie" };
    let rewards = banked.lines().join(" · ");
    (
        format!(
            "{} — {} {what} CLEARED{}{}{rewards}",
            m.id,
            arrival.line,
            if banked.fresh { "" } else { "; already on the board" },
            if rewards.is_empty() { "" } else { " · " },
        ),
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::{Host, Input};
    use crate::weapons::{Element, Verb};

    /// The flown raid and the flown nightfall reward through the same path the sims do: a
    /// fresh shard drops the warden's arm into the bag (and banks the arena's chi for a suited
    /// team); a fresh nightfall banks the burn's chi and its kit; a repeat banks nothing.
    #[test]
    fn a_fresh_clear_banks_its_drop_and_chi_on_the_save_and_a_repeat_banks_nothing() {
        let dir = crate::save::sandbox_this_thread();
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let boss = &crate::raidcon::BOSSES[0];
        let first = record_raid_outcome(boss.id, true);
        assert!(first.fresh);
        let drop = first.drop.as_deref().expect("a fresh shard drops the warden's arm");
        assert!(drop.contains("-> bag"), "{drop}");
        let e = crate::chi::arena_element(boss.arena);
        assert_eq!(first.chi.map(|c| (c.0, c.1)), Some((e, crate::chi::PER_SUITED_RAID)));
        assert_eq!(first.lines().len(), 2, "{:?}", first.lines());
        // on disk: the arm is in the bag, the chi on the log, the shard on the board
        let (sv, _, _) = crate::save::load();
        let gear = crate::gear::Gear::from_save(&sv.armed, &sv.worn, &sv.bag);
        assert!(gear.bag.iter().any(|it| it.label() == crate::gear::warden_drop(boss).label()), "bag: {:?}", sv.bag);
        assert_eq!(crate::chi::Chi::from_line(&sv.chi).points(e), crate::chi::PER_SUITED_RAID);
        assert!(crate::raidlog::RaidLog::from_line(&sv.wardens).is_cleared(boss.id));
        // the repeat
        let again = record_raid_outcome(boss.id, true);
        assert_eq!(again, Banked { fresh: false, drop: None, chi: None });
        assert!(again.lines().is_empty());
        // a machine-bodied team (the flown raid) gets the arm and no chi
        let boss2 = &crate::raidcon::BOSSES[1];
        let machine = record_raid_outcome(boss2.id, false);
        assert!(machine.fresh && machine.drop.is_some() && machine.chi.is_none());

        // the nightfall: the week's burn trains the bearer, and the week is on the board
        let week = 3;
        let card = crate::nightfall::card(week);
        let nf = record_outcome(card.mission.id, Some(week));
        assert!(nf.fresh);
        assert_eq!(nf.chi.map(|c| (c.0, c.1)), Some((card.burn, crate::chi::PER_NIGHTFALL)));
        assert_eq!(nf.drop.is_some(), crate::gear::nightfall_drop(card.burn).is_some());
        let (sv, _, _) = crate::save::load();
        assert!(crate::nightfall::NightfallLog::from_line(&sv.nightfalls).cleared(week));
        let repeat = record_outcome(card.mission.id, Some(week));
        assert!(!repeat.fresh && repeat.chi.is_none() && repeat.drop.is_none(), "{repeat:?}");
        // a plain mission's reward is the board
        let plain = record_outcome(crate::content::MISSIONS[1].id, None);
        assert!(plain.fresh && plain.lines().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_chartroom_is_bare_until_something_is_carried_home() {
        let mut sb = board();
        sb.anchor = 0.0; // FRESH: assume nothing
        let m = build(Station::Chartroom, &sb, 0);
        let text: String = m.rows.iter().map(|r| r.label.clone()).collect::<Vec<_>>().join("\n");
        assert!(text.contains("bare"), "{text}");
        let bare_rows = m.rows.len();

        // two independent sources for one claim, and a node appears
        sb.dig.record("bunker_alloy");
        sb.dig.record("museum_specimen");
        let m2 = build(Station::Chartroom, &sb, 0);
        assert!(m2.rows.len() > bare_rows, "the table did not grow");
        let text2: String =
            m2.rows.iter().map(|r| r.label.clone()).collect::<Vec<_>>().join("\n");
        assert!(text2.contains("possible"), "{text2}");
        // both are carry-home artifacts, so both reach the lamp
        assert!(text2.contains("2 home"), "{text2}");

        // THE ROWS MUST BE TELLABLE APART. The first version of this console
        // put the standing in the label and left the claim id out, so every
        // node rendered as the same sentence. A test that only checked for
        // the standing text passed happily through that.
        let node_rows: Vec<&String> =
            m2.rows.iter().filter(|r| archive::claim(r.id).is_some()).map(|r| &r.label).collect();
        assert!(node_rows.len() >= 2, "expected several nodes, got {node_rows:?}");
        for c in archive::CLAIMS {
            if let Some(row) = m2.rows.iter().find(|r| r.id == c.id) {
                assert!(
                    row.label.contains(c.id),
                    "row for {} does not name it: {:?}",
                    c.id,
                    row.label
                );
            }
        }
        let uniq: std::collections::BTreeSet<&String> = node_rows.iter().copied().collect();
        assert_eq!(uniq.len(), node_rows.len(), "two node rows read identically");
    }

    #[test]
    fn shipboard_writes_back_everything_it_loaded() {
        // `finds` was loaded into Shipboard and left out of `store` — the
        // exact bug `save::tests` now guards at its own layer. This is the
        // same check one layer up: every line Shipboard OWNS must survive a
        // store, or a console's work vanishes on the next load.
        let src = include_str!("stations.rs");
        let load_body = src
            .split("pub fn load() -> Shipboard {")
            .nth(1)
            .and_then(|s| s.split("\n    }").next())
            .expect("Shipboard::load");
        let store_body = src
            .split("pub fn store(&self) {")
            .nth(1)
            .and_then(|s| s.split("\n    }").next())
            .expect("Shipboard::store");

        // every `sv.<field>` load reads must have a matching store write
        for line in load_body.lines() {
            let Some(rest) = line.split("&sv.").nth(1) else { continue };
            let field: String =
                rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            if field.is_empty() {
                continue;
            }
            assert!(
                store_body.contains(&format!("sv.{field} =")),
                "Shipboard loads sv.{field} and never writes it back"
            );
        }
    }

    #[test]
    fn the_campaign_runs_in_sequence() {
        // PRIORITIES.md item 1 was "one mission end to end, badly". This is
        // the whole board: each sortie flies to its own destination and the
        // next one becomes available, which is the thing that had never been
        // asked to happen in sequence.
        //
        // Verified by driving the log directly rather than the console, so it
        // does not depend on menu navigation.
        let mut camp = crate::campaign::CampaignLog::from_line("");
        let mut flown = Vec::new();
        for _ in 0..6 {
            let Some(m) = camp.next() else { break };
            let a = arrive_at(m.dest);
            assert!(!a.line.is_empty(), "{} did not fly anywhere", m.id);
            flown.push((m.id, m.dest));
            camp.record(m.id);
        }
        assert!(flown.len() >= 5, "the board stalled after {}", flown.len());
        // and they are not all the same place — the drop is reading the
        // mission rather than a constant
        let mut worlds: Vec<&str> = flown.iter().map(|(_, d)| *d).collect();
        worlds.sort_unstable();
        worlds.dedup();
        assert!(worlds.len() >= 3, "five missions visited {worlds:?}");
    }

    #[test]
    fn a_sortie_flies_to_its_destination_before_it_resolves() {
        // "resolved, not flown" was the honest label when there was nothing
        // to fly. The mission names a dest, patrol knows its physics, and
        // drop.rs flies the fall — so the report says where it landed.
        for m in crate::content::MISSIONS.iter().take(6) {
            let a = arrive_at(m.dest);
            assert!(
                !a.line.is_empty(),
                "{} goes to '{}' and the drop said nothing",
                m.id,
                m.dest
            );
            assert!((0.0..=0.35).contains(&a.penalty), "{}: {}", m.id, a.penalty);
        }
    }

    #[test]
    fn every_mission_goes_somewhere_the_route_knows() {
        // a mission whose dest is not on the route silently skips the drop,
        // which would look like the feature being off rather than the data
        // being wrong
        for m in crate::content::MISSIONS {
            assert!(
                crate::patrol::ROUTE.iter().any(|d| d.id == m.dest),
                "{} goes to '{}', which is not a destination",
                m.id,
                m.dest
            );
        }
    }

    #[test]
    fn arriving_badly_costs_the_engagement() {
        // a cost that costs nothing is decoration. `power` is the multiplier
        // the whole fight runs on and `arrived_badly` is the only named way
        // to move it after it is resolved.
        use crate::campaign::{MissionRun, DEFAULT_DOCTRINE, DEFAULT_SQUAD};
        let m = &crate::content::MISSIONS[0];
        let squad: Vec<&str> = DEFAULT_SQUAD.to_vec();
        let clean = MissionRun::new(m, &squad, DEFAULT_DOCTRINE, 40);
        let rough = MissionRun::new(m, &squad, DEFAULT_DOCTRINE, 40).arrived_badly(0.3);
        // resolve both from the same start and the weakened one should not do
        // better
        let mut a = clean;
        let mut b = rough;
        let (wa, wb) = (a.resolve(600.0), b.resolve(600.0));
        assert!(wa || !wb, "arriving scattered helped");
        // and the loss is clamped so a bad landing is a setback, not a loss
        let floored = MissionRun::new(m, &squad, DEFAULT_DOCTRINE, 40).arrived_badly(5.0);
        let mut f = floored;
        let _ = f.resolve(600.0); // must not panic or hang
    }

    #[test]
    fn every_destination_resolves_to_a_real_map_and_they_are_not_all_one() {
        // The starmap loads a map now, so a destination that resolves to a
        // style index off the end is a crash on arrival — and one that
        // resolves them all to MEADOW is a starmap where every world looks
        // the same, which is worse than no starmap.
        use std::collections::BTreeSet;
        let mut seen = BTreeSet::new();
        for d in crate::patrol::ROUTE.iter() {
            let i = crate::mapsim::style_for(d);
            assert!(
                i < crate::world::STYLE_COUNT,
                "{} resolves to style {i} of {}",
                d.id,
                crate::world::STYLE_COUNT
            );
            seen.insert(i);
        }
        assert!(seen.len() >= 4, "eighteen destinations share {} maps", seen.len());
    }

    #[test]
    fn docking_costs_nothing_because_there_is_no_drop() {
        let a = arrive_at("outer_rim");
        assert_eq!(a.penalty, 0.0);
        assert!(a.line.contains("docked"), "{}", a.line);
        // and an unknown destination is silent rather than free-with-a-line
        let none = arrive_at("nowhere_at_all");
        assert!(none.line.is_empty());
        assert_eq!(none.penalty, 0.0);
    }

    #[test]
    fn the_starmap_makes_patrol_reachable_from_inside_the_game() {
        // `docs/PRIORITIES.md` counted zero references to `patrol` in
        // `main.rs`. Stations ARE in the game loop, so this is the smallest
        // honest fix — and a test that the destinations show up at all,
        // because "it exists and is tested" was already true and was not
        // enough.
        let sb = board();
        let m = build(Station::Starmap, &sb, 0);
        let labels: Vec<&str> = m.rows.iter().map(|r| r.label.as_str()).collect();
        assert!(
            labels.iter().any(|l| l.contains("EARTH")),
            "the starmap does not list Earth: {labels:?}"
        );
        assert!(
            m.rows.len() >= crate::patrol::ROUTE.len(),
            "only {} rows for {} destinations",
            m.rows.len(),
            crate::patrol::ROUTE.len()
        );
    }

    #[test]
    fn gravity_actually_changes_the_landing() {
        // The chain is only worth having if the destination's physics reach
        // the outcome. A low-gravity world should arrive slower; if every
        // world lands identically then `ground_for` is decorative and the
        // three modules are still three modules.
        use crate::patrol::ROUTE;
        let fly = |id: &str| {
            let d = ROUTE.iter().find(|d| d.id == id).expect(id);
            let g = crate::mapsim::ground_for(d);
            (d.gravity, fly_the_drop(d, &g))
        };
        let (g_earth, earth) = fly("earth");
        let (g_luna, luna) = fly("luna");
        assert!(g_luna < g_earth * 0.5, "the fixture worlds are too alike");
        assert!(
            luna.impact_speed < earth.impact_speed,
            "a sixth of a g arrived at {:.0} m/s against Earth's {:.0}",
            luna.impact_speed,
            earth.impact_speed
        );
        // and everywhere with a surface lands somewhere real
        for d in ROUTE.iter().filter(|d| !is_dock(d)) {
            let l = fly_the_drop(d, &crate::mapsim::ground_for(d));
            assert!(l.miss_distance.is_finite(), "{} never landed at all", d.id);
            assert!(l.impact_speed >= 0.0 && l.impact_speed < 400.0, "{}: {:?}", d.id, l);
        }
    }

    #[test]
    fn deep_space_is_a_dock_not_a_landing() {
        // Three destinations have no gravity to fall toward. Forcing a drop
        // there loops until the timeout and reports an infinite miss, which
        // is how this was found — the route table is right and my assumption
        // that every destination has a surface was not.
        use crate::patrol::ROUTE;
        let docks: Vec<&str> =
            ROUTE.iter().filter(|d| is_dock(d)).map(|d| d.id).collect();
        assert!(docks.len() >= 2, "expected deep-space destinations: {docks:?}");
        assert!(docks.contains(&"outer_rim"));

        let mut sb = board();
        sb.camp.record("the_scavenger_king");
        let out = apply(Station::Starmap, &Event::Pressed("outer_rim"), &mut sb, 0);
        let Some(Order::Did(line)) = out else { panic!("no report") };
        assert!(line.contains("dock"), "it tried to land on deep space: {line}");
        assert!(!line.contains("m/s"), "it reported an impact speed: {line}");
    }

    #[test]
    fn the_same_course_flies_the_same_way_twice() {
        // a console reporting a different outcome each press is telling you
        // about its RNG rather than about the world
        let d = crate::patrol::ROUTE.iter().find(|d| d.id == "earth").unwrap();
        let g = crate::mapsim::ground_for(d);
        let a = fly_the_drop(d, &g);
        let b = fly_the_drop(d, &g);
        assert_eq!(a.impact_speed, b.impact_speed);
        assert_eq!(a.miss_distance, b.miss_distance);
    }

    #[test]
    fn unreachable_places_are_greyed_rather_than_hidden() {
        // menu.rs's own note: "why can't I press this" is a better experience
        // than a row that is not there, and a starmap with holes in it is a
        // worse map
        let sb = board();
        let m = build(Station::Starmap, &sb, 0);
        // count by ID rather than by a substring of the label — "g " also
        // matches the "N of M plotted" heading, which is how this first read
        // nineteen destinations out of eighteen
        let ids: Vec<&str> = m.rows.iter().map(|r| r.id).filter(|i| !i.is_empty()).collect();
        let shown = crate::patrol::ROUTE.iter().filter(|d| ids.contains(&d.id)).count();
        let pressable = m.rows.iter().filter(|r| r.enabled).count();
        assert_eq!(shown, crate::patrol::ROUTE.len(), "some destinations are hidden");
        assert!(pressable < shown, "everywhere is reachable from a fresh save");
        assert!(pressable >= 1, "nowhere is reachable at all");
    }

    #[test]
    fn clearing_a_mission_opens_its_destination() {
        let mut sb = board();
        let before = build(Station::Starmap, &sb, 0)
            .rows
            .iter()
            .filter(|r| r.enabled)
            .count();
        // pluto is gated behind the mission that reaches it
        sb.camp.record("the_pluto_line");
        let after = build(Station::Starmap, &sb, 0)
            .rows
            .iter()
            .filter(|r| r.enabled)
            .count();
        assert!(after > before, "clearing a mission opened nothing: {before} -> {after}");
    }

    #[test]
    fn the_starmap_flies_the_drop_and_reports_the_landing() {
        // This test used to assert the console said "not wired yet", and it
        // FAILED when the drop was connected — which is the assertion working.
        // A claim about what a feature does not do has to be revisited when it
        // starts doing it, and a test is the only thing that makes that
        // happen on purpose rather than by somebody noticing.
        // Now returns `Depart` rather than `Did`, because plotting a course
        // that changes nothing is a line of text — the shell loads the map.
        // This test asserted `Did` and failed when that landed, which is the
        // assertion working: a claim about what a console RETURNS has to be
        // revisited when the console starts doing more.
        let mut sb = board();
        let out = apply(Station::Starmap, &Event::Pressed("earth"), &mut sb, 0);
        let Some(Order::Depart { dest, line }) = out else { panic!("no departure: {out:?}") };
        assert_eq!(dest, "earth");
        assert!(line.contains("EARTH"), "{line}");
        assert!(line.contains("m/s"), "no impact speed: {line}");
        assert!(
            line.contains("from the mark") || line.contains("SCATTERED"),
            "no landing verdict: {line}"
        );

        // and an unreachable one refuses rather than departing
        let out = apply(Station::Starmap, &Event::Pressed("pluto"), &mut sb, 0);
        let Some(Order::Did(line)) = out else { panic!("it departed for nowhere: {out:?}") };
        assert!(line.contains("no route"), "{line}");
    }

    #[test]
    fn you_can_only_examine_evidence_from_somewhere_you_have_been() {
        // the gate that stops the Chartroom being a button that hands over
        // the whole history
        let mut sb = board();
        assert!(unexamined(&sb).is_empty(), "a crew that has flown nothing has nothing to examine");

        sb.camp.record("the_beach_opens"); // the beach, the ridge, the farmhouse
        let crated = unexamined(&sb);
        assert!(!crated.is_empty(), "clearing an earth mission should open something");
        assert!(crated.iter().all(|f| f.site == "earth"), "evidence leaked from another world");

        // AND NOT ALL OF EARTH. This assertion is the one that was missing:
        // the gate was destination-level, so one mission opened all seventeen
        // Earth findings including an observatory it never went near, and a
        // test that only checked "something opened" could not see it.
        use crate::groundside;
        let all_earth = crate::archive::FINDINGS.iter().filter(|f| f.site == "earth").count();
        assert!(
            crated.len() < all_earth,
            "one mission opened every one of the {all_earth} earth findings"
        );
        for f in &crated {
            let p = groundside::place_of(f.id).expect("an earth finding has an address");
            assert!(
                p.reached_by.contains(&"the_beach_opens"),
                "{} is at {}, which the beach mission does not reach",
                f.id,
                p.id
            );
        }
        // and nothing gated stays visible
        assert!(
            crated.iter().all(|f| f.gate.is_none()),
            "a sealed finding showed up before its gate opened"
        );
    }

    #[test]
    fn examining_a_crate_records_it_and_moves_the_map() {
        let mut sb = board();
        sb.camp.record("the_beach_opens");
        let f = unexamined(&sb)[0];
        let n = sb.dig.count();

        let out = apply(Station::Chartroom, &Event::Pressed(f.id), &mut sb, 0);
        let Some(Order::Did(line)) = out else { panic!("expected a report, got {out:?}") };
        assert!(line.contains(f.id), "{line}");
        assert_eq!(sb.dig.count(), n + 1, "the crate was not recorded");
        assert!(sb.dig.has(f.id));
        // and it is gone from the pile
        assert!(!unexamined(&sb).iter().any(|x| x.id == f.id));
    }

    #[test]
    fn pressing_a_chartroom_NODE_is_still_read_only() {
        let mut sb = board();
        sb.dig.record("directorate_record");
        sb.dig.record("trench_excavation_scar");
        let before = sb.dig.line();
        let out = apply(Station::Chartroom, &Event::Pressed("facility_built_2003"), &mut sb, 0);
        let Some(Order::Did(line)) = out else { panic!("expected a report, got {out:?}") };
        assert!(line.contains("SOURCES DISAGREE"), "{line}");
        // the map is DERIVED from the evidence; it is not somewhere you write
        assert!(line.contains("for: directorate_record"), "{line}");
        assert!(line.contains("against: trench_excavation_scar"), "{line}");
        assert_eq!(sb.dig.line(), before, "the chartroom edited the dig");
    }

    #[test]
    fn every_station_is_in_a_room_that_exists_and_can_be_walked_to() {
        use crate::content::HUB;
        use crate::interior::Interior;
        // the floor plan is generated for a real hull; every console must be
        // in a cell of it, and every cell must be reachable from the bridge —
        // a station behind a door with no path is a menu nobody can open.
        let mut plan = Interior::generate(90.0);
        plan.unseal(); // the ship at rest, which is when consoles are used
        let reachable = plan.reachable_from("bridge");
        for st in STATIONS {
            let c = st.cell();
            assert!(HUB.iter().any(|r| r.id == c), "{} is in unknown room {c}", st.id());
            assert!(plan.cell(c).is_some(), "{} is in a room the plan did not carve: {c}", st.id());
            assert!(
                reachable.contains(&c),
                "{} sits in {c}, which cannot be reached from the bridge",
                st.id()
            );
        }
    }

    #[test]
    fn two_stations_do_not_share_a_console() {
        // six consoles, six rooms. if two ever collide the room prose and the
        // address disagree and one of them is decoration.
        let mut cells: Vec<&str> = STATIONS.iter().map(|s| s.cell()).collect();
        cells.sort_unstable();
        let n = cells.len();
        cells.dedup();
        assert_eq!(cells.len(), n, "two stations share a room");
    }

    #[test]
    fn the_bridge_offers_an_order_the_shell_can_actually_run() {
        // the end-to-end contract, minus the I/O: the Bridge builds a row,
        // `apply` turns pressing it into an Order, and `order_target`
        // resolves that Order to a mission. Those are three tables that have
        // to agree, and for a while they did not have to — the shell threw
        // every order away, so a disagreement was invisible.
        let sb = board();
        let menu = build(Station::Bridge, &sb, 3);
        let mut ran = 0;
        for row in &menu.rows {
            let Some(order) = apply(Station::Bridge, &Event::Pressed(row.id), &mut board(), 3)
            else {
                continue;
            };
            if matches!(order, Order::Sortie(_) | Order::Nightfall(_)) {
                assert!(
                    super::order_target(&order).is_some(),
                    "the Bridge offers {:?}, which resolves to no mission",
                    order
                );
                ran += 1;
            }
        }
        assert!(ran >= 2, "the Bridge should offer at least a sortie and a nightfall, got {ran}");
    }

    #[test]
    fn no_console_row_overflows_the_screen() {
        // The console truncates at `menusim::LABEL_W` and the tail is always
        // the informative part, so an overlong label loses exactly the words
        // worth reading — the Bridge shipped "— teaches tra~" and the
        // Chartroom's heading lost its anchor. Nothing told an author the
        // budget existed; now something does.
        use crate::menusim::LABEL_W;
        // BOTH STATES. The first version of this used only `board()`, whose
        // chi is populated — and `chi::sheet` returns a short summary when
        // there is chi and a long sentence when there is none, so the medbay
        // heading overflowed on a FRESH SAVE and this test never saw it.
        // Testing the state that passes is not testing.
        let mut dug = board();
        for f in ["bunker_alloy", "museum_specimen", "directorate_record", "trench_excavation_scar"] {
            dug.dig.record(f);
        }
        let mut fresh = board();
        fresh.chi = Chi::default();
        fresh.gear = Gear::default();

        for (what, sb) in [("a played save", &dug), ("a fresh save", &fresh)] {
            for st in STATIONS {
                for row in build(st, sb, 3).rows {
                    assert!(
                        row.label.chars().count() <= LABEL_W,
                        "{} row {:?} is {} chars on {what}, over the {LABEL_W} the console shows",
                        st.id(),
                        row.label,
                        row.label.chars().count()
                    );
                }
            }
        }
    }

    #[test]
    fn the_chartroom_is_a_real_station_like_the_others() {
        let st = Station::Chartroom;
        assert!(STATIONS.contains(&st));
        assert!(!st.id().is_empty() && !st.title().is_empty() && !st.room().is_empty());
        // ids stay unique, or the console router picks the wrong room
        let ids: std::collections::BTreeSet<_> = STATIONS.iter().map(|s| s.id()).collect();
        assert_eq!(ids.len(), STATIONS.len());
    }

    /// The test above catches a DUPLICATE in `STATIONS` (two entries with
    /// the same id) but not an OMISSION -- a Station variant that exists
    /// but was never added to the array, which is a different, quieter bug:
    /// the console router, the room layout, everything keyed off `STATIONS`
    /// simply never finds out that station exists. Same shape as
    /// `input.rs`'s `every_action_variant_is_in_the_actions_list_exactly_
    /// once` and this session's `weapons.rs`/`gear.rs` equivalents: scan the
    /// enum's own declared variants and check each is in `STATIONS` exactly
    /// once, so a duplicate-and-omit at the same length can't hide either.
    #[test]
    fn every_station_variant_is_in_stations_exactly_once() {
        let src = include_str!("stations.rs");
        let decl = src
            .split("pub enum Station {")
            .nth(1)
            .and_then(|s| s.split("\n}").next())
            .expect("the Station enum");
        let variants: Vec<&str> = decl
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty() && !l.starts_with("//") && !l.starts_with("///"))
            .filter_map(|l| l.strip_suffix(','))
            .collect();
        assert!(variants.len() >= 6, "only found {} variants; the scan broke", variants.len());
        assert_eq!(variants.len(), STATIONS.len(), "Station enum and STATIONS have different lengths");
        for v in &variants {
            let count = STATIONS.iter().filter(|s| format!("{s:?}") == *v).count();
            assert_eq!(count, 1, "Station::{v} appears {count} times in STATIONS, not exactly once");
        }
    }

    fn board() -> Shipboard {
        // `apply` stores on several events; without this the store lands in
        // the crate root as a real blackstar.save (see `save::sandbox_this_thread`).
        crate::save::sandbox_this_thread();
        Shipboard {
            camp: CampaignLog::default(),
            raids: RaidLog::default(),
            nf: NightfallLog::default(),
            chi: Chi::from_line("rock:15,fire:8"),
            gear: Gear::default(),
            vault: Vault::default(),
            watch: Watch::default(),
            hotbar: Hotbar::default(),
            dig: Dig::default(),
            anchor: 0.45,
            ladder: Ladder::default(),
            debug_ship: false,
        }
    }

    #[test]
    fn every_station_builds_and_has_a_character() {
        let sb = board();
        let mut styles = std::collections::HashSet::new();
        for st in STATIONS {
            let m = build(st, &sb, 7);
            assert!(!m.rows.is_empty(), "{} built nothing", st.id());
            assert!(!st.room().is_empty());
            styles.insert(format!("{:?}", st.style()));
        }
        assert!(styles.len() >= 3, "the stations all deploy the same way");
    }

    #[test]
    fn stations_obey_the_hull() {
        for st in STATIONS {
            assert!(st.available(Phase::Folding).is_err(), "{} open mid-fold", st.id());
            assert!(st.available(Phase::Cruise).is_ok());
        }
        // the nav console is an orbit thing; the armory is not
        assert!(Station::Bridge.available(Phase::Docked).is_err());
        assert!(Station::Armory.available(Phase::Docked).is_ok());
    }

    #[test]
    fn the_armory_equips_through_the_menu_and_worthiness_speaks() {
        let mut sb = board();
        sb.gear.take(Item::Weapon { verb: Verb::Pierce, element: Element::Ice, from: None }).unwrap();
        let mut host = Host::new();
        host.open(build(Station::Armory, &sb, 0));
        // the first bag row: navigate by id, not position
        let rows = &host.top().unwrap().rows;
        let bag_at = rows.iter().position(|r| r.id == "bag:0").unwrap();
        let ev = host.input(Input::Click(bag_at)).expect("pressing a bag row did nothing");
        match apply(Station::Armory, &ev, &mut sb, 0) {
            Some(Order::Did(msg)) => assert!(msg.contains("kinetic"), "{msg}"),
            other => panic!("{other:?}"),
        }
        assert!(sb.gear.kinetic.is_some(), "the menu press did not actually equip");
        // an unworthy shell refuses OUT LOUD through the same path
        let shell = crate::loadout::MODS.iter().find(|m| m.name == "LEAD-WEAVE SHELL").unwrap();
        sb.chi = Chi::default();
        sb.gear.take(Item::Armor(shell)).unwrap();
        host.open(build(Station::Armory, &sb, 0));
        let rows = &host.top().unwrap().rows;
        let at = rows.iter().position(|r| r.id == "bag:0").unwrap();
        let ev = host.input(Input::Click(at)).unwrap();
        match apply(Station::Armory, &ev, &mut sb, 0) {
            Some(Order::Refused(e)) => assert!(e.contains("unworthy"), "{e}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_hold_moves_cargo_both_ways_and_loses_nothing() {
        let mut sb = board();
        sb.gear.take(Item::Weapon { verb: Verb::Beam, element: Element::Ice, from: None }).unwrap();
        // deposit through the console
        let m = build(Station::Hold, &sb, 0);
        let at = m.rows.iter().position(|r| r.id == "dep:0").unwrap();
        let mut host = Host::new();
        host.open(m);
        let ev = host.input(Input::Click(at)).unwrap();
        assert!(matches!(apply(Station::Hold, &ev, &mut sb, 0), Some(Order::Did(_))));
        assert_eq!(sb.gear.bag.len(), 0);
        assert_eq!(sb.vault.count(), 1);
        // withdraw through a rebuilt console (the row now exists)
        let m = build(Station::Hold, &sb, 0);
        let at = m.rows.iter().position(|r| r.id == "v:0").unwrap();
        host.open(m);
        let ev = host.input(Input::Click(at)).unwrap();
        assert!(matches!(apply(Station::Hold, &ev, &mut sb, 0), Some(Order::Did(_))));
        assert_eq!(sb.gear.bag.len(), 1);
        assert_eq!(sb.vault.count(), 0);
    }

    #[test]
    fn the_medbay_shifts_the_watch_through_the_console_and_photon_refuses_a_press() {
        let mut sb = board();
        let m = build(Station::Medbay, &sb, 0);
        // METAL is a native form and must be pressable with no setup —
        // watch.rs's whole point is that nothing needs to be "found" first
        let metal = crate::watch::slot_named("METAL").unwrap();
        let at = m.rows.iter().position(|r| r.id == watch_row_id(metal)).unwrap();
        let mut host = Host::new();
        host.open(m);
        let ev = host.input(Input::Click(at)).unwrap();
        match apply(Station::Medbay, &ev, &mut sb, 0) {
            Some(Order::Did(msg)) => assert!(msg.contains("METAL"), "{msg}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(sb.watch.current_name(), "METAL");

        // a console press is never a declared last resort, so Photon
        // Man refuses through this path even with a full resolve pool
        sb.watch = Watch::default();
        let m = build(Station::Medbay, &sb, 0);
        let at = m.rows.iter().position(|r| r.id == "watch:photon").unwrap();
        host.open(m);
        let ev = host.input(Input::Click(at)).unwrap();
        match apply(Station::Medbay, &ev, &mut sb, 0) {
            Some(Order::Refused(e)) => assert!(e.contains("nothing else is enough"), "{e}"),
            other => panic!("{other:?}"),
        }
        assert_ne!(sb.watch.current_name(), "PHOTON", "a tap opened the ceiling");
    }

    #[test]
    fn the_armory_racks_an_armed_rail_onto_the_hotbar_and_clears_it() {
        let mut sb = board();
        sb.gear.take(Item::Weapon { verb: Verb::Pierce, element: Element::Ice, from: None }).unwrap();
        sb.gear.equip(0, &sb.chi.clone()).unwrap();
        assert!(sb.gear.kinetic.is_some(), "arming the rail is the precondition");
        // the send row exists only for an occupied rail, and pressing it racks
        let m = build(Station::Armory, &sb, 0);
        assert!(m.rows.iter().any(|r| r.id == "hbsend:kinetic"), "no send row for an armed rail");
        match apply(Station::Armory, &Event::Pressed("hbsend:kinetic"), &mut sb, 0) {
            Some(Order::Did(msg)) => assert!(msg.contains("slot 1"), "{msg}"),
            other => panic!("{other:?}"),
        }
        assert!(sb.hotbar.slot(0).is_some(), "the press did not reach the bar");
        // the rail keeps its item: the bar is quick access, not a move
        assert!(sb.gear.kinetic.is_some(), "racking should not disarm the rail");
        // pressing the slot clears it
        match apply(Station::Armory, &Event::Pressed("hb:0"), &mut sb, 0) {
            Some(Order::Did(msg)) => assert!(msg.contains("cleared"), "{msg}"),
            other => panic!("{other:?}"),
        }
        assert!(sb.hotbar.slot(0).is_none());
        // and an empty rail refuses out loud
        match apply(Station::Armory, &Event::Pressed("hbsend:energy"), &mut sb, 0) {
            Some(Order::Refused(e)) => assert!(e.contains("nothing armed"), "{e}"),
            other => panic!("{other:?}"),
        }
    }

    /// A board that has finished Film 1, so the Wardeck is open.
    fn raided() -> Shipboard {
        let mut sb = board();
        sb.camp.record(crate::raidcon::RAIDS_OPEN_AFTER);
        sb
    }

    #[test]
    fn the_sortie_list_names_the_film_not_its_number() {
        // "f5" is a fact the player has no way to resolve. Guarding the CALL
        // SITE, not `saga::film` — testing the function and leaving the
        // console on the old format is the failure this session has made
        // three times (settle, LOD, well_strength).
        let mut sb = board();
        sb.camp.record("the_pluto_line");
        let menu = build(Station::Bridge, &sb, 4);
        let labels: Vec<&str> = menu.rows.iter().map(|r| r.label.as_str()).collect();
        let joined = labels.join(" | ");
        assert!(!joined.is_empty(), "the bridge console has no rows; this test proves nothing");
        // at least one sortie row must carry a film TITLE
        let titled = crate::saga::all_films()
            .iter()
            .any(|f| joined.contains(f.short_title()));
        assert!(titled, "no sortie row names its film: {joined}");
        // and the bare "(fN," form must be gone
        for n in 1..=9 {
            assert!(
                !joined.contains(&format!("(f{n},")),
                "a sortie row still shows the raw film number: {joined}"
            );
        }
    }

    #[test]
    fn every_route_destination_is_classified_by_the_bridge_gate() {
        // ANTI-DRIFT. `beyond_sol` has a catch-all arm, so a destination added
        // later would be silently swept into "beyond" — or, if the arms were
        // ever reordered, into "in system", which opens it without a bridge.
        // Asserting every ROUTE id is named explicitly is what stops the gate
        // rotting the first time someone adds a world.
        use crate::patrol::{beyond_sol, ROUTE};
        let named = [
            "earth", "mars", "luna", "europa", "io", "titan", "pluto", "outer_rim",
            "glaciem", "carrion", "helix", "foundry", "uncharted", "shield_edge",
            "trappist1e", "trappist1g", "proxima_b", "kepler186f",
        ];
        assert!(!ROUTE.is_empty(), "the route table is empty; this test proves nothing");
        for d in ROUTE {
            assert!(
                named.contains(&d.id),
                "{} is not named in beyond_sol — it falls to the catch-all",
                d.id
            );
        }
        // and the split is real in both directions
        assert!(!beyond_sol("earth") && !beyond_sol("pluto"), "Sol is not in Sol");
        assert!(beyond_sol("foundry") && beyond_sol("glaciem"), "the bridge worlds are in Sol");
    }

    #[test]
    fn the_debug_kit_appears_only_under_master_access_and_fits_the_bag() {
        // Access without inventory is not playtestable: weapons and
        // customisation are gated by what you OWN, not by the campaign, so a
        // tester with every gate open still finds an empty hold.
        let mut sb = board();
        let plain = build(Station::Hold, &sb, 4);
        assert!(
            !plain.rows.iter().any(|r| r.id == "dbg:kit"),
            "the debug kit is offered on a normal save"
        );
        // and pressing it anyway does nothing
        assert!(matches!(
            apply(Station::Hold, &Event::Pressed("dbg:kit"), &mut sb, 4),
            Some(Order::Did(_))
        ));
        assert!(sb.gear.bag.is_empty(), "a normal save was stocked");

        sb.debug_ship = true;
        let armed = build(Station::Hold, &sb, 4);
        assert!(
            armed.rows.iter().any(|r| r.id == "dbg:kit"),
            "master access did not offer the kit"
        );
        apply(Station::Hold, &Event::Pressed("dbg:kit"), &mut sb, 4);
        assert!(!sb.gear.bag.is_empty(), "the kit did not arrive");
        assert!(
            sb.gear.bag.len() <= crate::gear::BAG_CAP,
            "the kit overflows the bag ({} > {}) — the tail would be dropped silently",
            sb.gear.bag.len(),
            crate::gear::BAG_CAP
        );
        // a spread, not six of the same thing: the attunement ring needs
        // different elements to be worth testing at all
        let codes: std::collections::HashSet<String> =
            sb.gear.bag.iter().map(|i| i.code()).collect();
        assert_eq!(codes.len(), sb.gear.bag.len(), "the kit contains duplicates");
        let weapons = sb
            .gear
            .bag
            .iter()
            .filter(|i| matches!(i, crate::gear::Item::Weapon { .. }))
            .count();
        assert!(weapons >= 4, "only {weapons} weapons in a weapons playtest kit");
    }

    #[test]
    fn the_master_ship_opens_every_gate_at_once() {
        // One override, three gates. The point of `unrestricted` being a
        // single method is that a tester who turns it on does not then hit
        // the NEXT gate they had not thought about — which is exactly what
        // would happen if each gate carried its own escape hatch.
        let mut sb = board();
        assert!(!sb.unrestricted(), "the master ship is on by default");
        // fresh save: everything shut
        assert!(!gate_open_for("foundry", &sb), "precondition");
        let next = sb.raids.next_up().expect("a ladder").id;
        assert!(
            matches!(apply(Station::Wardeck, &Event::Pressed(next), &mut sb, 4), Some(Order::Did(_))),
            "precondition: the Wardeck should be shut"
        );

        sb.debug_ship = true;
        assert!(gate_open_for("foundry", &sb), "an extra-solar world stayed shut");
        assert!(gate_open_for("trappist1e", &sb), "an exoworld stayed shut");
        assert!(gate_open_for("earth", &sb), "the master ship closed Sol");
        assert_eq!(
            apply(Station::Wardeck, &Event::Pressed(next), &mut sb, 4),
            Some(Order::Raid(next)),
            "the Wardeck stayed shut for the master ship"
        );
    }

    #[test]
    fn the_bridge_stays_dark_until_assay_is_dead() {
        // Reaching a world on a mission is not being able to GO there:
        // leaving Sol needs the bridge, and the bridge was shut because an
        // infestation was loose in the system.
        let mut sb = board();
        // a mission HAS reached the Foundry, which is what used to be enough
        sb.camp.record("beneath_the_pacific");
        assert!(!crate::patrol::bridge_powered(&sb.camp), "the bridge is already powered?");
        assert!(
            !gate_open_for("foundry", &sb),
            "an extra-solar destination opened with the bridge dark"
        );
        // and Sol is unaffected — the gate must not strand the player at home
        sb.camp.record("the_red_extraction");
        assert!(gate_open_for("mars", &sb), "the bridge gate closed an in-system world");
        assert!(gate_open_for("earth", &sb), "Earth is behind the bridge");
    }

    #[test]
    fn killing_assay_powers_it() {
        // The complement: a gate that never opens is not a gate. Both halves
        // are needed — the bridge AND a mission that reached the world.
        let mut sb = board();
        sb.camp.record("beneath_the_pacific");
        sb.camp.record(crate::patrol::BRIDGE_SAFE_AFTER);
        assert!(crate::patrol::bridge_powered(&sb.camp));
        assert!(gate_open_for("foundry", &sb), "Assay is dead and the bridge is still shut");
        // the bridge alone is not a route: nothing has reached Helix
        assert!(
            !gate_open_for("helix", &sb),
            "the bridge opened a world no mission has ever reached"
        );
    }

    #[test]
    fn the_wardeck_is_shut_until_the_beach_is_held() {
        // The story starts small: the campaign through Blackstar is what
        // readies a pilot for the cooperative endgame, and before this gate a
        // fresh save could open raids as its second experience.
        let mut sb = board();
        assert!(!crate::raidcon::unlocked(&sb.camp), "a fresh save is already cleared?");
        let next = sb.raids.next_up().expect("a ladder").id;
        match apply(Station::Wardeck, &Event::Pressed(next), &mut sb, 4) {
            Some(Order::Did(note)) => assert!(
                note.contains("wardeck"),
                "the refusal does not say why: {note}"
            ),
            other => panic!("the Wardeck launched a raid before Film 1: {other:?}"),
        }
    }

    #[test]
    fn finishing_film_one_opens_it() {
        // The complement (CLAUDE.md §5c). A gate that never opens is not a
        // gate, and this is the exact pair the campaign one has to honour.
        let mut sb = raided();
        let next = sb.raids.next_up().expect("a ladder").id;
        assert_eq!(
            apply(Station::Wardeck, &Event::Pressed(next), &mut sb, 4),
            Some(Order::Raid(next)),
            "Film 1 is cleared and the ladder still will not launch"
        );
    }

    #[test]
    fn the_forge_and_echoes_stay_open_before_film_one() {
        // These sit OUTSIDE the ladder's gate by design — an echo is not a
        // signal — so they must sit outside the campaign gate too, or the
        // new check quietly takes away the solo practice content that
        // prepares a player for the thing being gated.
        let mut sb = board();
        assert!(matches!(
            apply(Station::Wardeck, &Event::Pressed("forge:storm"), &mut sb, 4),
            Some(Order::Forge(_))
        ), "the forge was closed by the campaign gate");
        assert!(matches!(
            apply(Station::Wardeck, &Event::Pressed("echo:whatever"), &mut sb, 4),
            Some(Order::Echo(_))
        ), "echoes were closed by the campaign gate");
    }

    #[test]
    fn the_wardeck_launches_only_the_ladders_next_signal() {
        // This tests the LADDER, so it has to get past the campaign gate
        // first. `raided()` is `board()` with Film 1 finished.
        let mut sb = raided();
        let next = sb.raids.next_up().expect("a fresh board has a ladder").id;
        assert_eq!(
            apply(Station::Wardeck, &Event::Pressed(next), &mut sb, 4),
            Some(Order::Raid(next)),
            "the eligible signal must come back as a flyable order"
        );
        // any other boss is a held signal, not an order — the ladder gates
        if let Some(other) = BOSSES.iter().find(|b| b.id != next) {
            assert!(
                matches!(
                    apply(Station::Wardeck, &Event::Pressed(other.id), &mut raided(), 4),
                    Some(Order::Did(_))
                ),
                "a signal above the ladder must be refused with a reason, not launched"
            );
        }
    }

    #[test]
    fn the_bridge_offers_the_road_and_the_week() {
        let sb = board();
        let m = build(Station::Bridge, &sb, 4);
        // the next mission is the first sortie row
        let next = sb.camp.next().unwrap().id;
        assert!(m.rows.iter().any(|r| r.id == next), "the road's next step is not on the bridge");
        let mut host = Host::new();
        host.open(m);
        let find = |host: &Host, id: &str| {
            host.top().unwrap().rows.iter().position(|r| r.id == id).unwrap()
        };
        let at = find(&host, next);
        let ev = host.input(Input::Click(at)).unwrap();
        assert_eq!(apply(Station::Bridge, &ev, &mut board(), 4), Some(Order::Sortie(next)));
        // the nightfall row exists and knows the week
        let at = find(&host, "nf");
        let ev = host.input(Input::Click(at)).unwrap();
        assert_eq!(apply(Station::Bridge, &ev, &mut board(), 4), Some(Order::Nightfall(4)));
    }

    #[test]
    fn row_ids_are_stable_across_rebuilds() {
        // the contract `menu.rs` documents: the caller matches ids, so a
        // rebuild after a state change must keep them
        let mut sb = board();
        sb.gear.take(Item::Weapon { verb: Verb::Flak, element: Element::Fire, from: None }).unwrap();
        sb.gear.take(Item::Weapon { verb: Verb::Beam, element: Element::Ice, from: None }).unwrap();
        let a = build(Station::Armory, &sb, 0);
        sb.gear.equip(0, &sb.chi.clone()).unwrap();
        let b = build(Station::Armory, &sb, 0);
        // "bag:0" exists in both, pointing at whatever is first NOW
        assert!(a.rows.iter().any(|r| r.id == "bag:0"));
        assert!(b.rows.iter().any(|r| r.id == "bag:0"));
        assert!(b.rows.iter().any(|r| r.id == "rail:kinetic"));
    }
}

#[cfg(test)]
mod order_tests {
    use super::*;
    use crate::content;
    use crate::nightfall;

    #[test]
    fn a_sortie_order_names_a_real_mission() {
        // the bug this guards: the Bridge builds rows from MISSIONS and the
        // shell looks them up again by id. If those two ever disagree, the
        // console offers a mission that silently does nothing — which is
        // exactly the state this wiring replaced.
        for m in content::MISSIONS {
            let o = Order::Sortie(m.id);
            let (got, card) = order_target(&o).expect("every mission id must resolve");
            assert_eq!(got.id, m.id);
            assert!(card.is_none(), "a plain sortie should carry no burn");
        }
    }

    #[test]
    fn a_nightfall_order_resolves_to_that_weeks_card() {
        for week in [0u64, 1, 7, 52, 1000] {
            let c = nightfall::card(week);
            let (m, card) = order_target(&Order::Nightfall(week)).expect("card");
            assert_eq!(m.id, c.mission.id);
            let card = card.expect("a nightfall carries its modifiers");
            assert_eq!(card.week, week);
            assert_eq!(card.exposure, c.exposure);
        }
    }

    #[test]
    fn the_orders_the_deck_eats_do_not_reach_the_shell() {
        // Did / Refused / Goto are handled inside deck::on_event. If one
        // arrives here it means that contract moved, and treating it as a
        // mission lookup would be wrong rather than merely useless.
        assert!(order_target(&Order::Did("x".into())).is_none());
        assert!(order_target(&Order::Refused("x".into())).is_none());
        assert!(order_target(&Order::Goto(Station::Hold)).is_none());
    }

    #[test]
    fn an_unknown_mission_id_is_refused_rather_than_panicking() {
        assert!(order_target(&Order::Sortie("not_a_mission")).is_none());
    }
}
