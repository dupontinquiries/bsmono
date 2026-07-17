//! sim.rs — the Foldworks tech & economy layer, ported line-for-rule from
//! rts.py. This is the first slice of the Rust duplication: the PURE logic
//! that decides what a team may build, train, and research, how supply and
//! ink gate production, and how finished upgrades reshape units — the part
//! worth locking down before any rendering or pathing crosses over.
//!
//! Rule sources (rts.py):
//!   _struct_available   → TeamState::struct_available
//!   _unit_available     → TeamState::unit_available
//!   _queue_train        → TeamState::queue_train (same refusal order:
//!                         research gate → supply → ink)
//!   _supply_cap/_used   → supply_cap / supply_used
//!   _start_research /   → start_research / tick_research (retro-applies to
//!   research countdown    living units, preserving the HP fraction)
//!   _apply_upgrades     → bake_upgrades
//!   tower fire profile  → data::StructureSpec::tower_profile

use crate::data::Faction;
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct Building {
    pub kind: String,
    pub complete: bool,
    pub queue: Vec<String>,
    pub research: Option<(String, f64)>, // (upgrade, time_left)
}

impl Building {
    pub fn new(kind: &str, complete: bool) -> Self {
        Self { kind: kind.into(), complete, queue: Vec::new(), research: None }
    }
}

#[derive(Debug, Clone)]
pub struct SimUnit {
    pub kind: String,
    pub worker: bool,
    pub hp: f64,
    pub maxhp: f64,
    pub dmg: f64,
    pub spd: f64,
    pub rng: f64,
    pub supply: i64,
}

/// Why a training request was refused — the same order rts.py checks in.
#[derive(Debug, Clone, PartialEq)]
pub enum Refusal {
    NeedsResearch(String),
    SupplyBlocked,
    NotEnoughInk,
    UnknownUnit,
}

pub struct TeamState<'a> {
    pub fac: &'a Faction,
    pub ink: f64,
    pub buildings: Vec<Building>,
    pub units: Vec<SimUnit>,
    pub upgrades: HashSet<String>,
}

impl<'a> TeamState<'a> {
    pub fn new(fac: &'a Faction) -> Self {
        Self { fac, ink: 0.0, buildings: Vec::new(), units: Vec::new(),
               upgrades: HashSet::new() }
    }

    pub fn spawn_unit(&mut self, kind: &str) -> Option<usize> {
        let spec = self.fac.units.get(kind)?;
        let mut u = SimUnit {
            kind: kind.into(), worker: spec.worker,
            hp: spec.hp, maxhp: spec.hp, dmg: spec.dmg,
            spd: spec.spd, rng: spec.rng, supply: spec.supply,
        };
        self.bake_upgrades(&mut u);
        self.units.push(u);
        Some(self.units.len() - 1)
    }

    // ── supply (rts.py _supply_cap / _supply_used / _supply_free) ──
    pub fn supply_cap(&self) -> i64 {
        let cap: i64 = self
            .buildings
            .iter()
            .filter(|b| b.complete)
            .filter_map(|b| self.fac.structures.get(&b.kind))
            .map(|s| s.supply)
            .sum();
        cap.min(200) // SC-style hard ceiling
    }

    pub fn supply_used(&self) -> i64 {
        let live: i64 = self.units.iter().map(|u| u.supply).sum();
        let queued: i64 = self
            .buildings
            .iter()
            .flat_map(|b| b.queue.iter())
            .filter_map(|uk| self.fac.units.get(uk))
            .map(|s| s.supply)
            .sum();
        live + queued
    }

    pub fn supply_free(&self, kind: &str) -> bool {
        let cost = self.fac.units.get(kind).map(|s| s.supply).unwrap_or(1);
        self.supply_used() + cost <= self.supply_cap()
    }

    // ── availability gates ──
    pub fn struct_available(&self, kind: &str) -> bool {
        let Some(spec) = self.fac.structures.get(kind) else { return false };
        for r in &spec.requires {
            if !self
                .buildings
                .iter()
                .any(|b| b.complete && &b.kind == r)
            {
                return false;
            }
        }
        if let Some(need) = &spec.requires_research {
            if !self.upgrades.contains(need) {
                return false;
            }
        }
        true
    }

    pub fn unit_available(&self, kind: &str) -> bool {
        let Some(spec) = self.fac.units.get(kind) else { return false };
        if let Some(need) = &spec.requires_research {
            if !self.upgrades.contains(need) {
                return false;
            }
        }
        self.buildings.iter().any(|b| {
            b.complete
                && self
                    .fac
                    .structures
                    .get(&b.kind)
                    .map(|s| s.makes.iter().any(|m| m == kind))
                    .unwrap_or(false)
        })
    }

