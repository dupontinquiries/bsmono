//! match_sim.rs — slice 2 of the Rust duplication: the LIVE MATCH loop on top
//! of sim.rs's tech layer. Ported rule-for-rule from rts.py's update path:
//!
//!   worker harvest cycle   → toNode → harvest dwell → toBase → deposit
//!                            (haul × faction econ × harvest upgrades)
//!   movement               → straight-line seek at spd px/s (the Python
//!                            game's local avoidance is cosmetic; the
//!                            authoritative motion is the seek)
//!   combat                 → acquire nearest enemy in aggro, close to rng,
//!                            hit for dmg on a 0.9s swing (rts.py's cool)
//!   production             → construction build timers, then train queues
//!                            (build_t each), spawning beside the building
//!   towers                 → data-driven tower_dmg/rng/cool profiles
//!   victory                → a side loses when its scriptorium is unmade
//!
//! Deterministic: fixed-dt ticks, index-ordered iteration, no RNG.

use crate::data::Faction;
use crate::sim::Refusal;
use std::collections::HashSet;

pub const ATTACK_COOL: f64 = 0.9; // rts.py: u.cool = 0.9 after a swing
pub const AGGRO: f64 = 260.0; //      auto-acquire radius
pub const HARVEST_DWELL: f64 = 1.1; // seconds mining at the well
pub const HAUL: f64 = 10.0; //        base ink per trip (× econ × upgrades)

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Job {
    Idle,
    Seek { x: f64, y: f64 },
    ToNode(usize),
    Harvest { node: usize, t: f64 },
    ToBase(usize),
    Fight { enemy: usize },
}

#[derive(Debug, Clone)]
pub struct MUnit {
    pub team: usize,
    pub kind: String,
    pub x: f64,
    pub y: f64,
    pub hp: f64,
    pub maxhp: f64,
    pub dmg: f64,
    pub spd: f64,
    pub rng: f64,
    pub worker: bool,
    pub supply: i64,
    pub cool: f64,
    pub job: Job,
}

impl MUnit {
    pub fn alive(&self) -> bool {
        self.hp > 0.0
    }
}

#[derive(Debug, Clone)]
pub struct MBuilding {
    pub team: usize,
    pub kind: String,
    pub x: f64,
    pub y: f64,
    pub hp: f64,
    pub maxhp: f64,
    pub r: f64,
    pub complete: bool,
    pub build_left: f64,
    pub queue: Vec<(String, f64)>, // (unit kind, time left)
    pub cool: f64,                 // tower cooldown
}

impl MBuilding {
    pub fn alive(&self) -> bool {
        self.hp > 0.0
    }
}

#[derive(Debug, Clone)]
pub struct Node {
    pub x: f64,
    pub y: f64,
    pub amount: f64,
}

pub struct World<'a> {
    pub facs: [&'a Faction; 2],
    pub ink: [f64; 2],
    pub upgrades: [HashSet<String>; 2],
    pub units: Vec<MUnit>,
    pub buildings: Vec<MBuilding>,
    pub nodes: Vec<Node>,
    pub t: f64,
    pub winner: Option<usize>,
}

impl<'a> World<'a> {
    pub fn new(fac_a: &'a Faction, fac_b: &'a Faction) -> Self {
        World {
            facs: [fac_a, fac_b],
            ink: [120.0, 120.0], // rts.py starting ink
            upgrades: [HashSet::new(), HashSet::new()],
            units: Vec::new(),
            buildings: Vec::new(),
            nodes: Vec::new(),
            t: 0.0,
            winner: None,
        }
    }

    pub fn add_building(&mut self, team: usize, kind: &str, x: f64, y: f64, complete: bool) -> usize {
        let spec = &self.facs[team].structures[kind];
        self.buildings.push(MBuilding {
            team,
            kind: kind.into(),
            x,
            y,
            hp: spec.hp,
            maxhp: spec.hp,
            r: spec.r,
            complete,
            build_left: if complete { 0.0 } else { spec.build },
            queue: Vec::new(),
            cool: 0.0,
        });
        self.buildings.len() - 1
    }

