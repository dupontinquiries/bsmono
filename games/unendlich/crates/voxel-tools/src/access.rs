//! Getting up and down.
//!
//! Everything built so far makes terrain or makes buildings. A canyon
//! and a village on its rim are two correct things with no relation:
//! there is no way down. The road machine solves travel ACROSS ground
//! -- it cuts, fills, bridges and tunnels -- but a twenty-block cliff
//! defeats it, because the answer to a cliff is not a road, it is a
//! stair.
//!
//! Vertical access is its own problem and it has four answers, which
//! is what the corpus shows builders using:
//!
//! * `cliff_stair`  steps CUT INTO the face, with a handrail-height
//!                  lip left standing on the drop side. The face is
//!                  rock, so the stair is carved out of it, not built
//!                  onto it.
//! * `switchback`   a ramp that zigzags across a slope too steep to
//!                  climb directly, with landings at the turns. What a
//!                  cart can use where a stair cannot.
//! * `shaft`        a vertical hole with a ladder and landings: the
//!                  mine entrance, the well, the way into a cavern
//!                  from above.
//! * `terrace`      steps cut into a whole hillside, wide enough to
//!                  build or farm on. A hill nobody can use becomes a
//!                  hill covered in fields.
//!
//! Each reads the ground and follows it, and each returns changes.
//!
//! WHY THIS IS NOT `stairs`
//!
//! The `stairs` op draws a staircase between two points, one block up
//! per step, in the air. It is a building element. These are terrain
//! work: they find the face, follow its shape, cut into it, and leave
//! the rock they did not remove.

use crate::weather::Change;
use crate::VoxelWorld;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Access {
    /// Tread material, or None to leave bare rock.
    pub tread: Option<u16>,
    /// The rail or kerb left on the drop side, or None.
    pub rail: Option<u16>,
    /// Ladder or rung block for shafts.
    pub rung: u16,
    /// Torch or light block, every `light_every` steps. 0 for none.
    pub light: u16,
    pub light_every: i32,
}

