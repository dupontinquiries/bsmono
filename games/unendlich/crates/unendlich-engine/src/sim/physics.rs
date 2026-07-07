//! Character physics against the voxel world — brick 3.
//!
//! The simulation must collide entities against terrain without the terrain
//! ever entering the rollback snapshot. The bridge is [`WorldQuery`]: a
//! read-only, deterministic lookup of a block's *physical* properties at an
//! integer voxel coordinate. The sim reads it; it never owns or mutates it.
//! Given identical query results (terrain is synced deterministically,
//! separately) the step stays pure and bit-reproducible.
//!
//! Materials are graded, not binary (see `docs/SIM_ROLLBACK.md` and the
//! foliage-vs-player discussion): each block reports a [`CollisionClass`]
//! (does it block?) plus a fixed-point `friction` (how much it slows you).
//! That single table covers stone (solid, some friction), foliage (passable
//! but draggy), ice (solid, ~zero friction), and fluid (no block, heavy
//! drag) without special cases in the solver.
//!
//! Collision is a deterministic swept AABB resolved one axis at a time in a
//! fixed order (X, then Z, then Y). All math is [`Fixed`]; no floats, so it
//! reproduces exactly across machines.
//!
//! GPU note: [`step_character`] is written as a self-contained per-entity
//! kernel — the unit that ports to a compute shader unchanged once entity
//! counts justify it. The outer loop (in `state::step`) is the only thing
//! that would become a dispatch.

use std::collections::HashMap;

use super::fixed::Fixed;
use super::state::{
    Character, Input, BTN_JUMP, GRAVITY_PER_TICK, JUMP_VEL, MAX_SPEED, MOVE_ACCEL,
};

/// Character AABB half-width on X and Z (so the body is `2 * this` wide).
pub const PLAYER_HALF_WIDTH: Fixed = Fixed::from_frac(3, 10); // 0.3 → 0.6 wide
/// Character AABB height. `pos` is the feet-center, so the box spans
/// `[pos.y, pos.y + PLAYER_HEIGHT]`.
pub const PLAYER_HEIGHT: Fixed = Fixed::from_frac(18, 10); // 1.8 tall
/// Vertical offset to the body centre, used when sampling the material the
/// character is standing *in* (for foliage/fluid drag).
const BODY_OFFSET: Fixed = Fixed::from_frac(9, 10); // half of PLAYER_HEIGHT
/// One raw fixed-point unit, used as an exclusive-bound epsilon.
const EPS: Fixed = Fixed(1);

/// How a block responds to an entity trying to occupy it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum CollisionClass {
    /// Entities pass through (air, foliage). May still apply drag.
    Passable,
    /// Fully blocking (stone, dirt).
    Solid,
    /// No blocking, but heavy drag and damped gravity (water, lava).
    Fluid,
}

/// The physical properties of one block material. Static world *config* —
/// constant per material, never part of the rollback snapshot.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct BlockPhysics {
    pub class: CollisionClass,
    /// Per-tick velocity damping while in contact, in `[0, ONE]`. `0` is
    /// frictionless (ice); `ONE` stops almost immediately (deep mud).
    pub friction: Fixed,
}

impl BlockPhysics {
    /// Empty space: passable, no drag.
    pub const AIR: BlockPhysics =
        BlockPhysics { class: CollisionClass::Passable, friction: Fixed::ZERO };

    pub const fn solid(friction: Fixed) -> BlockPhysics {
        BlockPhysics { class: CollisionClass::Solid, friction }
    }
    /// Passable but draggy — the foliage case (player walks through, slowed).
    pub const fn foliage(friction: Fixed) -> BlockPhysics {
        BlockPhysics { class: CollisionClass::Passable, friction }
    }
    pub const fn fluid(friction: Fixed) -> BlockPhysics {
        BlockPhysics { class: CollisionClass::Fluid, friction }
    }

    #[inline]
    pub fn is_solid(self) -> bool {
        matches!(self.class, CollisionClass::Solid)
    }
}

