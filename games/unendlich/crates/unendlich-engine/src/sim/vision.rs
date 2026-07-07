//! Vision — deterministic, integer line-of-sight and field-of-view, the
//! sensory layer mob brains use to perceive players (and, later, each other).
//!
//! # Line of sight
//!
//! [`line_of_sight`] walks every voxel a segment passes through using the
//! Amanatides & Woo (1987) grid-DDA, entirely in fixed-point. Because it is
//! integer-only and reads the world solely through the same read-only
//! [`WorldQuery`] the physics kernel uses, it is bit-deterministic and rolls
//! back with the rest of the sim — the voxel world itself is never snapshotted.
//!
//! This is the **CPU reference** for the GPU batched-visibility pass we want at
//! scale (one ray per (observer, target/sample-point) thread, traced against
//! the GPU octree). The hierarchical `crate::world::octree_raycast` is the
//! presentation-side twin; this is the form the deterministic sim trusts.
//!
//! # Field of view
//!
//! [`can_see`] gates LOS behind a range check and a forward FOV cone, the cone
//! tested by comparing squared dot products so it stays trig- and sqrt-free
//! (`dot² ≥ cos²(½fov) · |horizontal|²`). Together they answer "does this
//! observer actually perceive that point?" — the input a goals/priority layer
//! reasons over.

use crate::sim::fixed::{Fixed, Vec3};
use crate::sim::physics::WorldQuery;

/// "Infinite" t for an axis with zero direction component (it never steps).
const T_INF: Fixed = Fixed(i64::MAX);

/// A default sight range (blocks) and a ~90°-total FOV (half-angle 45°, so
/// `cos²(45°) = 0.5`). Tunable per mob later.
pub const SIGHT_RANGE: Fixed = Fixed::from_int(20);
pub const FOV_COS2_90DEG: Fixed = Fixed::from_frac(1, 2);

/// True if nothing solid blocks the straight segment from `from` to `to`.
///
/// Walks each voxel the segment crosses (the two endpoint cells are treated as
/// the observer's and target's own cells and are not tested), returning `false`
/// on the first solid block in between.
pub fn line_of_sight<W: WorldQuery>(from: Vec3, to: Vec3, world: &W) -> bool {
    let (mut cx, mut cy, mut cz) = (from.x.floor_int(), from.y.floor_int(), from.z.floor_int());
    let (ex, ey, ez) = (to.x.floor_int(), to.y.floor_int(), to.z.floor_int());
    if (cx, cy, cz) == (ex, ey, ez) {
        return true; // same cell: nothing between
    }

    let (sx, mut tmx, tdx) = axis_setup(from.x, to.x - from.x, cx);
    let (sy, mut tmy, tdy) = axis_setup(from.y, to.y - from.y, cy);
    let (sz, mut tmz, tdz) = axis_setup(from.z, to.z - from.z, cz);

    // Bounded by Manhattan cell distance + slack: a hard guard against any
    // fixed-point rounding pathology leaving us in the loop.
    let max_steps = (ex - cx).abs() + (ey - cy).abs() + (ez - cz).abs() + 3;
    for _ in 0..max_steps {
        // Advance along the axis whose next cell boundary is nearest.
        if tmx <= tmy && tmx <= tmz {
            cx += sx;
            tmx = tmx + tdx;
        } else if tmy <= tmz {
            cy += sy;
            tmy = tmy + tdy;
        } else {
            cz += sz;
            tmz = tmz + tdz;
        }
        if (cx, cy, cz) == (ex, ey, ez) {
            return true; // reached the target cell unobstructed
        }
        if world.physics_at(cx as i32, cy as i32, cz as i32).is_solid() {
            return false;
        }
    }
    // Should be unreachable for well-formed input; fail safe (no false sight).
    false
}

/// Per-axis DDA setup: `(step, t_to_first_boundary, t_per_cell)`.
#[inline]
fn axis_setup(origin: Fixed, d: Fixed, cell: i64) -> (i64, Fixed, Fixed) {
    if d == Fixed::ZERO {
        return (0, T_INF, T_INF);
    }
    if d > Fixed::ZERO {
        let next = Fixed::from_int(cell + 1);
        (1, (next - origin) / d, Fixed::ONE / d)
    } else {
        let next = Fixed::from_int(cell);
        // numerator ≤ 0 and d < 0 ⇒ tMax ≥ 0; tDelta = |1/d|.
        (-1, (next - origin) / d, (Fixed::ONE / d).abs())
    }
}

