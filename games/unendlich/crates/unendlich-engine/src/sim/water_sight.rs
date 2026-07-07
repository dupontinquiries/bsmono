//! Underwater sight — how water between an eye and its target degrades what a
//! creature can make out, and how sharper, more aquatic perception fights that
//! degradation. This sits directly on top of the deterministic voxel
//! line-of-sight raycaster ([`crate::sim::vision`]): the sight ray now reports
//! the water it crosses, and that drives the perception model.
//!
//! Three effects, all built from the same fixed-point DDA so mob perception
//! stays bit-deterministic and rolls back:
//!
//! 1. **Attenuation.** Light is absorbed and scattered crossing water; clearer
//!    water (low turbidity), a shorter path, and a sharper eye all preserve
//!    clarity. `clarity = 1 / (1 + path·(absorption + turbidity + disturbance) /
//!    acuity)` — a rational Beer–Lambert stand-in (no `exp` in fixed-point),
//!    monotonic and in `(0,1]`.
//! 2. **Refraction / displacement.** A disturbed surface bends the ray, so the
//!    target *appears* shifted; a creature with high acuity (an aquatic eye)
//!    learns to correct most of it, so `apparent_offset` is the *residual*
//!    mislocation after compensation.
//! 3. **Scatter tracing.** Because the image is smeared, one ray isn't the whole
//!    story: [`scatter_detection`] re-casts the LOS through a small fixed spray
//!    of jittered target points (spread ∝ the displacement) and reports the
//!    fraction that still reach the target — the confidence the eye actually
//!    resolves it past an occluding edge. This is where the perception couples
//!    back into the raycasting system: N short LOS rays, not one.
//!
//! "Different levels of perceptual understanding" = the `acuity` input, which a
//! caller derives from `creature::GradedCapabilities::vision` via
//! [`acuity_from_vision`]: a dim-sighted land animal barely reads a target two
//! cells under murky water, while a sharp-eyed aquatic predator still tracks it.

use crate::sim::fixed::{Fixed, Vec3};
use crate::sim::physics::{CollisionClass, WorldQuery};
use crate::sim::vision::line_of_sight;

/// Baseline clarity loss per cell of water even when perfectly clear and still.
const ABSORPTION_PER_CELL: Fixed = Fixed::from_frac(1, 5); // 0.2
/// How strongly a disturbed surface displaces the apparent target.
const DISPLACE_SCALE: Fixed = Fixed::from_frac(3, 5); // 0.6
/// Minimum clarity at which a creature registers a target at all.
pub const DETECT_THRESHOLD: Fixed = Fixed::from_frac(12, 100); // 0.12

/// What a sight ray crosses on its way to the target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaterPath {
    /// No solid block interrupted the ray.
    pub clear: bool,
    /// Number of fluid cells the ray passed through (its water path length).
    pub water_cells: u32,
}

/// Walk `from → to` across the voxel grid (the same Amanatides–Woo DDA as
/// `line_of_sight`), reporting whether anything solid blocks it **and** how many
/// fluid cells it crosses. Endpoint cells aren't tested (they're the observer's
/// and target's own cells), matching `line_of_sight`.
pub fn water_path<W: WorldQuery>(from: Vec3, to: Vec3, world: &W) -> WaterPath {
    let (mut cx, mut cy, mut cz) = (from.x.floor_int(), from.y.floor_int(), from.z.floor_int());
    let (ex, ey, ez) = (to.x.floor_int(), to.y.floor_int(), to.z.floor_int());
    if (cx, cy, cz) == (ex, ey, ez) {
        return WaterPath { clear: true, water_cells: 0 };
    }
    let (sx, mut tmx, tdx) = axis_setup(from.x, to.x - from.x, cx);
    let (sy, mut tmy, tdy) = axis_setup(from.y, to.y - from.y, cy);
    let (sz, mut tmz, tdz) = axis_setup(from.z, to.z - from.z, cz);

    let mut water = 0u32;
    let max_steps = (ex - cx).abs() + (ey - cy).abs() + (ez - cz).abs() + 3;
    for _ in 0..max_steps {
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
            return WaterPath { clear: true, water_cells: water };
        }
        let p = world.physics_at(cx as i32, cy as i32, cz as i32);
        if p.is_solid() {
            return WaterPath { clear: false, water_cells: water };
        }
        if matches!(p.class, CollisionClass::Fluid) {
            water += 1;
        }
    }
    WaterPath { clear: false, water_cells: water }
}

const T_INF: Fixed = Fixed(i64::MAX);

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
        (-1, (next - origin) / d, (Fixed::ONE / d).abs())
    }
}

/// Map a creature's `vision` capability (`GradedCapabilities::vision`, ~0..1+)
/// to a perception acuity, clamped to a safe positive range so it's always a
/// valid divisor. 1.0 ≈ a typical sharp eye; below ~0.5 is dim-sighted.
pub fn acuity_from_vision(vision: f32) -> Fixed {
    Fixed::from_f64(vision as f64).clamp(Fixed::from_frac(1, 4), Fixed::from_int(2))
}