/// Read-only, deterministic view of the voxel world's *physics*.
///
/// Implemented on the presentation/world side over the real chunk store
/// (mapping each block id to its [`BlockPhysics`]); the sim only ever reads.
pub trait WorldQuery {
    /// Physics of the block occupying integer voxel `(x, y, z)`. Block
    /// `(x,y,z)` occupies the world-space cube `[x, x+1) × [y, y+1) × [z, z+1)`.
    fn physics_at(&self, x: i32, y: i32, z: i32) -> BlockPhysics;
}

/// Map a block id to the [`BlockPhysics`] a [`WorldQuery`] over the real chunk
/// store should report. Solidity (what perception/line-of-sight and navigation
/// actually read) follows [`BlockId::is_solid`]; fluids report as fluid; friction
/// is a sensible per-class default. This is the seam that lets the live voxel
/// world drive sim cognition (terrain occlusion, obstacle-aware paths).
pub fn physics_of_block(b: crate::world::blocks::BlockId) -> BlockPhysics {
    if b.is_fluid() {
        BlockPhysics::fluid(Fixed::from_frac(9, 10))
    } else if b.is_solid() {
        BlockPhysics::solid(Fixed::from_frac(6, 10))
    } else {
        BlockPhysics::AIR
    }
}

/// Move `cur` toward `target` by at most `max_step`, deterministically.
#[inline]
fn approach(cur: Fixed, target: Fixed, max_step: Fixed) -> Fixed {
    let delta = target - cur;
    if delta.abs() <= max_step {
        target
    } else {
        cur + delta.signum() * max_step
    }
}

/// Inclusive integer block indices a span `[lo, hi]` overlaps, treating each
/// block `b` as the half-open cell `[b, b+1)`. The upper bound is made
/// exclusive by subtracting one raw unit, so a span ending exactly on an
/// integer boundary does not include the next cell.
#[inline]
fn block_span(lo: Fixed, hi: Fixed) -> (i32, i32) {
    let b0 = lo.floor_int() as i32;
    let b1 = (hi - EPS).floor_int() as i32;
    (b0, b1)
}

#[derive(Clone, Copy)]
enum Axis {
    X,
    Y,
    Z,
}

/// Is any block solid in the slab at moving-axis index `a`, spanning the two
/// perpendicular block ranges `p1` and `p2`?
fn any_solid_in_slab<W: WorldQuery>(
    world: &W,
    axis: Axis,
    a: i32,
    p1: (i32, i32),
    p2: (i32, i32),
) -> bool {
    let mut i = p1.0;
    while i <= p1.1 {
        let mut j = p2.0;
        while j <= p2.1 {
            let (x, y, z) = match axis {
                Axis::X => (a, i, j), // p1=y, p2=z
                Axis::Y => (i, a, j), // p1=x, p2=z
                Axis::Z => (i, j, a), // p1=x, p2=y
            };
            if world.physics_at(x, y, z).is_solid() {
                return true;
            }
            j += 1;
        }
        i += 1;
    }
    false
}

/// Resolve motion along one axis against solid voxels.
///
/// Returns `(new_pos, new_vel, dir)` where `dir` is `-1` if it stopped while
/// moving in the negative direction, `+1` for positive, `0` for no contact.
/// `ext_neg`/`ext_pos` are the AABB extents below/above `pos` on this axis.
/// `perp1`/`perp2` are the block ranges on the two perpendicular axes,
/// computed at the already-resolved positions of the other axes.
fn resolve_axis<W: WorldQuery>(
    world: &W,
    pos: Fixed,
    vel: Fixed,
    ext_neg: Fixed,
    ext_pos: Fixed,
    axis: Axis,
    perp1: (i32, i32),
    perp2: (i32, i32),
) -> (Fixed, Fixed, i32) {
    if vel > Fixed::ZERO {
        // Leading face = pos + ext_pos, sweeping from old to new.
        let old_face = pos + ext_pos;
        let new_face = pos + vel + ext_pos;
        let (b0, b1) = block_span(old_face, new_face);
        let mut a = b0;
        while a <= b1 {
            if any_solid_in_slab(world, axis, a, perp1, perp2) {
                // Stop with the leading face flush against block `a`'s min.
                // Clamp so we never teleport backward through a penetration.
                let rest = (Fixed::from_int(a as i64) - ext_pos).max(pos);
                return (rest, Fixed::ZERO, 1);
            }
            a += 1;
        }
        (pos + vel, vel, 0)
    } else if vel < Fixed::ZERO {
        let old_face = pos - ext_neg;
        let new_face = pos + vel - ext_neg;
        let (b0, b1) = block_span(new_face, old_face);
        let mut a = b1; // nearest (highest) first
        while a >= b0 {
            if any_solid_in_slab(world, axis, a, perp1, perp2) {
                // Rest with the trailing face flush against block `a`'s top.
                let rest = (Fixed::from_int(a as i64 + 1) + ext_neg).min(pos);
                return (rest, Fixed::ZERO, -1);
            }
            a -= 1;
        }
        (pos + vel, vel, 0)
    } else {
        (pos, vel, 0)
    }
}