    pub fn add_unit(&mut self, team: usize, kind: &str, x: f64, y: f64) -> usize {
        let spec = &self.facs[team].units[kind];
        let mut u = MUnit {
            team,
            kind: kind.into(),
            x,
            y,
            hp: spec.hp,
            maxhp: spec.hp,
            dmg: spec.dmg,
            spd: spec.spd,
            rng: spec.rng,
            worker: spec.worker,
            supply: spec.supply,
            cool: 0.0,
            job: Job::Idle,
        };
        // bake finished upgrades (sim.rs semantics)
        for uk in &self.upgrades[team] {
            if let Some(up) = self.facs[team].upgrades.get(uk) {
                if up.hits(&u.kind, u.worker) {
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
        self.units.push(u);
        self.units.len() - 1
    }

    /// Queue a unit with sim.rs's refusal semantics (research → supply → ink).
    pub fn queue_train(&mut self, b_idx: usize, kind: &str) -> Result<(), Refusal> {
        let team = self.buildings[b_idx].team;
        let Some(spec) = self.facs[team].units.get(kind) else {
            return Err(Refusal::UnknownUnit);
        };
        if let Some(need) = &spec.requires_research {
            if !self.upgrades[team].contains(need) {
                return Err(Refusal::NeedsResearch(need.clone()));
            }
        }
        let cap: i64 = self
            .buildings
            .iter()
            .filter(|b| b.team == team && b.complete && b.alive())
            .filter_map(|b| self.facs[team].structures.get(&b.kind))
            .map(|s| s.supply)
            .sum::<i64>()
            .min(200);
        let used: i64 = self
            .units
            .iter()
            .filter(|u| u.team == team && u.alive())
            .map(|u| u.supply)
            .sum::<i64>()
            + self
                .buildings
                .iter()
                .filter(|b| b.team == team)
                .flat_map(|b| b.queue.iter())
                .filter_map(|(k, _)| self.facs[team].units.get(k))
                .map(|s| s.supply)
                .sum::<i64>();
        if used + spec.supply > cap {
            return Err(Refusal::SupplyBlocked);
        }
        if self.ink[team] < spec.cost as f64 {
            return Err(Refusal::NotEnoughInk);
        }
        self.ink[team] -= spec.cost as f64;
        let t = spec.build_t;
        self.buildings[b_idx].queue.push((kind.into(), t));
        Ok(())
    }

    fn harvest_mult(&self, team: usize) -> f64 {
        let mut m = 1.0;
        for uk in &self.upgrades[team] {
            if let Some(up) = self.facs[team].upgrades.get(uk) {
                if up.stat == "harvest" {
                    m *= up.mult;
                }
            }
        }
        m
    }

    fn nearest_enemy(&self, team: usize, x: f64, y: f64, within: f64) -> Option<usize> {
        let mut best = None;
        let mut bd = within;
        for (i, u) in self.units.iter().enumerate() {
            if u.team != team && u.alive() {
                let d = ((u.x - x).powi(2) + (u.y - y).powi(2)).sqrt();
                if d < bd {
                    bd = d;
                    best = Some(i);
                }
            }
        }
        best
    }

    fn nearest_base(&self, team: usize, x: f64, y: f64) -> Option<usize> {
        self.buildings
            .iter()
            .enumerate()
            .filter(|(_, b)| b.team == team && b.complete && b.alive() && b.kind == "scriptorium")
            .min_by(|a, b| {
                let da = (a.1.x - x).powi(2) + (a.1.y - y).powi(2);
                let db = (b.1.x - x).powi(2) + (b.1.y - y).powi(2);
                da.partial_cmp(&db).unwrap()
            })
            .map(|(i, _)| i)
    }

    fn nearest_node(&self, x: f64, y: f64) -> Option<usize> {
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.amount > 0.0)
            .min_by(|a, b| {
                let da = (a.1.x - x).powi(2) + (a.1.y - y).powi(2);
                let db = (b.1.x - x).powi(2) + (b.1.y - y).powi(2);
                da.partial_cmp(&db).unwrap()
            })
            .map(|(i, _)| i)
    }

    fn seek(u: &mut MUnit, tx: f64, ty: f64, dt: f64) -> f64 {
        let dx = tx - u.x;
        let dy = ty - u.y;
        let d = (dx * dx + dy * dy).sqrt();
        if d > 1e-6 {
            let step = (u.spd * dt).min(d);
            u.x += dx / d * step;
            u.y += dy / d * step;
        }
        d
    }

    pub fn tick(&mut self, dt: f64) {
        if self.winner.is_some() {
            return;
        }
        self.t += dt;

        // ── construction + training + tower fire ──
        for bi in 0..self.buildings.len() {
            if !self.buildings[bi].alive() {
                continue;
            }
            if !self.buildings[bi].complete {
                self.buildings[bi].build_left -= dt;
                if self.buildings[bi].build_left <= 0.0 {
                    self.buildings[bi].complete = true;
                }
                continue;
            }
            // train the queue head
            let mut spawn: Option<String> = None;
            if let Some((k, left)) = self.buildings[bi].queue.first_mut() {
                *left -= dt;
                if *left <= 0.0 {
                    spawn = Some(k.clone());
                }
            }
            if let Some(k) = spawn {
                self.buildings[bi].queue.remove(0);
                let (bx, by, r, team) = {
                    let b = &self.buildings[bi];
                    (b.x, b.y, b.r, b.team)
                };
                self.add_unit(team, &k, bx + r + 22.0, by);
            }
            // tower fire (data-driven profile; classic spire defaults)
            let (team, kind, bx, by) = {
                let b = &self.buildings[bi];
                (b.team, b.kind.clone(), b.x, b.y)
            };
            if let Some((dmg, rng, cool)) =
                self.facs[team].structures[&kind].tower_profile(&kind)
            {
                self.buildings[bi].cool = (self.buildings[bi].cool - dt).max(0.0);
                if self.buildings[bi].cool <= 0.0 {
                    if let Some(ei) = self.nearest_enemy(team, bx, by, rng) {
                        self.units[ei].hp -= dmg;
                        self.buildings[bi].cool = cool;
                    }
                }
            }
        }

        // ── units ──
        for i in 0..self.units.len() {
            if !self.units[i].alive() {
                continue;
            }
            self.units[i].cool = (self.units[i].cool - dt).max(0.0);
            let (team, worker, x, y) = {
                let u = &self.units[i];
                (u.team, u.worker, u.x, u.y)
            };
            // fighters auto-acquire
            if !worker {
                if let Job::Fight { enemy } = self.units[i].job {
                    if !self.units[enemy].alive() {
                        self.units[i].job = Job::Idle;
                    }
                }
                if matches!(self.units[i].job, Job::Idle) {
                    if let Some(ei) = self.nearest_enemy(team, x, y, AGGRO) {
                        self.units[i].job = Job::Fight { enemy: ei };
                    }
                }
            }
            match self.units[i].job {
                Job::Idle => {
                    // idle workers walk to the nearest live well (rts.py's
                    // auto-harvest on spawn)
                    if worker {
                        if let Some(ni) = self.nearest_node(x, y) {
                            self.units[i].job = Job::ToNode(ni);
                        }
                    }
                }
                Job::Seek { x: tx, y: ty } => {
                    let d = Self::seek(&mut self.units[i], tx, ty, dt);
                    if d < 4.0 {
                        self.units[i].job = Job::Idle;
                    }
                }
                Job::ToNode(ni) => {
                    if self.nodes[ni].amount <= 0.0 {
                        self.units[i].job = Job::Idle;
                    } else {
                        let (nx, ny) = (self.nodes[ni].x, self.nodes[ni].y);
                        let d = Self::seek(&mut self.units[i], nx, ny, dt);
                        if d < 26.0 {
                            self.units[i].job = Job::Harvest { node: ni, t: HARVEST_DWELL };
                        }
                    }
                }
                Job::Harvest { node, t } => {
                    let left = t - dt;
                    if left <= 0.0 {
                        let take = HAUL.min(self.nodes[node].amount);
                        self.nodes[node].amount -= take;
                        if let Some(bi) = self.nearest_base(team, x, y) {
                            self.units[i].job = Job::ToBase(bi);
                        } else {
                            self.units[i].job = Job::Idle;
                        }
                    } else {
                        self.units[i].job = Job::Harvest { node, t: left };
                    }
                }
                Job::ToBase(bi) => {
                    if !self.buildings[bi].alive() {
                        self.units[i].job = Job::Idle;
                    } else {
                        let (bx, by, br) = {
                            let b = &self.buildings[bi];
                            (b.x, b.y, b.r)
                        };
                        let d = Self::seek(&mut self.units[i], bx, by, dt);
                        if d < br + 14.0 {
                            let gain = HAUL * self.facs[team].econ * self.harvest_mult(team);
                            self.ink[team] += gain;
                            if let Some(ni) = self.nearest_node(x, y) {
                                self.units[i].job = Job::ToNode(ni);
                            } else {
                                self.units[i].job = Job::Idle;
                            }
                        }
                    }
                }
                Job::Fight { enemy } => {
                    let (ex, ey) = (self.units[enemy].x, self.units[enemy].y);
                    let d = ((ex - x).powi(2) + (ey - y).powi(2)).sqrt();
                    if d > self.units[i].rng {
                        Self::seek(&mut self.units[i], ex, ey, dt);
                    } else if self.units[i].cool <= 0.0 {
                        let dmg = self.units[i].dmg;
                        self.units[enemy].hp -= dmg;
                        self.units[i].cool = ATTACK_COOL;
                    }
                }
            }
        }

        // ── victory: a side falls with its scriptorium ──
        for side in 0..2 {
            let has_hall = self
                .buildings
                .iter()
                .any(|b| b.team == side && b.kind == "scriptorium" && b.alive());
            if !has_hall {
                self.winner = Some(1 - side);
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
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        load_factbook(&dir).expect("real game data")
    }

    #[test]
    fn worker_economy_loop_accrues_ink() {
        let b = book();
        let mut w = World::new(&b["court"], &b["ember"]);
        w.add_building(0, "scriptorium", 200.0, 200.0, true);
        w.add_building(1, "scriptorium", 5000.0, 5000.0, true);
        w.nodes.push(Node { x: 380.0, y: 200.0, amount: 2000.0 });
        w.add_unit(0, "scribe", 210.0, 210.0);
        let ink0 = w.ink[0];
        for _ in 0..(60 * 30) {
            w.tick(1.0 / 30.0);
        }
        assert!(w.ink[0] > ink0 + 40.0, "one worker minute should net several hauls: {}", w.ink[0]);
        assert!(w.nodes[0].amount < 2000.0, "the well should be drawn down");
        // rootbound's richer econ (1.08) out-earns court under identical setups
        let mut r = World::new(&b["rootbound"], &b["ember"]);
        r.add_building(0, "scriptorium", 200.0, 200.0, true);
        r.add_building(1, "scriptorium", 5000.0, 5000.0, true);
        r.nodes.push(Node { x: 380.0, y: 200.0, amount: 2000.0 });
        r.add_unit(0, "scribe", 210.0, 210.0);
        for _ in 0..(60 * 30) {
            r.tick(1.0 / 30.0);
        }
        // scribes differ in speed, so compare per-haul: rootbound gain/haul is higher
        let court_gain = w.ink[0] - 120.0;
        let root_gain = r.ink[0] - 120.0;
        let court_hauls = (2000.0 - w.nodes[0].amount) / HAUL;
        let root_hauls = (2000.0 - r.nodes[0].amount) / HAUL;
        assert!(root_gain / root_hauls > court_gain / court_hauls, "econ 1.08 should out-earn per trip");
    }

    #[test]
    fn training_pipeline_spawns_beside_the_building() {
        let b = book();
        let mut w = World::new(&b["swarm"], &b["court"]);
        w.add_building(0, "scriptorium", 100.0, 100.0, true); // supply 12
        w.add_building(1, "scriptorium", 5000.0, 5000.0, true);
        let t = w.add_building(0, "table", 260.0, 100.0, true);
        w.ink[0] = 500.0;
        w.queue_train(t, "soldier").unwrap();
        let n0 = w.units.len();
        let build_t = b["swarm"].units["soldier"].build_t;
        let mut steps = 0;
        while w.units.len() == n0 && steps < 10_000 {
            w.tick(1.0 / 30.0);
            steps += 1;
        }
        assert_eq!(w.units.len(), n0 + 1);
        assert!((steps as f64 / 30.0 - build_t).abs() < 0.2, "spawn near build_t");
        let u = w.units.last().unwrap();
        assert!((u.x - (260.0 + w.buildings[t].r + 22.0)).abs() < 1e-6);
    }

    #[test]
    fn combat_resolves_and_swarm_towers_sting() {
        let b = book();
        let mut w = World::new(&b["swarm"], &b["court"]);
        w.add_building(0, "scriptorium", 100.0, 400.0, true);
        w.add_building(1, "scriptorium", 900.0, 400.0, true);
        // a sting needle guards the swarm side
        w.add_building(0, "spire", 300.0, 400.0, true);
        // one court soldier walks into the needle's teeth
        let ci = w.add_unit(1, "soldier", 520.0, 400.0);
        w.units[ci].job = Job::Seek { x: 300.0, y: 400.0 };
        for _ in 0..(20 * 30) {
            w.tick(1.0 / 30.0);
            if !w.units[ci].alive() {
                break;
            }
        }
        assert!(!w.units[ci].alive(), "a lone soldier should die to the tower");
        // and a straight fight: two swarm mites vs one court soldier — the
        // pair wins on numbers (cohesion isn't ported yet; raw stats suffice)
        let mut f = World::new(&b["swarm"], &b["court"]);
        f.add_building(0, "scriptorium", 0.0, 0.0, true);
        f.add_building(1, "scriptorium", 2000.0, 0.0, true);
        f.add_unit(0, "soldier", 480.0, 300.0);
        f.add_unit(0, "soldier", 500.0, 320.0);
        let e = f.add_unit(1, "soldier", 560.0, 300.0);
        for _ in 0..(60 * 30) {
            f.tick(1.0 / 30.0);
            if !f.units[e].alive() {
                break;
            }
        }
        assert!(!f.units[e].alive(), "two mites beat one soldier");
        assert!(f.units.iter().any(|u| u.team == 0 && u.alive()));
    }

    #[test]
    fn victory_when_the_scriptorium_falls() {
        let b = book();
        let mut w = World::new(&b["windwrit"], &b["tide"]);
        w.add_building(0, "scriptorium", 100.0, 100.0, true);
        let hall = w.add_building(1, "scriptorium", 400.0, 100.0, true);
        assert!(w.winner.is_none());
        w.buildings[hall].hp = -1.0;
        w.tick(1.0 / 30.0);
        assert_eq!(w.winner, Some(0));
        // a decided world stops ticking
        let t0 = w.t;
        w.tick(1.0);
        assert_eq!(w.t, t0);
    }

    #[test]
    fn deterministic_replay() {
        let b = book();
        let run = || {
            let mut w = World::new(&b["swarm"], &b["rootbound"]);
            w.add_building(0, "scriptorium", 200.0, 300.0, true);
            w.add_building(1, "scriptorium", 1400.0, 300.0, true);
            w.nodes.push(Node { x: 400.0, y: 300.0, amount: 1500.0 });
            w.nodes.push(Node { x: 1200.0, y: 300.0, amount: 1500.0 });
            w.add_unit(0, "scribe", 220.0, 320.0);
            w.add_unit(1, "scribe", 1380.0, 320.0);
            w.add_unit(0, "soldier", 600.0, 300.0);
            w.add_unit(1, "soldier", 1000.0, 300.0);
            for _ in 0..(45 * 30) {
                w.tick(1.0 / 30.0);
            }
            (
                format!("{:.6},{:.6}", w.ink[0], w.ink[1]),
                w.units.iter().map(|u| format!("{:.4},{:.4},{:.2}", u.x, u.y, u.hp)).collect::<Vec<_>>(),
            )
        };
        assert_eq!(run(), run(), "same setup must replay identically");
    }
}
