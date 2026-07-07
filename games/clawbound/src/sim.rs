//! sim — the playable core: 6 cats vs 1 hunter. Headless and deterministic so
//! it unit-tests, and it's exactly what the `--play` window and the netcode
//! server drive. Cats seek the objective, flee the hunter, and crouch into
//! bushes to vanish; the hunter senses (cascading cone / omni / vibration),
//! runs its micro-FSM, and pursues — catching any cat it corners in the open.

use crate::hunter::{self, HState, Kind};
use crate::world;

pub const N_CATS: usize = 6;
const CAT_SPEED: f32 = 13.0;
const FLEE_RADIUS: f32 = 16.0;
const CATCH_DIST: f32 = 2.3;
const PERIPHERAL: f32 = 18.0;   // hunter notices nearby movement even off-cone
const MAP: f32 = world::SIZE * 0.92;
const OBJECTIVE_R: f32 = 5.0;   // reach the rendezvous to escape
const ROUND_LEN: f32 = 90.0;    // seconds before the round times out
const SCENT_DROP: u32 = 5;      // ticks between scent deposits per moving cat
const SCENT_DRIFT: f32 = 3.5;   // how fast scent slides downwind (u/s)
const SCENT_DECAY: f32 = 0.22;  // strength lost per second
const CLIMB_R: f32 = 3.0;       // reach a trunk to scramble up
const DODGE_DUR: f32 = 0.34;    // i-frame + burst window
const DODGE_CD: f32 = 1.5;      // cooldown between rolls
const DODGE_SPEED: f32 = CAT_SPEED * 2.3;
// ── ported from the three.js build ──
const LEAP_DUR: f32 = 0.5;                 // a committed forward bound, airborne (uncatchable)
const LEAP_CD: f32 = 1.8;                  // cooldown between leaps
const LEAP_SPEED: f32 = CAT_SPEED * 2.0;   // covers a gap — slower than a roll but longer
const CARRY_DUR: f32 = 2.4;                // the hawk hoists open prey aloft this long
const BREAK_FREE: f32 = 0.12;              // chance a carried cat wrenches free before it's taken
const CRIPPLE_DUR: f32 = 4.0;              // slowed for a while after breaking free
const POUNCE_R: f32 = 2.4;                 // reach to snatch an ambient critter mid-burst

/// Deterministic hash → [0,1) for the odd coin-flip (break-free) without an rng crate.
fn hashf(mut h: u32) -> f32 {
    h ^= h >> 16; h = h.wrapping_mul(0x7feb352d);
    h ^= h >> 15; h = h.wrapping_mul(0x846ca68b);
    h ^= h >> 16; (h as f32) / (u32::MAX as f32)
}


#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Outcome { CatsWin, HunterWins }

/// Match type. Rendezvous: reach the beacon to escape. Survive: no escape — just
/// outlast the hunter using cover for the full round.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode { Rendezvous, Survive }

/// A puff of scent a moving cat leaves behind; it drifts downwind and fades.
#[derive(Clone, Copy)]
pub struct Scent { pub x: f32, pub z: f32, pub strength: f32 }

pub struct Cat {
    pub id: u8,
    pub x: f32, pub z: f32, pub heading: f32,
    pub alive: bool, pub crouched: bool, pub hidden: bool, pub moving: bool, pub safe: bool, pub climbing: bool,
    pub stamina: f32, pub dodge_t: f32, pub dodge_cd: f32, pub dodge_dx: f32, pub dodge_dz: f32,
    pub goal: (f32, f32), pub player: bool,
    // ── ported mechanics ──
    pub leap_t: f32, pub leap_cd: f32, pub leap_dx: f32, pub leap_dz: f32,
    pub carried_t: f32,   // > 0 while the hawk is hoisting this cat aloft
    pub crippled_t: f32,  // > 0 = limping (just broke free of the hawk)
}

impl Cat {
    pub fn is_leaping(&self) -> bool { self.leap_t > 0.0 }
    pub fn is_carried(&self) -> bool { self.carried_t > 0.0 }
    /// Height offset for the leap arc (0 on the ground … peak mid-bound), for rendering.
    pub fn leap_arc(&self) -> f32 {
        if self.leap_t <= 0.0 { return 0.0; }
        let phase = 1.0 - (self.leap_t / LEAP_DUR).clamp(0.0, 1.0);
        (phase * std::f32::consts::PI).sin() * 1.7
    }
}

pub struct HunterUnit {
    pub kind: Kind,
    pub x: f32, pub z: f32, pub heading: f32,
    pub state: HState, pub suspicion: f32,
    pub target: Option<u8>, pub last_seen: Option<(f32, f32)>,
    pub carrying: bool,   // hawk: hovering with prey in its talons
}

pub struct Game {
    pub cats: Vec<Cat>,
    pub hunter: HunterUnit,
    pub bushes: Vec<(f32, f32, f32)>,
    pub trees: Vec<(f32, f32)>,
    pub objective: (f32, f32),
    pub tick: u32,
    pub caught: u32,
    pub escaped: u32,
    pub time: f32,
    pub outcome: Option<Outcome>,
    pub scent: Vec<Scent>,
    pub wind: (f32, f32),
    pub snake_home: Option<(f32, f32)>,
    pub mode: Mode,
    pub shifted: bool,      // Shadow: the afterlife is overlaid on the world (cover stripped)
    pub critters: Vec<crate::ecology::Critter>,
    pub prey_caught: u32,   // ambient critters pounced by the cats
    shift_t: f32,
    teleport_cd: f32,
    input: Option<(u8, f32, f32, bool, bool, bool)>,
}