/// Integrate velocity into position, resolving voxel collisions axis-by-axis
/// (X, then Z, then Y) so that horizontal walls are cleared before the
/// vertical ground/ceiling check. Sets `on_ground` on a downward contact.
/// Fraction of upward velocity kept when the jump button is released mid-rise
/// (variable jump height). 0.5 → a tap reaches roughly half the apex of a hold.
const JUMP_CUT: Fixed = Fixed::from_frac(1, 2);

/// Maximum ledge height a grounded body will auto-climb in one tick. At 1.1 it
/// clears a single full block (so you walk smoothly up terrain and stairs) but
/// never a two-block wall.
const STEP_HEIGHT: Fixed = Fixed::from_frac(11, 10);

/// Try to climb a low ledge that just blocked horizontal motion. Returns the
/// settled `(x, y, z)` on success — only when the body was grounded, the
/// obstacle is no taller than [`STEP_HEIGHT`], there is headroom for the lift,
/// and the step actually lands the feet *higher* than they started. Pure.
fn try_step_up<W: WorldQuery>(
    world: &W,
    start: &Character,
    hw: Fixed,
    h: Fixed,
) -> Option<(Fixed, Fixed, Fixed)> {
    let lift = STEP_HEIGHT;
    let raised_y = start.pos.y + lift;

    // Headroom: the lift must not drive the head into a ceiling at the start
    // column.
    {
        let (b0, b1) = block_span(start.pos.y + h, raised_y + h);
        let px = block_span(start.pos.x - hw, start.pos.x + hw);
        let pz = block_span(start.pos.z - hw, start.pos.z + hw);
        let mut a = b0;
        while a <= b1 {
            if any_solid_in_slab(world, Axis::Y, a, px, pz) {
                return None;
            }
            a += 1;
        }
    }

    // Re-attempt the horizontal move at the raised level.
    let py = block_span(raised_y, raised_y + h);
    let pz0 = block_span(start.pos.z - hw, start.pos.z + hw);
    let (nx, _, _) = resolve_axis(world, start.pos.x, start.vel.x, hw, hw, Axis::X, py, pz0);
    let px1 = block_span(nx - hw, nx + hw);
    let (nz, _, _) = resolve_axis(world, start.pos.z, start.vel.z, hw, hw, Axis::Z, px1, py);

    // If the obstacle is still in the way at the raised level, it's too tall.
    let moved = (nx - start.pos.x).abs() + (nz - start.pos.z).abs();
    if moved <= EPS {
        return None;
    }

    // Settle: drop from the raised level back down onto the ledge top.
    let pxs = block_span(nx - hw, nx + hw);
    let pzs = block_span(nz - hw, nz + hw);
    let (ny, _, dir) = resolve_axis(world, raised_y, -(lift + EPS), Fixed::ZERO, h, Axis::Y, pxs, pzs);
    if dir >= 0 || ny < start.pos.y {
        return None; // nothing to land on, or it wasn't actually a step up
    }
    Some((nx, ny, nz))
}

