//! Navigation — bridges a goal *target* to a movement *heading* using the
//! flow-field pathfinder ([`crate::world::pathing::NavField`]).
//!
//! A goal (from the goals layer) names a target cell; this turns "go there" into
//! an obstacle-aware compass heading: build a local nav region around the
//! creature, compute the distance-to-goal field with a locomotion-appropriate
//! `passable` predicate, read the downhill gradient at the creature's cell, and
//! snap its horizontal part to one of the 8 headings the mob brain already
//! understands.
//!
//! Server-side: `NavField` is `f32` and runs *outside* the rolled-back core; the
//! only thing that crosses back into the deterministic sim is the integer
//! heading (as the creature's input). The CPU reference here is also what a
//! future GPU flow-field pass would mirror.
//!
//! Vertical note: the heading is horizontal (the mob `Input` is move-x/z plus a
//! jump button). Ground creatures route around full-height obstacles on the
//! walkable surface; true 3D flight for dragons is a later locomotion mode with
//! its own action mapping.

use crate::sim::physics::WorldQuery;
use crate::world::pathing::NavField;

/// Standing vs crawling posture. A creature drops to a crawl when the ceiling is
/// too low for its upright (hair-tipped) height, and rises again only once there
/// is clear headroom. The gap between the two thresholds (hysteresis) keeps it
/// from flickering between stand and crawl while brushing the ceiling.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Posture {
    Upright,
    Crawl,
}

/// Decide posture from `clearance` (open height above the creature's feet) and
/// its `upright_height` (full standing height incl. hair — see
/// `CreatureBlueprint::standing_height`). Drops to `Crawl` once the hair would
/// brush the ceiling (clearance below the upright height) and only rises back to
/// `Upright` after clearance exceeds it by 15%.
pub fn posture_for_clearance(current: Posture, clearance: f32, upright_height: f32) -> Posture {
    let rise = upright_height * 1.15;
    match current {
        Posture::Upright if clearance < upright_height => Posture::Crawl,
        Posture::Crawl if clearance >= rise => Posture::Upright,
        other => other,
    }
}

/// How a creature is allowed to occupy cells, which decides what's `passable`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Locomotion {
    /// Walks the surface: a cell is passable if it's open *and* has solid ground
    /// directly beneath it.
    Walk,
    /// Moves through any open air (e.g. a flier planning a horizontal route).
    Fly,
    /// Moves through any open cell in a fluid volume (a swimmer). Same routing
    /// as `Fly` for now; buoyancy and staying-submerged await a fluid query.
    Swim,
}

/// A rectangular world-cell window the flow field is computed over. `min` is the
/// world cell mapped to local `(0,0,0)`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct NavRegion {
    pub min: (i32, i32, i32),
    pub dims: (usize, usize, usize),
}

impl NavRegion {
    /// A region of horizontal half-extent `radius` and vertical `height`,
    /// starting `drop` cells below `center` (so the ground beneath a walker is
    /// included).
    pub fn around(center: (i32, i32, i32), radius: i32, height: usize, drop: i32) -> NavRegion {
        NavRegion {
            min: (center.0 - radius, center.1 - drop, center.2 - radius),
            dims: ((2 * radius + 1) as usize, height, (2 * radius + 1) as usize),
        }
    }

    fn to_local(&self, w: (i32, i32, i32)) -> Option<(usize, usize, usize)> {
        let (lx, ly, lz) = (w.0 - self.min.0, w.1 - self.min.1, w.2 - self.min.2);
        if lx >= 0
            && ly >= 0
            && lz >= 0
            && (lx as usize) < self.dims.0
            && (ly as usize) < self.dims.1
            && (lz as usize) < self.dims.2
        {
            Some((lx as usize, ly as usize, lz as usize))
        } else {
            None
        }
    }

