//! Carving the land: canyons, caves, mesas, waterfalls.
//!
//! WHAT THE CORPUS SAYS
//!
//! Re-mined the build narration for what the landscape layer still
//! cannot say. Two answers, and they are the two highest counts after
//! the ones already built:
//!
//!   cave / underground   29   "dig out a grand canyon for the mesa
//!   water feature        23    biome, add some sand ... rivers and a
//!                              lake to break up the landscape"
//!
//! The brushes RAISE and LOWER a heightfield. Every one of those
//! sentences is SUBTRACTIVE and three-dimensional: a canyon has walls
//! that overhang, a cave has a roof, a waterfall needs a lip and a
//! plunge pool. A heightfield cannot express a roof over your head,
//! and that is the whole difference between terrain and a place you
//! can walk into.
//!
//! WHAT THESE ARE
//!
//! * `canyon`     a carved channel following a wandering course, with
//!                stepped walls and a floor that falls
//! * `cave`       a branching tunnel system from a mouth, with
//!                chambers where branches meet
//! * `mesa`       a plateau left standing when its surroundings drop,
//!                with a stepped skirt
//! * `waterfall`  a lip, a plunge, a pool, and the gorge below it
//! * `overhang`   a shelf carved back under itself: the thing a
//!                heightfield cannot do at all
//!
//! Each is a CHANGE LIST, not a mesh: the same `Change` the hive
//! applies, the plan undoes and the world tree records. Big -- a
//! canyon is hundreds of thousands of cells -- so each is bounded and
//! reports what it did.

use crate::weather::Change;
use crate::VoxelWorld;
use std::collections::BTreeSet;

