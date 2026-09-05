//! Feature FSMs: linear landforms as state machines.
//!
//! A river is not a shape. It is a PROCESS: it starts high, runs
//! downhill, pools where the ground flattens, drops where it steepens,
//! widens as it goes, and fans out where it meets the sea. Every step
//! of that is a decision made by reading the terrain at the current
//! position, and the decisions have a small fixed set of outcomes. That
//! is a state machine, and describing it as one is both shorter and
//! more honest than any fixed geometry.
//!
//! THE SHAPE OF EVERY FEATURE HERE
//!
//! * A **state**: what the feature is doing right now (running,
//!   pooling, falling).
//! * A **step**: read the terrain at the head, decide the next state,
//!   emit the ops for this segment, advance the head.
//! * **Transitions** driven by slope, water, and distance -- never by
//!   a coordinate the model wrote.
//!
//! The FSM is generic; `River` is the first feature. A road is the same
//! machine with different states (straight, curve, bridge, cut, tunnel)
//! and different reads (grade, obstacle, water ahead). A wall is
//! (run, tower, gate, corner). They share `Head`, `Reader` and the
//! trace, and each adds its own state enum and step function.
//!
//! EVERYTHING IS BOUNDED
//!
//! A feature has a maximum length. A river that finds no lower ground
//! stops; it does not wander. The trace records every step, so the
//! preview can show the path before a block is placed and the player
//! can see WHY it went where it went.

use crate::VoxelWorld;

/// Where the feature's head is and which way it is going.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Head {
    pub x: f32,
    pub z: f32,
    pub y: i32,
    /// Direction in radians.
    pub dir: f32,
}

/// What a step read from the terrain, for the trace.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub slope_ahead: f32,
    pub lowest_dir: f32,
    pub water_ahead: bool,
    pub height: i32,
}

/// One step of the trace: state, position, reading, and what was
/// emitted. This is how the player sees why the river went where it
/// went.
#[derive(Debug, Clone, PartialEq)]
pub struct Step<S: Copy + std::fmt::Debug> {
    pub state: S,
    pub head: Head,
    pub reading: Reading,
    pub ir: String,
}

/// Read the terrain around a head. Shared by every feature.
pub struct Reader<'a, W: VoxelWorld> {
    pub w: &'a W,
    pub from_y: i32,
    /// Lakes the feature has already placed: (centre x, centre z,
    /// radius, water level). The plan's IR has not run yet, so the
    /// world does not show them -- but the river must see them, or
    /// after overflowing a basin the lowest direction is straight back
    /// down into the basin it just filled.
    pub lakes: std::cell::RefCell<Vec<(f32, f32, f32, i32)>>,
}

impl<'a, W: VoxelWorld> Reader<'a, W> {
    pub fn new(w: &'a W, from_y: i32) -> Self {
        Self { w, from_y, lakes: std::cell::RefCell::new(Vec::new()) }
    }

    pub fn surface(&self, x: i32, z: i32) -> Option<(i32, u16)> {
        let mut y = self.from_y;
        while y > self.from_y - 512 {
            let b = self.w.get(x, y, z);
            if b != 0 { return Some((y, b)); }
            y -= 1;
        }
        None
    }

    /// Terrain height, or a placed lake's water level where one covers
    /// the column -- whichever is higher.
    pub fn height(&self, x: f32, z: f32) -> i32 {
        let ground = self.surface(x.round() as i32, z.round() as i32).map(|(y, _)| y).unwrap_or(0);
        let mut h = ground;
        for (cx, cz, r, level) in self.lakes.borrow().iter() {
            if (x - cx).powi(2) + (z - cz).powi(2) <= r * r { h = h.max(*level); }
        }
        h
    }

    pub fn is_water(&self, x: f32, z: f32) -> bool {
        self.surface(x.round() as i32, z.round() as i32).map(|(_, b)| b == 9).unwrap_or(false)
    }

