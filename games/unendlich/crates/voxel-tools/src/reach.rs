//! Can you actually get there?
//!
//! Everything in this crate produces geometry. Almost nothing checks
//! that the geometry is USABLE. The critique pass finds blocks with
//! nothing under them and a player sealed in a wall; it does not know
//! whether a village's doors open onto anything, whether the stair
//! just cut into a cliff reaches the floor, or whether the cave mouth
//! leads to the chamber.
//!
//! Those are all one question -- can a walking body get from A to B --
//! and it has one answer: flood fill the standable space.
//!
//! WHAT "STANDABLE" MEANS
//!
//! A cell you can stand in: solid underfoot, and `height` cells of air
//! above it. Moving from one to another is allowed when
//!
//!   * they are horizontally adjacent, and
//!   * the step up is at most `step_up` (a block, or two for a stair
//!     with a rail), or the drop is at most `max_drop` (you can jump
//!     down further than you can climb, which is what makes a terrace
//!     one-way unless it has a ramp), and
//!   * the diagonal is not squeezed between two solid corners
//!
//! That is a walking body, not a flying one, and the difference is the
//! whole point: a flying check says a canyon floor is reachable
//! because it is open air.
//!
//! WHAT IT IS FOR
//!
//! * `reaches` -- is B reachable from A, and if not, how close does it
//!   get? The near miss is the useful part: "the stair stops 4 blocks
//!   short" is actionable, "unreachable" is not.
//! * `unreachable_doors` -- every carved opening in a build that leads
//!   nowhere. The commonest failure a generated village has.
//! * `islands` -- the standable space split into components, so a
//!   feature can be told it made two places rather than one.
//!
//! Bounded: a flood fill over a canyon is millions of cells, so every
//! entry point takes a budget and reports when it ran out rather than
//! silently exploring half the world.