/// Visual clarity in `(0,1]` of a target seen through `water_cells` of water,
/// given `turbidity` and surface `disturbance` (both `[0,1]`) and the observer's
/// `acuity` (>0). Zero water → full clarity 1.
pub fn underwater_clarity(water_cells: u32, turbidity: Fixed, disturbance: Fixed, acuity: Fixed) -> Fixed {
    if water_cells == 0 {
        return Fixed::ONE;
    }
    let path = Fixed::from_int(water_cells as i64);
    let per_cell = ABSORPTION_PER_CELL + turbidity.max(Fixed::ZERO) + disturbance.max(Fixed::ZERO);
    let acu = acuity.max(Fixed::from_frac(1, 100));
    let k = (path * per_cell) / acu;
    (Fixed::ONE / (Fixed::ONE + k)).clamp(Fixed::ZERO, Fixed::ONE)
}

/// Residual mislocation (blocks) of the target's apparent position after the
/// creature's acuity compensates for refraction. Grows with water path and
/// surface disturbance, shrinks with acuity; zero out of water.
pub fn apparent_offset(water_cells: u32, disturbance: Fixed, acuity: Fixed) -> Fixed {
    if water_cells == 0 {
        return Fixed::ZERO;
    }
    let path = Fixed::from_int(water_cells as i64);
    let acu = acuity.max(Fixed::from_frac(1, 100));
    (path * disturbance.max(Fixed::ZERO) * DISPLACE_SCALE) / acu
}

/// Scatter-traced detection confidence in `[0,1]`: re-cast the line of sight to
/// the target plus a fixed spray of horizontally jittered points (offset by
/// `spread` blocks), and report the fraction that arrive unobstructed. With no
/// occluders every ray lands (→ 1.0); near an edge or through a gap the spray
/// splits, so a smeared underwater target is only partially resolved. `spread`
/// is normally [`apparent_offset`]. Deterministic (fixed 5-point pattern).
pub fn scatter_detection<W: WorldQuery>(eye: Vec3, target: Vec3, spread: Fixed, world: &W) -> Fixed {
    let s = spread.max(Fixed::ZERO);
    let neg = Fixed::ZERO - s;
    let offsets = [
        (Fixed::ZERO, Fixed::ZERO),
        (s, Fixed::ZERO),
        (neg, Fixed::ZERO),
        (Fixed::ZERO, s),
        (Fixed::ZERO, neg),
    ];
    let mut hits = 0i64;
    for (ox, oz) in offsets {
        let t = Vec3 { x: target.x + ox, y: target.y, z: target.z + oz };
        if line_of_sight(eye, t, world) {
            hits += 1;
        }
    }
    Fixed::from_frac(hits, offsets.len() as i64)
}

/// What a creature makes of a target through water.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Perception {
    /// The eye resolves the target (clarity ≥ threshold and not occluded).
    pub perceived: bool,
    /// Confidence `[0,1]` — clarity scaled by scatter detection.
    pub clarity: Fixed,
    /// Fluid cells between eye and target.
    pub water_cells: u32,
    /// Residual mislocation of the apparent position (blocks).
    pub apparent_offset: Fixed,
}

