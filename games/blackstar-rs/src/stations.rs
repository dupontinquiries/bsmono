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
}

pub const STATIONS: [Station; 5] =
    [Station::Bridge, Station::Armory, Station::Hold, Station::Medbay, Station::Wardeck];

impl Station {
    pub fn id(self) -> &'static str {
        match self {
            Station::Bridge => "bridge",
            Station::Armory => "armory",
            Station::Hold => "hold",
            Station::Medbay => "medbay",
            Station::Wardeck => "wardeck",
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Station::Bridge => "THE BRIDGE",
            Station::Armory => "THE ARMORY",
            Station::Hold => "THE HOLD",
            Station::Medbay => "THE MEDBAY",
            Station::Wardeck => "THE WARDECK",
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
        }
    }
    /// Whether the walk to this console is possible right now. Stations
    /// obey the hull: nobody browses the armory during reconfiguration.
    pub fn available(self, phase: Phase) -> Result<(), &'static str> {
        if !phase.interior() {
            return Err("the interior is not walkable right now");
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
}

impl Shipboard {
    pub fn load() -> Shipboard {
        let (sv, _, _) = save::load();
        Shipboard {
            camp: CampaignLog::from_line(&sv.missions),
            raids: RaidLog::from_line(&sv.wardens),
            nf: NightfallLog::from_line(&sv.nightfalls),
            chi: Chi::from_line(&sv.chi),
            gear: Gear::from_save(&sv.armed, &sv.worn, &sv.bag),
            vault: Vault::from_line(&sv.vault),
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

/// Build a station's console from the state.
pub fn build(st: Station, sb: &Shipboard, week: u64) -> Menu {
    let mut rows: Vec<Row> = Vec::new();
    match st {
        Station::Bridge => {
            rows.push(Row::heading(&format!(
                "ENERGON {}   ({} missions, {} wardens, {} nightfalls)",
                sb.energon(),
                sb.camp.done(),
                sb.raids.shards(),
                sb.nf.count()
            )));
            let card = nightfall::card(week);
            let nf_label = format!(
                "NIGHTFALL — {} under a {} burn{}",
                card.mission.id,
                card.burn.name(),
                if card.exposure { ", exposure" } else { "" }
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
                    let label = format!("{}  (film {}, rec {rec})", m.id, m.film);
                    let r = Row::action(m.id, &label);
                    rows.push(if have < rec {
                        r.hint("LOW ENERGON: this will hurt")
                    } else {
                        r.hint(m.beat)
                    });
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
            rows.push(Row::heading(&format!("THE BEARER — chi: {}", crate::chi::sheet(&sb.chi))));
            rows.push(
                Row::action(
                    "shift",
                    if sb.chi.can_shift() { "HOLD THE ICE-CLONE FORM" } else { "THE ICE-CLONE FORM (ice level 3 required)" },
                )
                .hint("the vat remembers the shape even when the body cannot"),
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
        Station::Medbay | Station::Wardeck => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::{Host, Input};
    use crate::weapons::{Element, Verb};

    fn board() -> Shipboard {
        Shipboard {
            camp: CampaignLog::default(),
            raids: RaidLog::default(),
            nf: NightfallLog::default(),
            chi: Chi::from_line("rock:15,fire:8"),
            gear: Gear::default(),
            vault: Vault::default(),
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