fn norm(x: f32, z: f32) -> (f32, f32) { let l = (x * x + z * z).sqrt(); if l > 1e-5 { (x / l, z / l) } else { (0.0, 0.0) } }
fn dist(ax: f32, az: f32, bx: f32, bz: f32) -> f32 { ((ax - bx).powi(2) + (az - bz).powi(2)).sqrt() }

impl Game {
    pub fn new_6v1(kind: Kind) -> Game { Self::new_match(kind, Mode::Rendezvous) }

    pub fn new_match(kind: Kind, mode: Mode) -> Game {
        world::init_world();
        let bushes = world::bush_positions(200);
        let trees = world::tree_positions(120);
        let obj = crate::objectives::place();
        let objective = (obj[0].x, obj[0].z);
        let mut cats = Vec::new();
        for i in 0..N_CATS {
            let a = i as f32 / N_CATS as f32 * std::f32::consts::TAU;
            let (x, z) = (a.cos() * 40.0, a.sin() * 40.0);
            cats.push(Cat { id: i as u8, x, z, heading: 0.0, alive: true, crouched: false, hidden: false, moving: false, safe: false, climbing: false, stamina: 1.0, dodge_t: 0.0, dodge_cd: 0.0, dodge_dx: 0.0, dodge_dz: 0.0, goal: objective, player: i == 0, leap_t: 0.0, leap_cd: 0.0, leap_dx: 0.0, leap_dz: 0.0, carried_t: 0.0, crippled_t: 0.0 });
        }
        let snake_home = if matches!(kind, Kind::Snake) && mode == Mode::Rendezvous { world::nearest_water(objective, 130.0) } else { None };
        let (hx, hz) = if mode == Mode::Survive { (34.0, 34.0) } else { snake_home.unwrap_or((objective.0 + 40.0, objective.1 + 40.0)) };
        let hunter = HunterUnit { kind, x: hx, z: hz, heading: 0.0, state: HState::Patrol, suspicion: 0.0, target: None, last_seen: None, carrying: false };
        let wind = norm(1.0, 0.4);   // a steady prevailing wind
        let critters = crate::ecology::populate(520, false);
        Game { cats, hunter, bushes, trees, objective, tick: 0, caught: 0, escaped: 0, time: 0.0, outcome: None, scent: Vec::new(), wind, snake_home, mode, shifted: false, critters, prey_caught: 0, shift_t: 0.0, teleport_cd: 5.0, input: None }
    }

    pub fn set_input(&mut self, id: u8, mx: f32, mz: f32, crouch: bool, dodge: bool, leap: bool) { self.input = Some((id, mx, mz, crouch, dodge, leap)); }

