//! locomotion_sim — a deterministic, headless mini-world for validating fauna
//! locomotion *as a whole*, by running the same pure pieces the bin runs:
//! [`sighted_step`](crate::sim::fall::sighted_step) for terrain-aware walking,
//! [`GroundProbe`](crate::sim::collision_probe::GroundProbe) for lean/roughness,
//! [`Leap`](crate::sim::leap::Leap) + a ballistic air state for parkour,
//! [`ForageMemory`](crate::sim::forage_memory::ForageMemory) for hunger-driven
//! goal seeking, the [`stumble`](crate::sim::stumble) model, and
//! [`resolve_overlap`](crate::sim::body_collision::resolve_overlap) for body
//! collision. The bin wires these into `main()` where they can't be tested; this
//! reproduces the wiring in a pure loop so scenarios (cross a gap to food, get
//! blocked by a parked mob, stumble off a cliff, spawn without stacking) are
//! reproducible unit tests — and can be recorded to a trace for visual review.

use std::collections::HashSet;

use crate::sim::body_collision::resolve_overlap;
use crate::sim::collision_probe::GroundProbe;
use crate::sim::fall::{sighted_step, SAFE_FALL_BLOCKS, STEP_UP_BLOCKS};
use crate::sim::feeding::{Vitality, GRASS_FOOD_VALUE, STARVE_DECAY_PER_SEC};
use crate::sim::forage_memory::ForageMemory;
use crate::sim::leap::{Leap, GRAVITY};
use crate::sim::posture::Posture;
use crate::sim::stumble::{stumble_kick, stumble_risk, trips, Stumble, STUMBLE_RECOVER_TICKS};

/// Minimum centre-to-centre distance two bodies are kept apart.
pub const BODY_RADIUS: f32 = 0.8;
/// Furthest forward (blocks) a creature scans for a leap landing.
pub const MAX_JUMP_SCAN: i32 = 6;
/// Dropping this far below the last ground a creature stood on = it fell off.
pub const FELL_DROP: f32 = 6.0;
/// Uncontrolled forward/side slide a stumble imparts (can send it over an edge).
pub const LURCH_DIST: f32 = 0.7;

/// One creature in the mini-world. A point body at `pos` (foot height = `pos[1]`).
#[derive(Clone)]
pub struct Agent {
    pub id: u32,
    pub pos: [f32; 3],
    pub yaw: f32,
    pub heading: f32,
    /// Base ground speed (blocks/s).
    pub speed: f32,
    pub leap: Leap,
    pub airborne: Option<([f32; 3], u16)>,
    pub forage: ForageMemory,
    pub vit: Vitality,
    pub stumble: Stumble,
    pub posture: Posture,
    pub mass: f32,
    pub surefoot: f32,
    /// Drive mode. `Some(h)` = always march along heading `h` (e.g. straight at a
    /// cliff). `None` = forage: seek remembered food when hungry, and **stop** when
    /// on food or sated (no aimless coasting).
    pub march_heading: Option<f32>,
    /// Once it has dropped off an edge it is inert (sits where it landed).
    pub fallen: bool,
    /// Highest ground it has recently stood on (for fall detection).
    last_ground_y: f32,
}

impl Agent {
    pub fn new(id: u32, pos: [f32; 3], speed: f32, leap: Leap, mass: f32) -> Agent {
        Agent {
            id,
            pos,
            yaw: 0.0,
            heading: 0.0,
            speed,
            leap,
            airborne: None,
            forage: ForageMemory::new(16),
            vit: Vitality::full(),
            stumble: Stumble::new(),
            posture: Posture::new(0.0),
            mass,
            surefoot: 0.3,
            march_heading: None,
            fallen: false,
            last_ground_y: pos[1],
        }
    }

    /// `true` if it leaps at all (the bin's agility gate: a fast leaper).
    pub fn agile(&self) -> bool {
        self.leap.forward > 5.0
    }
    pub fn xz(&self) -> (f32, f32) {
        (self.pos[0], self.pos[2])
    }
}