fn move_and_collide<W: WorldQuery>(c: &mut Character, world: &W) {
    let hw = PLAYER_HALF_WIDTH;
    let h = PLAYER_HEIGHT;
    let grounded_before = c.on_ground;
    let start = *c;
    c.on_ground = false;

    // X
    let mut blocked = false;
    {
        let py = block_span(c.pos.y, c.pos.y + h);
        let pz = block_span(c.pos.z - hw, c.pos.z + hw);
        let (np, nv, dir) = resolve_axis(world, c.pos.x, c.vel.x, hw, hw, Axis::X, py, pz);
        c.pos.x = np;
        c.vel.x = nv;
        if dir != 0 {
            blocked = true;
        }
    }
    // Z
    {
        let px = block_span(c.pos.x - hw, c.pos.x + hw);
        let py = block_span(c.pos.y, c.pos.y + h);
        let (np, nv, dir) = resolve_axis(world, c.pos.z, c.vel.z, hw, hw, Axis::Z, px, py);
        c.pos.z = np;
        c.vel.z = nv;
        if dir != 0 {
            blocked = true;
        }
    }

    // Step-up: a grounded body that just hit a low ledge climbs it instead of
    // stopping dead. Keeps the horizontal velocity so motion stays smooth.
    if blocked && grounded_before {
        if let Some((sx, sy, sz)) = try_step_up(world, &start, hw, h) {
            c.pos.x = sx;
            c.pos.y = sy;
            c.pos.z = sz;
            c.vel.x = start.vel.x;
            c.vel.z = start.vel.z;
            c.vel.y = Fixed::ZERO;
            c.on_ground = true;
            return;
        }
    }

    // Y
    {
        let px = block_span(c.pos.x - hw, c.pos.x + hw);
        let pz = block_span(c.pos.z - hw, c.pos.z + hw);
        let (np, nv, dir) =
            resolve_axis(world, c.pos.y, c.vel.y, Fixed::ZERO, h, Axis::Y, px, pz);
        c.pos.y = np;
        c.vel.y = nv;
        if dir < 0 {
            c.on_ground = true;
        }
    }
}

/// Material directly beneath the feet (for ground friction).
fn floor_material<W: WorldQuery>(world: &W, c: &Character) -> BlockPhysics {
    world.physics_at(
        c.pos.x.floor_int() as i32,
        (c.pos.y - EPS).floor_int() as i32,
        c.pos.z.floor_int() as i32,
    )
}

/// Material occupying the body centre (for foliage/fluid drag).
fn body_material<W: WorldQuery>(world: &W, c: &Character) -> BlockPhysics {
    world.physics_at(
        c.pos.x.floor_int() as i32,
        (c.pos.y + BODY_OFFSET).floor_int() as i32,
        c.pos.z.floor_int() as i32,
    )
}

/// Player body density (same arbitrary units as `Medium::density`). Slightly
/// below water (1000) so a submerged player drifts up and settles at the
/// surface rather than sinking like a stone.
pub const PLAYER_DENSITY: i64 = 950;

/// Deterministic submersion estimate in `[0,1]`: how many of the body's
/// feet / waist / head sample points sit in fluid. Needs only the `WorldQuery`
/// (no fluid field), so it's available right here in the physics kernel.
fn submersion<W: WorldQuery>(world: &W, c: &Character) -> Fixed {
    let x = c.pos.x.floor_int() as i32;
    let z = c.pos.z.floor_int() as i32;
    let samples = [
        c.pos.y,                          // feet
        c.pos.y + BODY_OFFSET,            // waist
        c.pos.y + PLAYER_HEIGHT - EPS,    // head
    ];
    let mut wet = 0i64;
    for sy in samples {
        if matches!(
            world.physics_at(x, sy.floor_int() as i32, z).class,
            CollisionClass::Fluid
        ) {
            wet += 1;
        }
    }
    Fixed::from_frac(wet, samples.len() as i64)
}