    /// Clamp a (possibly out-of-region) world cell to the nearest in-region
    /// local cell — so a distant target pulls the creature toward the region
    /// edge facing it.
    fn clamp_local(&self, w: (i32, i32, i32)) -> (usize, usize, usize) {
        let cl = |v: i32, min: i32, dim: usize| (v - min).clamp(0, dim as i32 - 1) as usize;
        (
            cl(w.0, self.min.0, self.dims.0),
            cl(w.1, self.min.1, self.dims.1),
            cl(w.2, self.min.2, self.dims.2),
        )
    }
}

/// The 8 compass headings as horizontal unit vectors. **Order must match
/// `crate::sim::mob::DIR`** so a returned index drives the intended movement.
const DIR8: [(f32, f32); 8] = [
    (1.0, 0.0),
    (0.707_106_77, 0.707_106_77),
    (0.0, 1.0),
    (-0.707_106_77, 0.707_106_77),
    (-1.0, 0.0),
    (-0.707_106_77, -0.707_106_77),
    (0.0, -1.0),
    (0.707_106_77, -0.707_106_77),
];

/// Snap a horizontal gradient to the nearest compass heading (argmax dot).
/// `None` when there's no meaningful horizontal component (at-goal, or a purely
/// vertical step).
fn snap_to_dir(dx: f32, dz: f32) -> Option<u8> {
    if dx.abs() < 1e-5 && dz.abs() < 1e-5 {
        return None;
    }
    let mut best = 0u8;
    let mut best_dot = f32::MIN;
    for (i, &(fx, fz)) in DIR8.iter().enumerate() {
        let dot = dx * fx + dz * fz;
        if dot > best_dot {
            best_dot = dot;
            best = i as u8;
        }
    }
    Some(best)
}

/// A reactive flee heading: the compass direction pointing directly away from
/// `threat`. Unlike [`heading_toward`] this does not route around terrain —
/// bolting is immediate and reflexive; terrain-aware flight can refine it later.
/// `None` if the threat is exactly on `from` (no direction to run).
pub fn flee_heading(from: (i32, i32, i32), threat: (i32, i32, i32)) -> Option<u8> {
    snap_to_dir((from.0 - threat.0) as f32, (from.2 - threat.2) as f32)
}

/// Vertical intent for a 3D mover (a flier or a swimmer).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Climb {
    Up,
    Level,
    Down,
}

/// An omnidirectional move: a horizontal compass heading (`None` when the route
/// is purely vertical or at-goal) plus a vertical intent. This is what lets a
/// dragon, bird, or fish move through a volume — climbing over and diving under
/// obstacles, rising or sinking toward a target at another height — rather than
/// being pinned to the ground plane like a walker.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Move3 {
    pub heading: Option<u8>,
    pub climb: Climb,
}

fn climb_of(dy: f32) -> Climb {
    if dy > 1e-4 {
        Climb::Up
    } else if dy < -1e-4 {
        Climb::Down
    } else {
        Climb::Level
    }
}

/// Shared core: the 3D downhill flow vector at `from` toward `target` within
/// `region`, or `None` if `from` is outside the region or the goal is
/// unreachable. Both the horizontal-only and the omnidirectional headings read
/// from this same field.
fn flow_at<W: WorldQuery>(
    from: (i32, i32, i32),
    target: (i32, i32, i32),
    region: &NavRegion,
    loco: Locomotion,
    world: &W,
) -> Option<[f32; 3]> {
    let fl = region.to_local(from)?;
    let goal = region.clamp_local(target);
    let min = region.min;

    let passable = |x: usize, y: usize, z: usize| {
        let is_solid = |xx: usize, yy: usize, zz: usize| {
            world
                .physics_at(min.0 + xx as i32, min.1 + yy as i32, min.2 + zz as i32)
                .is_solid()
        };
        match loco {
            // A 3D mover (flier or swimmer) routes through any open cell.
            Locomotion::Fly | Locomotion::Swim => !is_solid(x, y, z),
            // walkable: open cell with solid ground directly below
            Locomotion::Walk => !is_solid(x, y, z) && y > 0 && is_solid(x, y - 1, z),
        }
    };

    let field = NavField::compute(region.dims, goal, passable);
    if !field.reachable(fl.0, fl.1, fl.2) {
        return None;
    }
    Some(field.flow_dir(fl.0, fl.1, fl.2))
}

