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
use crate::energon;
use crate::gear::{Gear, Item, Rail};
use crate::menu::{Event, Menu, Row, Value};
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
use crate::watch::{self, Watch, NATIVE, PHOTON};

/// The rooms with consoles in them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Station {
    /// sorties, the week's nightfall, the energon ledger
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
}

impl Shipboard {
    pub fn load() -> Shipboard {
        let (sv, _, cfg) = save::load();
        Shipboard {
            anchor: cfg.story.anchor,
            ladder: Ladder::from_cfg(&cfg),
            camp: CampaignLog::from_line(&sv.missions),
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
    pub fn energon(&self) -> u32 {
        energon::energon(&self.camp, &self.raids, self.nf.count())
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

/// Is there a route to this destination yet?
///
/// Uses the same gate vocabulary `archive::gate_open` does — a bare id is a
/// mission, `warden:<id>` a raid clear — so the starmap and the archaeology
/// map cannot disagree about where you have been.
fn gate_open_for(dest: &str, sb: &Shipboard) -> bool {
    // the first two are always open; the rest follow the missions that reach
    // them, which is what `content::MISSIONS` already encodes
    if matches!(dest, "earth" | "outer_rim") {
        return true;
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
                "ENERGON {}  ({}m {}w {}n)",
                sb.energon(),
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
                    let rec = energon::rec_energon(m);
                    let have = sb.energon();
                    // A campaign built on introducing one mechanic per mission
                    // should SAY so on the row that offers it -- that is the
                    // whole shape of the thing (see curriculum.rs), and a
                    // sortie list that hides it is a list of names.
                    let teaches = curriculum::taught_by(m.id);
                    // terse: the console gives ~46 characters and the tail is
                    // the informative part, so the first version lost exactly
                    // the words worth reading ("— teaches tra~")
                    let label = match teaches.first() {
                        Some(k) => format!("{} (f{}, rec {rec}) +{}", m.id, m.film, k.id),
                        None => format!("{} (f{}, rec {rec})", m.id, m.film),
                    };
                    let r = Row::action(m.id, &label);
                    rows.push(if have < rec {
                        r.hint("LOW ENERGON: this will hurt")
                    } else if let Some(k) = teaches.first() {
                        r.hint(leak(format!("{}  |  {}", m.beat, k.note)))
                    } else {
                        r.hint(m.beat)
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
        }
        Station::Hold => {
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
        }
    }
    let mut m = Menu::new(st.title(), rows);
    m.view = 11;
    m
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
        Station::Starmap => {
            use crate::patrol::ROUTE;
            let Some(d) = ROUTE.iter().find(|d| d.id == id) else { return None };
            if !gate_open_for(d.id, sb) {
                return Some(Order::Did(format!("{} — no route yet.", d.name)));
            }
            // HONEST ABOUT WHAT IT DOES. Plotting is not flying, and a console
            // that pretends otherwise is worse than one that says so: the drop
            // physics exist (`drop.rs`), the landing decision exists
            // (`foothold.rs`), and neither is wired to a renderer yet.
            let g = crate::mapsim::ground_for(d);
            Some(Order::Did(format!(
                "course laid for {} — {:.2}g, {:?}, {}. flying it is not wired yet.",
                d.name,
                d.gravity,
                g.material,
                if d.weather { "weather" } else { "still air" }
            )))
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
        Station::Wardeck => None,
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
        // consumes them itself. Reaching here means that changed.
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
pub fn run_order(order: &Order) -> (String, bool) {
    use crate::campaign::{CampaignLog, MissionRun, DEFAULT_DOCTRINE, DEFAULT_SQUAD};
    let (sv, binds, cfg) = crate::save::load();
    let mut camp = CampaignLog::from_line(&sv.missions);
    let raids = crate::raidlog::RaidLog::from_line(&sv.wardens);
    let mut nf = crate::nightfall::NightfallLog::from_line(&sv.nightfalls);
    let have = crate::energon::energon(&camp, &raids, nf.count());

    let Some((m, card)) = order_target(order) else {
        return (format!("{order:?}"), false);
    };

    let squad: Vec<&str> = DEFAULT_SQUAD.to_vec();
    let mut run = MissionRun::new(m, &squad, DEFAULT_DOCTRINE, have);
    if let Some(c) = &card {
        run = run.with_modifiers(Some(c.burn), c.exposure);
    }
    // cap the resolve so a Survive objective cannot hang the frame
    let won = run.resolve(600.0);

    if !won {
        return (format!("{} — lost. the squad did not hold (resolved)", m.id), false);
    }
    let mut sv = sv;
    let fresh = camp.record(m.id);
    sv.missions = camp.line();
    if let Some(c) = &card {
        nf.record(c.week);
        sv.nightfalls = nf.line();
    }
    crate::save::store(&sv, &binds, &cfg);
    let what = if card.is_some() { "nightfall" } else { "sortie" };
    (
        format!(
            "{} — {what} CLEARED (resolved, not flown){}",
            m.id,
            if fresh { "" } else { "; already on the board" }
        ),
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::{Host, Input};
    use crate::weapons::{Element, Verb};

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
    fn plotting_a_course_says_it_is_not_flying_it() {
        // A console that implies the loop is wired when it is not is worse
        // than one that says so. This is the honest half of shipping a
        // half-built feature.
        let mut sb = board();
        let out = apply(Station::Starmap, &Event::Pressed("earth"), &mut sb, 0);
        let Some(Order::Did(line)) = out else { panic!("no report: {out:?}") };
        assert!(line.contains("EARTH"), "{line}");
        assert!(line.contains("not wired"), "it claimed more than it does: {line}");

        // and an unreachable one refuses rather than pretending
        let out = apply(Station::Starmap, &Event::Pressed("pluto"), &mut sb, 0);
        let Some(Order::Did(line)) = out else { panic!("no report") };
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

    fn board() -> Shipboard {
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