    /// Sample eight directions at `look` blocks and return the one that
    /// descends most, with the slope along the current heading.
    pub fn read(&self, h: &Head, look: f32) -> Reading {
        let here = self.height(h.x, h.z);
        let ahead = self.height(h.x + h.dir.cos() * look, h.z + h.dir.sin() * look);
        let mut lowest = (h.dir, i32::MAX);
        for k in 0..8 {
            let d = std::f32::consts::TAU * k as f32 / 8.0;
            let hh = self.height(h.x + d.cos() * look, h.z + d.sin() * look);
            // Prefer continuing: ties go to the current heading, so a
            // river on flat ground runs straight rather than jittering.
            let turn = ((d - h.dir + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
                        - std::f32::consts::PI).abs();
            let score = hh * 8 + (turn * 4.0) as i32;
            if score < lowest.1 { lowest = (d, score); }
        }
        Reading {
            slope_ahead: (ahead - here) as f32 / look,
            lowest_dir: lowest.0,
            water_ahead: self.is_water(h.x + h.dir.cos() * look, h.z + h.dir.sin() * look),
            height: here,
        }
    }
}

// ── river ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiverState {
    /// Where it begins: a spring, small.
    Source,
    /// Downhill, the common case.
    Run,
    /// Flat ground: it spreads into a pool.
    Pool,
    /// Steep: a drop, with a plunge pool below.
    Fall,
    /// A pool has filled: leave it over its lowest rim and keep going.
    /// This is what lets a river cross hummocky ground instead of dying
    /// in the first dip.
    Overflow,
    /// Reached water: fan out and stop.
    Delta,
    /// No lower ground found: stop cleanly.
    Ended,
}

#[derive(Debug, Clone)]
pub struct River {
    pub state: RiverState,
    pub head: Head,
    /// Grows with distance.
    pub width: f32,
    pub travelled: f32,
    pub max_len: f32,
    pub trace: Vec<Step<RiverState>>,
    pub seed: u64,
}

impl River {
    pub fn new(x: i32, z: i32, dir: f32, max_len: i32) -> Self {
        Self {
            state: RiverState::Source,
            head: Head { x: x as f32, z: z as f32, y: 0, dir },
            width: 1.0, travelled: 0.0, max_len: max_len.max(10) as f32,
            trace: Vec::new(), seed: 1,
        }
    }

    pub fn done(&self) -> bool { matches!(self.state, RiverState::Delta | RiverState::Ended) }