/// A snapshot of one agent at one tick (for the recorded trace / viewer).
#[derive(Clone, Copy)]
pub struct AgentSnap {
    pub id: u32,
    pub pos: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    pub airborne: bool,
    pub stumbling: bool,
    pub fallen: bool,
}

/// The mini-world: a heightmap (standable block Y per column, `None` = void), a
/// set of food cells, the agents, and counters/trace for assertions and review.
pub struct World {
    surface: Box<dyn Fn(i32, i32) -> Option<i32> + Send>, // + Send (S1): Service: Send
    pub grass: HashSet<(i32, i32)>,
    pub agents: Vec<Agent>,
    pub tick: u32,
    pub trips: u32,
    pub fell: u32,
    pub recording: bool,
    pub frames: Vec<Vec<AgentSnap>>,
}

impl World {
    pub fn new(
        surface: impl Fn(i32, i32) -> Option<i32> + Send + 'static,
        grass: HashSet<(i32, i32)>,
        agents: Vec<Agent>,
    ) -> World {
        World {
            surface: Box::new(surface),
            grass,
            agents,
            tick: 0,
            trips: 0,
            fell: 0,
            recording: false,
            frames: Vec::new(),
        }
    }

    pub fn surface_block(&self, x: i32, z: i32) -> Option<i32> {
        (self.surface)(x, z)
    }

    /// Advance every agent one fixed step, then resolve body overlaps, then
    /// (optionally) record a trace frame.
    pub fn step(&mut self, dt: f32) {
        self.tick = self.tick.wrapping_add(1);
        let tick = self.tick;
        for i in 0..self.agents.len() {
            // Split the borrow: surface is read-only, agent is mutated.
            let surf = &self.surface;
            let (tripped, fell_now) = update_agent(&mut self.agents[i], surf, &self.grass, tick, dt);
            if tripped {
                self.trips += 1;
            }
            if fell_now {
                self.fell += 1;
            }
        }
        self.resolve_collisions();
        if self.recording {
            self.record_frame();
        }
    }

    /// Run `n` steps.
    pub fn run(&mut self, n: u32, dt: f32) {
        for _ in 0..n {
            self.step(dt);
        }
    }

    /// Push apart any agents closer than [`BODY_RADIUS`], split by mass. A couple
    /// of relaxation passes settle chains (a queue behind a blocker).
    fn resolve_collisions(&mut self) {
        let n = self.agents.len();
        for _ in 0..3 {
            for a in 0..n {
                for b in (a + 1)..n {
                    if self.agents[a].fallen || self.agents[b].fallen {
                        continue;
                    }
                    let (pa, pb) = (self.agents[a].xz(), self.agents[b].xz());
                    let (ma, mb) = (self.agents[a].mass, self.agents[b].mass);
                    let (na, nb) = resolve_overlap(pa, ma, pb, mb, BODY_RADIUS);
                    self.agents[a].pos[0] = na.0;
                    self.agents[a].pos[2] = na.1;
                    self.agents[b].pos[0] = nb.0;
                    self.agents[b].pos[2] = nb.1;
                }
            }
        }
    }

    fn record_frame(&mut self) {
        let snaps = self
            .agents
            .iter()
            .map(|a| AgentSnap {
                id: a.id,
                pos: a.pos,
                yaw: a.yaw,
                pitch: a.posture.pitch,
                roll: a.posture.roll,
                airborne: a.airborne.is_some(),
                stumbling: a.stumble.is_stumbling(),
                fallen: a.fallen,
            })
            .collect();
        self.frames.push(snaps);
    }

    /// Closest pairwise separation right now (for spawn-overlap assertions).
    pub fn min_separation(&self) -> f32 {
        let mut m = f32::INFINITY;
        for a in 0..self.agents.len() {
            for b in (a + 1)..self.agents.len() {
                m = m.min(crate::sim::body_collision::separation(self.agents[a].xz(), self.agents[b].xz()));
            }
        }
        m
    }