/// Advance one character by one tick: the per-entity physics kernel.
///
/// Pure given `world`. Order: ground friction → horizontal control →
/// gravity → jump → body drag → integrate-and-collide.
pub fn step_character<W: WorldQuery>(ch: &Character, input: &Input, world: &W) -> Character {
    let mut c = *ch;

    // Ground friction: damp horizontal velocity by the floor material.
    if c.on_ground {
        let f = floor_material(world, &c).friction;
        if f > Fixed::ZERO {
            c.vel.x -= c.vel.x * f;
            c.vel.z -= c.vel.z * f;
        }
    }

    // Horizontal control toward the input's target velocity.
    c.vel.x = approach(c.vel.x, input.move_x * MAX_SPEED, MOVE_ACCEL);
    c.vel.z = approach(c.vel.z, input.move_z * MAX_SPEED, MOVE_ACCEL);

    // Gravity.
    c.vel.y += GRAVITY_PER_TICK;

    // Jump from the ground.
    if c.on_ground && input.is_held(BTN_JUMP) {
        c.vel.y = JUMP_VEL;
        c.on_ground = false;
    } else if !c.on_ground && c.vel.y > Fixed::ZERO && !input.is_held(BTN_JUMP) {
        // Variable jump height: releasing jump while still rising cuts the
        // remaining ascent, so a tap is a short hop and a hold is a full jump.
        c.vel.y = c.vel.y * JUMP_CUT;
    }

    // Body drag: foliage slows horizontally; fluid also damps the vertical.
    let body = body_material(world, &c);
    if body.friction > Fixed::ZERO && !body.is_solid() {
        c.vel.x -= c.vel.x * body.friction;
        c.vel.z -= c.vel.z * body.friction;
        if matches!(body.class, CollisionClass::Fluid) {
            c.vel.y -= c.vel.y * body.friction;
        }
    }

    // Buoyancy: when any of the body is submerged, add the buoyant lift on top
    // of the gravity already applied above, so a submerged player floats toward
    // the surface (PLAYER_DENSITY < water) instead of sinking. Wind/current
    // advection rides the same `medium` system but is supplied elsewhere; still
    // water needs only buoyancy here (horizontal drag is the foliage/fluid damp
    // above).
    let submerged = submersion(world, &c);
    if submerged > Fixed::ZERO {
        c.vel.y += crate::sim::medium::buoyancy_dv(
            submerged,
            crate::sim::medium::Medium::WATER.density,
            PLAYER_DENSITY,
            GRAVITY_PER_TICK,
        );
    }

    move_and_collide(&mut c, world);
    c.yaw = input.yaw;
    c
}

// ── Provided world implementations ─────────────────────────────────────

/// A flat world: solid below `y = 0`, air at and above. Reproduces brick-1's
/// placeholder ground plane (rest height `y = 0`) and is handy for tests.
pub struct FlatWorld {
    pub floor_friction: Fixed,
}

impl FlatWorld {
    pub fn new() -> FlatWorld {
        FlatWorld { floor_friction: Fixed::ZERO }
    }
    pub fn with_friction(friction: Fixed) -> FlatWorld {
        FlatWorld { floor_friction: friction }
    }
}

impl Default for FlatWorld {
    fn default() -> FlatWorld {
        FlatWorld::new()
    }
}

impl WorldQuery for FlatWorld {
    fn physics_at(&self, _x: i32, y: i32, _z: i32) -> BlockPhysics {
        if y < 0 {
            BlockPhysics::solid(self.floor_friction)
        } else {
            BlockPhysics::AIR
        }
    }
}

/// A sparse world backed by a map — arbitrary blocks over a default. Useful
/// for tests and small tools; unset coordinates return `default`.
pub struct SceneWorld {
    blocks: HashMap<(i32, i32, i32), BlockPhysics>,
    pub default: BlockPhysics,
}

impl SceneWorld {
    pub fn new() -> SceneWorld {
        SceneWorld { blocks: HashMap::new(), default: BlockPhysics::AIR }
    }
    pub fn set(&mut self, x: i32, y: i32, z: i32, p: BlockPhysics) {
        self.blocks.insert((x, y, z), p);
    }
}

impl Default for SceneWorld {
    fn default() -> SceneWorld {
        SceneWorld::new()
    }
}