    pub fn step(&mut self, dt: f32) {
        if self.outcome.is_some() { return; }
        self.time += dt;
        let objective = self.objective;
        let mode = self.mode;
        let (mut hx, mut hz) = (self.hunter.x, self.hunter.z);
        let input = self.input.take();

        // ── hawk carry: a grabbed cat is hoisted aloft, struggles, then breaks free
        //    (small chance, limps away crippled) or is taken. While carrying, the hawk
        //    hovers and hunts no one else — a window for the rest of the pack. ──
        if self.hunter.carrying {
            let tick = self.tick;
            for cat in self.cats.iter_mut().filter(|c| c.carried_t > 0.0) {
                cat.carried_t -= dt;
                cat.x += (hx - cat.x) * (dt * 4.0).min(1.0);
                cat.z += (hz - cat.z) * (dt * 4.0).min(1.0);
                cat.moving = false; cat.hidden = false; cat.crouched = false; cat.climbing = false;
                if cat.carried_t <= 0.0 {
                    if hashf(tick ^ (cat.id as u32).wrapping_mul(2_654_435_761)) < BREAK_FREE {
                        cat.crippled_t = CRIPPLE_DUR;    // wrenches free, drops, limps off
                    } else {
                        cat.alive = false;               // carried off for good
                    }
                }
            }
            if !self.cats.iter().any(|c| c.carried_t > 0.0) {
                self.hunter.carrying = false;
                self.hunter.state = HState::Patrol; self.hunter.suspicion = 0.0;
            }
        }

        // ── cats ──
        let bushes = self.bushes.clone();
        let trees = self.trees.clone();
        let hunter_aerial = self.hunter.kind.params().aerial;
        let nearest_bush = |x: f32, z: f32| -> Option<(f32, f32, f32)> {
            bushes.iter().copied().min_by(|a, b| dist(x, z, a.0, a.1).partial_cmp(&dist(x, z, b.0, b.1)).unwrap())
        };
        let in_bush = |x: f32, z: f32| -> bool { bushes.iter().any(|(bx, bz, r)| dist(x, z, *bx, *bz) < *r) };
        let nearest_tree = |x: f32, z: f32| -> Option<(f32, f32)> {
            trees.iter().copied().min_by(|a, b| dist(x, z, a.0, a.1).partial_cmp(&dist(x, z, b.0, b.1)).unwrap())
        };
        for cat in self.cats.iter_mut().filter(|c| c.alive && !c.safe && c.carried_t <= 0.0) {
            let dh = dist(cat.x, cat.z, hx, hz);
            let mut dir = (0.0, 0.0);
            let mut speed = CAT_SPEED;
            let mut want_crouch = false;
            let mut want_climb = false;
            let mut want_dodge = false;
            let mut want_leap = false;
            let can_sprint = cat.stamina > 0.12;
            let sprint = if can_sprint { 1.12 } else { 0.85 }; // winded cats slow below hunter speed
            let mut sprinting = false;

            if cat.player {
                if let Some((pid, mx, mz, cr, dg, lp)) = input {
                    if pid == cat.id {
                        dir = norm(mx, mz);
                        want_dodge = dg;
                        want_leap = lp;
                        // crouch key is contextual: at a trunk → climb, otherwise → hide
                        if cr {
                            match nearest_tree(cat.x, cat.z) {
                                Some((tx, tz)) if dist(cat.x, cat.z, tx, tz) < CLIMB_R => want_climb = true,
                                _ => want_crouch = true,
                            }
                        }
                        sprinting = dir.0.abs() + dir.1.abs() > 1e-4 && !cr;
                        if sprinting { speed = CAT_SPEED * sprint; }
                    }
                }
            } else if dh < FLEE_RADIUS {
                sprinting = true;
                // (AI relies on cover + escaping; the dodge/roll is a player skill move)
                // climbing beats a ground hunter but exposes you to the hawk, so only
                // consider a tree when the hunter can't fly; pick the closer refuge.
                let tree = if !hunter_aerial { nearest_tree(cat.x, cat.z) } else { None };
                let bush = nearest_bush(cat.x, cat.z);
                let td = tree.map(|t| dist(cat.x, cat.z, t.0, t.1)).unwrap_or(1e9);
                let bd = bush.map(|b| dist(cat.x, cat.z, b.0, b.1)).unwrap_or(1e9);
                if tree.is_some() && td <= bd {
                    let (tx, tz) = tree.unwrap();
                    if td < CLIMB_R { want_climb = true; sprinting = false; }
                    else { dir = norm(tx - cat.x, tz - cat.z); speed = CAT_SPEED * sprint; }
                } else if let Some((bx, bz, r)) = bush {
                    if bd < r { want_crouch = true; sprinting = false; }
                    else { dir = norm(bx - cat.x, bz - cat.z); speed = CAT_SPEED * sprint; }
                } else {
                    dir = norm(cat.x - hx, cat.z - hz); speed = CAT_SPEED * sprint;
                }
            } else if mode == Mode::Survive {
                // no beacon to reach — hole up in the nearest cover and wait it out
                if let Some((bx, bz, r)) = nearest_bush(cat.x, cat.z) {
                    if dist(cat.x, cat.z, bx, bz) < r { want_crouch = true; }
                    else { dir = norm(bx - cat.x, bz - cat.z); }
                }
            } else {
                dir = norm(cat.goal.0 - cat.x, cat.goal.1 - cat.z);
            }

            // dodge/roll: a short burst with brief invulnerability (i-frames) on a cooldown
            cat.dodge_cd = (cat.dodge_cd - dt).max(0.0);
            cat.dodge_t = (cat.dodge_t - dt).max(0.0);
            if want_dodge && cat.dodge_cd <= 0.0 && cat.dodge_t <= 0.0 && cat.stamina > 0.1 {
                let mut dd = dir;
                if dd.0.abs() + dd.1.abs() < 1e-4 { dd = norm(cat.x - hx, cat.z - hz); }  // roll away if no input
                cat.dodge_t = DODGE_DUR; cat.dodge_cd = DODGE_CD;
                cat.dodge_dx = dd.0; cat.dodge_dz = dd.1;
                cat.stamina = (cat.stamina - 0.18).max(0.0);
            }
            if cat.dodge_t > 0.0 {                      // mid-roll: fast, uncatchable, can't hide/climb
                dir = (cat.dodge_dx, cat.dodge_dz); speed = DODGE_SPEED;
                want_crouch = false; want_climb = false;
            }

            // leap: a committed forward bound. Longer and airborne (uncatchable while up)
            // — a gap-clearer to escape or to pounce prey, distinct from the snappy roll.
            cat.leap_cd = (cat.leap_cd - dt).max(0.0);
            cat.leap_t = (cat.leap_t - dt).max(0.0);
            if want_leap && cat.leap_t <= 0.0 && cat.leap_cd <= 0.0 && cat.dodge_t <= 0.0 && cat.stamina > 0.15 {
                let mut ld = dir;
                if ld.0.abs() + ld.1.abs() < 1e-4 { ld = (cat.heading.sin(), cat.heading.cos()); } // leap where you face
                cat.leap_t = LEAP_DUR; cat.leap_cd = LEAP_CD;
                cat.leap_dx = ld.0; cat.leap_dz = ld.1;
                cat.stamina = (cat.stamina - 0.20).max(0.0);
            }
            if cat.leap_t > 0.0 {                       // airborne: committed forward, can't hide/climb
                dir = (cat.leap_dx, cat.leap_dz); speed = LEAP_SPEED;
                want_crouch = false; want_climb = false;
            }

            // crippled after wrenching free of the hawk: half speed for a while
            cat.crippled_t = (cat.crippled_t - dt).max(0.0);
            if cat.crippled_t > 0.0 && cat.leap_t <= 0.0 && cat.dodge_t <= 0.0 { speed *= 0.5; }

            // stamina: sprinting drains, everything else recovers
            cat.stamina = (cat.stamina + if sprinting { -0.32 } else { 0.16 } * dt).clamp(0.0, 1.0);

            cat.crouched = want_crouch;
            cat.climbing = want_climb;
            let mv = dir.0.abs() + dir.1.abs() > 1e-4 && !want_crouch && !want_climb;
            if mv {
                cat.x = (cat.x + dir.0 * speed * dt).clamp(-MAP, MAP);
                cat.z = (cat.z + dir.1 * speed * dt).clamp(-MAP, MAP);
                cat.heading = dir.0.atan2(dir.1);
            }
            cat.moving = mv;
            cat.hidden = cat.crouched && in_bush(cat.x, cat.z);
            // reached the rendezvous → escaped for good (Rendezvous mode only)
            if mode == Mode::Rendezvous && dist(cat.x, cat.z, objective.0, objective.1) < OBJECTIVE_R { cat.safe = true; cat.moving = false; cat.hidden = false; cat.climbing = false; }
        }

        // ── scent: moving cats leave a trail that drifts downwind and fades ──
        if self.tick % SCENT_DROP == 0 {
            for c in self.cats.iter().filter(|c| c.alive && !c.safe && c.moving && !c.hidden && !c.climbing) {
                self.scent.push(Scent { x: c.x, z: c.z, strength: 1.0 });
            }
        }
        for s in self.scent.iter_mut() {
            s.x += self.wind.0 * SCENT_DRIFT * dt;
            s.z += self.wind.1 * SCENT_DRIFT * dt;
            s.strength -= SCENT_DECAY * dt;
        }
        self.scent.retain(|s| s.strength > 0.05);

        // ── hunter: sight → smell → FSM → pursue → catch ──
        // While the hawk is carrying prey it hovers and hunts no one else, so the
        // entire sensing/pursuit/catch step is skipped until the struggle resolves.
        if !self.hunter.carrying {
        let p = self.hunter.kind.params();
        // Shadow only: the otherworld phases in and out. While shifted, the afterlife
        // is overlaid on the world and cover no longer conceals the living.
        if self.hunter.kind.is_shadow() {
            self.shift_t += dt;
            if !self.shifted && self.shift_t > 9.0 { self.shifted = true; self.shift_t = 0.0; }
            else if self.shifted && self.shift_t > 6.0 { self.shifted = false; self.shift_t = 0.0; }
        }
        let strip = self.hunter.kind.is_shadow() && self.shifted; // cover stripped this tick
        // sweep the gaze while not locked on so prey off the narrow far-cone still gets noticed
        let gaze = if self.hunter.state == HState::Chase { self.hunter.heading }
                   else { self.hunter.heading + (self.tick as f32 * 0.09).sin() * 1.3 };
        let mut seen_cat: Option<(u8, f32, f32, f32)> = None; // id, x, z, dist
        for cat in self.cats.iter().filter(|c| c.alive && !c.safe) {
            let d = dist(hx, hz, cat.x, cat.z);
            let hidden = cat.hidden && !strip;   // the Shadow sees hiders during the shift
            let sees = hunter::detects(self.hunter.kind, hx, hz, gaze, cat.x, cat.z, cat.moving, hidden)
                || (cat.moving && !hidden && d < PERIPHERAL);
            if sees && seen_cat.map_or(true, |s| d < s.3) { seen_cat = Some((cat.id, cat.x, cat.z, d)); }
        }
        // during the shift the otherworld lays the living bare — the shadow senses
        // the nearest cat around it even if cover would normally hide them.
        let mut seen = seen_cat.is_some();
        if strip && !seen {
            let mut best = 46.0;
            for cat in self.cats.iter().filter(|c| c.alive && !c.safe) {
                let d = dist(hx, hz, cat.x, cat.z);
                if d < best { best = d; seen_cat = Some((cat.id, cat.x, cat.z, d)); }
            }
            seen = seen_cat.is_some();
        }

        // smell: when it can't see, follow the strongest scent within nose range.
        // fresher scent is stronger and nearer the cat, so climbing that gradient
        // (toward the source = upwind) leads the hunter in.
        let mut smelled: Option<(f32, f32)> = None;
        if !seen {
            let mut best = 0.0;
            for s in &self.scent {
                let d = dist(hx, hz, s.x, s.z);
                if d < p.smell {
                    let score = s.strength * (1.0 - d / p.smell);
                    if score > best { best = score; smelled = Some((s.x, s.z)); }
                }
            }
        }
        let has_scent = smelled.is_some();

        if let Some((id, cx, cz, _)) = seen_cat {
            self.hunter.suspicion = (self.hunter.suspicion + dt * 2.4).min(1.5);
            self.hunter.last_seen = Some((cx, cz));
            self.hunter.target = Some(id);
        } else if has_scent {
            self.hunter.suspicion = (self.hunter.suspicion + dt * 0.5).min(0.85); // a trail keeps it hunting, not charging
        } else {
            self.hunter.suspicion = (self.hunter.suspicion - dt * 0.3).max(0.0);
        }
        self.hunter.state = hunter::step(self.hunter.state, self.hunter.suspicion, has_scent, seen);

        let (mut goal, mut speed) = match self.hunter.state {
            HState::Chase => (self.hunter.last_seen.unwrap_or((hx, hz)), p.speed),
            HState::Alert | HState::Search | HState::Track => {
                let g = if seen { self.hunter.last_seen.unwrap() }
                        else { smelled.or(self.hunter.last_seen).unwrap_or(self.objective) };
                (g, p.speed * 0.85)
            }
            HState::Patrol => {
                if mode == Mode::Survive {
                    // no beacon to guard — sweep the ground where the pack hides
                    let a = self.tick as f32 * 0.02;
                    ((a.cos() * 38.0, a.sin() * 38.0), p.speed * 0.7)
                } else {
                    // orbit the objective (where cats converge) so the cone keeps sweeping
                    let a = self.tick as f32 * 0.016;
                    ((self.objective.0 + a.cos() * 22.0, self.objective.1 + a.sin() * 22.0), p.speed * 0.6)
                }
            }
        };
        // snake: an ambusher. It lurks in its water rather than orbiting, then
        // erupts with a strike-burst when prey strays near the pool.
        if let Some(home) = self.snake_home {
            if self.hunter.state == HState::Patrol { goal = home; speed = p.speed * 0.5; } // coil and wait
            if dist(hx, hz, home.0, home.1) < 22.0 && matches!(self.hunter.state, HState::Chase | HState::Alert | HState::Track) {
                speed *= 1.8; // explosive lunge from the water
            }
        }
        // Shadow: blink across the map to where the prey just was (Freddy-style —
        // you never outrun it). Cooldown-gated, only when its quarry has slipped far.
        if self.hunter.kind.is_shadow() {
            self.teleport_cd -= dt;
            if self.teleport_cd <= 0.0 {
                if let Some((lx, lz)) = self.hunter.last_seen {
                    if dist(hx, hz, lx, lz) > 16.0 {
                        hx = lx; hz = lz;
                        self.hunter.x = lx; self.hunter.z = lz;
                        self.teleport_cd = 5.0;
                    }
                }
            }
        }
        if self.hunter.kind.is_shadow() && self.shifted { speed *= 1.35; } // empowered by the otherworld
        let d = norm(goal.0 - hx, goal.1 - hz);
        if d.0.abs() + d.1.abs() > 1e-4 {
            self.hunter.x = (hx + d.0 * speed * dt).clamp(-MAP, MAP);
            self.hunter.z = (hz + d.1 * speed * dt).clamp(-MAP, MAP);
            self.hunter.heading = d.0.atan2(d.1);
        }

        // catch: open cats need the hunter aggressive + in reach; a hidden cat is
        // rooted out by proximity scaled by the nose. During the Shadow's shift,
        // cover is stripped so hiders are as catchable as cats in the open.
        let aggressive = self.hunter.state == HState::Chase || self.hunter.suspicion > 0.55;
        let flush_r = 1.2 + p.smell * 0.035;
        let is_hawk = matches!(self.hunter.kind, Kind::Hawk);
        let mut grabbed_any = false;
        {
            let (hx, hz) = (self.hunter.x, self.hunter.z);
            for cat in self.cats.iter_mut().filter(|c| c.alive && !c.safe && c.carried_t <= 0.0) {
                if cat.dodge_t > 0.0 || cat.leap_t > 0.0 { continue; } // rolling or airborne → invulnerable
                if cat.climbing && !hunter_aerial && !strip { continue; } // up the trunk — safe unless a hawk or the shift reaches it
                let d = dist(hx, hz, cat.x, cat.z);
                let hidden = cat.hidden && !strip;
                let taken = if hidden { d < flush_r } else { aggressive && d < CATCH_DIST };
                if taken {
                    if is_hawk && !hidden {
                        cat.carried_t = CARRY_DUR;   // the hawk hoists open prey aloft; the struggle resolves later
                        grabbed_any = true;
                    } else {
                        cat.alive = false;           // every other hunter takes prey outright
                    }
                }
            }
        }
        if grabbed_any { self.hunter.carrying = true; }
        }
        // ambient wildlife: flee the hunter and any living cats
        if !self.critters.is_empty() {
            let mut threats: Vec<(f32, f32)> = Vec::with_capacity(1 + self.cats.len());
            threats.push((self.hunter.x, self.hunter.z));
            for c in &self.cats { if c.alive { threats.push((c.x, c.z)); } }
            for cr in &mut self.critters { cr.step(dt, &threats); }
        }
        // pounce: a cat mid-burst (a leap or a roll) snatches any ambient critter it
        // lands on — a small meal that tops up stamina, tying the wildlife into the
        // hunt (from the JS build: caught when airborne or moving fast).
        let pounce_pts: Vec<(usize, f32, f32)> = self.cats.iter().enumerate()
            .filter(|(_, c)| c.alive && (c.leap_t > 0.0 || c.dodge_t > 0.0))
            .map(|(i, c)| (i, c.x, c.z)).collect();
        if !pounce_pts.is_empty() && !self.critters.is_empty() {
            let mut fed = [0u32; N_CATS];
            self.critters.retain(|cr| {
                for &(i, cx, cz) in &pounce_pts {
                    if dist(cx, cz, cr.x, cr.z) < POUNCE_R { fed[i] += 1; return false; }
                }
                true
            });
            for i in 0..N_CATS {
                if fed[i] > 0 {
                    self.cats[i].stamina = (self.cats[i].stamina + 0.25 * fed[i] as f32).min(1.0);
                    self.prey_caught += fed[i];
                }
            }
        }
        self.tick += 1;

        // tally + resolve: hunter wins by taking half the pack (3 of 6), else the cats do
        self.escaped = self.cats.iter().filter(|c| c.safe).count() as u32;
        self.caught = self.cats.iter().filter(|c| !c.alive).count() as u32;
        let resolved = (self.escaped + self.caught) as usize;
        if resolved >= N_CATS || self.time >= ROUND_LEN {
            self.outcome = Some(if self.caught >= 3 { Outcome::HunterWins } else { Outcome::CatsWin });
        }
    }