    /// Emit the recorded trace as JSON (terrain heightmap over the given inclusive
    /// cell range + every frame's agent snapshots), for the HTML viewer. `None`
    /// columns are encoded as the JSON `null`.
    pub fn trace_json(&self, x0: i32, x1: i32, z0: i32, z1: i32) -> String {
        let mut s = String::from("{\n");
        s.push_str(&format!("\"x0\":{x0},\"x1\":{x1},\"z0\":{z0},\"z1\":{z1},\n"));
        // heightmap: rows of z, columns of x
        s.push_str("\"height\":[");
        for z in z0..=z1 {
            if z > z0 {
                s.push(',');
            }
            s.push('[');
            for x in x0..=x1 {
                if x > x0 {
                    s.push(',');
                }
                match self.surface_block(x, z) {
                    Some(h) => s.push_str(&h.to_string()),
                    None => s.push_str("null"),
                }
            }
            s.push(']');
        }
        s.push_str("],\n");
        // grass cells
        s.push_str("\"grass\":[");
        for (i, (gx, gz)) in self.grass.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push_str(&format!("[{gx},{gz}]"));
        }
        s.push_str("],\n");
        // frames
        s.push_str("\"frames\":[");
        for (fi, frame) in self.frames.iter().enumerate() {
            if fi > 0 {
                s.push(',');
            }
            s.push('[');
            for (ai, a) in frame.iter().enumerate() {
                if ai > 0 {
                    s.push(',');
                }
                s.push_str(&format!(
                    "{{\"id\":{},\"x\":{:.3},\"y\":{:.3},\"z\":{:.3},\"yaw\":{:.3},\"pitch\":{:.3},\"roll\":{:.3},\"air\":{},\"stum\":{},\"fell\":{}}}",
                    a.id, a.pos[0], a.pos[1], a.pos[2], a.yaw, a.pitch, a.roll,
                    a.airborne as u8, a.stumbling as u8, a.fallen as u8
                ));
            }
            s.push(']');
        }
        s.push_str("]\n}\n");
        s
    }
}

