//! glades — somewhere for a jungle fight to happen.
//!
//! The report: the middle of the jungle was cluttered and its fights were
//! not interesting. A plot of the blockers (every prop, by kind and height)
//! said why:
//!
//! * the band either side of mid, |z| < 40, was an even speckle of trunks,
//!   rocks, ferns and bushes: everywhere a little in the way, nowhere a
//!   shape to use;
//! * the bands beyond it, toward each team's towers, were nearly empty
//!   ground;
//! * the boss pits had a ring round them, and nothing leading to it.
//!
//! A fight wants three things the speckle does not give: height to take
//! (a perch is a strong place to stand and a poor one to be hit on), a
//! line of sight to break (something to circle, juke round, or reset
//! behind), and cover to come out of. So the scatter is thinned in the band
//! (`scene`'s `thin_mid`), and two authored set pieces go where the fights
//! go, each mirrored through the origin like the rest of the map:
//!
//! ## The Steps (one to each pit, from the Hollow's side)
//!
//! A climbing leap chain, rock -> boulders -> pillar cap, each within a
//! leap of the last, ending a leap from the pit's ring. Take the pillar and
//! you can drop into the objective from above; hold the boulders and you
//! can contest the one who did. A broken ruin wall beside the chain hides
//! the climb from the pit, and a hedge of bushes along the other side is
//! somewhere to wait for whoever comes up it.
//!
//! ## The Ring (four: each side of mid, in each half, between the links)
//!
//! Two L-shaped ruin walls making a broken box round a clearing, open at
//! two opposite corners: a place to duel in which there is always a wall to
//! break line of sight at and a wall top (narrow, 3 u) to leap up onto,
//! with a low rock in the middle to take for the height. Ferns outside
//! both gaps: you arrive unseen.
//!
//! Both are data ([`Placement`], as `midjungle`'s are) and are tested
//! without a GPU; [`reserved`] keeps the scatter (and the groves) off them.

use crate::blockers::PropKind;
use crate::midjungle::Placement;

/// Where each piece stands (the +x, z >= 0 copy; [`layout`] mirrors).
/// Between the two lane-to-lane links (|z| 26 and 46), a tower's range
/// short of the side lane's outer tower.
pub const RING: (f32, f32) = (36.0, 36.0);
pub const RING_RADIUS: f32 = 11.5;
/// The Steps run from the pit's ring toward the map's centre, this far.
pub const STEPS_LEN: f32 = 20.0;
pub const STEPS_HALF_WIDTH: f32 = 6.5;

/// One copy of each piece, before mirroring: the Ring on the +x side of the
/// z > 0 band, the Steps climbing toward the +x pit.
fn ring_at(rx: f32, rz: f32) -> Vec<Placement> {
    use PropKind::*;
    let p = |kind, x: f32, z: f32, quarter: u8, scale: f32| Placement { kind, x, z, quarter, scale };
    let mut v = Vec::new();
    // Each wall is an L (`props::ruin_wall`). Turned 0 it walls the south
    // and west of its origin; turned 2, the north and east. The two, set a
    // little apart, make a broken box ~16 x 11 with a gap at two opposite
    // corners (2.2 u clear: a cat and a half) -- two ways in, walls to put
    // between you and a ranged cat on every side, a 3 u wall top to leap up
    // onto, and a low rock in the middle. Kept within z +-6.4 of the centre
    // so it never reaches the lane-to-lane links either side of it.
    v.push(p(RuinWall, rx - 1.5, rz - 0.4, 0, 1.0));
    v.push(p(RuinWall, rx + 1.5, rz + 0.4, 2, 1.0));
    // off the diagonal between the gaps, so the one long look through the
    // box is open
    v.push(p(Rock, rx + 0.8, rz + 1.2, 0, 1.0));
    // stalking beds outside each gap
    for (dx, dz) in [(-9.8f32, 4.5f32), (-8.8, 6.0), (9.8, -4.5), (8.8, -6.0)] {
        v.push(p(Fern, rx + dx, rz + dz, 0, 1.0));
    }
    v
}

/// The Steps' line for a pit: from the pit's ring toward the map's centre.
/// (There are two pits, not four, and they are not quite mirror images of
/// each other, so the Steps are built from where each pit really is.)
fn steps_line(pit: glam::Vec3) -> ((f32, f32), (f32, f32)) {
    let d = (-pit.x, -pit.z);
    let len = d.0.hypot(d.1);
    let (ux, uz) = (d.0 / len, d.1 / len);
    let ring = crate::bosses::PIT_RADIUS + 1.5;
    let a = (pit.x + ux * (ring + 3.0), pit.z + uz * (ring + 3.0));
    (a, (a.0 + ux * STEPS_LEN, a.1 + uz * STEPS_LEN))
}