    /// One step: read, transition, emit, advance.
    pub fn step<W: VoxelWorld>(&mut self, r: &Reader<W>) {
        if self.done() { return; }
        const STEP: f32 = 4.0;
        let reading = r.read(&self.head, STEP * 1.5);
        self.head.y = reading.height;

        // TRANSITIONS. Slope thresholds in blocks per block. These are
        // the whole character of the river: change them and it pools
        // more or falls more, without touching the geometry.
        let next = if reading.water_ahead {
            RiverState::Delta
        } else if self.travelled >= self.max_len {
            RiverState::Ended
        } else if reading.slope_ahead < -1.2 {
            RiverState::Fall
        } else {
            // Does ANY direction descend? Not "is any direction not
            // uphill": on a bowl's flat floor sideways is level, and
            // treating level as a way forward sent the river in circles
            // on the floor for its whole length.
            //
            // No descent means POOL, immediately. An earlier version
            // gave the river a few steps of "momentum" first, and the
            // head climbed the bowl's lip during them, read the bowl as
            // lower, and dropped straight back in. The pool branch is
            // the right place to tell a plain from a basin: it searches
            // for a rim, and a plain has none.
            let alt = r.height(self.head.x + reading.lowest_dir.cos() * STEP * 1.5,
                               self.head.z + reading.lowest_dir.sin() * STEP * 1.5);
            if alt < reading.height { RiverState::Run } else { RiverState::Pool }
        };

        // EMIT for this segment, in the state we are entering.
        let (x, z, y) = (self.head.x.round() as i32, self.head.z.round() as i32, self.head.y);
        let wi = self.width.round() as i32;
        let ir = match next {
            RiverState::Source => format!("sphere {x},{z} r=2 y={} block=air\n\
                                           sphere {x},{z} r=1 y={} block=water\n", y, y - 1),
            RiverState::Run => {
                // A channel: carve a shallow trough and fill it. Width
                // grows downstream.
                let (nx, nz) = (x + (self.head.dir.cos() * STEP).round() as i32,
                                z + (self.head.dir.sin() * STEP).round() as i32);
                format!("path {x},{z} {nx},{nz} w={} block=air\n\
                         path {x},{z} {nx},{nz} w={} block=water\n",
                        wi + 2, wi)
            }
            // The pool's water comes from the fill in the advance step,
            // sized to the basin; nothing to emit on entry.
            RiverState::Pool => String::new(),
            RiverState::Fall => {
                let below = r.height(self.head.x + self.head.dir.cos() * STEP,
                                     self.head.z + self.head.dir.sin() * STEP);
                let (nx, nz) = (x + (self.head.dir.cos() * STEP).round() as i32,
                                z + (self.head.dir.sin() * STEP).round() as i32);
                format!("fill {x},{},{z} {x},{},{z} block=water\n\
                         sphere {nx},{nz} r={} y={} block=air\n\
                         sphere {nx},{nz} r={} y={} block=water\n",
                        below + 1, y, wi + 2, below, wi + 1, below - 1)
            }
            RiverState::Delta => format!("sphere {x},{z} r={} y={} block=water\n", wi + 4, y),
            RiverState::Overflow | RiverState::Ended => String::new(),
        };

        self.trace.push(Step { state: next, head: self.head, reading: reading.clone(), ir });
        self.state = next;

        // ADVANCE. Toward the lowest direction, blended with the current
        // heading so it meanders rather than zigzags. A pool does not
        // advance: the next step reads from the same place and, finding
        // the basin filled to its rim, will run again or end.
        if !self.done() && next != RiverState::Pool {
            let blend = 0.6;
            let dd = ((reading.lowest_dir - self.head.dir + std::f32::consts::PI)
                      .rem_euclid(std::f32::consts::TAU)) - std::f32::consts::PI;
            // NORMALIZED. Without this the heading drifted to -12.9
            // radians after a few turns and every angular comparison
            // downstream -- the back-check, the turn score -- was
            // comparing garbage.
            self.head.dir = (self.head.dir + dd * blend).rem_euclid(std::f32::consts::TAU);
            self.head.x += self.head.dir.cos() * STEP;
            self.head.z += self.head.dir.sin() * STEP;
            self.travelled += STEP;
            self.width = (1.0 + self.travelled / 40.0).min(6.0);
        } else if next == RiverState::Pool {
            // A POOL FILLS. The first version left the water at the
            // floor of the basin and looked for a rim no higher than
            // floor+1, so an 8-deep bowl could never be left: the river
            // bounced around inside it until it ran out of length. Real
            // water rises until it finds the lowest point on the rim --
            // the spill -- and leaves there. So: search outward for the
            // spill height, flood the basin to it, and continue from the
            // spill point. This is what turns a dip into a lake with an
            // outlet rather than a puddle the river drowns in.
            self.travelled += STEP * 2.0;
            let floor = reading.height;
            // THE SPILL IS A PASS, not the nearest higher block. The
            // bowl's floor slopes gently, so the first version found a
            // "rim" one block higher still inside the bowl, set the lake
            // one block deep, and overflowed into the same basin fifteen
            // times. Water leaves a basin over the lowest PASS: walk
            // outward in each direction, track the highest ground
            // crossed, and stop once the ground drops back away from it.
            // The direction whose highest point is lowest is the outlet,
            // and that height is the lake level.
            let mut spill: Option<(f32, f32, i32)> = None;
            for k in 0..16 {
                let d = std::f32::consts::TAU * k as f32 / 16.0;
                let back = ((d - self.head.dir).rem_euclid(std::f32::consts::TAU)
                            - std::f32::consts::PI).abs() < 0.5;
                if back { continue; }
                let mut pass: Option<(f32, f32, i32)> = None;
                let mut highest = floor;
                let mut dist = 2.0;
                while dist <= 48.0 {
                    let (px, pz) = (self.head.x + d.cos() * dist, self.head.z + d.sin() * dist);
                    let h = r.height(px, pz);
                    if h > highest { highest = h; pass = Some((px, pz, h)); }
                    // past the pass: the ground has fallen away from it
                    else if pass.is_some() && h <= highest - 2 { break; }
                    dist += 2.0;
                }
                // only a pass we walked over and DOWN from counts; a ray
                // that only ever climbed is a wall, not an outlet
                if let Some((px, pz, h)) = pass {
                    let beyond = r.height(px + d.cos() * 4.0, pz + d.sin() * 4.0);
                    if beyond < h && spill.map(|s| h < s.2).unwrap_or(true) {
                        spill = Some((px, pz, h));
                    }
                }
            }
            match spill {
                Some((px, pz, level)) if level - floor <= 24 => {
                    let (x, z) = (self.head.x.round() as i32, self.head.z.round() as i32);
                    let radius = (((px - self.head.x).powi(2) + (pz - self.head.z).powi(2)).sqrt()
                                  as i32 + 2).clamp(3, 40);
                    // The lake: flood the basin to the spill level.
                    let ir = format!("lake {x},{z} r={radius} level={}\n", level - 1);
                    if let Some(last) = self.trace.last_mut() { last.ir = ir; }
                    // The river now sees the water it placed.
                    r.lakes.borrow_mut().push((self.head.x, self.head.z, radius as f32, level - 1));
                    self.head.dir = (pz - self.head.z).atan2(px - self.head.x)
                        .rem_euclid(std::f32::consts::TAU);
                    self.head.x = px; self.head.z = pz;
                    self.state = RiverState::Overflow;
                    self.trace.push(Step { state: RiverState::Overflow, head: self.head,
                                           reading: reading.clone(), ir: String::new() });
                }
                // No rim found at all: everything within reach is at or
                // below the floor. That is not a basin, it is a PLAIN, and
                // a river on a plain keeps its heading. Ending here killed
                // every river that crossed a plateau on its way to the
                // edge. A rim that IS found but too tall is a pit: end.
                None => {
                    self.state = RiverState::Run;
                    self.head.x += self.head.dir.cos() * STEP;
                    self.head.z += self.head.dir.sin() * STEP;
                    self.travelled += STEP;
                }
                Some(_) => { self.state = RiverState::Ended; }
            }
        }
    }