/// One agent's per-tick update. Returns `(tripped_this_tick, fell_this_tick)`.
fn update_agent(
    a: &mut Agent,
    surface: &dyn Fn(i32, i32) -> Option<i32>,
    grass: &HashSet<(i32, i32)>,
    tick: u32,
    dt: f32,
) -> (bool, bool) {
    if a.fallen {
        return (false, false);
    }
    let surf_block = |x: f32, z: f32| surface(x.floor() as i32, z.floor() as i32);
    let surf_foot = |x: f32, z: f32| surf_block(x, z).map(|t| (t + 1) as f32);
    let cell = (a.pos[0].floor() as i32, a.pos[2].floor() as i32);

    // --- Metabolize / graze ---
    a.vit.tick(dt, STARVE_DECAY_PER_SEC);
    if grass.contains(&cell) {
        a.vit.feed(GRASS_FOOD_VALUE * dt * 4.0);
        a.forage.remember(cell, tick);
    }

    // --- Airborne: integrate the leap (or free-fall) arc ---
    if let Some((v, aloft0)) = a.airborne {
        a.pos[0] += v[0] * dt;
        a.pos[1] += v[1] * dt;
        a.pos[2] += v[2] * dt;
        let nvy = v[1] - GRAVITY * dt;
        let aloft = aloft0.saturating_add(1);
        a.airborne = Some(([v[0], nvy, v[2]], aloft));
        let mut fell_now = false;
        let on_ground = nvy < 0.0
            && surf_foot(a.pos[0], a.pos[2]).map(|f| a.pos[1] <= f).unwrap_or(false);
        if a.pos[1] < a.last_ground_y - FELL_DROP {
            a.fallen = true;
            a.airborne = None;
            fell_now = true;
        } else if on_ground || aloft > 80 {
            if let Some(f) = surf_foot(a.pos[0], a.pos[2]) {
                a.pos[1] = f;
                a.last_ground_y = f;
                a.airborne = None;
            } else if aloft > 80 {
                a.fallen = true;
                a.airborne = None;
                fell_now = true;
            }
        }
        return (false, fell_now);
    }

    // --- Posture lean + footing roughness (one shared probe) ---
    let probe = GroundProbe::sample(a.pos[0], a.pos[2], a.yaw, 1.0, a.pos[1], surf_foot);
    let (tp, tr) = probe.lean_default();
    a.posture.settle_to_terrain(tp, tr, 4.0, dt);
    let roughness = probe.roughness();

    // --- Drive: march along a fixed heading, or forage (seek food, stop when fed) ---
    let on_food = grass.contains(&cell);
    let mut tripped = false;
    let moving = if let Some(mh) = a.march_heading {
        a.heading = mh;
        true
    } else if a.vit.is_hungry() && !on_food {
        if let Some((gx, gz)) = a.forage.nearest(a.xz(), tick, 4000).map(|c| (c.0 as f32 + 0.5, c.1 as f32 + 0.5)) {
            a.heading = (gz - a.pos[2]).atan2(gx - a.pos[0]);
            true
        } else {
            false
        }
    } else {
        false // on food or sated → stand and graze, don't coast off
    };

    let mut actual_speed = 0.0;
    if moving {
        let speed_mult = a.stumble.tick();
        let step = a.speed * speed_mult * dt;
        let (hc, hs) = (a.heading.cos(), a.heading.sin());
        // Is the ground one block ahead walkable (step up ≤1, drop ≤ safe-fall)? A
        // void or a tall wall is "blocked" and (if agile) prompts a leap.
        let forward_walkable = match surf_foot(a.pos[0] + hc, a.pos[2] + hs) {
            Some(f) => {
                let rise = f - a.pos[1];
                rise <= STEP_UP_BLOCKS as f32 + 1e-3 && rise >= -(SAFE_FALL_BLOCKS as f32)
            }
            None => false,
        };

        let mut launched = false;
        if !forward_walkable && a.agile() {
            // Scan for a real landing ahead and leap toward it (skip voids / deep pits).
            for d in 1..=MAX_JUMP_SCAN {
                let df = d as f32;
                if let Some(f) = surf_foot(a.pos[0] + hc * df, a.pos[2] + hs * df) {
                    let rise = f - a.pos[1];
                    if rise >= -(SAFE_FALL_BLOCKS as f32)
                        && rise < 8.0
                        && a.leap.clears_gap(df, rise, GRAVITY)
                    {
                        a.airborne = Some(([hc * a.leap.forward, a.leap.up, hs * a.leap.forward], 0));
                        launched = true;
                        break;
                    }
                }
            }
        }

        if !launched {
            let (dx, dz) = sighted_step(a.xz(), a.heading, step, 100, 0.0, surf_block);
            if dx != 0.0 || dz != 0.0 {
                a.pos[0] += dx;
                a.pos[2] += dz;
                actual_speed = (dx * dx + dz * dz).sqrt() / dt.max(1e-4);
                if let Some(f) = surf_foot(a.pos[0], a.pos[2]) {
                    let gap = f - a.pos[1];
                    if gap.abs() > 2.0 {
                        a.pos[1] = f;
                    } else {
                        a.pos[1] += gap * (dt * 12.0).min(1.0);
                    }
                    a.last_ground_y = f;
                }
                a.yaw = crate::sim::awareness::turn_toward(a.yaw, a.heading, a.speed.max(2.0) * dt);
            }

            // --- Stumble: fast movement over broken ground trips, deterministically ---
            let risk = stumble_risk(actual_speed, roughness, a.surefoot);
            if risk > 0.0 {
                let seed = (tick as u64) << 40
                    ^ (a.id as u64) << 20
                    ^ (cell.0 as u32 as u64)
                    ^ ((cell.1 as u32 as u64) << 10);
                if trips(risk, seed) {
                    a.stumble.trigger(STUMBLE_RECOVER_TICKS);
                    let catch_left = (seed & 1) == 0;
                    let (yk, pk) = stumble_kick(risk, catch_left);
                    a.yaw += yk;
                    a.posture.pitch += pk;
                    // An uncontrolled lurch in the new facing — bypasses the careful
                    // foothold check, so a trip at an edge can carry it over.
                    let (lc, ls) = (a.yaw.cos(), a.yaw.sin());
                    a.pos[0] += lc * LURCH_DIST;
                    a.pos[2] += ls * LURCH_DIST;
                    tripped = true;
                }
            }
        }
    }

    // --- Grounding after the move/lurch: unsupported → start to fall ---
    let mut fell_now = false;
    if a.airborne.is_none() {
        match surf_foot(a.pos[0], a.pos[2]) {
            Some(f) => {
                if f < a.last_ground_y - FELL_DROP {
                    a.fallen = true;
                    fell_now = true;
                } else {
                    a.last_ground_y = a.last_ground_y.max(f);
                }
            }
            None => {
                // Stepped/lurched over an edge: free-fall.
                a.airborne = Some(([0.0, 0.0, 0.0], 0));
            }
        }
    }

    (tripped, fell_now)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_world(agents: Vec<Agent>, grass: HashSet<(i32, i32)>) -> World {
        World::new(|_x, _z| Some(64), grass, agents)
    }

    // A two-platform world with a VOID gap of width `w` between them. Platform A
    // is x in [0,4], the gap is x in [5, 4+w], platform B is x in [5+w, 14]. Food
    // sits at (10,2) on B. z is a wide strip [0,4] so there are no side effects.
    fn gap_world(w: i32, agile: bool) -> World {
        let b_start = 5 + w;
        let surface = move |x: i32, z: i32| {
            if !(0..=4).contains(&z) {
                return None;
            }
            if (0..=4).contains(&x) || (b_start..=14).contains(&x) {
                Some(64)
            } else {
                None // the gap is open (must be jumped)
            }
        };
        let leap = if agile { Leap::from_genes(1.0, 1.0) } else { Leap::from_genes(0.0, 0.0) };
        let mut grass = HashSet::new();
        grass.insert((10, 2));
        let mut a = Agent::new(0, [1.0, 65.0, 2.0], 4.0, leap, 1.0);
        a.vit = Vitality::new(0.3); // hungry → it will seek the food
        a.forage.remember((10, 2), 0); // it knows the patch is over there
        a.heading = 0.0; // facing +X toward B
        World::new(surface, grass, vec![a])
    }

    #[test]
    fn agile_creature_crosses_one_two_three_block_gaps() {
        for w in 1..=3 {
            let mut world = gap_world(w, true);
            world.run(400, 0.05);
            let a = &world.agents[0];
            assert!(!a.fallen, "gap {w}: should not have fallen in");
            assert!(a.pos[0] >= (5 + w) as f32, "gap {w}: should reach platform B, x={}", a.pos[0]);
        }
    }

    #[test]
    fn non_leaper_is_stuck_on_the_near_side() {
        let mut world = gap_world(2, false);
        world.run(400, 0.05);
        let a = &world.agents[0];
        assert!(!a.fallen, "a careful walker won't step into the void");
        assert!(a.pos[0] <= 5.0, "no leap ⇒ stays on platform A, x={}", a.pos[0]);
    }

    #[test]
    fn a_gap_too_wide_is_not_crossed() {
        // B starts at x=12; even a strong leaper can't reach it from the edge.
        let mut world = gap_world(7, true);
        world.run(400, 0.05);
        let a = &world.agents[0];
        assert!(a.pos[0] < 12.0, "too-wide gap ⇒ never reaches B, x={}", a.pos[0]);
    }

    #[test]
    fn spawned_agents_never_stack_and_stay_grounded() {
        // Five agents on a small flat patch; two deliberately spawned ON TOP of
        // each other. They must separate and all stay at foot height 65 (no sink,
        // no fly) for the whole 10 s (sampled every 10 ticks).
        let mut agents = Vec::new();
        let leap = Leap::from_genes(0.5, 0.5);
        for (i, p) in [[2.0, 65.0, 2.0], [4.0, 65.0, 2.0], [2.0, 65.0, 4.0], [6.0, 65.0, 6.0], [6.0, 65.0, 6.0]]
            .into_iter()
            .enumerate()
        {
            let mut a = Agent::new(i as u32, p, 0.0, leap, 1.0); // speed 0: they just stand
            a.vit = Vitality::full(); // not hungry → no wandering
            agents.push(a);
        }
        let mut world = flat_world(agents, HashSet::new());
        // The two stacked spawns overlap at t0; one collision pass should separate.
        world.step(0.05);
        assert!(world.min_separation() > BODY_RADIUS - 1e-3, "must un-stack, sep={}", world.min_separation());
        for _ in 0..20 {
            world.run(10, 0.05); // 10 ticks between samples, 200 total = 10 s
            assert!(world.min_separation() > BODY_RADIUS - 1e-2, "stay separated");
            for a in &world.agents {
                assert!((a.pos[1] - 65.0).abs() < 0.5, "agent {} drifted off the floor: y={}", a.id, a.pos[1]);
                assert!(!a.fallen);
            }
        }
    }

    #[test]
    fn a_parked_heavy_mob_blocks_a_corridor() {
        // 1-wide corridor along x at z=2 (z=1 and z=3 are walls/void). A massive,
        // motionless mob sits at x=6. A light mover starts at x=2 and wants food
        // at x=10 — it must not get past the blocker.
        let surface = |_x: i32, z: i32| if z == 2 { Some(64) } else { None };
        let mut grass = HashSet::new();
        grass.insert((10, 2));
        let blocker = {
            let mut b = Agent::new(99, [6.0, 65.0, 2.5], 0.0, Leap::from_genes(0.0, 0.0), 1.0e6);
            b.vit = Vitality::full();
            b
        };
        let mover = {
            let mut m = Agent::new(0, [2.0, 65.0, 2.5], 4.0, Leap::from_genes(0.0, 0.0), 1.0);
            m.vit = Vitality::new(0.3);
            m.forage.remember((10, 2), 0);
            m
        };
        let mut world = World::new(surface, grass, vec![blocker, mover]);
        world.run(400, 0.05);
        let mover = world.agents.iter().find(|a| a.id == 0).unwrap();
        assert!(mover.pos[0] < 6.0, "mover should be stopped before the blocker, x={}", mover.pos[0]);

        // Control: without the blocker it reaches the food.
        let surface2 = |_x: i32, z: i32| if z == 2 { Some(64) } else { None };
        let mut g2 = HashSet::new();
        g2.insert((10, 2));
        let mut m2 = Agent::new(0, [2.0, 65.0, 2.5], 4.0, Leap::from_genes(0.0, 0.0), 1.0);
        m2.vit = Vitality::new(0.3);
        m2.forage.remember((10, 2), 0);
        let mut world2 = World::new(surface2, g2, vec![m2]);
        world2.run(400, 0.05);
        assert!(world2.agents[0].pos[0] > 9.0, "no blocker ⇒ reaches food, x={}", world2.agents[0].pos[0]);
    }

    // A small chaotic height bump (0..=2 blocks) that genuinely differs between
    // neighbours, so terrain_roughness reads it as broken ground (a parity jitter
    // would cancel in the front-back / left-right differences).
    fn bump(x: i32, z: i32) -> i32 {
        let h = (x as i64).wrapping_mul(73856093) ^ (z as i64).wrapping_mul(19349663);
        ((h as u64 >> 16) % 3) as i32
    }

    // A plateau at y=70 with the cliff edge running along +z: ground exists for
    // z in [0, EDGE], open air beyond. Agents run *along* the lip (in +x) over
    // rough ground, so a stumble toward the edge can carry one over while most
    // stay on. `rough=false` (and no edge) is the safe control.
    fn cliff_world(n: u32, rough: bool) -> World {
        const EDGE: i32 = 20;
        let surface = move |x: i32, z: i32| {
            if !(0..=EDGE).contains(&z) || !(0..=400).contains(&x) {
                return None; // the drop (beyond the lip)
            }
            if rough {
                Some(70 + bump(x, z))
            } else {
                Some(70)
            }
        };
        let mut agents = Vec::new();
        let leap = Leap::from_genes(0.2, 0.2); // not agile ⇒ they won't leap edges
        for i in 0..n {
            // Spread across the lip: some right at the edge (z≈20), some inland.
            let z = 14.0 + (i % 7) as f32;
            let mut a = Agent::new(i, [2.0, 71.0, z], 6.0, leap, 1.0);
            a.surefoot = 0.3;
            a.march_heading = Some(0.0); // run along the cliff (+x)
            a.vit = Vitality::full();
            agents.push(a);
        }
        World::new(surface, HashSet::new(), agents)
    }

    // Same rough ground and herd, but the plateau extends well past the lane
    // (no adjacent drop): they still trip, but there is nowhere to fall.
    fn rough_flat_world(n: u32) -> World {
        let surface = move |x: i32, z: i32| {
            if !(0..=200).contains(&z) || !(0..=400).contains(&x) {
                return None;
            }
            Some(70 + bump(x, z))
        };
        let mut agents = Vec::new();
        let leap = Leap::from_genes(0.2, 0.2);
        for i in 0..n {
            let z = 14.0 + (i % 7) as f32;
            let mut a = Agent::new(i, [2.0, 71.0, z], 6.0, leap, 1.0);
            a.surefoot = 0.3;
            a.march_heading = Some(0.0);
            a.vit = Vitality::full();
            agents.push(a);
        }
        World::new(surface, HashSet::new(), agents)
    }

    #[test]
    fn a_stumble_can_carry_an_animal_off_a_cliff_but_rarely() {
        // Flat ground at speed never trips.
        let mut flat = cliff_world(28, false);
        flat.run(100, 0.05);
        assert_eq!(flat.trips, 0, "flat ground never trips");

        // The same rough ground and herd, but with no adjacent drop: they trip,
        // yet there is nowhere to fall — so a trip alone doesn't kill.
        let mut safe = rough_flat_world(28);
        safe.run(160, 0.05);
        assert!(safe.trips > 0, "fast over broken ground should trip");
        let safe_fell = safe.agents.iter().filter(|a| a.fallen).count();
        assert_eq!(safe_fell, 0, "tripping on safe rough ground doesn't drop anyone");

        // Rough ground *along a cliff lip*: a few stumbles carry animals over the
        // edge — a believable minority, not the whole herd. Deterministic, so the
        // observed rate (≈1–2 of 28 here, ~5%) is asserted as a band.
        let mut cliff = cliff_world(28, true);
        cliff.run(160, 0.05);
        assert!(cliff.trips > 0, "rough lip should trip");
        let n = cliff.agents.len();
        let fell = cliff.agents.iter().filter(|a| a.fallen).count();
        assert!(fell > 0, "a stumble should be able to carry one over the edge");
        assert!(fell <= n / 5, "but only a minority go over ({fell}/{n})");
    }
}