fn steps_for(pit: glam::Vec3) -> Vec<Placement> {
    use PropKind::*;
    let p = |kind, x: f32, z: f32, quarter: u8, scale: f32| Placement { kind, x, z, quarter, scale };
    let (from, to) = steps_line(pit);
    // t = 0 at the pit end
    let at = |t: f32, side: f32| {
        let (dx, dz) = (to.0 - from.0, to.1 - from.1);
        let len = dx.hypot(dz);
        let (nx, nz) = (-dz / len, dx / len);
        (from.0 + dx * t + nx * side, from.1 + dz * t + nz * side)
    };
    let mut v = Vec::new();
    // pillar cap (4.0 u) nearest the pit, boulders (2.0), then the rock
    // (1.4) you start from: ~6.5 u apart, each a leap from the next
    let (cx, cz) = at(0.12, 0.0);
    v.push(p(RuinPillar, cx, cz, 2, 1.0));
    let (bx, bz) = at(0.45, 0.8);
    v.push(p(BoulderCluster, bx, bz, 1, 1.0));
    let (ax, az) = at(0.78, 0.0);
    v.push(p(Rock, ax, az, 0, 1.0));
    // a wall on one side of the climb, a hedge on the other
    let (wx, wz) = at(0.5, -5.2);
    v.push(p(RuinWall, wx, wz, if pit.x > 0.0 { 0 } else { 2 }, 1.0));
    for t in [0.15f32, 0.4, 0.65, 0.9] {
        let (hx, hz) = at(t, 4.8);
        v.push(p(Bush, hx, hz, 0, 1.0));
    }
    v
}

/// Every placement: [`one`], its copy across mid (x -> -x) on the same
/// side of the river, and both mirrored through the origin to the other
/// team's half.
pub fn layout() -> Vec<Placement> {
    // one Ring each side of mid in this half (the same Ring, moved: a
    // mirrored L would change which corners are open), and each turned a
    // half-turn through the origin into the other team's half
    let mut v = ring_at(RING.0, RING.1);
    v.extend(ring_at(-RING.0, RING.1));
    // a bed outside a gap that faces a tower is in its cleared ground (the
    // -x Ring's north-west one): leave it out, as midjungle does
    v.retain(|p| !crate::consts::in_tower_clearing(p.x, p.z));
    let n = v.len();
    for i in 0..n {
        let p = v[i];
        v.push(Placement { x: -p.x, z: -p.z, quarter: (p.quarter + 2) % 4, ..p });
    }
    for pit in crate::bosses::pit_positions() {
        v.extend(steps_for(pit));
    }
    v
}

/// Ground the scatter and the groves must leave alone: each Ring's
/// clearing and each Steps' climb.
pub fn reserved(x: f32, z: f32) -> bool {
    if crate::map::active() != crate::map::MapId::Jungle {
        return false;
    }
    let ring = [(RING.0, RING.1), (-RING.0, RING.1), (-RING.0, -RING.1), (RING.0, -RING.1)];
    if ring.iter().any(|&(cx, cz)| (x - cx).hypot(z - cz) < RING_RADIUS) {
        return true;
    }
    crate::bosses::pit_positions().iter().any(|&pit| {
        let (a, b) = steps_line(pit);
        seg_dist(x, z, a, b) < STEPS_HALF_WIDTH
    })
}

