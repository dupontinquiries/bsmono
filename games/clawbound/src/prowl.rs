//! prowl.rs — "CLAWBOUND · Prowl", the open-world objective mode, ported from the
//! SMITE-2-inspired browser prototype as deterministic, renderer-free logic.
//!
//! You prowl as a chosen [`crate::class`] creature while a [`crate::hunter`] stalks
//! you. **Objectives** rotate one at a time — Reach a landmark, hold a Rendezvous,
//! Survive the hunt, or Forage (catch prey) — each clearing into the next. You hunt
//! ambient prey ([`crate::ecology`]) by pouncing mid-leap for score, and getting
//! caught respawns you (no permadeath). Landmarks come from [`crate::objectives`].
//! Movement speed is terrain-scaled through the class, so the ground shapes the chase.

use crate::class::Class;
use crate::ecology::Critter;
use crate::hunter::{self, HState, Kind};
use crate::objectives;
use crate::ring::Ring;
use crate::world;

const PROWL_SCALE: f32 = 1.9;      // lift class speeds to open-world chase pace
const SPRINT: f32 = 1.15;
const CATCH_DIST: f32 = 2.3;
const LEAP_DUR: f32 = 0.5;
const LEAP_CD: f32 = 1.8;
const LEAP_SPEED: f32 = 26.0;
const DODGE_DUR: f32 = 0.34;
const DODGE_CD: f32 = 1.5;
const DODGE_SPEED: f32 = 30.0;
const POUNCE_R: f32 = 2.4;
const MAP: f32 = world::SIZE * 0.92;
const REACH_R: f32 = 8.0;
const RENDEZVOUS_R: f32 = 9.0;
const RENDEZVOUS_NEED: f32 = 4.0;
const SURVIVE_LEN: f32 = 45.0;
const FORAGE_NEED: u32 = 4;
const NEXT_DELAY: f32 = 1.8;

fn hashf(mut h: u32) -> f32 {
    h ^= h >> 16; h = h.wrapping_mul(0x7feb352d);
    h ^= h >> 15; h = h.wrapping_mul(0x846ca68b);
    h ^= h >> 16; (h as f32) / (u32::MAX as f32)
}
fn norm(x: f32, z: f32) -> (f32, f32) { let l = (x * x + z * z).sqrt(); if l > 1e-5 { (x / l, z / l) } else { (0.0, 0.0) } }
fn dist(ax: f32, az: f32, bx: f32, bz: f32) -> f32 { ((ax - bx).powi(2) + (az - bz).powi(2)).sqrt() }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProwlObj { Reach, Rendezvous, Survive, Forage }

pub struct Objective {
    pub kind: ProwlObj,
    pub tx: f32, pub tz: f32, pub name: &'static str,
    pub progress: f32,        // rendezvous seconds banked
    pub time_left: f32,       // survive countdown
    pub start_caught: u32,    // forage baseline
    pub done: bool,
    pub next_t: f32,
}