/// Compute a movement heading routing the creature at world cell `from` toward
/// `target`, around solid terrain, within `region`. `None` if `from` is outside
/// the region, the target is unreachable, or the creature is effectively at the
/// goal column.
///
/// Recomputes the field per call (simple + correct); caching per goal and
/// sharing one field across a flock are later optimizations.
pub fn heading_toward<W: WorldQuery>(
    from: (i32, i32, i32),
    target: (i32, i32, i32),
    region: &NavRegion,
    loco: Locomotion,
    world: &W,
) -> Option<u8> {
    let g = flow_at(from, target, region, loco, world)?;
    snap_to_dir(g[0], g[2])
}

/// Omnidirectional version of [`heading_toward`] for a 3D mover: returns both the
/// horizontal compass heading and the vertical [`Climb`], read from the same
/// obstacle-aware flow field. A flier climbs to clear a wall and levels off; a
/// fish dives toward food below. `None` if `from` is outside the region or the
/// target is unreachable.
pub fn heading_toward_3d<W: WorldQuery>(
    from: (i32, i32, i32),
    target: (i32, i32, i32),
    region: &NavRegion,
    loco: Locomotion,
    world: &W,
) -> Option<Move3> {
    let g = flow_at(from, target, region, loco, world)?;
    Some(Move3 { heading: snap_to_dir(g[0], g[2]), climb: climb_of(g[1]) })
}