impl WorldQuery for SceneWorld {
    fn physics_at(&self, x: i32, y: i32, z: i32) -> BlockPhysics {
        *self.blocks.get(&(x, y, z)).unwrap_or(&self.default)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::fixed::Vec3;
    use crate::sim::state::Character;

    #[test]
    fn block_physics_mapping_solidity() {
        use crate::world::blocks::BlockId;
        // Air is passable (see/move through); stone is a solid occluder/obstacle.
        assert!(!physics_of_block(BlockId::Air).is_solid());
        assert!(physics_of_block(BlockId::Stone).is_solid());
        // Water is a fluid, not a solid — perception sees through it and paths
        // aren't blocked by it (swimming is a separate locomotion concern).
        assert!(!physics_of_block(BlockId::Water).is_solid());
    }

    fn at(x: i64, y: i64, z: i64) -> Character {
        Character {
            pos: Vec3::new(Fixed::from_frac(x * 2 + 1, 2), Fixed::from_int(y), Fixed::from_frac(z * 2 + 1, 2)),
            ..Character::default()
        }
    }

    fn idle() -> Input {
        Input::default()
    }

    #[test]
    fn falls_and_lands_on_the_floor_top() {
        let w = FlatWorld::new();
        let mut c = at(0, 50, 0);
        for _ in 0..600 {
            c = step_character(&c, &idle(), &w);
        }
        assert_eq!(c.pos.y, Fixed::ZERO, "rests on the top of the y=-1 block");
        assert!(c.on_ground);
        assert_eq!(c.vel.y, Fixed::ZERO);
    }

    #[test]
    fn walking_into_a_wall_stops_horizontal_motion() {
        // Solid column of blocks at x = 2, full height around the player.
        let mut w = SceneWorld::new();
        for y in -1..4 {
            w.set(2, y, 0, BlockPhysics::solid(Fixed::ZERO));
        }
        let mut c = at(0, 0, 0); // standing at x = 0.5 on... nothing; place on floor
        c.pos.y = Fixed::ZERO;
        c.on_ground = true;
        // Drive hard into +x for a while.
        let push = Input { move_x: Fixed::from_int(1), ..Input::default() };
        for _ in 0..200 {
            c = step_character(&c, &push, &w);
            c.pos.y = Fixed::ZERO; // keep grounded; we only care about x
            c.on_ground = true;
        }
        // AABB max (pos.x + half width) must not pass the wall face at x = 2.
        assert!(c.pos.x + PLAYER_HALF_WIDTH <= Fixed::from_int(2), "stopped before the wall");
        assert_eq!(c.vel.x, Fixed::ZERO, "horizontal velocity killed by the wall");
    }

    #[test]
    fn steps_up_a_one_block_ledge() {
        // Lower floor (top y=0) wide, and a one-block-higher plateau (top y=1)
        // from x=2 onward — wide enough that the player can't walk off its far
        // edge during the test.
        let mut w = SceneWorld::new();
        for x in -3..40 {
            w.set(x, -1, 0, BlockPhysics::solid(Fixed::ZERO));
        }
        for x in 2..40 {
            w.set(x, 0, 0, BlockPhysics::solid(Fixed::ZERO));
        }
        let mut c = at(0, 0, 0);
        c.pos.y = Fixed::ZERO;
        c.on_ground = true;
        let push = Input { move_x: Fixed::from_int(1), ..Input::default() };
        let mut max_y = c.pos.y;
        for _ in 0..80 {
            c = step_character(&c, &push, &w);
            if c.pos.y > max_y {
                max_y = c.pos.y;
            }
        }
        // Climbed: feet rose to the plateau top (y ~= 1) and walked past the edge.
        assert!(max_y >= Fixed::from_frac(9, 10), "stepped up onto the ledge, max_y = {:?}", max_y);
        assert!(c.pos.x > Fixed::from_int(2), "kept moving past the ledge edge, x = {:?}", c.pos.x);
        assert!(c.on_ground, "resting on the plateau");
    }

    #[test]
    fn does_not_step_up_a_two_block_wall() {
        // Floor (top y=0), then a two-block obstacle at x=2 (occupies y in 0..2).
        let mut w = SceneWorld::new();
        for x in -3..6 {
            w.set(x, -1, 0, BlockPhysics::solid(Fixed::ZERO));
        }
        w.set(2, 0, 0, BlockPhysics::solid(Fixed::ZERO));
        w.set(2, 1, 0, BlockPhysics::solid(Fixed::ZERO));
        let mut c = at(0, 0, 0);
        c.pos.y = Fixed::ZERO;
        c.on_ground = true;
        let push = Input { move_x: Fixed::from_int(1), ..Input::default() };
        for _ in 0..200 {
            c = step_character(&c, &push, &w);
        }
        // Too tall to climb: stayed at floor level and stopped before the wall.
        assert!(c.pos.y < Fixed::from_frac(1, 2), "did not climb the 2-block wall, y = {:?}", c.pos.y);
        assert!(c.pos.x + PLAYER_HALF_WIDTH <= Fixed::from_int(2), "stopped before the wall");
    }

    #[test]
    fn does_not_step_up_under_a_low_ceiling() {
        // A one-block ledge at x=2, but a ceiling one block above the player's
        // head blocks the lift, so the body cannot climb.
        let mut w = SceneWorld::new();
        for x in -3..6 {
            w.set(x, -1, 0, BlockPhysics::solid(Fixed::ZERO));
        }
        for x in 2..6 {
            w.set(x, 0, 0, BlockPhysics::solid(Fixed::ZERO));
        }
        // Ceiling over the approach column at y=2 (bottom at 2.0, just above the
        // 1.8-tall head). Lifting by 1.1 would drive the head to 2.9 into it.
        for x in -3..6 {
            w.set(x, 2, 0, BlockPhysics::solid(Fixed::ZERO));
        }
        let mut c = at(0, 0, 0);
        c.pos.y = Fixed::ZERO;
        c.on_ground = true;
        let push = Input { move_x: Fixed::from_int(1), ..Input::default() };
        for _ in 0..200 {
            c = step_character(&c, &push, &w);
        }
        assert!(c.pos.y < Fixed::from_frac(1, 2), "ceiling prevented the step, y = {:?}", c.pos.y);
    }

    #[test]
    fn releasing_jump_early_gives_a_shorter_hop() {
        let w = {
            let mut w = SceneWorld::new();
            w.set(0, -1, 0, BlockPhysics::solid(Fixed::ZERO)); // floor, top at y=0
            w
        };
        let start = {
            let mut c = at(0, 0, 0);
            c.pos.y = Fixed::ZERO;
            c.on_ground = true;
            c
        };
        let jump = Input { buttons: BTN_JUMP, ..Input::default() };

        // Full hold: keep the button down for the whole ascent.
        let apex = |held: bool| {
            let mut c = start;
            c = step_character(&c, &jump, &w); // takeoff
            let mut max_y = c.pos.y;
            for _ in 0..60 {
                let inp = if held { jump } else { idle() };
                c = step_character(&c, &inp, &w);
                if c.pos.y > max_y {
                    max_y = c.pos.y;
                }
                if c.on_ground {
                    break; // landed; first arc done
                }
            }
            max_y
        };

        let full = apex(true);
        let tapped = apex(false);
        assert!(tapped < full, "a tap ({tapped:?}) should peak lower than a hold ({full:?})");
        assert!(tapped > Fixed::ZERO, "but the tap still leaves the ground");
    }

    #[test]
    fn passes_through_foliage_but_is_slowed() {
        // Foliage everywhere above the floor; floor solid below y=0.
        let mut w = SceneWorld::new();
        for y in -2..0 {
            for x in -3..6 {
                w.set(x, y, 0, BlockPhysics::solid(Fixed::ZERO));
            }
        }
        // body-centre block is foliage with heavy drag
        w.set(0, 0, 0, BlockPhysics::foliage(Fixed::from_frac(1, 2)));
        w.set(0, 1, 0, BlockPhysics::foliage(Fixed::from_frac(1, 2)));
        let mut c = at(0, 0, 0);
        c.pos.y = Fixed::ZERO;
        c.on_ground = true;
        let push = Input { move_x: Fixed::from_int(1), ..Input::default() };
        let drag_step = step_character(&c, &push, &w);
        // With foliage the speed gained in one tick is below the frictionless cap.
        let no_drag_world = FlatWorld::new();
        let mut free = c;
        free.pos.y = Fixed::ZERO;
        free.on_ground = true;
        let free_step = step_character(&free, &push, &no_drag_world);
        assert!(drag_step.vel.x < free_step.vel.x, "foliage drag slows the player");
        // And it never blocked motion (passable): x advanced.
        assert!(drag_step.pos.x > c.pos.x);
    }

    #[test]
    fn jumping_into_a_ceiling_stops_upward_motion() {
        let mut w = SceneWorld::new();
        w.set(0, -1, 0, BlockPhysics::solid(Fixed::ZERO)); // floor
        // ceiling slab at y = 2
        w.set(0, 2, 0, BlockPhysics::solid(Fixed::ZERO));
        let mut c = at(0, 0, 0);
        c.pos.y = Fixed::ZERO;
        c.on_ground = true;
        let jump = Input { buttons: BTN_JUMP, ..Input::default() };
        c = step_character(&c, &jump, &w);
        // Now airborne, rising. Step until we either hit the ceiling or fall.
        let mut max_y = c.pos.y;
        for _ in 0..60 {
            c = step_character(&c, &idle(), &w);
            if c.pos.y > max_y {
                max_y = c.pos.y;
            }
        }
        // Head (pos.y + height) can never exceed the ceiling block's bottom (y=2).
        assert!(max_y + PLAYER_HEIGHT <= Fixed::from_int(2), "head stopped at the ceiling");
    }

    #[test]
    fn walks_off_an_edge_and_falls() {
        // Floor only for x <= 0; nothing at x >= 1.
        let mut w = SceneWorld::new();
        for x in -3..1 {
            w.set(x, -1, 0, BlockPhysics::solid(Fixed::ZERO));
        }
        let mut c = at(0, 0, 0);
        c.pos.y = Fixed::ZERO;
        c.on_ground = true;
        let push = Input { move_x: Fixed::from_int(1), ..Input::default() };
        for _ in 0..120 {
            c = step_character(&c, &push, &w);
        }
        assert!(!c.on_ground, "should have walked off the edge");
        assert!(c.pos.y < Fixed::ZERO, "and fallen below the floor top");
    }

    #[test]
    fn deterministic_across_runs() {
        use crate::sim::state::checksum;
        use crate::sim::state::SimState;
        let w = FlatWorld::new();
        let run = || {
            let mut c = at(0, 5, 0);
            for t in 0..400 {
                let inp = Input {
                    move_x: Fixed::from_int(1),
                    buttons: if t % 50 == 0 { BTN_JUMP } else { 0 },
                    ..Input::default()
                };
                c = step_character(&c, &inp, &w);
            }
            let s = SimState { tick: super::super::state::Tick(0), players: vec![c], ..Default::default() };
            checksum(&s)
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn submerged_player_floats_up() {
        // A column of water around the player; PLAYER_DENSITY < water → rises.
        let mut w = SceneWorld::new();
        for y in 0..=8 {
            w.set(0, y, 0, BlockPhysics::fluid(Fixed::from_frac(3, 10)));
        }
        let c = at(0, 5, 0); // feet at y=5, fully inside the water column
        let after = step_character(&c, &Input::default(), &w);
        assert!(after.vel.y > Fixed::ZERO, "buoyancy lifts the submerged player, vy={:?}", after.vel.y);
    }

    #[test]
    fn player_in_air_still_falls() {
        // No fluid anywhere → no buoyancy → ordinary downward gravity.
        let w = SceneWorld::new(); // all air
        let c = at(0, 5, 0);
        let after = step_character(&c, &Input::default(), &w);
        assert!(after.vel.y < Fixed::ZERO, "in air the player falls, vy={:?}", after.vel.y);
    }

    #[test]
    fn deeper_submersion_gives_more_lift() {
        // Fully-submerged column vs only feet wet → fully submerged rises more.
        let mut full = SceneWorld::new();
        for y in 0..=8 {
            full.set(0, y, 0, BlockPhysics::fluid(Fixed::from_frac(3, 10)));
        }
        let mut feet = SceneWorld::new();
        feet.set(0, 5, 0, BlockPhysics::fluid(Fixed::from_frac(3, 10))); // only feet cell wet

        let c = at(0, 5, 0);
        let vy_full = step_character(&c, &Input::default(), &full).vel.y;
        let vy_feet = step_character(&c, &Input::default(), &feet).vel.y;
        assert!(vy_full > vy_feet, "more submerged → more lift ({vy_full:?} vs {vy_feet:?})");
    }
}