/// Ceiling on one carve, so a fumbled radius cannot eat a continent.
pub const MAX_CARVE: usize = 3_000_000;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Carve {
    /// What to leave where rock is removed above the waterline.
    pub air: u16,
    /// What to line the floor with, if anything.
    pub floor: Option<u16>,
    /// Fill below this height with water rather than air.
    pub water_level: Option<i32>,
    pub water: u16,
    /// Expose this block on freshly cut walls: a stone canyon in
    /// sandstone reads as a canyon because the cut face differs.
    pub wall: Option<u16>,
    /// Dress the cut face from a STRATA PROFILE instead: each exposed
    /// cell gets the block its depth calls for, so a canyon shows the
    /// bands it cut through and they line up across it. Wins over
    /// `wall`, which paints the whole face one colour -- a wall, not a
    /// cliff.
    pub strata: Option<(&'static crate::strata::Profile, u64)>,
}

impl Default for Carve {
    fn default() -> Self { Self { air: 0, floor: None, water_level: None, water: 9, wall: None, strata: None } }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CarveResult {
    pub changes: Vec<Change>,
    pub removed: usize,
    pub filled: usize,
    /// The box that was touched.
    pub bounds: Option<((i32, i32, i32), (i32, i32, i32))>,
    pub truncated: bool,
    pub notes: Vec<String>,
    /// Where each touched cell sits in `changes`, so a second write to
    /// the same cell replaces the first rather than appending.
    index: std::collections::BTreeMap<(i32, i32, i32), usize>,
}

impl CarveResult {
    pub fn summary(&self) -> String {
        format!("{} cell(s) removed, {} filled{}{}", self.removed, self.filled,
                self.bounds.map(|(lo, hi)| format!(", {}x{}x{}", hi.0 - lo.0 + 1, hi.1 - lo.1 + 1, hi.2 - lo.2 + 1))
                    .unwrap_or_default(),
                if self.truncated { " -- TRUNCATED, reduce the size" } else { "" })
    }
    fn push(&mut self, at: (i32, i32, i32), to: u16, removing: bool) -> bool {
        if self.changes.len() >= MAX_CARVE && !self.index.contains_key(&at) {
            self.truncated = true; return false;
        }
        // COUNT CELLS, NOT WRITES. A cave's bore revisits a cell every
        // step the head advances, so `removed` read 98,105 for 19,155
        // actual cells -- and every ratio against it (the roofed
        // fraction, the block estimate the player is shown) was five
        // times too small. The change list keeps the duplicates, which
        // is harmless -- last write wins -- but the counts must not.
        // A cell touched twice is one change. The bore revisits cells
        // every step the head advances; keeping the duplicates made a
        // 19k-cell cave a 98k-change list, which the apply cap then
        // refused outright. Last write wins, so the LATER one replaces
        // the earlier -- a water fill over an air cut, say.
        if let Some(i) = self.index.get(&at) {
            let i = *i;
            let was_removal = self.changes[i].to == 0;
            if was_removal != removing { if removing { self.removed += 1; self.filled -= 1 }
                                         else { self.filled += 1; self.removed -= 1 } }
            self.changes[i].to = to;
            return true;
        }
        self.index.insert(at, self.changes.len());
        if removing { self.removed += 1 } else { self.filled += 1 }
        self.bounds = Some(match self.bounds {
            None => (at, at),
            Some((lo, hi)) => ((lo.0.min(at.0), lo.1.min(at.1), lo.2.min(at.2)),
                               (hi.0.max(at.0), hi.1.max(at.1), hi.2.max(at.2))),
        });
        self.changes.push(Change { at, to });
        true
    }
}

fn hash(a: i64, b: i64, s: u64) -> u64 {
    let mut h = (a as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (b as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F) ^ s;
    h ^= h >> 31; h = h.wrapping_mul(0x7FB5_D329_728E_A185); h ^ (h >> 27)
}
fn unit(a: i64, b: i64, s: u64) -> f32 { (hash(a, b, s) % 10_000) as f32 / 10_000.0 }
/// Smooth-ish value noise in one dimension, for a wandering course.
fn wander(t: f32, seed: u64, scale: f32) -> f32 {
    let i = t.floor() as i64;
    let f = t - i as f32;
    let f = f * f * (3.0 - 2.0 * f);
    let a = unit(i, 0, seed) - 0.5;
    let b = unit(i + 1, 0, seed) - 0.5;
    (a + (b - a) * f) * scale
}

/// The surface height of a column: the highest non-air block.
fn surface<W: VoxelWorld>(w: &W, x: i32, z: i32, from_y: i32) -> i32 {
    (0..=from_y + 96).rev().find(|&y| w.get(x, y, z) != 0).unwrap_or(0)
}

/// Remove a column of rock from `y0` to `y1`, honouring the carve's
/// water level and wall dressing. Returns false when truncated.
fn cut_column<W: VoxelWorld>(w: &W, r: &mut CarveResult, c: &Carve, x: i32, z: i32, y0: i32, y1: i32) -> bool {
    for y in y0..=y1 {
        if w.get(x, y, z) == 0 { continue; }
        let to = match c.water_level { Some(wl) if y <= wl => c.water, _ => c.air };
        let removing = to == 0;
        if !r.push((x, y, z), to, removing) { return false; }
    }
    // dress the floor and the cut walls
    if let Some(f) = c.floor {
        if y0 > 0 && w.get(x, y0 - 1, z) != 0 && !r.push((x, y0 - 1, z), f, false) { return false; }
    }
    if let Some((profile, seed)) = c.strata {
        // The face, banded. Depth is measured from each column's OWN
        // surface, not from the cut's rim: the bands belong to the
        // ground, so they stay level across a cut whose rim is not.
        for d in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let (nx, nz) = (x + d.0, z + d.1);
            let ntop = (y1 + 40).min(y1 + 40);
            let _ = ntop;
            let surf = (0..=y1 + 40).rev().find(|&yy| w.get(nx, yy, nz) != 0).unwrap_or(y1);
            for y in y0..=y1 {
                if w.get(nx, y, nz) == 0 { continue; }
                if r.index.contains_key(&(nx, y, nz)) { continue; }
                let block = profile.at(nx, nz, surf - y, seed);
                if !r.push((nx, y, nz), block, false) { return false; }
            }
        }
    } else if let Some(wall) = c.wall {
        // Only the cells that are actually EXPOSED by this cut: rock
        // whose neighbour toward the channel was removed. Dressing all
        // four sides of every column re-wrote the whole cut face once
        // per column -- 78,000 "filled" against 771 removed, the
        // dressing drowning the carve it was meant to trim.
        for d in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let (nx, nz) = (x + d.0, z + d.1);
            // a face cell, not a column: the wall is one block thick
            for y in y0..=y1 {
                if w.get(nx, y, nz) == 0 { continue; }
                // it is exposed only if the cut reached this column's edge
                if r.index.contains_key(&(nx, y, nz)) { continue; }
                if !r.push((nx, y, nz), wall, false) { return false; }
            }
        }
    }
    true
}

/// A canyon: a channel that wanders from A to B, its floor falling
/// along the way, its walls stepped back so they are climbable in
/// places and sheer in others.
pub fn canyon<W: VoxelWorld>(w: &W, from: (i32, i32), to: (i32, i32), width: i32, depth: i32,
                             seed: u64, c: &Carve, from_y: i32) -> CarveResult {
    let mut r = CarveResult::default();
    let (dx, dz) = ((to.0 - from.0) as f32, (to.1 - from.1) as f32);
    let len = (dx * dx + dz * dz).sqrt().max(1.0);
    let steps = len.ceil() as i32;
    let (ux, uz) = (dx / len, dz / len);
    let (px, pz) = (-uz, ux);                       // perpendicular
    let half = (width.max(2) / 2).max(1);
    let start_y = surface(w, from.0, from.1, from_y);
    let end_y = surface(w, to.0, to.1, from_y);
    for i in 0..=steps {
        let t = i as f32;
        // the course wanders; a straight canyon is a trench
        let off = wander(t / 18.0, seed, half as f32 * 1.4);
        let cx = from.0 as f32 + ux * t + px * off;
        let cz = from.1 as f32 + uz * t + pz * off;
        // the floor falls from one end to the other, plus a little noise
        let f = t / steps.max(1) as f32;
        let floor_y = (start_y as f32 * (1.0 - f) + end_y as f32 * f) as i32 - depth
            + (wander(t / 30.0, seed ^ 0x5eed, 2.5)) as i32;
        // the width breathes, so the walls are not parallel
        let wide = half + (wander(t / 22.0, seed ^ 0xbeef, half as f32 * 0.6)) as i32;
        for o in -wide..=wide {
            let x = (cx + px * o as f32).round() as i32;
            let z = (cz + pz * o as f32).round() as i32;
            let top = surface(w, x, z, from_y);
            // STEPPED WALLS: the cut is shallower near the rim, so the
            // wall is a series of ledges rather than a sheer box. The
            // step pattern varies along the canyon: sheer in places.
            let edge = (o.abs() as f32 / wide.max(1) as f32).min(1.0);
            let sheer = unit((t / 12.0) as i64, 0, seed ^ 0xfa11) > 0.55;
            let lift = if sheer { (edge * edge * depth as f32 * 0.25) as i32 }
                       else { (edge * edge * depth as f32 * 0.8) as i32 };
            let y0 = (floor_y + lift).min(top);
            if y0 > top { continue; }
            if !cut_column(w, &mut r, c, x, z, y0, top) { r.notes.push("canyon truncated".into()); return r; }
        }
    }
    r.notes.push(format!("canyon {} long, {} wide, {} deep", steps, width, depth));
    r
}

/// A cave system: tunnels branching from a mouth, with chambers where
/// they meet. Rooms have ROOFS, which is the whole point.
pub fn cave<W: VoxelWorld>(w: &W, mouth: (i32, i32), heading_deg: i32, length: i32, radius: i32,
                           branches: usize, seed: u64, c: &Carve, from_y: i32) -> CarveResult {
    let mut r = CarveResult::default();
    let mut heads: Vec<(f32, f32, f32, f32, i32, u64)> = Vec::new();   // x, y, z, heading, life, seed
    let my = surface(w, mouth.0, mouth.1, from_y) as f32 - radius as f32;
    heads.push((mouth.0 as f32, my, mouth.1 as f32, (heading_deg as f32).to_radians(), length, seed));
    let mut chambers: Vec<(i32, i32, i32, i32)> = Vec::new();
    let mut made = 0usize;
    while let Some((mut x, mut y, mut z, mut h, life, s)) = heads.pop() {
        made += 1;
        if made > branches + 1 { break; }
        let mut pitch: f32 = -0.08;
        for step in 0..life {
            // wander in heading and pitch; a straight tunnel is a pipe
            h += (unit(step as i64, made as i64, s) - 0.5) * 0.35;
            pitch += (unit(step as i64, made as i64 + 91, s) - 0.5) * 0.12;
            pitch = pitch.clamp(-0.45, 0.25);
            x += h.cos(); z += h.sin(); y += pitch;
            // STAY UNDER THE ROCK. The pitch wanders up as freely as
            // down, so a tunnel drifted to the surface and became a
            // trench: 4% of its cells had anything above them. The head
            // is held at least `radius + 3` below the local surface,
            // and pushed down when it gets close -- which is also what
            // makes a cave follow the terrain rather than ignore it.
            let here = surface(w, x as i32, z as i32, from_y) as f32;
            let ceiling = here - radius as f32 - 3.0;
            if y > ceiling { y = ceiling; pitch = pitch.min(-0.05); }
            // the tunnel's bore breathes
            let bore = (radius as f32 * (0.7 + unit(step as i64 / 6, made as i64, s) * 0.8)).max(1.5);
            let top = surface(w, x as i32, z as i32, from_y);
            if y < 4.0 || y as i32 > top { break; }
            let _ = top;
            let br = bore.ceil() as i32;
            for dx in -br..=br { for dy in -br..=br { for dz in -br..=br {
                let d = ((dx * dx + dy * dy + dz * dz) as f32).sqrt();
                if d > bore { continue; }
                let (px, py, pz) = (x as i32 + dx, y as i32 + dy, z as i32 + dz);
                if py < 1 { continue; }
                // KEEP THE ROOF, per column. The head is held below the
                // surface at its own x,z, but the bore is a sphere: on
                // a slope its uphill side is fine and its downhill side
                // breaks out into daylight. Every cell checks the
                // surface of ITS OWN column and leaves two blocks of
                // rock above -- so a tunnel through a hillside stays a
                // tunnel rather than becoming a trench where the land
                // falls away.
                if py > surface(w, px, pz, from_y) - 2 { continue; }
                if w.get(px, py, pz) == 0 { continue; }
                let to = match c.water_level { Some(wl) if py <= wl => c.water, _ => c.air };
                if !r.push((px, py, pz), to, to == 0) { r.notes.push("cave truncated".into()); return r; }
            }}}
            // a chamber every so often, and a branch from it
            if step > 8 && step % (length / (branches as i32 + 1).max(1)).max(12) == 0 {
                chambers.push((x as i32, y as i32, z as i32, (bore * 2.2) as i32));
                if heads.len() + made <= branches {
                    heads.push((x, y, z, h + if made % 2 == 0 { 1.1 } else { -1.1 },
                                (life as f32 * 0.6) as i32, s ^ (step as u64).wrapping_mul(7919)));
                }
            }
        }
    }
    // the chambers: bigger rooms, flattened so they have a floor and a
    // roof rather than being spheres
    for (cx, cy, cz, cr) in chambers {
        let cr = cr.clamp(2, 14);
        // A chamber is a ROOM: it must keep its roof. The centre is
        // pushed down so that even its highest cell has rock above,
        // and a chamber that cannot fit under the local surface is
        // shrunk rather than allowed to punch out of the hillside --
        // which is what made a cave 77% open sky.
        let local = surface(w, cx, cz, from_y);
        let headroom = local - cy;
        let cr = if headroom < cr + 3 { (headroom - 3).min(cr) } else { cr };
        if cr < 2 { continue; }
        for dx in -cr..=cr { for dy in -(cr / 2)..=cr { for dz in -cr..=cr {
            let dyf = dy as f32 * 1.8;
            if ((dx * dx + dz * dz) as f32 + dyf * dyf).sqrt() > cr as f32 { continue; }
            let (px, py, pz) = (cx + dx, cy + dy, cz + dz);
            if py < 1 || w.get(px, py, pz) == 0 { continue; }
            if py > surface(w, px, pz, from_y) { continue; }
            let to = match c.water_level { Some(wl) if py <= wl => c.water, _ => c.air };
            if !r.push((px, py, pz), to, to == 0) { r.notes.push("cave truncated".into()); return r; }
        }}}
    }
    r.notes.push(format!("cave from {:?}, {} branch(es)", mouth, made.saturating_sub(1)));
    r
}

/// A mesa: everything around a footprint drops away, leaving it
/// standing, with a stepped skirt so it is not a cylinder.
pub fn mesa<W: VoxelWorld>(w: &W, centre: (i32, i32), radius: i32, drop: i32, _seed: u64,
                           c: &Carve, from_y: i32) -> CarveResult {
    let mut r = CarveResult::default();
    let outer = (radius as f32 * 2.2) as i32;
    for dx in -outer..=outer { for dz in -outer..=outer {
        let (x, z) = (centre.0 + dx, centre.1 + dz);
        let d = ((dx * dx + dz * dz) as f32).sqrt();
        // the rim wanders, so the mesa is not a disc
        let rim = radius as f32 * (1.0 + wander((dz as f32).atan2(dx as f32) * 3.0, _seed, 0.22));
        if d <= rim { continue; }                       // the plateau stands
        let beyond = (d - rim) / (outer as f32 - rim).max(1.0);
        // STEPPED: the drop happens in terraces, which is what makes a
        // mesa read as sedimentary rather than as a cake
        let terraces = 3.0 + unit(dx as i64 / 8, dz as i64 / 8, _seed) * 2.0;
        let stepped = (beyond * terraces).floor() / terraces;
        let cut = (stepped.min(1.0) * drop as f32) as i32;
        if cut <= 0 { continue; }
        let top = surface(w, x, z, from_y);
        if !cut_column(w, &mut r, c, x, z, (top - cut + 1).max(1), top) {
            r.notes.push("mesa truncated".into()); return r;
        }
    }}
    r.notes.push(format!("mesa radius {radius}, surroundings down {drop}"));
    r
}

/// A waterfall: a lip cut into a height, the fall itself, a plunge
/// pool below, and the gorge it has worn.
pub fn waterfall<W: VoxelWorld>(w: &W, at: (i32, i32), heading_deg: i32, width: i32, height: i32,
                                _seed: u64, c: &Carve, from_y: i32) -> CarveResult {
    let mut r = CarveResult::default();
    let h = (heading_deg as f32).to_radians();
    let (ux, uz) = (h.cos(), h.sin());
    let (px, pz) = (-uz, ux);
    let half = (width.max(1) / 2).max(1);
    let lip_y = surface(w, at.0, at.1, from_y);
    let pool_y = lip_y - height.max(3);
    let water = c.water;
    // the lip: a notch cut back into the cliff so the water leaves cleanly
    for o in -half..=half {
        let bulge = (1.0 - (o as f32 / half as f32).powi(2)).max(0.0);
        for back in 0..=(2 + (bulge * 2.0) as i32) {
            let x = (at.0 as f32 + px * o as f32 - ux * back as f32).round() as i32;
            let z = (at.1 as f32 + pz * o as f32 - uz * back as f32).round() as i32;
            let top = surface(w, x, z, from_y);
            let cut_to = lip_y - 1 - (bulge * 2.0) as i32;
            if top >= cut_to && !cut_column(w, &mut r, c, x, z, cut_to, top) { return r; }
            // and water at the lip
            if !r.push((x, cut_to, z), water, false) { return r; }
        }
    }
    // The fall: a curtain of water down the face. It leans OUT from the
    // cliff as it falls -- water does -- so the column directly under
    // the lip is not where the water is at the bottom. The lean is
    // small and the pool is placed under the curtain's foot, not under
    // the lip.
    let lean = 0.12f32;
    for o in -half..=half {
        for y in pool_y..lip_y {
            let x = (at.0 as f32 + px * o as f32 + ux * ((lip_y - y) as f32 * lean)).round() as i32;
            let z = (at.1 as f32 + pz * o as f32 + uz * ((lip_y - y) as f32 * lean)).round() as i32;
            if !r.push((x, y, z), water, false) { return r; }
            // Clear the rock behind the curtain: a space behind a
            // waterfall is the thing every builder puts a cave in. The
            // cut is BEHIND the curtain's leaning position, so it must
            // reach back past the lean as well as into the cliff --
            // cutting two blocks back from the curtain left solid rock
            // between the water and the space once the lean had carried
            // the water clear of the face.
            let reach = 2 + ((lip_y - y) as f32 * lean).ceil() as i32;
            for back in 1..=reach {
                let (bx, bz) = ((x as f32 - ux * back as f32).round() as i32,
                                (z as f32 - uz * back as f32).round() as i32);
                if w.get(bx, y, bz) != 0 && !r.push((bx, y, bz), c.air, true) { return r; }
            }
        }
    }
    // the plunge pool: a bowl below, filled
    let pr = (half as f32 * 2.4) as i32 + 2;
    // under the curtain's FOOT: where the lean has carried it
    let foot = (lip_y - pool_y) as f32 * lean;
    let (cx, cz) = ((at.0 as f32 + ux * foot) as i32, (at.1 as f32 + uz * foot) as i32);
    for dx in -pr..=pr { for dz in -pr..=pr {
        let d = ((dx * dx + dz * dz) as f32).sqrt();
        if d > pr as f32 { continue; }
        let depth = ((1.0 - d / pr as f32) * 5.0) as i32 + 1;
        let (x, z) = (cx + dx, cz + dz);
        let top = surface(w, x, z, from_y);
        let floor = pool_y - depth;
        if top >= floor {
            for y in floor..=top.min(pool_y) {
                if !r.push((x, y, z), if y <= pool_y { water } else { c.air }, false) { return r; }
            }
        }
    }}
    r.notes.push(format!("waterfall {width} wide, {height} tall, pool {}", pr * 2));
    r
}

/// An overhang: a shelf cut back UNDER itself, leaving rock above open
/// air. A heightfield cannot express this at all, which is exactly why
/// it is here -- it is the difference between a slope and a cliff you
/// can shelter beneath.
pub fn overhang<W: VoxelWorld>(w: &W, at: (i32, i32), heading_deg: i32, width: i32, depth: i32,
                               height: i32, seed: u64, c: &Carve, from_y: i32) -> CarveResult {
    let mut r = CarveResult::default();
    let h = (heading_deg as f32).to_radians();
    let (ux, uz) = (h.cos(), h.sin());
    let (px, pz) = (-uz, ux);
    let half = (width.max(2) / 2).max(1);
    for o in -half..=half {
        // the cut is deepest in the middle and tapers to the ends
        let taper = (1.0 - (o as f32 / half as f32).powi(2)).max(0.0);
        let d = (depth as f32 * taper * (0.7 + unit(o as i64, 0, seed) * 0.5)) as i32;
        for back in 0..=d {
            let x = (at.0 as f32 + px * o as f32 + ux * back as f32).round() as i32;
            let z = (at.1 as f32 + pz * o as f32 + uz * back as f32).round() as i32;
            let top = surface(w, x, z, from_y);
            // leave a ROOF: cut a band, not up to the surface
            let ceiling = (top - height.max(2)).max(1);
            let floor = (ceiling - (height as f32 * 0.8) as i32).max(1);
            if floor >= ceiling { continue; }
            if !cut_column(w, &mut r, c, x, z, floor, ceiling) {
                r.notes.push("overhang truncated".into()); return r;
            }
        }
    }
    r.notes.push(format!("overhang {width} wide, cut back {depth}"));
    r
}

/// Cells with rock directly above them: what makes a carve a PLACE
/// rather than a hole.
///
/// `w` must be the world AS IT WAS BEFORE the carve -- the carve's own
/// removals are known from `changes`, and a world that already has
/// them applied double-counts. Reported in the summary and used by the
/// tests, which is where the double-count showed up: a cave measured
/// 23% roofed against a world with its own tunnels already dug.
pub fn roofed(changes: &[Change], w: &impl VoxelWorld) -> usize {
    let removed: BTreeSet<(i32, i32, i32)> = changes.iter().filter(|c| c.to == 0).map(|c| c.at).collect();
    removed.iter().filter(|c| {
        // Walk up past the cells this carve removed, then ask whether
        // ANY rock stands above. What makes a place sheltered is rock
        // somewhere overhead, not rock in the very next cell.
        let mut d = 1;
        while d < 160 && removed.contains(&(c.0, c.1 + d, c.2)) { d += 1; }
        (0..64).any(|k| w.get(c.0, c.1 + d + k, c.2) != 0)
    }).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A hillside: ground at 64 rising to 110 in the east.
    struct Hill(BTreeMap<(i32, i32, i32), u16>);
    impl Hill {
        fn new() -> Self { Hill(BTreeMap::new()) }
        fn h(x: i32) -> i32 { (64 + (x + 120).max(0) / 4).min(110) }
    }
    impl VoxelWorld for Hill {
        fn get(&self, x: i32, y: i32, z: i32) -> u16 {
            if let Some(&b) = self.0.get(&(x, y, z)) { return b; }
            if y <= Hill::h(x) { 1 } else { 0 }
        }
        fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.0.insert((x, y, z), b); true }
    }

    fn apply(w: &mut Hill, r: &CarveResult) { for c in &r.changes { w.set(c.at.0, c.at.1, c.at.2, c.to); } }

    #[test]
    fn a_canyon_wanders_falls_and_has_stepped_walls() {
        let w = Hill::new();
        let r = canyon(&w, (0, 0), (120, 0), 14, 20, 7, &Carve::default(), 64);
        assert!(r.removed > 5_000, "{}", r.summary());
        assert!(!r.truncated);
        let (lo, hi) = r.bounds.unwrap();
        // it wanders: the cut is wider in z than a straight channel
        assert!(hi.2 - lo.2 > 14, "the course wanders: {} wide in z", hi.2 - lo.2);
        // the floor falls: the deepest cut at the east end is higher
        // than at the west, because the ground rises
        let low_at = |x: i32| r.changes.iter().filter(|c| c.at.0 == x && c.to == 0).map(|c| c.at.1).min();
        if let (Some(a), Some(b)) = (low_at(10), low_at(110)) { assert!(b > a, "the canyon climbs with the land: {a} -> {b}"); }
        // stepped: at a given cross-section the cut depth varies with
        // distance from the middle
        let mid: Vec<i32> = r.changes.iter().filter(|c| c.at.0 == 60 && c.to == 0).map(|c| c.at.1).collect();
        assert!(!mid.is_empty());
        let (top, bot) = (mid.iter().max().unwrap(), mid.iter().min().unwrap());
        assert!(top - bot > 8, "a canyon has depth: {bot}..{top}");
    }

    #[test]
    fn a_cave_has_a_roof_over_it_which_is_the_whole_point() {
        // The world BEFORE the carve: `roofed` knows the carve's own
        // removals from the change list, so a world with them applied
        // would count them twice.
        let w = Hill::new();
        let r = cave(&w, (100, 0), 180, 90, 4, 3, 11, &Carve::default(), 64);
        assert!(r.removed > 2_000, "{}", r.summary());
        let roofed_cells = roofed(&r.changes, &w);
        assert!(roofed_cells as f32 / r.removed as f32 > 0.7,
                "most of a cave is under rock: {roofed_cells}/{}", r.removed);
        // it goes somewhere: the far end is well away from the mouth
        let (lo, hi) = r.bounds.unwrap();
        assert!((hi.0 - lo.0) + (hi.2 - lo.2) > 40, "it travels: {}x{}", hi.0 - lo.0, hi.2 - lo.2);
        // and it descends
        assert!(lo.1 < 90, "it goes down: floor at {}", lo.1);
    }

    #[test]
    fn a_mesa_leaves_its_plateau_standing_and_steps_down_around_it() {
        let mut w = Hill::new();
        let before = Hill::h(0);
        let r = mesa(&w, (0, 0), 20, 18, 3, &Carve::default(), 64);
        assert!(r.removed > 1_000, "{}", r.summary());
        apply(&mut w, &r);
        // the plateau is untouched
        assert_eq!(surface(&w, 0, 0, 64), before, "the top still stands");
        // the surroundings dropped
        let out = surface(&w, 44, 0, 64);
        assert!(out < Hill::h(44), "the skirt dropped: {out} vs {}", Hill::h(44));
        // stepped, not a cone: heights repeat across a terrace
        let profile: Vec<i32> = (22..44).map(|x| surface(&w, x, 0, 64)).collect();
        let flats = profile.windows(2).filter(|p| p[0] == p[1]).count();
        assert!(flats >= 4, "terraces, not a smooth cone: {profile:?}");
    }

    #[test]
    fn a_waterfall_has_a_lip_a_curtain_a_pool_and_a_space_behind_it() {
        let mut w = Hill::new();
        let c = Carve { water_level: None, ..Default::default() };
        let r = waterfall(&w, (60, 0), 180, 7, 16, 5, &c, 64);
        assert!(r.filled > 100, "{}", r.summary());
        apply(&mut w, &r);
        let lip = Hill::h(60);
        // Water all the way down, FOLLOWING THE CURTAIN: it leans out
        // from the cliff as it falls, so a fixed column misses it.
        let curtain: Vec<u16> = ((lip - 16)..=lip)
            .map(|y| { let x = 60 - ((lip - y) as f32 * 0.12).round() as i32; w.get(x, y, 0) }).collect();
        assert!(curtain.iter().filter(|b| **b == 9).count() > 12, "a curtain of water: {curtain:?}");
        // a pool at the curtain's foot
        let foot_x = 60 - ((lip - (lip - 16)) as f32 * 0.12).round() as i32;
        assert_eq!(w.get(foot_x, lip - 16, 0), 9, "the plunge pool under the fall");
        // and air behind the curtain
        let behind: Vec<u16> = ((lip - 12)..(lip - 4)).map(|y| w.get(62, y, 0)).collect();
        assert!(behind.iter().any(|b| *b == 0), "a space behind the fall: {behind:?}");
    }

    #[test]
    fn an_overhang_leaves_rock_above_open_air() {
        let mut w = Hill::new();
        let r = overhang(&w, (80, 0), 180, 16, 10, 6, 3, &Carve::default(), 64);
        assert!(r.removed > 200, "{}", r.summary());
        apply(&mut w, &r);
        // the defining property: somewhere, air with solid above it
        let mut found = false;
        for x in 70..=90 { for y in 60..=Hill::h(90) {
            if w.get(x, y, 0) == 0 && w.get(x, y + 3, 0) != 0 { found = true; }
        }}
        assert!(found, "rock above open air -- what a heightfield cannot do");
        // and the shelf is still there: the surface above is unbroken
        assert_eq!(surface(&w, 80, 0, 64), Hill::h(80), "the roof of the shelf survived");
    }

    #[test]
    fn every_carve_is_bounded_and_says_when_it_truncates() {
        let w = Hill::new();
        // absurd: a canyon a thousand long and two hundred wide
        let r = canyon(&w, (-500, 0), (500, 0), 200, 60, 1, &Carve::default(), 64);
        assert!(r.truncated, "a fumbled size truncates rather than eating the world");
        assert!(r.changes.len() <= MAX_CARVE + 8);
        assert!(r.summary().contains("TRUNCATED"));
        assert!(r.notes.iter().any(|n| n.contains("truncated")));
    }

    #[test]
    fn water_level_fills_what_is_below_it_and_leaves_air_above() {
        let w = Hill::new();
        let c = Carve { water_level: Some(70), ..Default::default() };
        let r = canyon(&w, (0, 0), (60, 0), 10, 24, 2, &c, 64);
        let below: Vec<&Change> = r.changes.iter().filter(|c| c.at.1 <= 70).collect();
        let above: Vec<&Change> = r.changes.iter().filter(|c| c.at.1 > 70).collect();
        assert!(!below.is_empty() && below.iter().all(|c| c.to == 9), "flooded below the level");
        assert!(!above.is_empty() && above.iter().all(|c| c.to == 0), "air above it");
    }

    #[test]
    fn a_carve_dresses_its_floor_and_walls_when_asked() {
        let w = Hill::new();
        let c = Carve { floor: Some(12), wall: Some(24), ..Default::default() };
        let r = canyon(&w, (0, 0), (40, 0), 8, 12, 4, &c, 64);
        assert!(r.changes.iter().any(|c| c.to == 12), "a sand floor");
        assert!(r.changes.iter().any(|c| c.to == 24), "sandstone walls where the cut exposed rock");
    }
}