pub struct Prowl {
    pub class: Class,
    pub x: f32, pub z: f32, pub heading: f32,
    pub stamina: f32,
    pub moving: bool, pub crouched: bool, pub hidden: bool,
    pub leap_t: f32, pub leap_cd: f32,
    pub dodge_t: f32, pub dodge_cd: f32, pub bx: f32, pub bz: f32,
    pub hunter_kind: Kind,
    pub hx: f32, pub hz: f32, pub hheading: f32,
    pub hstate: HState, pub suspicion: f32, pub last_seen: Option<(f32, f32)>,
    pub critters: Vec<Critter>,
    pub bushes: Vec<(f32, f32, f32)>,
    pub pois: Vec<(f32, f32, &'static str)>,
    pub ring: Ring,
    pub exposed: bool,       // outside the hunt-ring: cover fails and stamina bleeds
    pub obj: Objective,
    pub cleared: u32,
    pub prey_caught: u32,
    pub escapes: u32,
    pub times_caught: u32,
    pub respawn_flash: f32,
    pub time: f32,
    pub difficulty: f32,
    tick: u32,
    input: Option<(f32, f32, bool, bool, bool)>,   // mx, mz, leap, dodge, crouch
}

impl Prowl {
    pub fn new(class: Class, hunter_kind: Kind) -> Prowl {
        world::init_world();
        let pois: Vec<(f32, f32, &'static str)> = objectives::place().into_iter()
            .map(|o| (o.x, o.z, world::biome_at(o.x, o.z).name)).collect();
        let critters = crate::ecology::populate(400, false);
        let bushes = world::bush_positions(200);
        let mut g = Prowl {
            class, x: 0.0, z: 0.0, heading: 0.0, stamina: 1.0,
            moving: false, crouched: false, hidden: false,
            leap_t: 0.0, leap_cd: 0.0, dodge_t: 0.0, dodge_cd: 0.0, bx: 0.0, bz: 0.0,
            hunter_kind, hx: 60.0, hz: 60.0, hheading: 0.0,
            hstate: HState::Patrol, suspicion: 0.0, last_seen: None,
            critters, bushes, pois,
            ring: Ring::new(0.0, 0.0, world::SIZE * 0.34, world::SIZE * 0.12, 120.0, 0.0),
            exposed: false,
            obj: Objective { kind: ProwlObj::Reach, tx: 0.0, tz: 0.0, name: "", progress: 0.0, time_left: 0.0, start_caught: 0, done: false, next_t: 0.0 },
            cleared: 0, prey_caught: 0, escapes: 0, times_caught: 0, respawn_flash: 0.0, time: 0.0, difficulty: 1.0, tick: 0, input: None,
        };
        g.new_objective(Some(ProwlObj::Reach));
        g
    }

    pub fn set_input(&mut self, mx: f32, mz: f32, leap: bool, dodge: bool, crouch: bool) {
        self.input = Some((mx, mz, leap, dodge, crouch));
    }

    fn a_poi(&self) -> (f32, f32, &'static str) {
        if self.pois.is_empty() { return (0.0, 0.0, "Landmark"); }
        let i = (hashf(self.tick ^ 0xA53F) * self.pois.len() as f32) as usize % self.pois.len();
        self.pois[i]
    }

    fn new_objective(&mut self, force: Option<ProwlObj>) {
        let kind = force.unwrap_or_else(|| {
            match (hashf(self.tick ^ 0x51D0) * 4.0) as u32 {
                0 => ProwlObj::Reach, 1 => ProwlObj::Rendezvous, 2 => ProwlObj::Survive, _ => ProwlObj::Forage,
            }
        });
        let (tx, tz, name) = self.a_poi();
        self.obj = Objective {
            kind, tx, tz, name,
            progress: 0.0,
            time_left: if kind == ProwlObj::Survive { SURVIVE_LEN } else { 0.0 },
            start_caught: self.prey_caught,
            done: false, next_t: 0.0,
        };
    }

    fn complete_objective(&mut self) {
        self.obj.done = true;
        self.cleared += 1;
        self.obj.next_t = NEXT_DELAY;
    }

    fn respawn(&mut self) {
        self.x = 0.0; self.z = 0.0; self.heading = 0.0;
        self.leap_t = 0.0; self.dodge_t = 0.0;
        self.hx = -self.hx.signum().max(1.0) * 60.0; self.hz = -60.0;   // reset far away
        self.hstate = HState::Patrol; self.suspicion = 0.0; self.last_seen = None;
        self.times_caught += 1; self.respawn_flash = 1.2;
        if self.obj.kind == ProwlObj::Rendezvous { self.obj.progress = 0.0; }
    }

    pub fn step(&mut self, dt: f32) {
        self.time += dt;
        self.ring.tick(dt);
        self.exposed = !self.ring.contains(self.x, self.z);
        self.respawn_flash = (self.respawn_flash - dt).max(0.0);
        let input = self.input.take();

        // ── player movement ──
        let (ix, iz, want_leap, want_dodge, want_crouch) = input.unwrap_or((0.0, 0.0, false, false, false));
        let (mut dx, mut dz) = norm(ix, iz);
        let sprinting = (dx.abs() + dz.abs() > 1e-4) && !want_crouch;
        let can_sprint = self.stamina > 0.12;
        let mut speed = self.class.speed_at(self.x, self.z) * PROWL_SCALE * if sprinting && can_sprint { SPRINT } else { 1.0 };

        // dodge (i-frames) and leap (airborne) bursts
        self.dodge_cd = (self.dodge_cd - dt).max(0.0);
        self.dodge_t = (self.dodge_t - dt).max(0.0);
        if want_dodge && self.dodge_cd <= 0.0 && self.dodge_t <= 0.0 && self.stamina > 0.1 {
            let mut d = (dx, dz);
            if d.0.abs() + d.1.abs() < 1e-4 { d = norm(self.x - self.hx, self.z - self.hz); }
            self.dodge_t = DODGE_DUR; self.dodge_cd = DODGE_CD; self.bx = d.0; self.bz = d.1;
            self.stamina = (self.stamina - 0.18).max(0.0);
        }
        self.leap_cd = (self.leap_cd - dt).max(0.0);
        self.leap_t = (self.leap_t - dt).max(0.0);
        if want_leap && self.leap_cd <= 0.0 && self.leap_t <= 0.0 && self.dodge_t <= 0.0 && self.stamina > 0.15 {
            let mut d = (dx, dz);
            if d.0.abs() + d.1.abs() < 1e-4 { d = (self.heading.sin(), self.heading.cos()); }
            self.leap_t = LEAP_DUR; self.leap_cd = LEAP_CD; self.bx = d.0; self.bz = d.1;
            self.stamina = (self.stamina - 0.20).max(0.0);
        }
        let mut airborne = false;
        if self.dodge_t > 0.0 { dx = self.bx; dz = self.bz; speed = DODGE_SPEED; airborne = true; }
        if self.leap_t > 0.0 { dx = self.bx; dz = self.bz; speed = LEAP_SPEED; airborne = true; }

        self.crouched = want_crouch && !airborne;
        let mv = (dx.abs() + dz.abs() > 1e-4) && !self.crouched;
        if mv {
            self.x = (self.x + dx * speed * dt).clamp(-MAP, MAP);
            self.z = (self.z + dz * speed * dt).clamp(-MAP, MAP);
            self.heading = dx.atan2(dz);
        }
        self.moving = mv;
        self.hidden = self.crouched && self.bushes.iter().any(|(bx, bz, r)| dist(self.x, self.z, *bx, *bz) < *r);
        self.stamina = (self.stamina + if sprinting { -0.30 } else { 0.18 } * dt).clamp(0.0, 1.0);
        if self.exposed { self.stamina = (self.stamina - 0.12 * dt).max(0.0); }   // the ring wears you down

        // ── prey: flee, and get pounced when we're mid-burst on top of one ──
        {
            let threats = [(self.x, self.z), (self.hx, self.hz)];
            for cr in &mut self.critters { cr.step(dt, &threats); }
        }
        if airborne && !self.critters.is_empty() {
            let (px, pz) = (self.x, self.z);
            let before = self.critters.len();
            self.critters.retain(|cr| dist(px, pz, cr.x, cr.z) >= POUNCE_R);
            let eaten = (before - self.critters.len()) as u32;
            if eaten > 0 {
                self.prey_caught += eaten;
                self.stamina = (self.stamina + 0.25 * eaten as f32).min(1.0);
            }
        }

        // ── the stalker: sense → chase → catch (respawn) ──
        self.step_hunter(dt);

        // ── objectives ──
        self.update_objective(dt);
        self.tick += 1;
    }

    fn step_hunter(&mut self, dt: f32) {
        let p = self.hunter_kind.params();
        // outside the ring, cover no longer conceals you
        let eff_hidden = self.hidden && !self.exposed;
        let sees = hunter::detects(self.hunter_kind, self.hx, self.hz, self.hheading, self.x, self.z, self.moving, eff_hidden);
        if sees {
            self.suspicion = (self.suspicion + dt * 2.4 * self.difficulty).min(1.5);
            self.last_seen = Some((self.x, self.z));
        } else {
            self.suspicion = (self.suspicion - dt * (if eff_hidden { 1.6 } else { 0.5 })).max(0.0);
        }
        let prev = self.hstate;
        self.hstate = if sees && self.suspicion > 0.9 { HState::Chase }
            else if self.suspicion > 0.4 { HState::Track }
            else if self.last_seen.is_some() && self.suspicion > 0.1 { HState::Search }
            else { HState::Patrol };
        // lost the quarry after a chase → that's an escape
        if prev == HState::Chase && self.hstate != HState::Chase { self.escapes += 1; }

        // move toward the quarry (or its last seen spot); patrol drifts toward the objective
        let goal = match self.hstate {
            HState::Chase => (self.x, self.z),
            HState::Track | HState::Search => self.last_seen.unwrap_or((self.obj.tx, self.obj.tz)),
            _ => (self.obj.tx, self.obj.tz),
        };
        let (gx, gz) = norm(goal.0 - self.hx, goal.1 - self.hz);
        let hspeed = p.speed * if self.hstate == HState::Chase { 1.0 } else { 0.55 } * self.difficulty;
        self.hx = (self.hx + gx * hspeed * dt).clamp(-MAP, MAP);
        self.hz = (self.hz + gz * hspeed * dt).clamp(-MAP, MAP);
        if gx.abs() + gz.abs() > 1e-4 { self.hheading = gx.atan2(gz); }

        // catch: aggressive + in reach + not airborne/hidden → respawn the prowler
        let aggressive = self.hstate == HState::Chase || self.suspicion > 0.55;
        if aggressive && self.leap_t <= 0.0 && self.dodge_t <= 0.0 && !(self.hidden && !self.exposed)
            && dist(self.hx, self.hz, self.x, self.z) < CATCH_DIST {
            self.respawn();
        }
    }

    fn update_objective(&mut self, dt: f32) {
        if self.obj.done {
            self.obj.next_t -= dt;
            if self.obj.next_t <= 0.0 { self.new_objective(None); }
            return;
        }
        match self.obj.kind {
            ProwlObj::Reach => {
                if dist(self.x, self.z, self.obj.tx, self.obj.tz) < REACH_R { self.complete_objective(); }
            }
            ProwlObj::Rendezvous => {
                if dist(self.x, self.z, self.obj.tx, self.obj.tz) < RENDEZVOUS_R {
                    self.obj.progress += dt;
                    if self.obj.progress >= RENDEZVOUS_NEED { self.complete_objective(); }
                } else {
                    self.obj.progress = (self.obj.progress - dt * 0.5).max(0.0);
                }
            }
            ProwlObj::Survive => {
                self.obj.time_left -= dt;
                if self.obj.time_left <= 0.0 { self.complete_objective(); }
            }
            ProwlObj::Forage => {
                if self.prey_caught - self.obj.start_caught >= FORAGE_NEED { self.complete_objective(); }
            }
        }
    }

    /// One-line objective banner for a HUD.
    pub fn objective_text(&self) -> String {
        if self.obj.done { return format!("✔ complete!   ·   cleared: {}", self.cleared); }
        let body = match self.obj.kind {
            ProwlObj::Reach => format!("Reach the {}", self.obj.name),
            ProwlObj::Rendezvous => format!("Hold the {} ({:.1}/{:.0}s)", self.obj.name, self.obj.progress, RENDEZVOUS_NEED),
            ProwlObj::Survive => format!("Survive the hunt — {}s", self.obj.time_left.max(0.0).ceil() as i32),
            ProwlObj::Forage => format!("Catch prey — {}/{}", self.prey_caught - self.obj.start_caught, FORAGE_NEED),
        };
        format!("{}   ·   cleared: {}", body, self.cleared)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reach_objective_clears_and_rotates() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.new_objective(Some(ProwlObj::Reach));
        let cleared0 = g.cleared;
        // teleport onto the target
        g.x = g.obj.tx; g.z = g.obj.tz;
        g.set_input(0.0, 0.0, false, false, false);
        g.step(1.0 / 30.0);
        assert!(g.obj.done, "standing on the target should clear a Reach");
        assert_eq!(g.cleared, cleared0 + 1);
        // after the delay, a fresh objective spawns
        for _ in 0..((NEXT_DELAY + 0.2) * 30.0) as i32 { g.set_input(0.0, 0.0, false, false, false); g.step(1.0 / 30.0); }
        assert!(!g.obj.done, "a new objective should be live after the delay");
    }

    #[test]
    fn survive_objective_counts_down_and_clears() {
        let mut g = Prowl::new(Class::Goat, Kind::Snake);
        g.new_objective(Some(ProwlObj::Survive));
        g.hx = 300.0; g.hz = 300.0;   // keep the hunter far so we aren't caught
        let mut cleared = false;
        for _ in 0..((SURVIVE_LEN + 2.0) * 30.0) as i32 {
            g.hx = 300.0; g.hz = 300.0;
            g.set_input(0.0, 0.0, false, false, false);
            g.step(1.0 / 30.0);
            if g.cleared > 0 { cleared = true; break; }
        }
        assert!(cleared, "outlasting the survive timer should clear it");
    }

    #[test]
    fn leaping_onto_prey_scores_and_feeds_forage() {
        let mut g = Prowl::new(Class::Cat, Kind::Fox);
        g.new_objective(Some(ProwlObj::Forage));
        g.hx = 300.0; g.hz = 300.0;
        g.x = 0.0; g.z = 0.0; g.heading = 0.0;
        // a critter right in front; leap onto it
        g.critters.clear();
        g.critters.push(Critter::new(crate::ecology::Species::Hare, 0.0, 1.0, 7));
        let before = g.prey_caught;
        g.set_input(0.0, 1.0, true, false, false);
        g.step(1.0 / 30.0);
        assert!(g.prey_caught > before, "a leap onto prey should score");
    }

    #[test]
    fn getting_caught_respawns_the_prowler() {
        let mut g = Prowl::new(Class::Cat, Kind::Dog);
        // put the hunter on top, locked on
        g.x = 20.0; g.z = 0.0; g.hx = 20.0; g.hz = 0.0;
        g.hstate = HState::Chase; g.suspicion = 1.4;
        let caught0 = g.times_caught;
        g.set_input(0.0, 0.0, false, false, false);
        g.step(1.0 / 30.0);
        assert_eq!(g.times_caught, caught0 + 1, "a point-blank aggressive hunter catches you");
        assert!(g.x.abs() < 1e-3 && g.z.abs() < 1e-3, "you respawn at the origin");
        assert!(g.respawn_flash > 0.0);
    }

    #[test]
    fn airborne_prowler_is_uncatchable() {
        let mut g = Prowl::new(Class::Frog, Kind::Dog);
        g.x = 20.0; g.z = 0.0; g.hx = 20.0; g.hz = 0.0;
        g.hstate = HState::Chase; g.suspicion = 1.4;
        let caught0 = g.times_caught;
        g.set_input(0.0, 1.0, true, false, false);   // leap
        g.step(1.0 / 30.0);
        assert_eq!(g.times_caught, caught0, "a leaping prowler can't be caught");
    }

    #[test]
    fn prowl_invariants_hold_across_combos() {
        // Prowl every class against a spread of hunters with a live, wiggling player.
        // Stamina stays in range, the prowler and hunter stay finite and on the map,
        // cleared/prey/caught only climb, and nothing panics.
        for cls in [Class::Cat, Class::Otter, Class::Goat, Class::Hawk] {
            for hk in [Kind::Fox, Kind::Hawk, Kind::Snake, Kind::Shadow] {
                let mut g = Prowl::new(cls, hk);
                let (mut pc, mut cc) = (0u32, 0u32);
                for t in 0..(60 * 30) {
                    g.set_input((t as f32 * 0.10).sin(), (t as f32 * 0.08).cos(), t % 40 == 0, t % 25 == 0, t % 12 < 3);
                    g.step(1.0 / 30.0);
                    assert!(g.stamina >= 0.0 && g.stamina <= 1.0, "stamina out of range {} ({cls:?}/{hk:?})", g.stamina);
                    assert!(g.x.is_finite() && g.z.is_finite() && g.hx.is_finite() && g.hz.is_finite(), "position not finite");
                    assert!(g.x.abs() <= MAP + 1.0 && g.z.abs() <= MAP + 1.0, "prowler left the map");
                    assert!(g.hx.abs() <= MAP + 1.0 && g.hz.abs() <= MAP + 1.0, "hunter left the map");
                    assert!(g.prey_caught >= pc && g.times_caught >= cc, "a tally went backwards");
                    pc = g.prey_caught; cc = g.times_caught;
                }
            }
        }
    }

    #[test]
    fn ring_exposure_strips_cover() {
        // hunker in a bush with a hunter staring point-blank: inside the ring cover
        // hides you, outside it the exposure lets the hunter see straight through.
        let probe = |x: f32, z: f32| -> (bool, f32) {
            let mut g = Prowl::new(Class::Cat, Kind::Fox);
            g.x = x; g.z = z; g.bushes.push((x, z, 4.0));
            g.hx = x + 3.0; g.hz = z; g.hheading = (g.x - g.hx).atan2(g.z - g.hz);
            g.suspicion = 0.0;
            g.set_input(0.0, 0.0, false, false, true);   // crouch/hide
            g.step(1.0 / 30.0);
            (g.exposed, g.suspicion)
        };
        let (inside_exposed, inside_susp) = probe(0.0, 0.0);
        assert!(!inside_exposed, "the origin sits inside the ring");
        assert!(inside_susp < 1e-3, "cover should conceal you inside the ring");
        let (out_exposed, out_susp) = probe(world::SIZE * 0.5, 0.0);
        assert!(out_exposed, "the far point sits outside the ring");
        assert!(out_susp > 0.0, "outside the ring, cover no longer hides you");
    }
}
