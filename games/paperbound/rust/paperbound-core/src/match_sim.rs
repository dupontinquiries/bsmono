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

pub const ATTACK_COOL: f64 = 0.6; // rts.py:2746 — u.cool = 0.6 after a swing
                                  // (an earlier port slice said 0.9; that
                                  // was a parity bug, caught by re-reading)
pub const AGGRO: f64 = 260.0; //      auto-acquire radius
pub const HARVEST_DWELL: f64 = 1.1; // seconds mining at the well
pub const HAUL: f64 = 10.0; //        base ink per trip (× econ × upgrades)

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Job {
    Idle,
    Seek { x: f64, y: f64 },
    /// rts.py's A-move: advance, but engage anything inside aggro on the way,
    /// then resume the march when the field is clear
    AttackMove { x: f64, y: f64 },
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
    // ── faction-mechanic state (rts.py per-unit attrs) ──
    pub burn: f64,        // seconds of Ember fire left
    pub burn_dps: f64,
    pub momentum: f64,    // Press wind-up 0..1
    pub mom_target: Option<usize>,
    pub combusted: bool,  // an Ember corpse detonates exactly once
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
    /// high-ground plateaus (x, y, r) — the SC ramp defence lives here
    pub plateaus: Vec<(f64, f64, f64)>,
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
            plateaus: Vec::new(),
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
            burn: 0.0,
            burn_dps: 0.0,
            momentum: 0.0,
            mom_target: None,
            combusted: false,
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

    pub fn on_high(&self, x: f64, y: f64) -> bool {
        self.plateaus
            .iter()
            .any(|&(px, py, r)| (x - px).powi(2) + (y - py).powi(2) < r * r)
    }

    /// rts.py _hurt: cohesion soak (−11% per ally within 100px, capped −42%)
    /// then the high-ground defence (×0.7 attacking uphill).
    pub fn hurt_unit(&mut self, ti: usize, mut dmg: f64, attacker: Option<usize>) {
        let (tx, ty, tteam) = {
            let t = &self.units[ti];
            (t.x, t.y, t.team)
        };
        if self.facs[tteam].mechanic.as_deref() == Some("cohesion") {
            let allies = self
                .units
                .iter()
                .enumerate()
                .filter(|(j, a)| {
                    *j != ti
                        && a.team == tteam
                        && a.alive()
                        && ((a.x - tx).powi(2) + (a.y - ty).powi(2)).sqrt() < 100.0
                })
                .count();
            dmg *= 1.0 - (allies as f64 * 0.11).min(0.42);
        }
        if let Some(ai) = attacker {
            let (ax, ay) = (self.units[ai].x, self.units[ai].y);
            if self.on_high(tx, ty) && !self.on_high(ax, ay) {
                dmg *= 0.7;
            }
        }
        self.units[ti].hp -= dmg;
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

        // ── faction passives (rts.py _tick_mechanics) ──
        for i in 0..self.units.len() {
            if !self.units[i].alive() {
                continue;
            }
            // Ember burns chip while their timer runs
            if self.units[i].burn > 0.0 {
                self.units[i].burn -= dt;
                let dps = self.units[i].burn_dps;
                self.units[i].hp -= dps * dt;
                if self.units[i].burn <= 0.0 {
                    self.units[i].burn = 0.0;
                    self.units[i].burn_dps = 0.0;
                }
            }
            // regen: 1.5%/s of maxhp, 4%/s within r+140 of an own complete structure
            let team = self.units[i].team;
            if self.facs[team].mechanic.as_deref() == Some("regen")
                && self.units[i].hp < self.units[i].maxhp
            {
                let (x, y) = (self.units[i].x, self.units[i].y);
                let near_home = self.buildings.iter().any(|b| {
                    b.team == team
                        && b.complete
                        && b.alive()
                        && ((b.x - x).powi(2) + (b.y - y).powi(2)).sqrt() < b.r + 140.0
                });
                let rate = if near_home { 0.04 } else { 0.015 };
                self.units[i].hp =
                    (self.units[i].hp + self.units[i].maxhp * rate * dt).min(self.units[i].maxhp);
            }
        }
        // Ember combustion (rts.py _cull): a dying burn-faction fighter
        // detonates once — area damage + ignition on everything hostile in 70px
        for i in 0..self.units.len() {
            if self.units[i].alive() || self.units[i].combusted {
                continue;
            }
            let team = self.units[i].team;
            if self.facs[team].mechanic.as_deref() != Some("burn") || self.units[i].worker {
                continue;
            }
            self.units[i].combusted = true;
            let (ux, uy, maxhp, udmg) = {
                let u = &self.units[i];
                (u.x, u.y, u.maxhp, u.dmg)
            };
            for j in 0..self.units.len() {
                if j == i || !self.units[j].alive() || self.units[j].team == team {
                    continue;
                }
                let d = ((self.units[j].x - ux).powi(2) + (self.units[j].y - uy).powi(2)).sqrt();
                if d < 70.0 {
                    self.hurt_unit(j, maxhp * 0.12, None);
                    self.units[j].burn = (self.units[j].burn + 1.5).min(4.0);
                    self.units[j].burn_dps = self.units[j].burn_dps.max(udmg * 0.2);
                }
            }
        }

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
                        self.hurt_unit(ei, dmg, None);
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
                Job::AttackMove { x: tx, y: ty } => {
                    if !worker {
                        if let Some(ei) = self.nearest_enemy(team, x, y, AGGRO) {
                            // engage, remembering nothing: when the foe falls
                            // the Fight arm drops to Idle and the commander's
                            // next order (or fresh aggro) picks the unit up —
                            // matching rts.py where A-move re-scans each tick
                            self.units[i].job = Job::Fight { enemy: ei };
                            continue;
                        }
                    }
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
                        // Gilded Press momentum: staying on one target winds up
                        // to +70%; switching keeps some wind-up (−0.4), rts.py
                        let mut mult = 1.0;
                        if self.facs[team].mechanic.as_deref() == Some("momentum") {
                            if self.units[i].mom_target == Some(enemy) {
                                self.units[i].momentum =
                                    (self.units[i].momentum + 0.28).min(1.0);
                            } else {
                                self.units[i].mom_target = Some(enemy);
                                self.units[i].momentum =
                                    (self.units[i].momentum - 0.4).max(0.0);
                            }
                            mult = 1.0 + 0.7 * self.units[i].momentum;
                        }
                        let dmg = self.units[i].dmg * mult;
                        self.hurt_unit(enemy, dmg, Some(i));
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

    #[test]
    fn parity_cohesion_soak() {
        // rts.py _hurt: −11% per ally within 100px, capped at −42%
        let b = book();
        let mut w = World::new(&b["court"], &b["ember"]);
        w.add_building(0, "scriptorium", 0.0, 0.0, true);
        w.add_building(1, "scriptorium", 5000.0, 0.0, true);
        let t = w.add_unit(0, "soldier", 500.0, 500.0);
        for k in 0..2 {
            w.add_unit(0, "soldier", 520.0 + k as f64 * 10.0, 500.0);
        }
        let hp0 = w.units[t].hp;
        w.hurt_unit(t, 100.0, None);
        assert!((hp0 - w.units[t].hp - 100.0 * (1.0 - 0.22)).abs() < 1e-9,
                "2 allies → −22%");
        // pile on 6 allies: capped at 42
        for k in 0..4 {
            w.add_unit(0, "soldier", 460.0 - k as f64 * 8.0, 500.0);
        }
        let hp1 = w.units[t].hp;
        w.hurt_unit(t, 100.0, None);
        assert!((hp1 - w.units[t].hp - 58.0).abs() < 1e-9, "cap at −42%");
        // ember (no cohesion) takes it raw
        let e = w.add_unit(1, "soldier", 500.0, 900.0);
        let hp2 = w.units[e].hp;
        w.hurt_unit(e, 100.0, None);
        assert!((hp2 - w.units[e].hp - 100.0).abs() < 1e-9);
    }

    #[test]
    fn parity_regen_rates() {
        // rts.py: 1.5%/s of maxhp in the field, 4%/s within r+140 of home
        let b = book();
        let mut w = World::new(&b["tide"], &b["court"]);
        w.add_building(0, "scriptorium", 200.0, 200.0, true);
        w.add_building(1, "scriptorium", 5000.0, 5000.0, true);
        let field = w.add_unit(0, "soldier", 2000.0, 2000.0);
        let home = w.add_unit(0, "soldier", 240.0, 220.0);
        for i in [field, home] {
            w.units[i].hp = w.units[i].maxhp * 0.5;
            w.units[i].job = Job::Seek { x: w.units[i].x, y: w.units[i].y };
        }
        let (f0, h0) = (w.units[field].hp, w.units[home].hp);
        for _ in 0..30 {
            w.tick(1.0 / 30.0);
        }
        let fg = w.units[field].hp - f0;
        let hg = w.units[home].hp - h0;
        let max = w.units[field].maxhp;
        assert!((fg - max * 0.015).abs() < max * 0.002, "field ≈1.5%/s: {fg}");
        assert!((hg - max * 0.04).abs() < max * 0.004, "home ≈4%/s: {hg}");
    }

    #[test]
    fn parity_momentum_windup() {
        // rts.py: +0.28 per same-target swing (cap 1.0), −0.4 on switch,
        // damage × (1 + 0.7 × momentum)
        let b = book();
        let mut w = World::new(&b["press"], &b["court"]);
        w.add_building(0, "scriptorium", 0.0, 5000.0, true);
        w.add_building(1, "scriptorium", 5000.0, 5000.0, true);
        let a = w.add_unit(0, "soldier", 100.0, 100.0);
        let v = w.add_unit(1, "colossus", 120.0, 100.0); // fat target survives
        w.units[a].job = Job::Fight { enemy: v };
        // turn the colossus into a training dummy: parked, harmless, vast —
        // so the ramp can be observed over many swings
        w.units[v].job = Job::Seek { x: 120.0, y: 100.0 };
        w.units[v].dmg = 0.0;
        w.units[v].maxhp = 1.0e9;
        w.units[v].hp = 1.0e9;
        let base = w.units[a].dmg;
        let mut deltas = Vec::new();
        let mut last = w.units[v].hp;
        for _ in 0..(4 * 30) {
            w.tick(1.0 / 30.0);
            if w.units[v].hp < last {
                deltas.push(last - w.units[v].hp);
                last = w.units[v].hp;
            }
        }
        assert!(deltas.len() >= 5, "{deltas:?}");
        // swing 1 lands at momentum 0 — a FRESH target routes through the
        // switch branch (rts.py: _mom_target is None ≠ tgt → −0.4 floor 0);
        // each further same-target swing winds +0.28 toward the +70% cap
        assert!((deltas[0] - base).abs() < 1e-4, "{deltas:?}");
        assert!((deltas[1] - base * (1.0 + 0.7 * 0.28)).abs() < 1e-4);
        assert!((deltas[2] - base * (1.0 + 0.7 * 0.56)).abs() < 1e-4);
        let cap = base * 1.7;
        assert!((deltas.last().unwrap() - cap).abs() < 1e-4, "cap at +70%");
        assert!(deltas.windows(2).all(|p| p[1] >= p[0] - 1e-6), "monotone ramp");
    }

    #[test]
    fn parity_ember_combustion_chain() {
        // rts.py _cull: a dying Ember fighter deals maxhp×0.12 in 70px and
        // ignites (burn 1.5s at dmg×0.2); burning then chips over time
        let b = book();
        let mut w = World::new(&b["ember"], &b["court"]);
        w.add_building(0, "scriptorium", 0.0, 5000.0, true);
        w.add_building(1, "scriptorium", 5000.0, 5000.0, true);
        let bomb = w.add_unit(0, "soldier", 100.0, 100.0);
        let near = w.add_unit(1, "soldier", 140.0, 100.0);
        let far = w.add_unit(1, "soldier", 400.0, 100.0);
        // park everyone so nobody closes distance mid-test
        for i in [near, far] {
            w.units[i].job = Job::Seek { x: w.units[i].x, y: w.units[i].y };
        }
        let (maxhp, hp_near0, hp_far0) = (w.units[bomb].maxhp, w.units[near].hp, w.units[far].hp);
        w.units[bomb].hp = -1.0;
        w.tick(1.0 / 30.0);
        assert!(w.units[bomb].combusted);
        let blast = hp_near0 - w.units[near].hp;
        assert!(blast >= maxhp * 0.12 - 1e-6, "blast: {blast}");
        assert!(w.units[near].burn > 0.0, "ignited");
        assert!((hp_far0 - w.units[far].hp).abs() < 1e-6, "out of radius");
        // the burn keeps chipping afterwards
        let hp1 = w.units[near].hp;
        for _ in 0..30 {
            w.tick(1.0 / 30.0);
        }
        assert!(w.units[near].hp < hp1, "burn dps ticked");
        // a second detonation never happens
        let hp2 = w.units[near].hp;
        w.tick(1.0 / 30.0);
        let chip = hp2 - w.units[near].hp;
        assert!(chip < maxhp * 0.12 * 0.5, "no re-detonation");
    }

    #[test]
    fn parity_high_ground_defence() {
        // rts.py: attacking uphill deals ×0.7
        let b = book();
        let mut w = World::new(&b["ember"], &b["ember"]);
        w.add_building(0, "scriptorium", 0.0, 5000.0, true);
        w.add_building(1, "scriptorium", 5000.0, 5000.0, true);
        w.plateaus.push((500.0, 500.0, 120.0));
        let low = w.add_unit(0, "soldier", 700.0, 500.0);
        let high = w.add_unit(1, "soldier", 520.0, 500.0);
        let hp0 = w.units[high].hp;
        let d = 100.0;
        w.hurt_unit(high, d, Some(low));
        assert!((hp0 - w.units[high].hp - 70.0).abs() < 1e-9, "uphill ×0.7");
        // high hitting low is full strength
        let hp1 = w.units[low].hp;
        w.hurt_unit(low, d, Some(high));
        assert!((hp1 - w.units[low].hp - 100.0).abs() < 1e-9);
    }

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