    /// rts.py _queue_train, refusals in the same order it flashes them.
    pub fn queue_train(&mut self, b_idx: usize, kind: &str) -> Result<(), Refusal> {
        let Some(spec) = self.fac.units.get(kind) else {
            return Err(Refusal::UnknownUnit);
        };
        if let Some(need) = &spec.requires_research {
            if !self.upgrades.contains(need) {
                return Err(Refusal::NeedsResearch(need.clone()));
            }
        }
        if !self.supply_free(kind) {
            return Err(Refusal::SupplyBlocked);
        }
        if self.ink < spec.cost as f64 {
            return Err(Refusal::NotEnoughInk);
        }
        self.ink -= spec.cost as f64;
        self.buildings[b_idx].queue.push(kind.to_string());
        Ok(())
    }

    // ── research ──
    pub fn start_research(&mut self, b_idx: usize, upgrade: &str) -> bool {
        let b = &self.buildings[b_idx];
        if !b.complete || b.research.is_some() {
            return false;
        }
        let Some(sspec) = self.fac.structures.get(&b.kind) else { return false };
        if !sspec.researches.iter().any(|r| r == upgrade) {
            return false;
        }
        if self.upgrades.contains(upgrade) {
            return false;
        }
        let Some(up) = self.fac.upgrades.get(upgrade) else { return false };
        if self.ink < up.cost as f64 {
            return false;
        }
        self.ink -= up.cost as f64;
        let t = up.time;
        self.buildings[b_idx].research = Some((upgrade.to_string(), t));
        true
    }

    /// Advance research timers; finished upgrades retro-apply to every living
    /// unit, preserving the HP fraction (rts.py _retro_upgrade).
    pub fn tick_research(&mut self, dt: f64) -> Vec<String> {
        let mut finished = Vec::new();
        for i in 0..self.buildings.len() {
            if let Some((uk, left)) = self.buildings[i].research.clone() {
                let left = left - dt;
                if left <= 0.0 {
                    self.buildings[i].research = None;
                    self.upgrades.insert(uk.clone());
                    self.retro_upgrade(&uk);
                    finished.push(uk);
                } else {
                    self.buildings[i].research = Some((uk, left));
                }
            }
        }
        finished
    }

    fn retro_upgrade(&mut self, uk: &str) {
        let Some(up) = self.fac.upgrades.get(uk) else { return };
        if up.stat == "harvest" {
            return; // worker economy stat: no unit fields to touch
        }
        for u in &mut self.units {
            if !up.hits(&u.kind, u.worker) {
                continue;
            }
            let frac = if u.maxhp > 0.0 { u.hp / u.maxhp } else { 1.0 };
            match up.stat.as_str() {
                "maxhp" => {
                    u.maxhp *= up.mult;
                    u.hp = u.maxhp * frac;
                }
                "dmg" => u.dmg *= up.mult,
                "spd" => u.spd *= up.mult,
                "rng" => u.rng *= up.mult,
                _ => {}
            }
        }
    }