impl Default for Access {
    fn default() -> Self { Self { tread: None, rail: Some(1), rung: 65, light: 50, light_every: 8 } }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AccessResult {
    pub changes: Vec<Change>,
    /// Where it starts and ends, so something else can join it.
    pub from: Option<(i32, i32, i32)>,
    pub to: Option<(i32, i32, i32)>,
    pub steps: usize,
    pub notes: Vec<String>,
}

impl AccessResult {
    pub fn summary(&self) -> String {
        match (self.from, self.to) {
            (Some(a), Some(b)) => format!("{} step(s), {} to {}, {} blocks",
                                          self.steps, fmt(a), fmt(b), self.changes.len()),
            _ => format!("nothing built: {}", self.notes.join("; ")),
        }
    }
}

fn fmt(p: (i32, i32, i32)) -> String { format!("({},{},{})", p.0, p.1, p.2) }

fn surface<W: VoxelWorld>(w: &W, x: i32, z: i32, from_y: i32) -> i32 {
    (0..=from_y + 96).rev().find(|&y| w.get(x, y, z) != 0).unwrap_or(0)
}

/// Carve a stair into a face, from `at` going in `heading`, descending
/// (or climbing) until it reaches the level of `target_y`.
///
/// The stair CUTS: at each step it removes the rock in front of and
/// above the tread, so the result is a slot in the cliff. A lip is
/// left on the open side, which is what stops it reading as a ramp
/// scratched onto a wall.
pub fn cliff_stair<W: VoxelWorld>(w: &W, at: (i32, i32), heading_deg: i32, target_y: i32,
                                  width: i32, a: &Access, from_y: i32) -> AccessResult {
    let mut r = AccessResult::default();
    let h = (heading_deg as f32).to_radians();
    let (ux, uz) = (h.cos(), h.sin());
    let (px, pz) = (-uz, ux);
    let width = width.clamp(1, 8);
    let start_y = surface(w, at.0, at.1, from_y);
    let down = target_y < start_y;
    let total = (start_y - target_y).abs();
    if total < 2 { r.notes.push("no height to climb".into()); return r; }
    r.from = Some((at.0, start_y + 1, at.1));
    let headroom = 3;
    let (mut fx, mut fz) = (at.0 as f32, at.1 as f32);
    let mut y = start_y;
    let mut steps = 0usize;
    // one step forward per block of height: a 45-degree stair, which is
    // what fits in a cliff. A shallower one is a switchback's job.
    while (down && y > target_y) || (!down && y < target_y) {
        fx += ux; fz += uz;
        y += if down { -1 } else { 1 };
        steps += 1;
        if steps > total as usize * 2 + 8 { r.notes.push("gave up: the face runs on".into()); break; }
        for o in 0..width {
            let x = (fx + px * o as f32).round() as i32;
            let z = (fz + pz * o as f32).round() as i32;
            // cut the slot: the tread's own cell and headroom above it
            for k in 0..=headroom {
                if w.get(x, y + k, z) != 0 { r.changes.push(Change { at: (x, y + k, z), to: 0 }); }
            }
            // the tread itself, if the rock did not provide one
            if w.get(x, y - 1, z) == 0 {
                r.changes.push(Change { at: (x, y - 1, z), to: a.tread.unwrap_or(1) });
            } else if let Some(t) = a.tread {
                r.changes.push(Change { at: (x, y - 1, z), to: t });
            }
        }
        // the lip on the open side: the side away from the rock
        if let Some(rail) = a.rail {
            let ox = (fx + px * width as f32).round() as i32;
            let oz = (fz + pz * width as f32).round() as i32;
            // only where there is a drop -- a lip against solid rock is
            // just rock, and placing it there fills the stair in
            if w.get(ox, y - 1, oz) == 0 || w.get(ox, y - 2, oz) == 0 {
                r.changes.push(Change { at: (ox, y, oz), to: rail });
            }
        }
        if a.light != 0 && a.light_every > 0 && steps as i32 % a.light_every == 0 {
            let lx = (fx - px).round() as i32;
            let lz = (fz - pz).round() as i32;
            r.changes.push(Change { at: (lx, y + 1, lz), to: a.light });
        }
    }
    r.to = Some(((fx).round() as i32, y, (fz).round() as i32));
    r.steps = steps;
    r
}

/// A switchback ramp across a slope: legs at a walkable grade, turning
/// back on themselves, with a landing at each turn.
pub fn switchback<W: VoxelWorld>(w: &W, at: (i32, i32), heading_deg: i32, target_y: i32,
                                 width: i32, leg: i32, a: &Access, from_y: i32) -> AccessResult {
    let mut r = AccessResult::default();
    let width = width.clamp(2, 10);
    let leg = leg.clamp(6, 60);
    let start_y = surface(w, at.0, at.1, from_y);
    let down = target_y < start_y;
    if (start_y - target_y).abs() < 3 { r.notes.push("no height to climb".into()); return r; }
    r.from = Some((at.0, start_y + 1, at.1));
    let base = (heading_deg as f32).to_radians();
    let (fx, fz) = (at.0 as f32, at.1 as f32);
    let mut y = start_y;
    let mut legs = 0;
    let mut steps = 0usize;
    // the ramp climbs one block every three forward: a grade a cart
    // takes. That is the whole reason a switchback exists.
    while ((down && y > target_y) || (!down && y < target_y)) && legs < 24 {
        // the leg direction alternates: out and back across the slope
        let dir = base + if legs % 2 == 0 { 0.0 } else { std::f32::consts::PI };
        let (ux, uz) = (dir.cos(), dir.sin());
        let (px, pz) = (-uz, ux);
        // each leg drifts sideways so the switchback climbs the face
        let drift = (legs as f32) * (width as f32 + 1.0);
        let sway = (base + std::f32::consts::FRAC_PI_2).sin_cos();
        for i in 0..leg {
            if !((down && y > target_y) || (!down && y < target_y)) { break; }
            let x = (fx + ux * i as f32 + sway.1 * drift).round() as i32;
            let z = (fz + uz * i as f32 + sway.0 * drift).round() as i32;
            if i % 3 == 0 { y += if down { -1 } else { 1 }; steps += 1; }
            for o in 0..width {
                let (cx, cz) = ((x as f32 + px * o as f32).round() as i32,
                                (z as f32 + pz * o as f32).round() as i32);
                for k in 0..=3 { if w.get(cx, y + k, cz) != 0 { r.changes.push(Change { at: (cx, y + k, cz), to: 0 }); } }
                if w.get(cx, y - 1, cz) == 0 { r.changes.push(Change { at: (cx, y - 1, cz), to: a.tread.unwrap_or(1) }); }
                else if let Some(t) = a.tread { r.changes.push(Change { at: (cx, y - 1, cz), to: t }); }
            }
            if let Some(rail) = a.rail {
                let (ox, oz) = ((x as f32 + px * width as f32).round() as i32,
                                (z as f32 + pz * width as f32).round() as i32);
                if w.get(ox, y - 1, oz) == 0 { r.changes.push(Change { at: (ox, y, oz), to: rail }); }
            }
        }
        // the landing at the turn: a wider flat square
        let lx = (fx + ux * leg as f32 + sway.1 * drift).round() as i32;
        let lz = (fz + uz * leg as f32 + sway.0 * drift).round() as i32;
        for dx in -(width / 2)..=(width / 2) { for dz in -(width / 2)..=(width / 2) {
            for k in 0..=3 { if w.get(lx + dx, y + k, lz + dz) != 0 { r.changes.push(Change { at: (lx + dx, y + k, lz + dz), to: 0 }); } }
            if w.get(lx + dx, y - 1, lz + dz) == 0 { r.changes.push(Change { at: (lx + dx, y - 1, lz + dz), to: a.tread.unwrap_or(1) }); }
        }}
        legs += 1;
    }
    r.to = Some((fx.round() as i32, y, fz.round() as i32));
    r.steps = steps;
    r.notes.push(format!("{legs} leg(s)"));
    r
}

/// A vertical shaft with rungs and landings: a mine head, a well, the
/// way into a cavern from above.
pub fn shaft<W: VoxelWorld>(w: &W, at: (i32, i32), to_y: i32, radius: i32, a: &Access,
                            from_y: i32) -> AccessResult {
    let mut r = AccessResult::default();
    let radius = radius.clamp(1, 8);
    let top = surface(w, at.0, at.1, from_y);
    if top - to_y < 3 { r.notes.push("no depth to sink".into()); return r; }
    r.from = Some((at.0, top + 1, at.1));
    for y in to_y..=top {
        for dx in -radius..=radius { for dz in -radius..=radius {
            if dx * dx + dz * dz > radius * radius { continue; }
            let (x, z) = (at.0 + dx, at.1 + dz);
            if w.get(x, y, z) != 0 { r.changes.push(Change { at: (x, y, z), to: 0 }); }
        }}
        // rungs up one wall, all the way
        r.changes.push(Change { at: (at.0 + radius, y, at.1), to: a.rung });
        // a landing every eight: somewhere to stand, and what stops a
        // shaft being a hole you die in
        if (top - y) % 8 == 0 && y != top {
            for dx in -radius..=radius { for dz in -radius..=radius {
                if dx * dx + dz * dz > radius * radius { continue; }
                // half the landing, so the ladder still passes
                if dz > 0 { r.changes.push(Change { at: (at.0 + dx, y, at.1 + dz), to: a.tread.unwrap_or(1) }); }
            }}
        }
        if a.light != 0 && a.light_every > 0 && (top - y) % a.light_every == 0 {
            r.changes.push(Change { at: (at.0 - radius, y, at.1), to: a.light });
        }
    }
    r.to = Some((at.0, to_y, at.1));
    r.steps = (top - to_y) as usize;
    r
}

/// Terrace a hillside: steps wide enough to build or farm on, cut
/// along the contour so each is level.
pub fn terrace<W: VoxelWorld>(w: &W, min: (i32, i32), max: (i32, i32), step_h: i32, tread: i32,
                              a: &Access, from_y: i32) -> AccessResult {
    let mut r = AccessResult::default();
    let step_h = step_h.clamp(1, 12);
    let tread = tread.clamp(2, 24);
    let (x0, x1) = (min.0.min(max.0), min.0.max(max.0));
    let (z0, z1) = (min.1.min(max.1), min.1.max(max.1));
    // the heights present, to choose the terrace levels
    let mut lo = i32::MAX; let mut hi = i32::MIN;
    for x in (x0..=x1).step_by(4) { for z in (z0..=z1).step_by(4) {
        let s = surface(w, x, z, from_y); lo = lo.min(s); hi = hi.max(s);
    }}
    if hi - lo < step_h { r.notes.push("too flat to terrace".into()); return r; }
    r.from = Some((x0, lo, z0));
    let mut cut = 0usize;
    for x in x0..=x1 { for z in z0..=z1 {
        let s = surface(w, x, z, from_y);
        // the level this column belongs to: snapped DOWN, so a terrace
        // is cut into the hill rather than filled out from it
        let level = lo + ((s - lo) / step_h) * step_h;
        if s <= level { continue; }
        for y in (level + 1)..=s {
            r.changes.push(Change { at: (x, y, z), to: 0 });
            cut += 1;
        }
        if let Some(t) = a.tread { r.changes.push(Change { at: (x, level, z), to: t }); }
    }}
    // the risers: a face at each level change, left as rock, plus a
    // kerb if one was asked for
    if let Some(rail) = a.rail {
        for x in x0..=x1 { for z in z0..=z1 {
            let s = surface(w, x, z, from_y);
            let level = lo + ((s - lo) / step_h) * step_h;
            let east = surface(w, x + 1, z, from_y);
            let elevel = lo + ((east - lo) / step_h) * step_h;
            if elevel != level && (x + z) % tread == 0 {
                r.changes.push(Change { at: (x, level.max(elevel) + 1, z), to: rail });
            }
        }}
    }
    r.to = Some((x1, hi, z1));
    r.steps = ((hi - lo) / step_h) as usize;
    r.notes.push(format!("{} level(s), {cut} cells cut", r.steps));
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A cliff: ground at 70 west of x=0, at 100 east of it.
    struct Cliff(BTreeMap<(i32, i32, i32), u16>);
    impl Cliff {
        fn new() -> Self { Cliff(BTreeMap::new()) }
        fn h(x: i32) -> i32 { if x < 0 { 70 } else { 100 } }
    }
    impl VoxelWorld for Cliff {
        fn get(&self, x: i32, y: i32, z: i32) -> u16 {
            if let Some(&b) = self.0.get(&(x, y, z)) { return b; }
            if y <= Cliff::h(x) { 1 } else { 0 }
        }
        fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.0.insert((x, y, z), b); true }
    }