/// Whether an observer at `eye` facing the horizontal unit vector
/// `(facing_x, facing_z)` can perceive `target`: within `range`, inside the
/// horizontal FOV cone (`fov_cos2` = cos² of the half-angle), and not occluded.
#[allow(clippy::too_many_arguments)]
pub fn can_see<W: WorldQuery>(
    eye: Vec3,
    facing_x: Fixed,
    facing_z: Fixed,
    target: Vec3,
    range: Fixed,
    fov_cos2: Fixed,
    world: &W,
) -> bool {
    let dx = target.x - eye.x;
    let dy = target.y - eye.y;
    let dz = target.z - eye.z;
    // Per-axis early-out: rejects far targets cheaply and keeps the squared
    // distance from overflowing for distant coordinates.
    if dx.abs() > range || dy.abs() > range || dz.abs() > range {
        return false;
    }
    if dx * dx + dy * dy + dz * dz > range * range {
        return false;
    }
    // Horizontal FOV cone, sqrt-free: dot must be positive (ahead) and
    // dot² ≥ cos²(½fov) · |horizontal|².
    let dot = facing_x * dx + facing_z * dz;
    if dot <= Fixed::ZERO {
        return false;
    }
    let horiz2 = dx * dx + dz * dz;
    if dot * dot < fov_cos2 * horiz2 {
        return false;
    }
    line_of_sight(eye, target, world)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::physics::{BlockPhysics, SceneWorld};

    fn at(x: f64, y: f64, z: f64) -> Vec3 {
        Vec3 { x: Fixed::from_f64(x), y: Fixed::from_f64(y), z: Fixed::from_f64(z) }
    }
    fn wall(cells: &[(i32, i32, i32)]) -> SceneWorld {
        let mut w = SceneWorld::new();
        for &(x, y, z) in cells {
            w.set(x, y, z, BlockPhysics::solid(Fixed::ZERO));
        }
        w
    }

    #[test]
    fn clear_line_in_empty_world() {
        let w = SceneWorld::new();
        assert!(line_of_sight(at(0.5, 1.5, 0.5), at(4.5, 1.5, 0.5), &w));
    }

    #[test]
    fn wall_blocks_horizontal_line() {
        let w = wall(&[(2, 1, 0)]);
        assert!(!line_of_sight(at(0.5, 1.5, 0.5), at(4.5, 1.5, 0.5), &w));
        // a parallel row that doesn't pass through the wall cell is clear
        assert!(line_of_sight(at(0.5, 1.5, 2.5), at(4.5, 1.5, 2.5), &w));
    }

    #[test]
    fn vertical_and_diagonal() {
        let vw = wall(&[(0, 3, 0)]);
        assert!(line_of_sight(at(0.5, 0.5, 0.5), at(0.5, 5.5, 0.5), &SceneWorld::new()));
        assert!(!line_of_sight(at(0.5, 0.5, 0.5), at(0.5, 5.5, 0.5), &vw));
        let dw = wall(&[(2, 2, 0)]);
        assert!(!line_of_sight(at(0.5, 0.5, 0.5), at(4.5, 4.5, 0.5), &dw));
    }

    #[test]
    fn same_cell_is_visible() {
        let w = wall(&[(2, 1, 0)]);
        assert!(line_of_sight(at(0.5, 0.5, 0.5), at(0.7, 0.7, 0.7), &w));
    }

    #[test]
    fn can_see_respects_range_cone_and_walls() {
        let w = SceneWorld::new();
        let eye = at(0.5, 1.5, 0.5);
        let (fx, fz) = (Fixed::ONE, Fixed::ZERO); // facing +x
        // ahead, in range, clear → visible
        assert!(can_see(eye, fx, fz, at(5.5, 1.5, 0.5), SIGHT_RANGE, FOV_COS2_90DEG, &w));
        // 90° to the side → outside the cone
        assert!(!can_see(eye, fx, fz, at(0.5, 1.5, 5.5), SIGHT_RANGE, FOV_COS2_90DEG, &w));
        // behind → rejected
        assert!(!can_see(eye, fx, fz, at(-5.5, 1.5, 0.5), SIGHT_RANGE, FOV_COS2_90DEG, &w));
        // too far → rejected
        assert!(!can_see(eye, fx, fz, at(100.5, 1.5, 0.5), SIGHT_RANGE, FOV_COS2_90DEG, &w));
        // ahead & in range but a wall between → not visible
        let ww = wall(&[(2, 1, 0)]);
        assert!(!can_see(eye, fx, fz, at(5.5, 1.5, 0.5), SIGHT_RANGE, FOV_COS2_90DEG, &ww));
    }
}