    pub fn alive_cats(&self) -> usize { self.cats.iter().filter(|c| c.alive).count() }
    pub fn active_cats(&self) -> usize { self.cats.iter().filter(|c| c.alive && !c.safe).count() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roster_is_6v1() {
        let g = Game::new_6v1(Kind::Fox);
        assert_eq!(g.cats.len(), N_CATS);
        assert_eq!(g.alive_cats(), 6);
    }

    #[test]
    fn hunter_chases_a_cat_it_sees() {
        let mut g = Game::new_6v1(Kind::Fox);
        // put the hunter right behind cat 0, facing it, in the open
        g.cats[0].x = 0.0; g.cats[0].z = 10.0; g.cats[0].moving = true; g.cats[0].hidden = false;
        g.hunter.x = 0.0; g.hunter.z = 0.0; g.hunter.heading = 0.0; // faces +z toward the cat
        let d0 = dist(g.hunter.x, g.hunter.z, g.cats[0].x, g.cats[0].z);
        for _ in 0..30 { g.step(1.0 / 30.0); }
        assert!(matches!(g.hunter.state, HState::Chase | HState::Alert));
        let d1 = dist(g.hunter.x, g.hunter.z, g.cats[0].x, g.cats[0].z);
        assert!(d1 < d0, "hunter should have closed distance ({d0} -> {d1})");
    }

    #[test]
    fn cat_flees_a_near_hunter() {
        let mut g = Game::new_6v1(Kind::Snake); // slow enough it can't catch, so we isolate flee behaviour
        g.bushes.clear();                        // no cover → flee straight away from the hunter
        g.hunter.x = 0.0; g.hunter.z = 0.0;
        g.cats[1].x = 8.0; g.cats[1].z = 0.0; g.cats[1].player = false;
        let x0 = g.cats[1].x;
        for _ in 0..10 { g.step(1.0 / 30.0); }
        assert!(g.cats[1].alive && g.cats[1].x > x0, "a threatened cat should flee away from the hunter");
    }

    #[test]
    fn flee_with_no_cover_does_not_panic() {
        // Regression: a fleeing cat with neither a tree (the Hawk forbids climbing) nor
        // a bush in range used to hit `tree.unwrap()` on None and crash the whole sim.
        let mut g = Game::new_6v1(Kind::Hawk);
        g.bushes.clear();
        g.trees.clear();
        g.hunter.x = 0.0; g.hunter.z = 0.0;
        g.cats[1].x = 10.0; g.cats[1].z = 0.0; g.cats[1].player = false;
        let x0 = g.cats[1].x;
        for _ in 0..5 { g.step(1.0 / 30.0); }     // must not panic
        assert!(g.cats[1].x > x0, "with no cover at all, the cat should just run from the hunter");
    }

    #[test]
    fn reaching_rendezvous_escapes() {
        let mut g = Game::new_6v1(Kind::Snake);
        g.hunter.x = 600.0; g.hunter.z = 600.0;   // no threat nearby
        let (ox, oz) = g.objective;
        g.cats[2].x = ox + 2.0; g.cats[2].z = oz; g.cats[2].player = false;
        g.step(1.0 / 30.0);
        assert!(g.cats[2].safe && g.escaped >= 1, "a cat at the rendezvous should escape");
    }

    #[test]
    fn sim_invariants_hold_every_tick() {
        // Property check: run every archetype/mode and assert the core invariants on
        // each tick. Catches subtle logic slips (a cat both safe and caught, stamina
        // drifting out of range, an actor escaping the map, counts going backwards).
        for k in [Kind::Fox, Kind::Dog, Kind::Badger, Kind::Hawk, Kind::Snake, Kind::Shadow] {
            for m in [Mode::Rendezvous, Mode::Survive] {
                let mut g = Game::new_match(k, m);
                let bound = MAP + 1.0;
                let (mut prev_caught, mut prev_escaped) = (0u32, 0u32);
                let mut t = 0i32;
                while g.outcome.is_none() && t < 95 * 30 {
                    g.set_input(0, (t as f32 * 0.3).sin(), (t as f32 * 0.2).cos(), t % 30 < 6, t % 50 == 0, t % 70 == 0);
                    g.step(1.0 / 30.0);
                    t += 1;

                    let mut safe_n = 0u32; let mut dead_n = 0u32;
                    for c in &g.cats {
                        assert!(c.stamina >= 0.0 && c.stamina <= 1.0, "stamina out of range: {}", c.stamina);
                        assert!(c.x.is_finite() && c.z.is_finite(), "cat position not finite");
                        assert!(c.x.abs() <= bound && c.z.abs() <= bound, "cat left the map: {},{}", c.x, c.z);
                        assert!(!(c.safe && !c.alive), "a cat is both safe and caught");
                        if c.safe { safe_n += 1; }
                        if !c.alive { dead_n += 1; }
                    }
                    assert!(g.hunter.x.abs() <= bound && g.hunter.z.abs() <= bound, "hunter left the map");
                    assert!(g.hunter.x.is_finite() && g.hunter.z.is_finite(), "hunter position not finite");
                    assert_eq!(g.escaped, safe_n, "escaped count desynced");
                    assert_eq!(g.caught, dead_n, "caught count desynced");
                    assert!(g.escaped + g.caught <= N_CATS as u32, "more resolved cats than exist");
                    assert!(g.caught >= prev_caught && g.escaped >= prev_escaped, "resolved counts went backwards");
                    prev_caught = g.caught; prev_escaped = g.escaped;
                }
                // once resolved, the outcome is frozen and stepping is a no-op
                let snap = (g.outcome, g.hunter.x, g.hunter.z, g.caught);
                g.step(1.0 / 30.0);
                assert_eq!((g.outcome, g.hunter.x, g.hunter.z, g.caught), snap, "a finished round kept simulating");
            }
        }
    }

    #[test]
    fn round_resolves_to_an_outcome() {
        let mut g = Game::new_6v1(Kind::Dog);
        for _ in 0..(95 * 30) { if g.outcome.is_some() { break; } g.step(1.0 / 30.0); }
        assert!(g.outcome.is_some(), "the round must resolve to a winner");
    }

    #[test]
    fn every_hunter_and_mode_soaks_without_panic() {
        // Full rounds for all six archetypes in both modes, with a live player wiggling
        // through move/dodge/crouch/climb — shakes out panics in less-common branches
        // and confirms the round always terminates. Also exercises the ecology at scale.
        let hunters = [Kind::Fox, Kind::Dog, Kind::Badger, Kind::Hawk, Kind::Snake, Kind::Shadow];
        for k in hunters {
            for m in [Mode::Rendezvous, Mode::Survive] {
                let mut g = Game::new_match(k, m);
                let mut t = 0i32;
                while g.outcome.is_none() && t < 95 * 30 {
                    let mx = (t as f32 * 0.30).sin();
                    let mz = (t as f32 * 0.21).cos();
                    g.set_input(0, mx, mz, t % 40 < 8, t % 55 == 0, t % 63 == 0);   // crouch bursts, roll, occasional leap
                    g.step(1.0 / 30.0);
                    t += 1;
                }
                assert!(g.outcome.is_some(), "hunter {:?} / mode {:?} should resolve", k, m);
            }
        }
    }

    #[test]
    fn sprinting_drains_stamina() {
        let mut g = Game::new_6v1(Kind::Fox);
        g.hunter.x = 5.0; g.hunter.z = 0.0;        // right on cat 1 → it sprints to flee
        g.cats[1].x = 0.0; g.cats[1].z = 0.0; g.cats[1].player = false; g.bushes.clear();
        let s0 = g.cats[1].stamina;
        for _ in 0..30 { g.step(1.0 / 30.0); }
        assert!(g.cats[1].stamina < s0, "fleeing should burn stamina");
    }

    #[test]
    fn hunter_tracks_scent_when_blind() {
        let mut g = Game::new_6v1(Kind::Badger); // biggest nose
        for c in g.cats.iter_mut() { c.x = 500.0; c.z = 500.0; c.hidden = true; c.player = false; } // nothing visible
        g.hunter.x = 0.0; g.hunter.z = 0.0; g.hunter.heading = 0.0;
        g.scent.push(Scent { x: 0.0, z: 8.0, strength: 1.0 });   // a fresh trail ahead
        let d0 = dist(0.0, 0.0, 0.0, 8.0);
        for _ in 0..20 { g.step(1.0 / 30.0); }
        let d1 = dist(g.hunter.x, g.hunter.z, 0.0, 8.0);
        assert!(d1 < d0, "a blind hunter should follow a scent it can smell ({d0} -> {d1})");
    }

    #[test]
    fn climbing_escapes_ground_hunter_but_not_the_hawk() {
        let setup = |kind| {
            let mut g = Game::new_6v1(kind);
            g.trees = vec![(0.0, 0.0)];                 // a climbable trunk at the origin
            g.cats[0].x = 0.0; g.cats[0].z = 0.0;       // player cat stands on it
            g.hunter.x = 0.0; g.hunter.z = 0.0; g.hunter.state = HState::Chase; g.hunter.suspicion = 1.2; g.hunter.last_seen = Some((0.0, 0.0));
            g.set_input(0, 0.0, 0.0, true, false, false);             // crouch at a trunk → climb
            g.step(1.0 / 30.0);
            g
        };
        let fox = setup(Kind::Fox);
        assert!(fox.cats[0].climbing && fox.cats[0].alive, "a climbing cat is safe from a ground hunter");
        let hawk = setup(Kind::Hawk);
        assert!(hawk.cats[0].is_carried() || !hawk.cats[0].alive, "the hawk plucks a climbing cat off the trunk (grabs/carries it)");
    }

    #[test]
    fn leap_is_a_fast_committed_bound() {
        // a leap step covers more ground than a plain run and is uncatchable mid-air
        let run_dz = {
            let mut r = Game::new_6v1(Kind::Fox);
            r.bushes.clear(); r.trees.clear();
            r.cats[0].x = 0.0; r.cats[0].z = 0.0;
            r.set_input(0, 0.0, 1.0, false, false, false);   // plain run +z
            r.step(1.0 / 30.0);
            r.cats[0].z
        };
        let mut g = Game::new_6v1(Kind::Fox);
        g.bushes.clear(); g.trees.clear();
        g.cats[0].x = 0.0; g.cats[0].z = 0.0; g.cats[0].heading = 0.0;
        g.set_input(0, 0.0, 1.0, false, false, true);        // leap +z
        g.step(1.0 / 30.0);
        assert!(g.cats[0].is_leaping(), "the leap should engage");
        assert!(g.cats[0].z > run_dz + 1e-3, "a leap covers more ground than a run ({} vs {})", g.cats[0].z, run_dz);

        // point-blank aggressive hunter can't take an airborne cat
        g.hunter.x = g.cats[0].x; g.hunter.z = g.cats[0].z;
        g.hunter.state = HState::Chase; g.hunter.suspicion = 1.3;
        g.set_input(0, 0.0, 1.0, false, false, false);
        g.step(1.0 / 30.0);
        assert!(g.cats[0].alive && g.cats[0].is_leaping(), "a leaping cat survives a point-blank frame");
    }

    #[test]
    fn hawk_grabs_carries_then_resolves() {
        let mut g = Game::new_6v1(Kind::Hawk);
        g.bushes.clear();
        g.cats[0].x = 0.0; g.cats[0].z = 0.0; g.cats[0].player = false;
        g.hunter.x = 0.0; g.hunter.z = 0.0; g.hunter.state = HState::Chase; g.hunter.suspicion = 1.3;
        for _ in 0..6 { g.step(1.0 / 30.0); if g.hunter.carrying { break; } }
        assert!(g.hunter.carrying && g.cats[0].is_carried() && g.cats[0].alive, "the hawk hoists open prey aloft");
        // the struggle resolves within CARRY_DUR: taken (dead) or wrenched free (alive+crippled)
        for _ in 0..120 { g.step(1.0 / 30.0); if !g.hunter.carrying { break; } }
        assert!(!g.hunter.carrying, "the carry resolves and frees the hawk");
        assert!(!g.cats[0].alive || g.cats[0].crippled_t > 0.0, "resolved to taken, or broke free and is crippled");
    }

    #[test]
    fn leaping_cat_pounces_prey() {
        let mut g = Game::new_6v1(Kind::Fox);
        g.cats[0].x = 0.0; g.cats[0].z = 0.0; g.cats[0].heading = 0.0; g.cats[0].stamina = 0.5;
        g.critters.clear();
        g.critters.push(crate::ecology::Critter::new(crate::ecology::Species::Hare, 0.0, 1.0, 3));
        let s0 = g.cats[0].stamina;
        g.set_input(0, 0.0, 1.0, false, false, true);        // leap onto the critter
        g.step(1.0 / 30.0);
        assert!(g.critters.is_empty(), "a leaping cat snatches the critter it lands on");
        assert!(g.cats[0].stamina > s0, "a caught critter tops up stamina");
        assert!(g.prey_caught >= 1, "the pounce is tallied");
    }

    #[test]
    fn snake_ambushes_from_water() {
        let mut g = Game::new_6v1(Kind::Snake);
        let home = g.snake_home.unwrap_or((g.hunter.x, g.hunter.z));
        g.snake_home = Some(home);
        g.hunter.x = home.0; g.hunter.z = home.1;
        g.hunter.state = HState::Chase; g.hunter.suspicion = 1.2;
        g.hunter.last_seen = Some((home.0 + 5.0, home.1));
        let hx0 = g.hunter.x;
        g.step(1.0 / 30.0);
        // a plain 11 u/s snake moves ~0.37 per tick; the 1.8x strike moves ~0.66
        assert!(g.hunter.x - hx0 > 0.5, "the snake should lunge from its pool with a burst");
    }

    #[test]
    fn dodge_gives_iframes() {
        let mut g = Game::new_6v1(Kind::Fox);
        g.hunter.x = 0.0; g.hunter.z = 0.0; g.hunter.state = HState::Chase; g.hunter.suspicion = 1.2; g.hunter.last_seen = Some((0.5, 0.0));
        g.bushes.clear(); g.trees.clear();
        g.cats[0].x = 0.5; g.cats[0].z = 0.0;       // player cat point-blank, in the open
        g.set_input(0, 1.0, 0.0, false, true, false);       // roll east on the catch frame
        g.step(1.0 / 30.0);
        assert!(g.cats[0].dodge_t > 0.0 && g.cats[0].alive, "a dodging cat survives a point-blank catch");
    }

    #[test]
    fn survive_mode_has_no_escape() {
        let mut g = Game::new_match(Kind::Snake, Mode::Survive);
        assert_eq!(g.mode, Mode::Survive);
        for _ in 0..300 { if g.outcome.is_some() { break; } g.step(1.0 / 30.0); }
        assert_eq!(g.escaped, 0, "there is no rendezvous escape in survive mode");
    }

    #[test]
    fn shadow_blinks_to_the_quarry() {
        let mut g = Game::new_match(Kind::Shadow, Mode::Rendezvous);
        g.teleport_cd = 0.0;                      // blink ready
        g.hunter.x = 0.0; g.hunter.z = 0.0;
        g.hunter.last_seen = Some((80.0, 0.0));   // prey slipped far across the map
        g.step(1.0 / 30.0);
        assert!(dist(g.hunter.x, g.hunter.z, 80.0, 0.0) < 25.0, "the shadow should teleport to its quarry");
    }

    #[test]
    fn shadow_shift_reveals_hiders() {
        // the shadow can't see a hidden cat normally, but the otherworld shift reveals it
        let reveals = |shifted: bool| {
            let mut g = Game::new_match(Kind::Shadow, Mode::Rendezvous);
            g.shifted = shifted; g.shift_t = 0.0;
            g.bushes = vec![(0.0, 12.0, 3.0)];   // cover the cat actually hides in
            for c in g.cats.iter_mut() { c.x = 500.0; c.z = 500.0; c.player = false; }
            g.hunter.x = 0.0; g.hunter.z = 0.0; g.hunter.heading = 0.0;
            g.cats[1].x = 0.0; g.cats[1].z = 12.0; g.cats[1].hidden = true; g.cats[1].moving = false; g.cats[1].player = false;
            g.step(1.0 / 30.0);
            g.hunter.last_seen.is_some()
        };
        assert!(!reveals(false), "a hider is unseen in the living world");
        assert!(reveals(true), "the otherworld shift reveals the hider");
    }
}