    /// Run to completion and return the plan.
    pub fn run<W: VoxelWorld>(&mut self, r: &Reader<W>) -> String {
        let mut guard = 0;
        while !self.done() && guard < 500 {
            self.step(r);
            guard += 1;
        }
        let mut ir = format!("# river from {},{}\n", self.trace.first().map(|s| s.head.x.round() as i32).unwrap_or(0),
                             self.trace.first().map(|s| s.head.z.round() as i32).unwrap_or(0));
        for s in &self.trace { ir.push_str(&s.ir); }
        ir
    }

    /// The path as a readable trace, for the preview.
    pub fn describe(&self) -> String {
        let mut counts = std::collections::BTreeMap::new();
        for s in &self.trace { *counts.entry(format!("{:?}", s.state)).or_insert(0) += 1; }
        let last = self.trace.last();
        format!("river: {} steps, {:.0} blocks long, ending {:?} at {}",
                self.trace.len(), self.travelled, self.state,
                last.map(|s| format!("{},{}", s.head.x.round(), s.head.z.round()))
                    .unwrap_or_default())
            + &counts.iter().map(|(k, v)| format!(", {v} {k}")).collect::<String>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{parse_plan, Severity};
    use std::collections::BTreeMap;

    struct T { f: fn(i32, i32) -> i32, cells: BTreeMap<(i32, i32, i32), u16> }
    impl VoxelWorld for T {
        fn get(&self, x: i32, y: i32, z: i32) -> u16 {
            if let Some(&b) = self.cells.get(&(x, y, z)) { return b; }
            if y <= (self.f)(x, z) { 2 } else { 0 }
        }
        fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.cells.insert((x, y, z), b); true }
    }

    #[test]
    fn a_river_runs_downhill() {
        // ground falling to the east
        let w = T { f: |x, _| 100 - x / 3, cells: BTreeMap::new() };
        let r = Reader::new(&w, 200);
        let mut river = River::new(0, 0, 0.0, 200);
        river.run(&r);
        assert!(river.travelled > 100.0, "{}", river.describe());
        let start = river.trace.first().unwrap().head;
        let end = river.trace.last().unwrap().head;
        assert!(end.x > start.x + 50.0, "went east, downhill: {} -> {}", start.x, end.x);
        assert!(end.y < start.y, "and down: {} -> {}", start.y, end.y);
        assert!(river.trace.iter().filter(|s| s.state == RiverState::Run).count() > 20);
    }

    #[test]
    fn a_river_finds_its_way_when_pointed_uphill() {
        // pointed west, but the land falls east: it should turn.
        let w = T { f: |x, _| 100 - x / 3, cells: BTreeMap::new() };
        let r = Reader::new(&w, 200);
        let mut river = River::new(0, 0, std::f32::consts::PI, 200);
        river.run(&r);
        let end = river.trace.last().unwrap().head;
        assert!(end.x > 20.0, "turned to run east: ended at x={}", end.x);
    }

    #[test]
    fn a_river_pools_in_a_basin_and_ends() {
        // a bowl: everything slopes to the origin
        let w = T { f: |x, z| 64 + ((x * x + z * z) as f32).sqrt() as i32 / 2, cells: BTreeMap::new() };
        let r = Reader::new(&w, 200);
        let mut river = River::new(40, 0, std::f32::consts::PI, 300);
        river.run(&r);
        assert!(river.trace.iter().any(|s| s.state == RiverState::Pool), "{}", river.describe());
        assert_eq!(river.state, RiverState::Ended, "does not wander forever");
        let end = river.trace.last().unwrap().head;
        assert!(end.x.abs() < 12.0 && end.z.abs() < 12.0, "pooled near the bottom: {end:?}");
    }