    /// rts.py _apply_upgrades — bake finished research into a fresh unit.
    fn bake_upgrades(&self, u: &mut SimUnit) {
        for uk in &self.upgrades {
            let Some(up) = self.fac.upgrades.get(uk) else { continue };
            if !up.hits(&u.kind, u.worker) {
                continue;
            }
            match up.stat.as_str() {
                "maxhp" => {
                    u.maxhp *= up.mult;
                    u.hp = u.maxhp;
                }
                "dmg" => u.dmg *= up.mult,
                "spd" => u.spd *= up.mult,
                "rng" => u.rng *= up.mult,
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::load_factbook;
    use std::path::Path;

    fn book() -> std::collections::BTreeMap<String, Faction> {
        // the crate lives at games/paperbound/rust/paperbound-core
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        load_factbook(&dir).expect("load real game data")
    }

    #[test]
    fn real_data_loads_all_seven_factions() {
        let b = book();
        for k in ["court", "ember", "tide", "press", "swarm", "windwrit", "rootbound"] {
            assert!(b.contains_key(k), "missing faction {k}");
        }
    }

    // parity facts below were PROVEN in the Python behavioural suites first;
    // the Rust port must reproduce them from the same files.

    #[test]
    fn parity_tower_profiles() {
        let b = book();
        let root = &b["rootbound"].structures["spire"];
        assert_eq!(root.tower_profile("spire"), Some((36.0, 225.0, 1.5)));
        let swarm = &b["swarm"].structures["spire"];
        assert_eq!(swarm.tower_profile("spire"), Some((9.0, 215.0, 0.32)));
        // the classic court spire declares nothing → engine defaults
        let court = &b["court"].structures["spire"];
        assert_eq!(court.tower_profile("spire"), Some((22.0, 240.0, 0.8)));
        // walls never fire
        let hedge = &b["rootbound"].structures["hedge"];
        assert_eq!(hedge.tower_profile("hedge"), None);
    }

    #[test]
    fn parity_supply_caps() {
        let b = book();
        let mut t = TeamState::new(&b["swarm"]);
        t.buildings.push(Building::new("scriptorium", true));
        assert_eq!(t.supply_cap(), 12, "swarm halls breed widest");
        t.buildings.push(Building::new("well", true));
        assert_eq!(t.supply_cap(), 20);
        let mut c = TeamState::new(&b["court"]);
        c.buildings.push(Building::new("scriptorium", true));
        c.buildings.push(Building::new("well", false)); // incomplete: no supply
        assert_eq!(c.supply_cap(), 10);
    }

    #[test]
    fn parity_research_gate_holds_and_releases() {
        let b = book();
        let mut t = TeamState::new(&b["rootbound"]);
        t.ink = 5000.0;
        t.buildings.push(Building::new("scriptorium", true));
        for _ in 0..3 {
            t.buildings.push(Building::new("well", true));
        }
        t.buildings.push(Building::new("bindery", true));
        let bi = t.buildings.len() - 1;
        // gate holds: Warden-Elders demand the Rite of Verdance
        assert_eq!(
            t.queue_train(bi, "wardenelder"),
            Err(Refusal::NeedsResearch("verdance".into()))
        );
        assert!(!t.unit_available("wardenelder"));
        // …and releases
        t.upgrades.insert("verdance".into());
        assert!(t.queue_train(bi, "wardenelder").is_ok());
        assert!(t.unit_available("wardenelder"));
        // structure gate: the Elder Ring sits behind the same rite
        let mut t2 = TeamState::new(&b["rootbound"]);
        t2.buildings.push(Building::new("bindery", true));
        assert!(!t2.struct_available("menagerie"));
        t2.upgrades.insert("verdance".into());
        assert!(t2.struct_available("menagerie"));
    }

    #[test]
    fn parity_refusal_order_matches_python() {
        // rts.py checks research → supply → ink, in that order
        let b = book();
        let mut t = TeamState::new(&b["rootbound"]);
        t.buildings.push(Building::new("bindery", true));
        // no research, no supply, no ink: research wins
        assert!(matches!(t.queue_train(0, "wardenelder"),
                         Err(Refusal::NeedsResearch(_))));
        t.upgrades.insert("verdance".into());
        // research ok, supply 0: supply wins over ink
        assert_eq!(t.queue_train(0, "wardenelder"), Err(Refusal::SupplyBlocked));
        t.buildings.push(Building::new("scriptorium", true));
        t.buildings.push(Building::new("well", true));
        // supply ok, ink 0
        assert_eq!(t.queue_train(0, "wardenelder"), Err(Refusal::NotEnoughInk));
        t.ink = 1000.0;
        assert!(t.queue_train(0, "wardenelder").is_ok());
        assert_eq!(t.ink, 1000.0 - 150.0, "wardenelder costs 150");
    }

    #[test]
    fn parity_farsight_rng_upgrade() {
        // Python suite proved: windwrit archer rng ×1.2 under Farsight,
        // retroactively, preserving the hp fraction on maxhp upgrades.
        let b = book();
        let mut t = TeamState::new(&b["windwrit"]);
        t.spawn_unit("archer");
        let rng0 = t.units[0].rng;
        t.upgrades.insert("farsight".into());
        t.retro_upgrade("farsight");
        assert!((t.units[0].rng - rng0 * 1.2).abs() < 1e-9);
        // freshly baked units come pre-stretched
        t.spawn_unit("archer");
        assert!((t.units[1].rng - rng0 * 1.2).abs() < 1e-9);
        // workers are untouched (applies is a kind list)
        t.spawn_unit("scribe");
        let scribe_rng = b["windwrit"].units["scribe"].rng;
        assert!((t.units[2].rng - scribe_rng).abs() < 1e-9);
    }

    #[test]
    fn research_flow_ticks_and_retro_applies() {
        let b = book();
        let mut t = TeamState::new(&b["windwrit"]);
        t.ink = 1000.0;
        t.buildings.push(Building::new("skychart", true));
        t.spawn_unit("archer");
        let rng0 = t.units[0].rng;
        assert!(t.start_research(0, "farsight"));
        assert_eq!(t.ink, 1000.0 - 120.0);
        // can't double-book the lab
        assert!(!t.start_research(0, "tailwind"));
        let done = t.tick_research(19.9);
        assert!(done.is_empty());
        let done = t.tick_research(0.2);
        assert_eq!(done, vec!["farsight".to_string()]);
        assert!((t.units[0].rng - rng0 * 1.2).abs() < 1e-9);
        // finished research can't be restarted
        assert!(!t.start_research(0, "farsight"));
    }

    #[test]
    fn maxhp_retro_preserves_wound_fraction() {
        let b = book();
        let mut t = TeamState::new(&b["rootbound"]);
        t.spawn_unit("soldier");
        t.units[0].hp = t.units[0].maxhp * 0.5; // half-wounded rootling
        let max0 = t.units[0].maxhp;
        t.upgrades.insert("bark".into()); // +35% army health
        t.retro_upgrade("bark");
        assert!((t.units[0].maxhp - max0 * 1.35).abs() < 1e-9);
        assert!((t.units[0].hp - t.units[0].maxhp * 0.5).abs() < 1e-9);
    }
}