// ── S1 (r94): register as a sim::service_registry service (co-located) ───────
// Adapter ticks a minimal NEUTRAL world: flat ground (y=64), no grass, one idle
// agent. Real terrain surface + populated agents are a later wiring pass.
struct LocomotionService(World);
impl crate::sim::service_registry::Service for LocomotionService {
    fn name(&self) -> &'static str { "locomotion_sim" }
    fn tick(&mut self, ctx: &crate::sim::service_registry::ServiceCtx) {
        self.0.step(ctx.dt);
    }
}
fn make_neutral_locomotion_world() -> World {
    let agents = vec![Agent::new(0, [0.0, 65.0, 0.0], 0.0, Leap::from_genes(0.0, 0.0), 1.0)];
    World::new(|_x, _z| Some(64), std::collections::HashSet::new(), agents)
}
#[linkme::distributed_slice(crate::sim::service_registry::SERVICES)]
static LOCOMOTION_SERVICE: crate::sim::service_registry::ServiceEntry =
    crate::sim::service_registry::ServiceEntry {
        name: "locomotion_sim",
        make: || Box::new(LocomotionService(make_neutral_locomotion_world())),
    };

#[cfg(test)]
mod s1_wiring_tests {
    use crate::sim::service_registry::{Service, ServiceCtx};
    #[test]
    fn locomotion_service_ticks_panic_free() {
        let mut s = super::LocomotionService(super::make_neutral_locomotion_world());
        let ctx = ServiceCtx { dt: 1.0 / 60.0 };
        for _ in 0..240 { s.tick(&ctx); }
        assert_eq!(s.name(), "locomotion_sim");
    }
}