use crate::VoxelWorld;
use std::collections::{BTreeSet, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Body {
    /// Cells of air needed above the floor.
    pub height: i32,
    /// The tallest step up that can be climbed.
    pub step_up: i32,
    /// The furthest drop that can be survived.
    pub max_drop: i32,
    /// Cells visited before giving up.
    pub budget: usize,
    /// How far from the straight line between the ends the search may
    /// wander, or None for a free flood. A body walking to a door does
    /// not explore the continent on the way, and on open ground a free
    /// flood spends its whole budget going the wrong way -- the first
    /// version did exactly that and never reached a target twenty
    /// blocks off.
    pub corridor: Option<i32>,
}

impl Default for Body {
    fn default() -> Self { Self { height: 2, step_up: 1, max_drop: 4, budget: 200_000, corridor: Some(48) } }
}

/// Is this a cell a body can stand in?
pub fn standable<W: VoxelWorld>(w: &W, at: (i32, i32, i32), b: &Body) -> bool {
    let floor = w.get(at.0, at.1 - 1, at.2);
    if floor == 0 { return false; }                                // nothing underfoot
    // DEEP WATER IS NOT A FLOOR. A flooded canyon read as walkable
    // ground because water is solid enough to stand ON in this check
    // and passable enough to stand IN; you can wade a shallow edge,
    // but two blocks of water over your head is a swim, and a body
    // that cannot swim should not be told the lake bottom is reachable.
    // How deep is the water under the feet? A body wades one block and
    // swims two. The first version asked only about the cell below and
    // the one below that -- but a body standing ON the surface of a
    // seven-deep lake has water at -1 and -2 and so does one standing
    // in a ford, once the ford is a block deep. Count down to the bed.
    if liquid(floor) {
        let below = (1..=4).take_while(|k| liquid(w.get(at.0, at.1 - k, at.2))).count();
        if below >= 2 { return false; }
    }
    // And how much water is OVER the head? A body wading a ford has
    // sky above it; one on the bed of a seven-deep lake has seven
    // blocks of water, and that is a swim however solid the bed is.
    // Checking only downward let a body walk the whole lake floor.
    if liquid(w.get(at.0, at.1, at.2)) || liquid(w.get(at.0, at.1 + 1, at.2)) {
        let over = (0..=6).take_while(|k| liquid(w.get(at.0, at.1 + k, at.2))).count();
        if over >= 2 { return false; }
    }
    (0..b.height).all(|k| passable(w.get(at.0, at.1 + k, at.2)))
}

/// A block a body can occupy: air, or something you walk through.
fn passable(block: u16) -> bool { matches!(block, 0 | 31 | 38 | 37 | 175 | 8 | 9) }

/// Is it liquid? Wading is walking; a swim is not.
fn liquid(block: u16) -> bool { matches!(block, 8 | 9 | 10 | 11) }

/// The cells reachable from a start, and whether the budget ran out.
pub fn flood<W: VoxelWorld>(w: &W, from: (i32, i32, i32), b: &Body)
    -> (BTreeSet<(i32, i32, i32)>, bool) {
    flood_toward(w, from, None, b)
}

/// The flood, optionally bounded to a corridor around the line from
/// `from` to `toward`, and stopping as soon as `toward` is reached.
pub fn flood_toward<W: VoxelWorld>(w: &W, from: (i32, i32, i32), toward: Option<(i32, i32, i32)>,
                                   b: &Body) -> (BTreeSet<(i32, i32, i32)>, bool) {
    let mut seen: BTreeSet<(i32, i32, i32)> = BTreeSet::new();
    let mut q: VecDeque<(i32, i32, i32)> = VecDeque::new();
    // the start may be a block above or below a standable cell: a door
    // sill, a stair tread. Snap to the nearest standable within a step.
    let start = (-2..=2).map(|dy| (from.0, from.1 + dy, from.2))
        .find(|c| standable(w, *c, b));
    let Some(start) = start else { return (seen, false) };
    q.push_back(start);
    seen.insert(start);
    let mut exhausted = false;
    // Distance from the line between the ends, for the corridor bound.
    let in_corridor = |c: (i32, i32, i32)| -> bool {
        let (Some(t), Some(pad)) = (toward, b.corridor) else { return true };
        let (ax, az) = (start.0 as f32, start.2 as f32);
        let (bx, bz) = (t.0 as f32, t.2 as f32);
        let (dx, dz) = (bx - ax, bz - az);
        let len2 = dx * dx + dz * dz;
        if len2 < 1.0 { return (c.0 - t.0).abs() <= pad && (c.2 - t.2).abs() <= pad; }
        let u = (((c.0 as f32 - ax) * dx + (c.2 as f32 - az) * dz) / len2).clamp(-0.2, 1.2);
        let (px, pz) = (ax + dx * u, az + dz * u);
        let off = ((c.0 as f32 - px).powi(2) + (c.2 as f32 - pz).powi(2)).sqrt();
        off <= pad as f32
    };
    // A body walking somewhere stops when it gets there.
    let arrived = |c: (i32, i32, i32)| toward.map(|t|
        (c.0 - t.0).abs() <= 1 && (c.1 - t.1).abs() <= 1 && (c.2 - t.2).abs() <= 1).unwrap_or(false);
    if arrived(start) { return (seen, false); }
    while let Some(c) = q.pop_front() {
        if seen.len() >= b.budget { exhausted = true; break; }
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
            let diagonal = dx != 0 && dz != 0;
            // a diagonal squeezed between two corners is not a move a
            // body makes: it would clip through the corner
            if diagonal {
                let a = passable(w.get(c.0 + dx, c.1, c.2));
                let bb = passable(w.get(c.0, c.1, c.2 + dz));
                if !a || !bb { continue; }
            }
            // up to step_up, or down to max_drop
            for dy in (-b.max_drop)..=b.step_up {
                let n = (c.0 + dx, c.1 + dy, c.2 + dz);
                if seen.contains(&n) { continue; }
                if !standable(w, n, b) { continue; }
                if !in_corridor(n) { continue; }
                // climbing needs headroom over the STEP as well, or you
                // walk into the ceiling on the way up
                if dy > 0 && !(0..b.height).all(|k| passable(w.get(c.0, c.1 + dy + k, c.2))) { continue; }
                seen.insert(n);
                if arrived(n) { return (seen, false); }
                q.push_back(n);
                break;      // the first standable at this column wins
            }
        }
    }
    (seen, exhausted)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reach {
    pub ok: bool,
    /// How close the flood got, in blocks.
    pub nearest: i32,
    /// The cell it got closest at.
    pub nearest_at: Option<(i32, i32, i32)>,
    pub visited: usize,
    pub exhausted: bool,
}

impl Reach {
    pub fn summary(&self, to: (i32, i32, i32)) -> String {
        if self.ok { return format!("reachable ({} cells)", self.visited); }
        match self.nearest_at {
            Some(a) => format!("NOT reachable: gets within {} blocks, at ({},{},{}), short of ({},{},{}){}",
                               self.nearest, a.0, a.1, a.2, to.0, to.1, to.2,
                               if self.exhausted { " -- search budget ran out" } else { "" }),
            None => "NOT reachable: nowhere to stand at the start".into(),
        }
    }
}

/// Can a body walk from `from` to `to`?
pub fn reaches<W: VoxelWorld>(w: &W, from: (i32, i32, i32), to: (i32, i32, i32), b: &Body) -> Reach {
    let (seen, exhausted) = flood_toward(w, from, Some(to), b);
    let near = |c: &(i32, i32, i32)| (c.0 - to.0).abs().max((c.1 - to.1).abs()).max((c.2 - to.2).abs());
    let ok = seen.iter().any(|c| near(c) <= 1);
    let nearest_at = seen.iter().min_by_key(|c| near(c)).copied();
    Reach { ok, nearest: nearest_at.map(|c| near(&c)).unwrap_or(i32::MAX),
            nearest_at, visited: seen.len(), exhausted }
}

/// A DOORWAY in a build's shell: a cell of air in a wall, with solid
/// on two opposite sides, standing on the ground outside.
///
/// The "on the ground" part is the whole distinction. Without it this
/// finds every upper-storey WINDOW too, and a window is correctly
/// unreachable on foot -- a village reported twelve unreachable
/// openings and every one of them was a first-floor window doing its
/// job. A doorway is an opening you can walk through.
pub fn openings<W: VoxelWorld>(w: &W, min: (i32, i32, i32), max: (i32, i32, i32))
    -> Vec<(i32, i32, i32)> {
    let mut out = Vec::new();
    for x in min.0..=max.0 { for y in min.1..=max.1 { for z in min.2..=max.2 {
        if !passable(w.get(x, y, z)) { continue; }
        if w.get(x, y - 1, z) == 0 { continue; }            // must have a sill
        let ew = w.get(x - 1, y, z) != 0 && w.get(x + 1, y, z) != 0;
        let ns = w.get(x, y, z - 1) != 0 && w.get(x, y, z + 1) != 0;
        if ew == ns { continue; }                            // a corridor or open air
        // and it must be an opening in something: solid above
        if w.get(x, y + 2, z) == 0 && w.get(x, y + 3, z) == 0 { continue; }
        // A DOOR, not a window: the ground outside is within a step of
        // the sill. The direction "outside" is whichever side is open,
        // and both are checked because a door faces either way.
        let (ax, az) = if ew { (0, 1) } else { (1, 0) };
        // A door's sill is at GROUND LEVEL outside: the standable
        // surface two blocks out is within a step of the sill. A
        // first-floor window fails this because the ground is metres
        // below, and a gap between two roof planks fails it because
        // the "ground" beside it is the roof it is part of, which is
        // level with the sill but with the BUILD under it rather than
        // the world -- so the walk from outside is what settles it,
        // and that is exactly what `unreachable_doors` does. What is
        // needed here is only that a body could stand outside it.
        let ground_near = [(ax, az), (-ax, -az)].iter().any(|(dx, dz)| {
            let (ox, oz) = (x + dx * 2, z + dz * 2);
            (-1..=1).any(|dy| w.get(ox, y + dy - 1, oz) != 0
                              && (0..2).all(|k| w.get(ox, y + dy + k, oz) == 0))
        });
        if !ground_near { continue; }
        out.push((x, y, z));
    }}}
    out
}

/// Openings that lead nowhere: from the outside you cannot reach them,
/// or from them you cannot reach the outside. The commonest failure a
/// generated building has.
pub fn unreachable_doors<W: VoxelWorld>(w: &W, min: (i32, i32, i32), max: (i32, i32, i32),
                                        outside: (i32, i32, i32), b: &Body)
    -> (Vec<(i32, i32, i32)>, usize) {
    let doors = openings(w, min, max);
    let total = doors.len();
    if doors.is_empty() { return (Vec::new(), 0); }
    // One CORRIDOR-BOUNDED walk per door, not one free flood of the
    // world. A free flood spends its whole budget on open ground around
    // a hut whose doors are all fine -- 147 ms for a hut -- and it
    // stops as soon as it arrives, so a reachable door costs almost
    // nothing. Only a door that is actually blocked pays for a search.
    let near = Body { corridor: Some(24), budget: b.budget.min(8_000), ..*b };
    let bad = doors.into_iter()
        .filter(|d| !reaches(w, outside, *d, &near).ok)
        .collect();
    (bad, total)
}

/// The standable space in a box, split into connected components.
/// Two islands where one was intended is a feature that made a place
/// and a place beside it.
pub fn islands<W: VoxelWorld>(w: &W, min: (i32, i32, i32), max: (i32, i32, i32), b: &Body,
                              sample: i32) -> Vec<(usize, (i32, i32, i32))> {
    let sample = sample.max(1);
    let mut unassigned: Vec<(i32, i32, i32)> = Vec::new();
    for x in (min.0..=max.0).step_by(sample as usize) {
        for z in (min.2..=max.2).step_by(sample as usize) {
            // the top standable cell of the column: where a body would be
            if let Some(y) = (min.1..=max.1).rev().find(|&y| standable(w, (x, y, z), b)) {
                unassigned.push((x, y, z));
            }
        }
    }
    let mut out = Vec::new();
    let mut claimed: BTreeSet<(i32, i32, i32)> = BTreeSet::new();
    for seed in unassigned {
        if claimed.contains(&seed) { continue; }
        let (comp, _) = flood(w, seed, &Body { corridor: None, ..*b });
        // which of the samples this component covers
        let mut n = 0;
        for c in &comp { if claimed.insert(*c) { n += 1; } }
        if n > 0 { out.push((comp.len(), seed)); }
    }
    out.sort_by_key(|(n, _)| std::cmp::Reverse(*n));
    out
}

/// A finding in the same shape the critique pass uses.
pub fn check_access<W: VoxelWorld>(w: &W, from: (i32, i32, i32), to: (i32, i32, i32),
                                   what: &str, b: &Body) -> Option<crate::critique::Finding> {
    let r = reaches(w, from, to, b);
    if r.ok { return None; }
    Some(crate::critique::Finding {
        kind: "unreachable",
        message: format!("{what}: {}", r.summary(to)),
        at: r.nearest_at.unwrap_or(from),
    })
}

/// Where each island's samples are, for a report.
pub fn describe_islands(islands: &[(usize, (i32, i32, i32))]) -> String {
    match islands.len() {
        0 => "nowhere to stand".into(),
        1 => format!("one connected place ({} cells)", islands[0].0),
        n => format!("{n} SEPARATE places: {}", islands.iter().take(4)
            .map(|(c, a)| format!("{c} cells at ({},{},{})", a.0, a.1, a.2))
            .collect::<Vec<_>>().join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct W(BTreeMap<(i32, i32, i32), u16>, i32);
    impl W {
        fn flat(h: i32) -> Self { W(BTreeMap::new(), h) }
        fn fill(&mut self, lo: (i32, i32, i32), hi: (i32, i32, i32), b: u16) {
            for x in lo.0..=hi.0 { for y in lo.1..=hi.1 { for z in lo.2..=hi.2 { self.0.insert((x, y, z), b); } } }
        }
    }
    impl VoxelWorld for W {
        fn get(&self, x: i32, y: i32, z: i32) -> u16 {
            if let Some(&b) = self.0.get(&(x, y, z)) { return b; }
            if y <= self.1 { 1 } else { 0 }
        }
        fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.0.insert((x, y, z), b); true }
    }

    #[test]
    fn a_body_walks_flat_ground_and_climbs_one_block_but_not_three() {
        let mut w = W::flat(64);
        let b = Body::default();
        assert!(reaches(&w, (0, 65, 0), (20, 65, 0), &b).ok, "flat ground");
        // a one-block step, right across the corridor: climbable
        w.fill((10, 65, -60), (10, 65, 60), 1);
        assert!(reaches(&w, (0, 65, 0), (20, 65, 0), &b).ok, "a kerb is climbable");
        // a three-block wall across it: not. It has to span the whole
        // corridor -- a wall with an end is a wall you walk around, and
        // the first version of this test built one five blocks wide.
        w.fill((10, 66, -60), (10, 67, 60), 1);
        let r = reaches(&w, (0, 65, 0), (20, 65, 0), &b);
        assert!(!r.ok, "a wall stops it");
        assert!(r.nearest >= 9 && r.nearest <= 11, "it gets to the wall: {}", r.nearest);
        assert!(r.summary((20, 65, 0)).contains("gets within"), "{}", r.summary((20, 65, 0)));
    }

    #[test]
    fn a_drop_is_one_way_which_is_what_a_terrace_is() {
        let mut w = W::flat(64);
        // a plateau: everything east of x=0 is four higher
        w.fill((1, 65, -80), (40, 68, 80), 1);
        let b = Body::default();
        // down off it: fine
        assert!(reaches(&w, (20, 69, 0), (-10, 65, 0), &b).ok, "you can jump down");
        // back up: not
        assert!(!reaches(&w, (-10, 65, 0), (20, 69, 0), &b).ok, "you cannot climb four blocks");
        // and with a stair cut into it, you can
        for (i, x) in (1..=4).enumerate() { w.fill((x, 65 + i as i32, -1), (x, 68, 1), 0); }
        assert!(reaches(&w, (-10, 65, 0), (20, 69, 0), &b).ok, "the stair makes it two-way");
    }

    #[test]
    fn headroom_is_needed_over_the_step_not_just_the_landing() {
        let mut w = W::flat(64);
        // a step up with a ceiling right over it: you would walk into it
        w.fill((5, 65, -60), (5, 65, 60), 1);        // the step, right across
        w.fill((5, 67, -60), (6, 68, 60), 1);        // a low ceiling beyond
        let b = Body::default();
        let r = reaches(&w, (0, 65, 0), (10, 65, 0), &b);
        assert!(!r.ok, "a body does not squeeze under a ceiling onto a step");
    }

    #[test]
    fn a_diagonal_through_a_corner_is_refused() {
        let mut w = W::flat(64);
        // A pocket with only a diagonal out: walls on all four sides of
        // (0,65,0) except the corner toward (1,·,1). A body must not
        // squeeze through it.
        w.fill((1, 65, -1), (1, 68, 0), 1);
        w.fill((-1, 65, 1), (0, 68, 1), 1);
        w.fill((-1, 65, -1), (-1, 68, 1), 1);
        w.fill((-1, 65, -1), (1, 68, -1), 1);
        let b = Body::default();
        let r = reaches(&w, (0, 65, 0), (4, 65, 4), &b);
        assert!(!r.ok, "you cannot cut the corner of two walls: {}", r.summary((4, 65, 4)));
    }

    #[test]
    fn a_window_is_not_a_door() {
        let mut w = W::flat(64);
        // a two-storey hut: a door at ground level, a window above it
        w.fill((0, 65, 0), (8, 73, 8), 1);
        w.fill((1, 65, 1), (7, 72, 7), 0);
        w.fill((4, 65, 0), (4, 66, 0), 0);           // the door
        w.fill((4, 70, 0), (4, 71, 0), 0);           // the window
        let doors = openings(&w, (0, 65, 0), (8, 73, 8));
        assert!(doors.iter().any(|d| d.1 <= 66), "the door is found: {doors:?}");
        assert!(!doors.iter().any(|d| d.1 >= 70), "the window is not a door: {doors:?}");
        // a gap between two roof planks has no headroom beside it: it
        // is not somewhere a body stands
        let mut roof = W::flat(64);
        roof.fill((0, 65, 0), (8, 70, 8), 1);
        roof.fill((0, 71, 0), (8, 71, 8), 86);
        roof.fill((4, 71, 4), (4, 71, 4), 0);
        let found = openings(&roof, (0, 65, 0), (8, 72, 8));
        assert!(found.iter().all(|d| d.1 < 71), "a hole in a roof is not a door: {found:?}");
    }

    #[test]
    fn a_door_that_leads_nowhere_is_found() {
        let mut w = W::flat(64);
        // a hut with one door
        w.fill((0, 65, 0), (8, 69, 8), 1);
        w.fill((1, 65, 1), (7, 68, 7), 0);           // hollow it
        w.fill((4, 65, 0), (4, 66, 0), 0);           // a door in the south wall
        let b = Body::default();
        let (bad, total) = unreachable_doors(&w, (0, 65, 0), (8, 69, 8), (4, 65, -6), &b);
        assert!(total >= 1, "it found the door: {total}");
        assert!(bad.is_empty(), "and it is reachable: {bad:?}");
        // now wall it off from outside
        w.fill((4, 65, -2), (4, 68, -2), 1);
        w.fill((3, 65, -2), (5, 68, -1), 1);
        let (bad2, _) = unreachable_doors(&w, (0, 65, 0), (8, 69, 8), (4, 65, -6), &b);
        assert!(!bad2.is_empty(), "a door behind a wall leads nowhere");
    }

    #[test]
    fn islands_finds_two_places_where_one_was_meant() {
        let mut w = W::flat(64);
        // a chasm splitting the ground
        w.fill((10, 50, -80), (14, 64, 80), 0);
        let b = Body::default();
        // a modest budget: the point is the COUNT of places, and a flat
        // world's rim is unbounded
        let b = Body { budget: 20_000, ..b };
        let is = islands(&w, (-20, 50, -20), (30, 70, 20), &b, 8);
        assert!(is.len() >= 3, "two rims and a chasm floor: {}", describe_islands(&is));
        assert!(describe_islands(&is).contains("SEPARATE"));
        // Bridge the two rims. The chasm FLOOR stays its own place --
        // which is right, and is exactly what this check is for: a
        // bridge over a canyon does not make the canyon floor reachable.
        w.fill((10, 64, -1), (14, 64, 1), 1);
        let joined = islands(&w, (-20, 50, -20), (30, 70, 20), &b, 8);
        assert!(joined.len() < is.len(), "the bridge joined the rims: {}", describe_islands(&joined));
        assert!(joined.len() >= 2, "and the floor is still cut off, which is true");
    }

    #[test]
    fn the_budget_is_respected_and_says_so() {
        let w = W::flat(64);
        let b = Body { budget: 500, corridor: Some(6), ..Default::default() };
        let r = reaches(&w, (0, 65, 0), (5000, 65, 0), &b);
        assert!(!r.ok);
        assert!(r.exhausted, "it ran out rather than exploring the world");
        assert!(r.visited <= 520, "{} cells", r.visited);
        assert!(r.summary((5000, 65, 0)).contains("budget ran out"));
    }

    #[test]
    fn check_access_reads_like_a_critique_finding() {
        let mut w = W::flat(64);
        w.fill((10, 65, -80), (10, 70, 80), 1);
        let f = check_access(&w, (0, 65, 0), (20, 65, 0), "the stair to the mill", &Body::default())
            .expect("a finding");
        assert!(f.message.starts_with("the stair to the mill: NOT reachable"), "{}", f.message);
        assert_eq!(f.kind, "unreachable");
        // and nothing when it works
        let clear = W::flat(64);
        assert!(check_access(&clear, (0, 65, 0), (20, 65, 0), "x", &Body::default()).is_none());
    }

    #[test]
    fn a_shallow_ford_is_walkable_and_a_lake_is_not() {
        let mut w = W::flat(64);
        // a channel two deep, filled: the bottom is at 63
        w.fill((10, 63, -90), (14, 64, 90), 9);
        let b = Body::default();
        // one block of water over a solid bed: a ford
        assert!(reaches(&w, (0, 65, 0), (20, 65, 0), &b).ok, "you can wade a ford");
        // dig it deeper and fill it: now it is a lake
        w.fill((10, 58, -90), (14, 64, 90), 9);
        assert!(!reaches(&w, (0, 65, 0), (20, 65, 0), &b).ok, "you cannot walk a lake bottom");
    }
}