/// Reflexive omnidirectional flee: away from `threat` in 3D — a horizontal
/// compass bearing plus a vertical escape (a flier climbs away from a ground
/// threat; a fish dives from one above). Like [`flee_heading`] this is reactive
/// and does not route around terrain.
pub fn flee_3d(from: (i32, i32, i32), threat: (i32, i32, i32)) -> Move3 {
    Move3 {
        heading: snap_to_dir((from.0 - threat.0) as f32, (from.2 - threat.2) as f32),
        climb: climb_of((from.1 - threat.1) as f32),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::physics::{BlockPhysics, SceneWorld};

    fn solid(w: &mut SceneWorld, x: i32, y: i32, z: i32) {
        w.set(x, y, z, BlockPhysics::solid(crate::sim::fixed::Fixed::ZERO));
    }

    #[test]
    fn snap_octants() {
        assert_eq!(snap_to_dir(1.0, 0.0), Some(0));
        assert_eq!(snap_to_dir(0.7, 0.7), Some(1));
        assert_eq!(snap_to_dir(0.0, 1.0), Some(2));
        assert_eq!(snap_to_dir(-1.0, 0.0), Some(4));
        assert_eq!(snap_to_dir(0.0, -1.0), Some(6));
        assert_eq!(snap_to_dir(0.0, 0.0), None);
    }

    #[test]
    fn clear_path_heads_straight_at_goal() {
        // empty world ⇒ all air ⇒ all passable for a flier
        let world = SceneWorld::new();
        let region = NavRegion { min: (0, 0, 0), dims: (9, 3, 3) };
        let h = heading_toward((0, 1, 1), (8, 1, 1), &region, Locomotion::Fly, &world);
        assert_eq!(h, Some(0), "clear east path should head due East");
    }

    #[test]
    fn routes_around_a_full_height_wall() {
        // wall column at x=3 for z=0..=3 (full height), gap at z=4
        let mut world = SceneWorld::new();
        for y in 0..3 {
            for z in 0..4 {
                solid(&mut world, 3, y, z);
            }
        }
        let region = NavRegion { min: (0, 0, 0), dims: (7, 3, 5) };
        // standing just west of the wall, the heading must steer around it,
        // not due-East into it.
        let h = heading_toward((2, 1, 1), (6, 1, 1), &region, Locomotion::Fly, &world);
        assert!(h.is_some(), "a route around the wall exists");
        assert_ne!(h, Some(0), "must not head straight into the wall");
    }

    #[test]
    fn unreachable_target_yields_none() {
        // box the target cell in solid on all sides (and the cell itself)
        let mut world = SceneWorld::new();
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    solid(&mut world, 6 + dx, 1 + dy, 1 + dz);
                }
            }
        }
        let region = NavRegion { min: (0, 0, 0), dims: (9, 3, 3) };
        let h = heading_toward((0, 1, 1), (6, 1, 1), &region, Locomotion::Fly, &world);
        assert_eq!(h, None, "a sealed-off target is unreachable");
    }

    #[test]
    fn flier_rises_and_dives_toward_a_target_at_another_height() {
        let world = SceneWorld::new(); // open air
        let region = NavRegion { min: (0, 0, 0), dims: (5, 9, 5) };
        let up = heading_toward_3d((2, 1, 2), (2, 7, 2), &region, Locomotion::Fly, &world).unwrap();
        assert_eq!(up.climb, Climb::Up, "rise toward a target overhead");
        let down = heading_toward_3d((2, 7, 2), (2, 1, 2), &region, Locomotion::Fly, &world).unwrap();
        assert_eq!(down.climb, Climb::Down, "dive toward a target below");
        let level = heading_toward_3d((0, 4, 2), (4, 4, 2), &region, Locomotion::Fly, &world).unwrap();
        assert_eq!(level.climb, Climb::Level);
        assert_eq!(level.heading, Some(0), "head east toward the goal");
    }

    #[test]
    fn flier_climbs_to_clear_a_wall() {
        let mut world = SceneWorld::new();
        for y in 0..3 {
            for z in 0..5 {
                solid(&mut world, 3, y, z);
            }
        }
        let region = NavRegion { min: (0, 0, 0), dims: (7, 6, 5) };
        let m = heading_toward_3d((1, 1, 2), (5, 1, 2), &region, Locomotion::Fly, &world).unwrap();
        assert_eq!(m.climb, Climb::Up, "climb to clear a wall that blocks the level route");
    }

    #[test]
    fn swim_routes_in_3d_like_fly() {
        let world = SceneWorld::new();
        let region = NavRegion { min: (0, 0, 0), dims: (5, 9, 5) };
        let f = heading_toward_3d((2, 1, 2), (2, 7, 2), &region, Locomotion::Fly, &world);
        let s = heading_toward_3d((2, 1, 2), (2, 7, 2), &region, Locomotion::Swim, &world);
        assert_eq!(f, s, "a swimmer navigates the open volume just like a flier");
    }

    #[test]
    fn flee_3d_escapes_vertically() {
        assert_eq!(flee_3d((0, 0, 0), (0, 5, 0)).climb, Climb::Down);
        assert_eq!(flee_3d((0, 5, 0), (0, 0, 0)).climb, Climb::Up);
        let m = flee_3d((0, 0, 0), (5, 0, 0));
        assert_eq!(m.climb, Climb::Level);
        assert_eq!(m.heading, Some(4), "run west, away from an eastern threat");
    }

    #[test]
    fn posture_drops_to_crawl_under_a_low_ceiling_with_hysteresis() {
        let h = 2.0; // upright (hair-tipped) height
        // A tight ceiling forces a crawl.
        assert_eq!(posture_for_clearance(Posture::Upright, 1.5, h), Posture::Crawl);
        // Once crawling, brushing the ceiling (just over h) does NOT pop back up...
        assert_eq!(posture_for_clearance(Posture::Crawl, 2.1, h), Posture::Crawl);
        // ...only clear headroom (>15% over) lets it stand again.
        assert_eq!(posture_for_clearance(Posture::Crawl, 2.4, h), Posture::Upright);
        // Standing with marginal headroom stays standing (no early crouch).
        assert_eq!(posture_for_clearance(Posture::Upright, 2.05, h), Posture::Upright);
    }
}