fn seg_dist(x: f32, z: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dz) = (b.0 - a.0, b.1 - a.1);
    let t = (((x - a.0) * dx + (z - a.1) * dz) / (dx * dx + dz * dz)).clamp(0.0, 1.0);
    (x - a.0 - dx * t).hypot(z - a.1 - dz * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blockers::{Blockers, Perch};

    fn world() -> Blockers {
        let mut b = Blockers::new(200.0, 6.0);
        for p in layout() {
            b.push_placed(p.kind, p.x, p.z, 0.0, p.quarter, p.scale);
        }
        b
    }

    /// Four of each piece, the map's point symmetry kept, and nothing in
    /// the way of a lane, a tower's ground, a pit or the authored centre.
    #[test]
    fn the_glades_are_fair_and_keep_off_what_is_already_there() {
        let l = layout();
        // two walls a Ring, four Rings; one wall each Steps, two Steps --
        // whatever beds a tower's ground took
        assert_eq!(l.iter().filter(|p| p.kind == PropKind::RuinWall).count(), 4 * 2 + 2);
        assert_eq!(l.iter().filter(|p| p.kind == PropKind::RuinPillar).count(), 2);
        for p in &l {
            // the Rings are mirrored exactly; the Steps follow the two pits
            if p.z.abs() > 30.0 {
                assert!(l.iter().any(|q| q.kind == p.kind && (q.x + p.x).abs() < 1e-4 && (q.z + p.z).abs() < 1e-4), "{p:?} has no mirror");
            }
            let r = p.kind.footprint().radius.max(1.0);
            assert!(crate::consts::lane_clearance(p.x, p.z) > crate::midjungle::LANE_KEEP + r, "{p:?} crowds a lane");
            assert!(!crate::consts::in_tower_clearing(p.x, p.z), "{p:?} in a tower's ground");
            assert!(!crate::bosses::in_pit(p.x, p.z), "{p:?} in a pit");
            assert!(!crate::midjungle::reserved(p.x, p.z), "{p:?} on the Hollow, a run or a link");
            assert!(reserved(p.x, p.z), "{p:?} not on its own reserved ground: the scatter would land in it");
        }
    }

    /// The Steps climb: rock, boulders, pillar, each higher and each within
    /// a leap of the last, the pillar a leap from the pit's ring.
    #[test]
    fn the_steps_climb_to_the_pit_a_leap_at_a_time() {
        use crate::leap::{LEAP_RANGE, LEAP_UP};
        let l = layout();
        let pits = crate::bosses::pit_positions();
        let mut chains = 0;
        for kind_pos in l.iter().filter(|p| p.kind == PropKind::RuinPillar) {
            let near = |k: PropKind| l.iter().filter(|p| p.kind == k).map(|p| (p.x, p.z, (p.x - kind_pos.x).hypot(p.z - kind_pos.z))).fold((0.0, 0.0, f32::MAX), |a, b| if b.2 < a.2 { b } else { a });
            let boulder = near(PropKind::BoulderCluster);
            let rock = l.iter().filter(|p| p.kind == PropKind::Rock).map(|p| (p.x - boulder.0).hypot(p.z - boulder.1)).fold(f32::MAX, f32::min);
            assert!(boulder.2 < LEAP_RANGE, "boulders to pillar {:.1} u", boulder.2);
            assert!(rock < LEAP_RANGE, "rock to boulders {rock:.1} u");
            let tops = [PropKind::Rock, PropKind::BoulderCluster, PropKind::RuinPillar].map(|k| k.footprint().top);
            assert!(tops[0] < tops[1] && tops[1] < tops[2] && tops[2] - tops[1] < LEAP_UP, "a climb: {tops:?}");
            let ring = pits.iter().map(|q| (q.x - kind_pos.x).hypot(q.z - kind_pos.z) - crate::bosses::PIT_RADIUS - 1.5).fold(f32::MAX, f32::min);
            assert!((2.0..LEAP_RANGE + 1.0).contains(&ring), "pillar to the pit's ring {ring:.1} u");
            chains += 1;
        }
        assert_eq!(chains, crate::bosses::pit_positions().len());
    }

    /// A Ring is somewhere to fight: open in the middle, a perch in it,
    /// walls to break a line of sight at, and a way in at each open corner.
    #[test]
    fn a_ring_has_walls_to_break_sight_a_perch_and_ways_in() {
        let b = world();
        for (sx, sz) in [(1.0f32, 1.0f32), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)] {
            let (cx, cz) = (RING.0 * sx, RING.1 * sz);
            // walkable ground in the clearing: most of the disc inside 7 u
            let mut open = 0;
            let mut n = 0;
            for i in 0..24 {
                for rr in [2.5f32, 4.0, 5.0] {
                    let a = i as f32 / 24.0 * std::f32::consts::TAU;
                    n += 1;
                    open += !b.blocked(cx + a.cos() * rr, cz + a.sin() * rr, 0.55) as usize;
                }
            }
            assert!(open * 10 >= n * 6, "ring at ({cx}, {cz}): only {open}/{n} of its clearing walkable");
            // a perch to take
            let perches = b.with_perch().filter(|(bl, p)| *p != Perch::None && bl.shape.nearest_axis(cx, cz).0.hypot(0.0) >= 0.0 && {
                let (ax, az, _) = bl.shape.nearest_axis(cx, cz);
                (ax - cx).hypot(az - cz) < RING_RADIUS
            }).count();
            assert!(perches >= 4, "ring at ({cx}, {cz}): {perches} perches");
            // and one of them is a flat one in the middle: height to take
            // that is not a 3 u wall top
            let flat = b.with_perch().filter(|(bl, p)| *p == Perch::Flat && {
                let (ax, az, _) = bl.shape.nearest_axis(cx, cz);
                (ax - cx).hypot(az - cz) < 4.0
            }).count();
            assert_eq!(flat, 1, "ring at ({cx}, {cz}): a flat perch in the middle");
            // sight lines across the clearing: some blocked, some open
            let mut blocked = 0;
            for i in 0..12 {
                let a = i as f32 / 12.0 * std::f32::consts::TAU;
                let from = (cx + a.cos() * 9.5, cz + a.sin() * 9.5);
                let to = (cx - a.cos() * 9.5, cz - a.sin() * 9.5);
                blocked += (b.sight_obstruction(from, to) >= 1.0) as usize;
            }
            assert!((3..=11).contains(&blocked), "ring at ({cx}, {cz}): {blocked}/12 sight lines across it blocked");
            // ways in: from 13 u out, walk straight to the middle along how
            // many of 12 bearings? (the walls leave gaps between them)
            let mut ways = 0;
            for i in 0..12 {
                let a = i as f32 / 12.0 * std::f32::consts::TAU;
                let clear = (0..=20).all(|k| {
                    let rr = 3.0 + k as f32 * 0.5;
                    !b.blocked(cx + a.cos() * rr, cz + a.sin() * rr, 0.55)
                });
                ways += clear as usize;
            }
            assert!(ways >= 2, "ring at ({cx}, {cz}): {ways} straight ways in");
        }
    }
}