/// Full water-aware perception of `target` from `eye`. Combines occlusion + the
/// water path + clarity attenuation + scatter detection, gated by the creature's
/// acuity-derived detection threshold. (Range/FOV gating is the caller's
/// `vision::can_see`; this answers *how well* a target already in view reads
/// through water.)
#[allow(clippy::too_many_arguments)]
pub fn perceive_through_water<W: WorldQuery>(
    eye: Vec3,
    target: Vec3,
    acuity: Fixed,
    turbidity: Fixed,
    disturbance: Fixed,
    world: &W,
) -> Perception {
    let path = water_path(eye, target, world);
    if !path.clear {
        return Perception { perceived: false, clarity: Fixed::ZERO, water_cells: path.water_cells, apparent_offset: Fixed::ZERO };
    }
    let clarity = underwater_clarity(path.water_cells, turbidity, disturbance, acuity);
    let offset = apparent_offset(path.water_cells, disturbance, acuity);
    let detect = scatter_detection(eye, target, offset, world);
    let confidence = (clarity * detect).clamp(Fixed::ZERO, Fixed::ONE);
    Perception {
        perceived: confidence >= DETECT_THRESHOLD,
        clarity: confidence,
        water_cells: path.water_cells,
        apparent_offset: offset,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::physics::{BlockPhysics, SceneWorld};

    fn at(x: f64, y: f64, z: f64) -> Vec3 {
        Vec3 { x: Fixed::from_f64(x), y: Fixed::from_f64(y), z: Fixed::from_f64(z) }
    }
    fn water_world(cells: &[(i32, i32, i32)]) -> SceneWorld {
        let mut w = SceneWorld::new();
        for &(x, y, z) in cells {
            w.set(x, y, z, BlockPhysics::fluid(Fixed::from_frac(3, 10)));
        }
        w
    }

    #[test]
    fn water_path_counts_fluid_and_detects_walls() {
        // Fluid in cells x=1,2,3 along the line; eye at 0.5, target at 4.5.
        let w = water_world(&[(1, 0, 0), (2, 0, 0), (3, 0, 0)]);
        let p = water_path(at(0.5, 0.5, 0.5), at(4.5, 0.5, 0.5), &w);
        assert!(p.clear, "no solid between");
        assert_eq!(p.water_cells, 3, "crossed three water cells");
        // Drop a wall in the middle.
        let mut ww = w;
        ww.set(2, 0, 0, BlockPhysics::solid(Fixed::ZERO));
        let p2 = water_path(at(0.5, 0.5, 0.5), at(4.5, 0.5, 0.5), &ww);
        assert!(!p2.clear, "wall blocks the ray");
    }

    #[test]
    fn clarity_falls_with_water_turbidity_and_disturbance_rises_with_acuity() {
        let acu = Fixed::ONE;
        let clear_air = underwater_clarity(0, Fixed::ZERO, Fixed::ZERO, acu);
        assert_eq!(clear_air, Fixed::ONE, "no water → perfect clarity");

        let shallow = underwater_clarity(2, Fixed::from_frac(1, 10), Fixed::ZERO, acu);
        let deep = underwater_clarity(8, Fixed::from_frac(1, 10), Fixed::ZERO, acu);
        assert!(deep < shallow, "more water → less clarity");

        let clean = underwater_clarity(4, Fixed::from_frac(1, 10), Fixed::ZERO, acu);
        let murky = underwater_clarity(4, Fixed::from_frac(8, 10), Fixed::ZERO, acu);
        assert!(murky < clean, "more turbidity → less clarity");

        let calm = underwater_clarity(4, Fixed::from_frac(2, 10), Fixed::ZERO, acu);
        let choppy = underwater_clarity(4, Fixed::from_frac(2, 10), Fixed::from_frac(7, 10), acu);
        assert!(choppy < calm, "a disturbed surface scatters more");

        let dim = underwater_clarity(4, Fixed::from_frac(3, 10), Fixed::from_frac(2, 10), Fixed::from_frac(1, 2));
        let sharp = underwater_clarity(4, Fixed::from_frac(3, 10), Fixed::from_frac(2, 10), Fixed::from_int(2));
        assert!(sharp > dim, "a sharper eye sees more clearly through the same water");
    }

    #[test]
    fn apparent_offset_grows_with_disturbance_and_shrinks_with_acuity() {
        assert_eq!(apparent_offset(0, Fixed::from_frac(5, 10), Fixed::ONE), Fixed::ZERO, "no water → no displacement");
        let calm = apparent_offset(5, Fixed::from_frac(1, 10), Fixed::ONE);
        let rough = apparent_offset(5, Fixed::from_frac(8, 10), Fixed::ONE);
        assert!(rough > calm, "rougher surface → more apparent displacement");
        let dim = apparent_offset(5, Fixed::from_frac(5, 10), Fixed::from_frac(1, 2));
        let sharp = apparent_offset(5, Fixed::from_frac(5, 10), Fixed::from_int(2));
        assert!(sharp < dim, "acuity compensates → less residual mislocation");
    }

    #[test]
    fn scatter_detection_open_vs_walled() {
        let w = SceneWorld::new();
        // Open: every jittered ray reaches → full confidence.
        let open = scatter_detection(at(0.5, 0.5, 0.5), at(6.5, 0.5, 0.5), Fixed::ONE, &w);
        assert_eq!(open, Fixed::ONE, "clear water resolves fully");
        // A solid wall across the path blocks all five rays → zero.
        let mut wall = SceneWorld::new();
        for z in -2..=2 {
            for y in -2..=2 {
                wall.set(3, y, z, BlockPhysics::solid(Fixed::ZERO));
            }
        }
        let blocked = scatter_detection(at(0.5, 0.5, 0.5), at(6.5, 0.5, 0.5), Fixed::ONE, &wall);
        assert_eq!(blocked, Fixed::ZERO, "a wall stops the whole spray");
    }

    #[test]
    fn dim_and_sharp_eyes_perceive_the_same_murky_target_differently() {
        // A murky, choppy water path between eye and prey.
        let cells: Vec<_> = (1..=6).map(|x| (x, 0, 0)).collect();
        let w = water_world(&cells);
        let eye = at(0.5, 0.5, 0.5);
        let prey = at(7.5, 0.5, 0.5);
        let turbidity = Fixed::from_frac(5, 10);
        let disturb = Fixed::from_frac(3, 10);

        let dim = perceive_through_water(eye, prey, acuity_from_vision(0.3), turbidity, disturb, &w);
        let sharp = perceive_through_water(eye, prey, acuity_from_vision(1.5), turbidity, disturb, &w);

        assert_eq!(dim.water_cells, 6);
        assert!(sharp.clarity > dim.clarity, "the aquatic eye reads it more clearly");
        assert!(!dim.perceived, "the dim-sighted animal loses it in the murk");
        assert!(sharp.perceived, "the sharp-eyed predator still tracks it");
    }
}