    /// A steady slope, 1 in 4 rising east.
    struct Slope(BTreeMap<(i32, i32, i32), u16>);
    impl Slope { fn h(x: i32) -> i32 { 70 + (x.max(0) / 4).min(40) } }
    impl VoxelWorld for Slope {
        fn get(&self, x: i32, y: i32, z: i32) -> u16 {
            if let Some(&b) = self.0.get(&(x, y, z)) { return b; }
            if y <= Slope::h(x) { 1 } else { 0 }
        }
        fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.0.insert((x, y, z), b); true }
    }

    fn apply<W: VoxelWorld>(w: &mut W, r: &AccessResult) {
        for c in &r.changes { w.set(c.at.0, c.at.1, c.at.2, c.to); }
    }

    #[test]
    fn a_cliff_stair_descends_and_is_walkable_the_whole_way() {
        let mut w = Cliff::new();
        // from the top of the cliff, heading west, down to the low ground
        let r = cliff_stair(&w, (20, 0), 180, 71, 3, &Access::default(), 100);
        assert!(r.steps > 25, "{}", r.summary());
        apply(&mut w, &r);
        let (from, to) = (r.from.unwrap(), r.to.unwrap());
        assert!(from.1 > to.1, "it descends: {} to {}", from.1, to.1);
        assert!(to.1 <= 72, "it reaches the bottom: {}", to.1);
        // WALKABLE: from the top, following the steps down, every tread
        // has headroom and the next is one below and one along
        let mut y = from.1 - 1;
        let mut x = from.0;
        let mut walked = 0;
        while y > to.1 + 1 && walked < 60 {
            x -= 1; y -= 1;
            assert_ne!(w.get(x, y - 1, 0), 0, "a tread under the foot at ({x},{y})");
            for k in 0..3 { assert_eq!(w.get(x, y + k, 0), 0, "headroom at ({x},{}) ", y + k); }
            walked += 1;
        }
        assert!(walked > 20, "walked {walked} steps down");
    }

    #[test]
    fn the_stair_leaves_a_lip_on_the_drop_side_only() {
        let w = Cliff::new();
        let r = cliff_stair(&w, (20, 0), 180, 71, 2, &Access { rail: Some(4), ..Default::default() }, 100);
        let rails: Vec<&Change> = r.changes.iter().filter(|c| c.to == 4).collect();
        assert!(!rails.is_empty(), "a lip");
        // every rail sits where there was a drop, not inside the rock
        for c in &rails {
            let below = w.get(c.at.0, c.at.1 - 1, c.at.2);
            let two = w.get(c.at.0, c.at.1 - 2, c.at.2);
            assert!(below == 0 || two == 0, "a lip against solid rock at {:?}", c.at);
        }
    }

    #[test]
    fn a_switchback_climbs_at_a_walkable_grade_with_landings() {
        let mut w = Slope(BTreeMap::new());
        let r = switchback(&w, (0, 0), 0, 100, 3, 20, &Access::default(), 70);
        assert!(r.steps > 10, "{}", r.summary());
        assert!(r.notes.iter().any(|n| n.contains("leg")), "{:?}", r.notes);
        apply(&mut w, &r);
        // the grade: it rises one in three, so it travels at least three
        // times the height it gains
        let (from, to) = (r.from.unwrap(), r.to.unwrap());
        let climbed = (to.1 - from.1).abs();
        assert!(climbed > 8, "it climbed {climbed}");
        // several legs means it does not run straight off the map
        let xs: Vec<i32> = r.changes.iter().map(|c| c.at.0).collect();
        let span = xs.iter().max().unwrap() - xs.iter().min().unwrap();
        assert!(span < 20 * 6, "it zigzags rather than running away: {span}");
    }

    #[test]
    fn a_shaft_sinks_with_rungs_and_landings_and_refuses_a_shallow_one() {
        let mut w = Cliff::new();
        let r = shaft(&w, (20, 0), 60, 2, &Access::default(), 100);
        assert_eq!(r.steps, 40);
        apply(&mut w, &r);
        // the bore is open all the way
        for y in 62..99 { assert_eq!(w.get(20, y, 0), 0, "open at {y}"); }
        // rungs the whole way up one wall
        let rungs = r.changes.iter().filter(|c| c.to == 65).count();
        assert!(rungs >= 40, "{rungs} rungs");
        // landings: some solid partway down, not just at the bottom
        let landings: Vec<i32> = r.changes.iter().filter(|c| c.to == 1 && c.at.1 > 62 && c.at.1 < 99)
            .map(|c| c.at.1).collect();
        assert!(!landings.is_empty(), "somewhere to stand on the way down");
        // and a shaft with nowhere to go says so
        let none = shaft(&w, (20, 0), 99, 2, &Access::default(), 100);
        assert!(none.changes.is_empty() && none.notes.iter().any(|n| n.contains("no depth")));
    }

    #[test]
    fn terracing_a_hillside_leaves_level_steps_and_a_flat_one_is_refused() {
        let mut w = Slope(BTreeMap::new());
        let r = terrace(&w, (0, -10), (80, 10), 4, 8, &Access { tread: Some(3), ..Default::default() }, 70);
        assert!(r.steps >= 4, "{}", r.summary());
        apply(&mut w, &r);
        // each terrace is LEVEL: sample along x and count distinct heights
        let profile: Vec<i32> = (0..80).map(|x| surface(&w, x, 0, 70)).collect();
        let flats = profile.windows(2).filter(|p| p[0] == p[1]).count();
        assert!(flats > 50, "mostly level ground now: {flats}/79");
        let levels: std::collections::BTreeSet<i32> = profile.iter().copied().collect();
        assert!(levels.len() <= 12 && levels.len() >= 4, "a handful of levels: {levels:?}");
        // cut into the hill, never filled above it
        for x in 0..80 { assert!(surface(&w, x, 0, 70) <= Slope::h(x), "filled above the hill at {x}"); }
        // and flat ground is refused rather than churned
        struct Flat;
        impl VoxelWorld for Flat {
            fn get(&self, _: i32, y: i32, _: i32) -> u16 { if y <= 70 { 1 } else { 0 } }
            fn set(&mut self, _: i32, _: i32, _: i32, _: u16) -> bool { true }
        }
        let none = terrace(&Flat, (0, 0), (40, 40), 4, 8, &Access::default(), 70);
        assert!(none.changes.is_empty() && none.notes.iter().any(|n| n.contains("too flat")));
    }

    #[test]
    fn every_access_reports_its_ends_so_something_can_join_it() {
        let w = Cliff::new();
        let a = Access::default();
        for r in [cliff_stair(&w, (20, 0), 180, 71, 3, &a, 100),
                  switchback(&Slope(BTreeMap::new()), (0, 0), 0, 100, 3, 20, &a, 70),
                  shaft(&w, (20, 0), 60, 2, &a, 100)] {
            let (f, t) = (r.from.expect("a start"), r.to.expect("an end"));
            assert_ne!(f, t, "it goes somewhere: {}", r.summary());
            assert!(!r.changes.is_empty());
        }
    }
}