    #[test]
    fn a_river_falls_at_a_cliff() {
        let w = T { f: |x, _| if x < 20 { 100 } else { 70 }, cells: BTreeMap::new() };
        let r = Reader::new(&w, 200);
        let mut river = River::new(0, 0, 0.0, 120);
        river.run(&r);
        assert!(river.trace.iter().any(|s| s.state == RiverState::Fall), "{}", river.describe());
        let fall = river.trace.iter().find(|s| s.state == RiverState::Fall).unwrap();
        assert!(fall.ir.contains("block=water") && fall.ir.lines().count() >= 3,
                "a fall emits the column, the plunge pool carve and its fill");
    }

    #[test]
    fn a_river_reaching_water_becomes_a_delta_and_stops() {
        let mut w = T { f: |x, _| 90 - x / 4, cells: BTreeMap::new() };
        for x in 60..120 { for z in -60..60 { w.set(x, 90 - x / 4, z, 9); } }   // a sea
        let r = Reader::new(&w, 200);
        let mut river = River::new(0, 0, 0.0, 400);
        river.run(&r);
        assert_eq!(river.state, RiverState::Delta, "{}", river.describe());
        assert!(river.trace.last().unwrap().head.x >= 50.0);
    }

    #[test]
    fn the_river_widens_downstream() {
        let w = T { f: |x, _| 100 - x / 3, cells: BTreeMap::new() };
        let r = Reader::new(&w, 200);
        let mut river = River::new(0, 0, 0.0, 200);
        river.run(&r);
        assert!(river.width > 2.0, "grew from 1.0: {}", river.width);
    }

    #[test]
    fn the_whole_river_is_valid_ir() {
        let w = T { f: |x, _| 100 - x / 3, cells: BTreeMap::new() };
        let r = Reader::new(&w, 200);
        let mut river = River::new(0, 0, 0.0, 150);
        let ir = river.run(&r);
        let (plan, issues) = parse_plan(&ir);
        let errs: Vec<_> = issues.iter().filter(|i| i.severity == Severity::Error).collect();
        assert!(errs.is_empty(), "{errs:?}\n{ir}");
        assert!(plan.ops.len() > 40, "a river is many ops: {}", plan.ops.len());
    }

    #[test]
    fn the_trace_explains_the_path() {
        let w = T { f: |x, _| 100 - x / 3, cells: BTreeMap::new() };
        let r = Reader::new(&w, 200);
        let mut river = River::new(0, 0, 0.0, 80);
        river.run(&r);
        let d = river.describe();
        assert!(d.contains("steps") && d.contains("Run"), "{d}");
        for s in &river.trace {
            assert!(s.reading.slope_ahead.is_finite(), "every step has a reading");
        }
    }

    #[test]
    fn a_river_overflows_a_dip_instead_of_dying_in_it() {
        // A gentle slope east with a small dip in the way. Before the
        // Overflow state the river pooled in the dip and ended there.
        //
        // The dip is a BOWL, finite in z. The first version of this test
        // made it a trench running the whole length of z, and the river
        // correctly followed the trench forever -- a trench lower than
        // both banks IS where a river goes. The test was wrong.
        // Deep enough that the river MUST pool: 8 below the slope, 20
        // long, 12 wide. A three-deep bowl was sailed straight over.
        let w = T { f: |x, z| { let base = 100 - x / 4;
                                 if (30..50).contains(&x) && z.abs() < 6 { base - 8 } else { base } },
                    cells: BTreeMap::new() };
        let r = Reader::new(&w, 200);
        let mut river = River::new(0, 0, 0.0, 300);
        river.run(&r);
        let end = river.trace.last().unwrap().head;
        assert!(end.x > 70.0, "got past the dip at x=30..50: ended at x={}\n{}",
                end.x, river.describe());
        assert!(river.trace.iter().any(|s| s.state == RiverState::Overflow),
                "by overflowing: {}", river.describe());
    }

    #[test]
    fn the_heading_stays_normalized() {
        // It drifted to -12.9 radians once; every angular comparison
        // after that was garbage.
        let w = T { f: |x, z| 64 + ((x * x + z * z) as f32).sqrt() as i32 / 3, cells: BTreeMap::new() };
        let r = Reader::new(&w, 200);
        let mut river = River::new(60, 0, 0.0, 400);
        river.run(&r);
        for s in &river.trace {
            assert!(s.head.dir >= 0.0 && s.head.dir < std::f32::consts::TAU + 1e-3,
                    "dir {} out of range", s.head.dir);
        }
    }
}
