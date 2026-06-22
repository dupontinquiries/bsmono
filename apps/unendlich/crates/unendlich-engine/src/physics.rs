//! Physics — gravity, jump, AABB body step against the block world.
//!
//! Entity-agnostic: a `PhysicsBody` is just a position + velocity + AABB
//! half-extents. Players, mobs, and dropped items all use the same step
//! function. The world is accessed through the `BlockSampler` trait so we
//! can plug in chunks today and a server-authoritative source later.
//!
//! Tunable constants (the "API glue buzzwords" the design notes call out)
//! live in `physics::constants` so other code can tweak gravity per-region,
//! per-biome, or per-effect (slow fall, low gravity zones, etc.) by passing
//! a `PhysicsParams` override to `step_body`.
//!
//! Collision strategy: separating-axis. Each frame we move the body one
//! axis at a time (Y, then X, then Z). After each axis move we sweep the
//! body's AABB against integer block cells it overlaps; if any are solid,
//! we snap back to the contact plane and zero that axis's velocity. This
//! is the classic Minecraft-style approach: simple, stable, no tunneling
//! at normal speeds, gives the "stand on top" / "wall slide" feel for free.
//!
//! Limitations (all OK for now):
//! * No swept-AABB tunneling protection at very high speeds. Fast-moving
//!   entities can pass through 1-block walls if dt × speed > 1. Cap player
//!   speed under a block per step for now; revisit when bullets/arrows land.
//! * Gravity is constant; no per-region override yet (the param plumbing is
//!   in place — you can pass a `PhysicsParams` to `step_body` — but the
//!   field machinery to vary it spatially is future work).
//! * No fluid drag yet.

use crate::world::blocks::BlockId;

/// Tunable defaults — the "buzzword constants" — with units called out so
/// nobody has to reverse-engineer them later.
pub mod constants {
    /// Acceleration due to gravity, in **blocks/second²**. Minecraft uses
    /// roughly 32 blocks/s² (9.81 m/s² with 1 block ≈ 0.3 m). We start at
    /// 28 — slightly slower-feeling than Minecraft, more "floaty" — and can
    /// retune once we have a sense of what jump heights feel right.
    pub const GRAVITY: f32 = 28.0;

    /// Terminal fall velocity in blocks/second. Caps gravity so falls into
    /// deep pits don't accelerate forever (and so dt-coupled imprecision
    /// stays bounded). Real-world ~55 m/s ≈ 180 blocks/s; we use 78 for
    /// playability.
    pub const TERMINAL_VELOCITY: f32 = 78.0;

    /// Initial upward velocity from a jump, in blocks/second. With GRAVITY=28,
    /// time to peak = JUMP_VELOCITY/GRAVITY ≈ 0.36 s, peak height = v²/(2g)
    /// ≈ 1.79 blocks. Raised from 8.0 so a jump clears more than a single step
    /// (operator: jump felt too low).
    pub const JUMP_VELOCITY: f32 = 10.0;

    /// Walk speed in blocks/second.
    pub const WALK_SPEED: f32 = 4.5;

    /// Sprint multiplier on walk speed.
    pub const SPRINT_MULTIPLIER: f32 = 1.6;

    /// Sneak multiplier on walk speed.
    pub const SNEAK_MULTIPLIER: f32 = 0.3;

    /// Ground friction "rate constant" — how fast horizontal velocity decays
    /// when no input is applied while on the ground, in 1/seconds. Higher =
    /// more grippy, stop faster. The actual decay is multiplied by
    /// (1 - block.slipperiness()) so ice (slipperiness 0.85) gives 15% of
    /// this effective friction.
    ///
    /// Bumped from 12.0 to 20.0 because users reported "always sliding"
    /// on default surfaces. At 20.0/s, velocity drops to 1% in ~0.23s
    /// after releasing input — close to Minecraft's feel without being
    /// instant-stop. MOVE_ACCEL scales with this so target_speed stays
    /// at WALK_SPEED.
    pub const GROUND_FRICTION: f32 = 20.0;

    /// Air control factor in [0, 1] — how much of the input acceleration
    /// reaches the player while airborne. 1.0 = full Minecraft-style aerial
    /// strafing (cheap-feeling but very player-friendly); 0.0 = no aerial
    /// influence at all (realistic but punishing). 0.35 is a middle ground:
    /// you can still adjust mid-jump but feel committed to the trajectory.
    pub const AIR_CONTROL: f32 = 0.35;

    /// Air drag rate constant — how fast horizontal velocity bleeds off
    /// while airborne. Much smaller than ground friction (1/s vs 20/s) so
    /// jumps still feel light. Without any air drag, players preserved
    /// infinite lateral momentum across short hops — manifested as the
    /// "always sliding" feel even on default surfaces.
    pub const AIR_DRAG: f32 = 1.0;

    /// Acceleration applied per second when input is held. Together with
    /// GROUND_FRICTION this defines the steady-state speed: target_speed =
    /// MOVE_ACCEL / GROUND_FRICTION. Tune so that target_speed ≈ WALK_SPEED.
    pub const MOVE_ACCEL: f32 = WALK_SPEED * GROUND_FRICTION;

    /// Player AABB half-extents in blocks: width 0.6, height 1.8 (eye 1.62).
    /// Minecraft-equivalent dimensions — feels familiar, fits through 1-block
    /// gaps comfortably.
    pub const PLAYER_HALF_WIDTH: f32 = 0.30;
    pub const PLAYER_HALF_HEIGHT: f32 = 0.90;
    /// Eye offset above the AABB center (so position.y + EYE_OFFSET = camera
    /// y). Center at AABB center, eye toward top.
    pub const PLAYER_EYE_OFFSET: f32 = 0.72;
}

/// Per-body physics parameters. Defaults to `PhysicsParams::player_default()`.
/// Pass an override to `step_body` for entities that need different physics
/// (heavy mobs, floaty effects, no-clip dev mode).
#[derive(Debug, Clone, Copy)]
pub struct PhysicsParams {
    pub gravity: f32,
    pub terminal_velocity: f32,
    /// If false, gravity is not applied this step (used for fly mode).
    pub apply_gravity: bool,
    /// If false, collision response is skipped (used for noclip).
    pub apply_collision: bool,
    /// Optional ambient wind force (x, z components, world units / s).
    /// Applied as a constant acceleration on horizontal motion. Defaults
    /// to (0, 0). The bin queries `wind::wind_at` per frame and sets this
    /// before calling `step_body`. Wind force is scaled small enough that
    /// it doesn't dominate input — at typical magnitudes (≤1.0) it adds
    /// roughly 0.5 m/s² laterally.
    pub wind_force: [f32; 2],
    /// Auto-step-up height. When horizontal motion is blocked AND the
    /// body is on ground, lift up to this many world units and try the
    /// move again.
    ///
    /// Default 1.1 — enough to clear a full 1-block step (Minecraft-
    /// like auto-jump up small ledges). Set to 0 to disable.
    ///
    /// Note: bumpy-block tops don't need step-up at all because they're
    /// purely VISUAL — the physical AABB still uses integer-Y top
    /// faces. So walking on bumpy terrain doesn't stutter at the
    /// physics level. Step-up is for actual block-height changes.
    pub step_height: f32,
}

impl PhysicsParams {
    pub fn player_default() -> Self {
        Self {
            gravity: constants::GRAVITY,
            terminal_velocity: constants::TERMINAL_VELOCITY,
            apply_gravity: true,
            apply_collision: true,
            wind_force: [0.0, 0.0],
            step_height: 1.1,
        }
    }

    /// Fly mode: no gravity, still collides (so you can't fly into walls).
    pub fn fly_mode() -> Self {
        Self {
            gravity: 0.0,
            terminal_velocity: f32::INFINITY,
            apply_gravity: false,
            apply_collision: true,
            wind_force: [0.0, 0.0],
            // Disable auto-step in fly mode — flying through a wall by
            // accidentally stepping up would feel weird.
            step_height: 0.0,
        }
    }

    /// No-clip / dev camera: no gravity, no collision.
    pub fn noclip() -> Self {
        Self {
            gravity: 0.0,
            terminal_velocity: f32::INFINITY,
            apply_gravity: false,
            apply_collision: false,
            wind_force: [0.0, 0.0],
            step_height: 0.0,
        }
    }
}

/// Trait for "what's the block at this world position?". Implemented by
/// `ChunkManager` (today) and any future world-state source (server snapshot,
/// rollback buffer, predicted state).
///
/// Returns `None` if the block isn't loaded (treated as Air for collision —
/// avoids freezing the player in unloaded chunks).
pub trait BlockSampler {
    fn block_at(&self, x: i32, y: i32, z: i32) -> Option<BlockId>;
}

/// Stable identifier for a physics body across the lifetime of the
/// simulation. Used to refer to a body without holding a reference.
///
/// Multiplayer notes:
///   - The server assigns BodyIds; clients receive them and use the
///     same ID space. Replication packets are keyed by BodyId.
///   - Local-only entities (the local player BEFORE network handshake,
///     particles) can use locally-assigned IDs that the server will
///     later remap.
///   - 64 bits is overkill for a single-process game but cheap and
///     means we never have to worry about ID exhaustion in long
///     persistent worlds.
///
/// `Option<BodyId>` is the right shape today: many test bodies and
/// the legacy single-player flow don't care about identity. Once
/// there's a body registry, every body in it will have an ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BodyId(pub u64);

impl BodyId {
    /// Reserved value for "the local player" in single-player flows
    /// that don't yet have a real ID server. Multiplayer code should
    /// allocate its own IDs and not rely on this constant.
    pub const LOCAL_PLAYER: BodyId = BodyId(1);
}

/// AABB-bounded physics body.
#[derive(Debug, Clone, Copy)]
pub struct PhysicsBody {
    /// Stable identity of this body. `None` for legacy/test code
    /// that doesn't track identity; populated for any body that
    /// participates in the multiplayer body registry.
    pub id: Option<BodyId>,
    /// Position of the AABB center.
    pub position: [f32; 3],
    /// Velocity in blocks/second.
    pub velocity: [f32; 3],
    /// Half-extents (X, Y, Z).
    pub half_extents: [f32; 3],
    /// Whether the body was on the ground at the end of the last step.
    /// Set by `step_body`; jump logic should consult this before adding
    /// vertical velocity.
    pub on_ground: bool,
    /// Visual-only Y offset, used to animate step-up. When the body
    /// auto-steps up onto a ledge, physics moves the body to the new Y
    /// instantly (so further movement is consistent), but we set
    /// `render_y_offset = -step_distance` and decay toward 0 over a
    /// frame or two. The renderer uses `position[1] + render_y_offset`
    /// for camera/visual placement, producing smooth step-up animation
    /// instead of a teleport.
    ///
    /// Always ≤ 0 (we step UP, the visual lags BELOW the physics).
    /// Decayed each `step_body` call by step_lerp_speed × dt.
    pub render_y_offset: f32,
}

impl PhysicsBody {
    pub fn new_player(position: [f32; 3]) -> Self {
        Self {
            id: None,
            position,
            velocity: [0.0; 3],
            half_extents: [
                constants::PLAYER_HALF_WIDTH,
                constants::PLAYER_HALF_HEIGHT,
                constants::PLAYER_HALF_WIDTH,
            ],
            on_ground: false,
            render_y_offset: 0.0,
        }
    }

    /// Construct a player-sized body with a stable BodyId.
    /// Multiplayer-aware code should prefer this over `new_player`
    /// so the body can be replicated and addressed by ID.
    pub fn new_player_with_id(position: [f32; 3], id: BodyId) -> Self {
        let mut body = Self::new_player(position);
        body.id = Some(id);
        body
    }

    /// Visual Y for rendering: physics Y plus the (negative) lag offset
    /// from step-up animation. When no step-up is active, equals
    /// `position[1]`. Use this for camera positioning.
    pub fn render_y(&self) -> f32 {
        self.position[1] + self.render_y_offset
    }

    /// Trigger a jump if on the ground. Returns true if the jump was applied.
    /// Idempotent if called multiple times within the same airborne moment
    /// (which is what we want — holding Space shouldn't double-jump).
    pub fn try_jump(&mut self) -> bool {
        if self.on_ground {
            self.velocity[1] = constants::JUMP_VELOCITY;
            self.on_ground = false;
            true
        } else {
            false
        }
    }

    /// Apply horizontal movement input over `dt`. Replaces the old "set
    /// velocity directly" pattern with an acceleration model that respects
    /// air vs ground state, surface slipperiness, and air control.
    ///
    /// Parameters:
    /// * `desired_dir`: world-space horizontal direction the player wants to
    ///   move (will be normalized internally if non-zero).
    /// * `target_speed`: how fast they want to go (modified by sprint/sneak
    ///   in the caller).
    /// * `surface_slipperiness`: 0..1 — slipperiness of the block under the
    ///   player's feet. Affects ground friction only.
    /// * `dt`: real seconds.
    ///
    /// Behavior:
    /// * On ground: velocity decays per-axis at `GROUND_FRICTION × (1 - slip)`,
    ///   then `MOVE_ACCEL × desired_dir × dt` is added. Steady state: target_speed.
    /// * In air: friction is essentially zero (preserves momentum). Input
    ///   contributes `MOVE_ACCEL × AIR_CONTROL × desired_dir × dt`.
    pub fn apply_input(
        &mut self,
        desired_dir: [f32; 3],
        target_speed: f32,
        surface_slipperiness: f32,
        dt: f32,
    ) {
        let dir_len = (desired_dir[0]*desired_dir[0] + desired_dir[2]*desired_dir[2]).sqrt();
        let normalized = if dir_len > 1e-6 {
            [desired_dir[0]/dir_len, 0.0, desired_dir[2]/dir_len]
        } else {
            [0.0, 0.0, 0.0]
        };

        if self.on_ground {
            let friction_rate = constants::GROUND_FRICTION * (1.0 - surface_slipperiness.clamp(0.0, 1.0));

            // Hard-stop friction: when there's no input direction AND we're
            // on a non-slippery surface, snap velocity hard toward zero
            // instead of just decaying exponentially. The exponential
            // decay leaves a long tail of small residual velocity that
            // FEELS slippery to users even though it's mathematically
            // small. Hard-stop matches Minecraft's "release-keys-to-stop"
            // expectation.
            //
            // Slippery surfaces (ice) skip the hard stop and use only
            // exponential decay so they remain skiddy.
            let hard_stop = dir_len <= 1e-6 && surface_slipperiness < 0.1;
            if hard_stop {
                // Apply strong decay — over a full tick at 20/s rate,
                // velocity drops to ~e^-10 = 0.005% — effectively zero.
                let decay = (-friction_rate * dt).exp();
                self.velocity[0] *= decay;
                self.velocity[2] *= decay;
                // Final hard zero clamp: anything below 0.05 m/s becomes
                // exactly 0. Removes the long tail entirely.
                if self.velocity[0].abs() < 0.05 { self.velocity[0] = 0.0; }
                if self.velocity[2].abs() < 0.05 { self.velocity[2] = 0.0; }
            } else {
                // Normal exponential decay.
                let decay = (-friction_rate * dt).exp();
                self.velocity[0] *= decay;
                self.velocity[2] *= decay;
                // Acceleration toward target speed in the input direction.
                let accel = constants::MOVE_ACCEL * (target_speed / constants::WALK_SPEED);
                self.velocity[0] += normalized[0] * accel * dt;
                self.velocity[2] += normalized[2] * accel * dt;
            }
        } else {
            // Air: minimal friction (light drag), partial input control.
            // The drag prevents the player from preserving infinite lateral
            // momentum after a short hop — fixes "always sliding" feel
            // when crossing minor bumps. Decay is gentle (1/s) so jumping
            // and air-strafing still feels light.
            let air_decay = (-constants::AIR_DRAG * dt).exp();
            self.velocity[0] *= air_decay;
            self.velocity[2] *= air_decay;
            let accel = constants::MOVE_ACCEL * constants::AIR_CONTROL
                * (target_speed / constants::WALK_SPEED);
            self.velocity[0] += normalized[0] * accel * dt;
            self.velocity[2] += normalized[2] * accel * dt;
        }

        // Cap horizontal speed at `target_speed` to prevent input from infinitely
        // accelerating the player. The cap applies only to the input contribution
        // direction — momentum from other sources (knockback, falling) isn't affected.
        let h_speed = (self.velocity[0]*self.velocity[0] + self.velocity[2]*self.velocity[2]).sqrt();
        if h_speed > target_speed && dir_len > 1e-6 {
            // Only cap if we're moving roughly in the input direction.
            // (If you're knocked sideways, the cap shouldn't slow you down.)
            let dot = self.velocity[0]*normalized[0] + self.velocity[2]*normalized[2];
            if dot > 0.0 {
                let scale = target_speed / h_speed;
                self.velocity[0] *= scale;
                self.velocity[2] *= scale;
            }
        }
    }

    /// AABB min/max corners derived from position + half-extents.
    pub fn aabb_min(&self) -> [f32; 3] {
        [
            self.position[0] - self.half_extents[0],
            self.position[1] - self.half_extents[1],
            self.position[2] - self.half_extents[2],
        ]
    }
    pub fn aabb_max(&self) -> [f32; 3] {
        [
            self.position[0] + self.half_extents[0],
            self.position[1] + self.half_extents[1],
            self.position[2] + self.half_extents[2],
        ]
    }
}

/// Find a position near `(x, y, z)` where a player AABB doesn't overlap
/// any solid blocks. Searches upward first (player toggling fly-mode
/// inside leaves should pop out the top, not fall through the ground),
/// then downward. Returns the original position if no clear spot is
/// found within search range — caller should treat that as "best effort".
///
/// Used when toggling from fly-mode to physics-mode: the camera might be
/// inside a tree's leaf canopy or grazing a wall, and dropping a body
/// straight there would have it stuck immediately.
pub fn find_safe_spawn(world: &dyn BlockSampler, x: f32, y: f32, z: f32) -> [f32; 3] {
    // Probe up to 16 blocks up, then 4 blocks down. Step in 0.5-block
    // increments so we don't miss tight gaps.
    const SEARCH_UP: i32 = 32;     // 16 blocks at 0.5/step
    const SEARCH_DOWN: i32 = 8;    // 4 blocks
    const STEP: f32 = 0.5;

    let test_clear = |candidate_y: f32| -> bool {
        // Build a temporary body at the candidate position; check if its
        // AABB overlaps any solid block. The body's actual position
        // determines AABB, so construct one cheaply.
        let body = PhysicsBody::new_player([x, candidate_y, z]);
        let min = body.aabb_min();
        let max = body.aabb_max();
        let bx0 = min[0].floor() as i32;
        let by0 = min[1].floor() as i32;
        let bz0 = min[2].floor() as i32;
        let bx1 = (max[0] - 1e-4).floor() as i32;
        let by1 = (max[1] - 1e-4).floor() as i32;
        let bz1 = (max[2] - 1e-4).floor() as i32;
        for by in by0..=by1 {
            for bz in bz0..=bz1 {
                for bx in bx0..=bx1 {
                    if let Some(b) = world.block_at(bx, by, bz) {
                        if b.blocks_collision() {
                            return false;
                        }
                    }
                }
            }
        }
        true
    };

    // Try the requested position first.
    if test_clear(y) { return [x, y, z]; }

    // Probe upward first.
    for i in 1..=SEARCH_UP {
        let dy = i as f32 * STEP;
        if test_clear(y + dy) { return [x, y + dy, z]; }
    }
    // Then downward.
    for i in 1..=SEARCH_DOWN {
        let dy = i as f32 * STEP;
        if test_clear(y - dy) { return [x, y - dy, z]; }
    }
    // Give up — caller gets the original. Body will be stuck briefly
    // but physics resolution will (eventually) resolve it.
    [x, y, z]
}

/// Step a body forward by `dt` seconds, applying gravity and resolving
/// collisions against the block world. Mutates `body` in place.
pub fn step_body(
    body: &mut PhysicsBody,
    dt: f32,
    params: PhysicsParams,
    world: &dyn BlockSampler,
) {
    // Decay render_y_offset toward 0 over ~120ms. This is the
    // visual catch-up after step-up: physics teleported the body up
    // a step, but the visible position lags behind for a moment.
    //
    // Lerp speed of 8.0/sec means: a -1.0 offset (full block step)
    // catches up by ~8 blocks/sec, so the lag of 1 block clears in
    // ~125 ms — fast enough to feel responsive, slow enough to read
    // as a smooth lift instead of a teleport.
    if body.render_y_offset != 0.0 {
        const RENDER_LERP_PER_SECOND: f32 = 8.0;
        let max_change = RENDER_LERP_PER_SECOND * dt;
        if body.render_y_offset < 0.0 {
            body.render_y_offset = (body.render_y_offset + max_change).min(0.0);
        } else {
            body.render_y_offset = (body.render_y_offset - max_change).max(0.0);
        }
    }

    // Apply gravity to vertical velocity.
    if params.apply_gravity {
        body.velocity[1] -= params.gravity * dt;
        if body.velocity[1] < -params.terminal_velocity {
            body.velocity[1] = -params.terminal_velocity;
        }
    }

    // Apply ambient wind force as a small lateral acceleration. Scale is
    // chosen so a max wind (magnitude=1.0) adds noticeable lateral push
    // — produces ~0.1-0.3 m/s drift at gust peaks (was 0.025 m/s, invisible).
    // Wind only affects bodies on the ground or in flight; jumping in calm
    // doesn't suddenly drift sideways from a tiny gust.
    //
    // Future: scale by exposed-surface-area (a player above ground catches
    // more wind than one in a tunnel), couple to body's mass, etc.
    const WIND_FORCE_SCALE: f32 = 4.0; // bumped from 0.5 — gusts now noticeable
    body.velocity[0] += params.wind_force[0] * WIND_FORCE_SCALE * dt;
    body.velocity[2] += params.wind_force[1] * WIND_FORCE_SCALE * dt;

    // Motion factor from overlapping non-solid cells. Walking through tall
    // grass slows you ~15%, water ~55%, cobweb 90%. The factor is the MIN
    // of all overlapping cells (the most-restrictive medium dominates), so
    // partial submersion in water still slows you. We sample the body's
    // AABB at integer block coords and read motion_factor() for each.
    //
    // Solid blocks contribute 1.0 (they don't slow you — collision will
    // stop you anyway). Air contributes 1.0. Plants/fluids contribute their
    // < 1.0 factor.
    let motion = motion_factor_in_aabb(body, world);
    let dx = body.velocity[0] * dt * motion;
    let dy = body.velocity[1] * dt * motion;
    let dz = body.velocity[2] * dt * motion;

    if !params.apply_collision {
        body.position[0] += dx;
        body.position[1] += dy;
        body.position[2] += dz;
        body.on_ground = false;
        return;
    }

    // Move axis-by-axis. Order matters slightly — Y first means we resolve
    // ground contact before sliding sideways into walls (avoids "ramping up"
    // off corners). Then X, then Z.
    body.on_ground = false;

    if dy != 0.0 {
        let collided = move_axis(body, 1, dy, world);
        if collided {
            // If we were moving down and got blocked, we landed on something.
            if dy < 0.0 {
                body.on_ground = true;
            }
            body.velocity[1] = 0.0;
        }
    }

    if dx != 0.0 {
        if try_move_with_step_up(body, 0, dx, world, params.step_height) {
            body.velocity[0] = 0.0;
        }
    }

    if dz != 0.0 {
        if try_move_with_step_up(body, 2, dz, world, params.step_height) {
            body.velocity[2] = 0.0;
        }
    }
}

/// Move along an axis (X or Z), with auto-step-up. If the standard
/// move is blocked AND the body is on ground, try lifting up to
/// `step_height` and moving again. If THAT succeeds, accept the
/// stepped position. Otherwise revert and report blocked.
///
/// This is essential once bumpy block tops ship — a soft block's
/// visible top sags ~0.2-0.3 below integer Y, so walking from one
/// soft block to the next bumps the body's leading face into the
/// next block's side. Without auto-step, the player stutters at
/// every block boundary.
///
/// Step height is small (default 0.6) — enough to clear a half-block
/// bump but not enough to scale a full 1-block step (which would
/// feel like jumping).
fn try_move_with_step_up(
    body: &mut PhysicsBody,
    axis: usize,
    delta: f32,
    world: &dyn BlockSampler,
    step_height: f32,
) -> bool {
    // Try standard move first.
    let saved_pos = body.position;
    let saved_on_ground = body.on_ground;
    let collided = move_axis(body, axis, delta, world);
    if !collided { return false; }

    // Blocked — only try step-up if we WERE on the ground and step_height > 0.
    if !saved_on_ground || step_height <= 0.0 {
        return true;
    }

    // Capture where the standard move snapped us to (against the obstacle).
    let snapped_pos = body.position;

    // Revert to original position, then try with the body lifted by step_height.
    body.position = saved_pos;
    body.position[1] += step_height;

    // First check that the lifted position itself is clear (no ceiling
    // overlap). If overlapping, fall back to original snap.
    if body_overlaps_solid(body, world) {
        body.position = snapped_pos;
        return true;
    }

    // Apply the horizontal move at the elevated Y.
    let lifted_collided = move_axis(body, axis, delta, world);
    if lifted_collided {
        // Even with lift, blocked — revert lift and accept original snap.
        body.position = snapped_pos;
        return true;
    }

    // Step-up succeeded. Now drop the body back down by up to step_height
    // to land on the new step. move_axis will snap us against the surface.
    let pre_drop_y = body.position[1];
    let dropped_collided = move_axis(body, 1, -step_height, world);
    if dropped_collided {
        // Landed on something within step_height — body now rests on the step.
        body.on_ground = true;
        body.velocity[1] = 0.0;
    }
    // Record visual lag: physics teleported up by (final_y - saved_y).
    // The renderer should lerp from saved_y up to final_y over ~120ms.
    // We accumulate (subtract) into render_y_offset because successive
    // step-ups in the same dt window should compound, not overwrite —
    // a player walking up several steps fast still sees one smooth lift.
    //
    // Sign convention: render_y_offset is NEGATIVE when the visible
    // body is BELOW the physics body. So we subtract the upward
    // delta to make the visible body lag behind.
    let total_y_change = body.position[1] - saved_pos[1];
    if total_y_change > 0.0 {
        body.render_y_offset -= total_y_change;
        // Cap accumulated lag so multi-step bursts don't trail
        // unrealistically. 1.5 blocks of lag is plenty.
        if body.render_y_offset < -1.5 {
            body.render_y_offset = -1.5;
        }
    }
    let _ = pre_drop_y;
    // The horizontal move ultimately succeeded; don't report collision.
    false
}

/// Test if the body's AABB at its current position overlaps any
/// collision-blocking block. Used by step-up to verify a candidate
/// elevated position is clear before committing.
fn body_overlaps_solid(body: &PhysicsBody, world: &dyn BlockSampler) -> bool {
    let min = body.aabb_min();
    let max = body.aabb_max();
    const EPS: f32 = 1e-4;
    let bx0 = (min[0] + EPS).floor() as i32;
    let bx1 = (max[0] - EPS).floor() as i32;
    let by0 = (min[1] + EPS).floor() as i32;
    let by1 = (max[1] - EPS).floor() as i32;
    let bz0 = (min[2] + EPS).floor() as i32;
    let bz1 = (max[2] - EPS).floor() as i32;
    for by in by0..=by1 {
        for bz in bz0..=bz1 {
            for bx in bx0..=bx1 {
                if let Some(block) = world.block_at(bx, by, bz) {
                    if block.blocks_collision() { return true; }
                }
            }
        }
    }
    false
}

/// Scan the body's AABB for non-solid cells with `motion_factor < 1.0` and
/// return the minimum factor found. Used to slow the player when wading
/// through water, walking through tall grass, etc. Returns 1.0 if no
/// special cells overlap.
///
/// Sampled at all integer cells the AABB intersects. For typical bodies
/// this is ≤ 6 cells per frame, cheap.
fn motion_factor_in_aabb(body: &PhysicsBody, world: &dyn BlockSampler) -> f32 {
    let min_aabb = body.aabb_min();
    let max_aabb = body.aabb_max();
    let xmin = min_aabb[0].floor() as i32;
    let ymin = min_aabb[1].floor() as i32;
    let zmin = min_aabb[2].floor() as i32;
    let xmax = (max_aabb[0] - 1e-4).floor() as i32;
    let ymax = (max_aabb[1] - 1e-4).floor() as i32;
    let zmax = (max_aabb[2] - 1e-4).floor() as i32;

    let mut min_factor = 1.0_f32;
    for y in ymin..=ymax {
        for z in zmin..=zmax {
            for x in xmin..=xmax {
                let b = match world.block_at(x, y, z) {
                    Some(b) => b,
                    None => continue,
                };
                // Skip blocks that block translational motion — collision
                // resolves them. Only non-blocking blocks (fluids, plants,
                // cobwebs, air) contribute a motion factor here.
                if b.blocks_collision() { continue; }
                let f = b.motion_factor();
                if f < min_factor { min_factor = f; }
            }
        }
    }
    min_factor
}

/// Move the body along a single axis by `delta`. Returns `true` if it
/// collided (and was snapped to the contact plane).
fn move_axis(
    body: &mut PhysicsBody,
    axis: usize,
    delta: f32,
    world: &dyn BlockSampler,
) -> bool {
    // Tentative new position.
    let mut new_pos = body.position;
    new_pos[axis] += delta;

    // Compute the AABB at the new position.
    let mut min = [
        new_pos[0] - body.half_extents[0],
        new_pos[1] - body.half_extents[1],
        new_pos[2] - body.half_extents[2],
    ];
    let mut max = [
        new_pos[0] + body.half_extents[0],
        new_pos[1] + body.half_extents[1],
        new_pos[2] + body.half_extents[2],
    ];

    // We need to check block cells the AABB overlaps. Use floor/ceil to get
    // the integer grid range. Use a tiny epsilon to avoid registering a
    // collision at the exact boundary (player standing on ground would
    // otherwise self-collide).
    const EPS: f32 = 1e-4;
    min[0] += EPS; min[1] += EPS; min[2] += EPS;
    max[0] -= EPS; max[1] -= EPS; max[2] -= EPS;

    let bx0 = min[0].floor() as i32;
    let bx1 = max[0].floor() as i32;
    let by0 = min[1].floor() as i32;
    let by1 = max[1].floor() as i32;
    let bz0 = min[2].floor() as i32;
    let bz1 = max[2].floor() as i32;

    let mut collided = false;
    for by in by0..=by1 {
        for bz in bz0..=bz1 {
            for bx in bx0..=bx1 {
                if let Some(block) = world.block_at(bx, by, bz) {
                    if block.blocks_collision() {
                        collided = true;
                        break;
                    }
                }
            }
            if collided { break; }
        }
        if collided { break; }
    }

    if collided {
        // Snap the body so its leading face just touches the blocking cell.
        if delta > 0.0 {
            // Moving in +axis direction; the leading face is the max side.
            // The blocking cell starts at the floor of (max[axis]+EPS).
            // Snap so max[axis] = floor(max[axis]) - EPS.
            let blocking = max[axis].floor();
            new_pos[axis] = blocking - body.half_extents[axis] - EPS;
        } else {
            // Moving in -axis; leading face is min side.
            let blocking = min[axis].floor() + 1.0;
            new_pos[axis] = blocking + body.half_extents[axis] + EPS;
        }
        body.position[axis] = new_pos[axis];
        true
    } else {
        body.position[axis] = new_pos[axis];
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Test sampler: in-memory map of (x,y,z) → BlockId. Anything not in the
    /// map is Air.
    struct MapWorld(HashMap<(i32, i32, i32), BlockId>);

    impl MapWorld {
        fn empty() -> Self { Self(HashMap::new()) }
        fn floor_at_y(y: i32) -> Self {
            let mut m = HashMap::new();
            for x in -16..16 {
                for z in -16..16 {
                    m.insert((x, y, z), BlockId::Stone);
                }
            }
            Self(m)
        }
        fn set(&mut self, x: i32, y: i32, z: i32, b: BlockId) {
            self.0.insert((x, y, z), b);
        }
    }

    impl BlockSampler for MapWorld {
        fn block_at(&self, x: i32, y: i32, z: i32) -> Option<BlockId> {
            Some(self.0.get(&(x, y, z)).copied().unwrap_or(BlockId::Air))
        }
    }

    #[test]
    fn body_falls_under_gravity() {
        let world = MapWorld::empty();
        let mut body = PhysicsBody::new_player([0.0, 100.0, 0.0]);
        step_body(&mut body, 0.5, PhysicsParams::player_default(), &world);
        // After 0.5s: v = -g*0.5 = -14 b/s, displacement = -g*0.25/2 = wait,
        // we apply gravity first then move at the new velocity for the full
        // dt. So displacement = velocity_after_gravity * dt = -14 * 0.5 = -7.
        // (Symplectic Euler — not strictly accurate but standard for games.)
        assert!(body.position[1] < 100.0, "body should have fallen");
        assert!(!body.on_ground);
    }

    #[test]
    fn body_lands_on_floor() {
        let world = MapWorld::floor_at_y(50);
        // Drop the body from y=60. Floor occupies y=50..51 (block at integer y=50).
        // Body half-height is 0.9, so body.position.y = 51.9 means feet at y=51.
        let mut body = PhysicsBody::new_player([0.5, 60.0, 0.5]);
        for _ in 0..200 {
            step_body(&mut body, 1.0 / 60.0, PhysicsParams::player_default(), &world);
            if body.on_ground { break; }
        }
        assert!(body.on_ground, "body should have landed; ended at y={}", body.position[1]);
        // Standing on floor: AABB min Y must be at top of floor (y=51).
        let aabb_min_y = body.aabb_min()[1];
        assert!((aabb_min_y - 51.0).abs() < 0.01,
            "feet should be just above floor (y=51); got aabb_min_y={}",
            aabb_min_y);
        assert!(body.velocity[1].abs() < 0.01, "vertical velocity should be zeroed");
    }

    #[test]
    fn jump_only_works_on_ground() {
        let world = MapWorld::floor_at_y(50);
        let mut body = PhysicsBody::new_player([0.5, 51.91, 0.5]);
        body.on_ground = false;
        // First step: not on ground yet; jump should fail.
        assert!(!body.try_jump());
        // Step physics until grounded.
        for _ in 0..30 {
            step_body(&mut body, 1.0 / 60.0, PhysicsParams::player_default(), &world);
            if body.on_ground { break; }
        }
        assert!(body.on_ground);
        // Now jump should work.
        assert!(body.try_jump());
        assert!((body.velocity[1] - constants::JUMP_VELOCITY).abs() < 1e-3);
        // Subsequent jump should fail until landing.
        assert!(!body.try_jump());
    }

    #[test]
    fn body_does_not_pass_through_wall() {
        let mut world = MapWorld::floor_at_y(50);
        // Wall at x=2 from y=51..53.
        for y in 51..53 {
            for z in -2..3 {
                world.set(2, y, z, BlockId::Stone);
            }
        }
        let mut body = PhysicsBody::new_player([0.0, 51.91, 0.0]);
        body.on_ground = true;
        body.velocity[0] = 5.0; // moving +X
        // Use fly_mode (no gravity) so the body doesn't fall past the wall
        // before the X step. Tests just the X collision response.
        step_body(&mut body, 0.5, PhysicsParams::fly_mode(), &world);
        // Body half-width is 0.3, wall starts at x=2. Body center should be
        // capped at 2 - 0.3 - eps ≈ 1.6999.
        assert!(body.position[0] < 1.71,
            "body should be blocked by wall, but at x={}", body.position[0]);
        assert!(body.velocity[0].abs() < 1e-3,
            "x velocity should be zeroed by wall hit");
    }

    #[test]
    fn noclip_passes_through_blocks() {
        let mut world = MapWorld::floor_at_y(50);
        for y in 50..55 {
            for x in -2..3 {
                for z in -2..3 {
                    world.set(x, y, z, BlockId::Stone);
                }
            }
        }
        let mut body = PhysicsBody::new_player([0.0, 60.0, 0.0]);
        body.velocity = [0.0, -10.0, 0.0];
        step_body(&mut body, 1.0, PhysicsParams::noclip(), &world);
        // Should pass right through the blocks.
        assert!(body.position[1] < 51.0, "noclip should ignore blocks; y={}", body.position[1]);
        assert!(!body.on_ground);
    }

    #[test]
    fn ground_input_accelerates_to_target_speed() {
        let world = MapWorld::floor_at_y(50);
        let mut body = PhysicsBody::new_player([0.0, 51.91, 0.0]);
        body.on_ground = true;
        // Apply forward input over many small steps; should converge near WALK_SPEED.
        for _ in 0..120 {
            body.apply_input([1.0, 0.0, 0.0], constants::WALK_SPEED, 0.0, 1.0/60.0);
            super::step_body(&mut body, 1.0/60.0, PhysicsParams::player_default(), &world);
        }
        let h_speed = (body.velocity[0]*body.velocity[0] + body.velocity[2]*body.velocity[2]).sqrt();
        assert!((h_speed - constants::WALK_SPEED).abs() < 0.5,
            "expected speed near {}, got {}", constants::WALK_SPEED, h_speed);
    }

    #[test]
    fn ground_friction_stops_body_when_no_input() {
        let world = MapWorld::floor_at_y(50);
        let mut body = PhysicsBody::new_player([0.0, 51.91, 0.0]);
        body.on_ground = true;
        body.velocity[0] = 5.0;
        // No input over many frames — friction should slow body to ~0.
        for _ in 0..120 {
            body.apply_input([0.0, 0.0, 0.0], constants::WALK_SPEED, 0.0, 1.0/60.0);
            super::step_body(&mut body, 1.0/60.0, PhysicsParams::player_default(), &world);
        }
        assert!(body.velocity[0].abs() < 0.1,
            "expected near-zero velocity from friction, got {}", body.velocity[0]);
    }

    #[test]
    fn slippery_surface_decays_slower() {
        let _world = MapWorld::floor_at_y(50);
        let mut a = PhysicsBody::new_player([0.0, 51.91, 0.0]);
        let mut b = PhysicsBody::new_player([0.0, 51.91, 0.0]);
        a.on_ground = true; b.on_ground = true;
        a.velocity[0] = 5.0; b.velocity[0] = 5.0;
        // a runs on a normal surface (slip=0), b runs on ice (slip=0.85).
        for _ in 0..30 {
            a.apply_input([0.0, 0.0, 0.0], constants::WALK_SPEED, 0.0, 1.0/60.0);
            b.apply_input([0.0, 0.0, 0.0], constants::WALK_SPEED, 0.85, 1.0/60.0);
        }
        assert!(b.velocity[0].abs() > a.velocity[0].abs() * 5.0,
            "ice should slow ~5x slower than normal surface; a={}, b={}",
            a.velocity[0], b.velocity[0]);
    }

    #[test]
    fn air_control_takes_longer_to_reach_target_speed() {
        let mut grounded = PhysicsBody::new_player([0.0, 51.91, 0.0]);
        let mut airborne = PhysicsBody::new_player([0.0, 100.0, 0.0]);
        grounded.on_ground = true;
        airborne.on_ground = false;
        // 5 frames of input each — short enough that neither hits the speed cap.
        for _ in 0..5 {
            grounded.apply_input([1.0, 0.0, 0.0], constants::WALK_SPEED, 0.0, 1.0/60.0);
            airborne.apply_input([1.0, 0.0, 0.0], constants::WALK_SPEED, 0.0, 1.0/60.0);
        }
        let g = grounded.velocity[0];
        let a = airborne.velocity[0];
        // Airborne body gains less velocity per frame because air control
        // gates input acceleration. Exact ratio depends on friction-decay
        // interaction (grounded has friction biting into its gain too) so
        // we just verify air clearly underperforms ground.
        assert!(a < g * 0.7,
            "air should accumulate <70% of ground velocity; ground={}, air={}", g, a);
    }

    #[test]
    fn motion_factor_slows_body_in_water() {
        // Body in water moves slower than the same body in air, with the
        // same velocity input.
        let mut air_world = MapWorld::empty();
        let mut water_world = MapWorld::empty();
        // Fill the cells the body will occupy with water.
        for x in -2..=2 {
            for y in 99..=102 {
                for z in -2..=2 {
                    water_world.set(x, y, z, BlockId::Water);
                }
            }
        }
        // Air world unchanged (default Air).
        let _ = &mut air_world;

        let start = [0.0_f32, 100.0, 0.0];
        let mut body_air = PhysicsBody::new_player(start);
        let mut body_water = PhysicsBody::new_player(start);
        body_air.velocity[0] = 5.0;
        body_water.velocity[0] = 5.0;

        let p = PhysicsParams { apply_gravity: false, ..PhysicsParams::player_default() };
        super::step_body(&mut body_air, 0.1, p, &air_world);
        super::step_body(&mut body_water, 0.1, p, &water_world);

        assert!(body_water.position[0] < body_air.position[0],
            "water-walking body should travel less; air={}, water={}",
            body_air.position[0], body_water.position[0]);
        // Water motion_factor is 0.45, so water-body should travel ~45% of
        // air-body distance. Allow generous tolerance.
        let ratio = body_water.position[0] / body_air.position[0];
        assert!(ratio > 0.30 && ratio < 0.60,
            "water motion ratio off: {} (expected near 0.45)", ratio);
    }

    /// Step-up: a body walking from a flat floor toward a 1-block step
    /// up should auto-step onto the step. With step_height=1.1, the
    /// body should clear a full 1-block step.
    #[test]
    fn auto_step_up_climbs_full_block() {
        let mut world = MapWorld::floor_at_y(63);
        // Place a 1-block step at x>=2.
        for x in 2..16 {
            for z in -2..2 {
                world.set(x, 64, z, BlockId::Stone);
            }
        }
        // Body sits on floor (y=63 top = 64.0). half_extents [0.3, 0.9, 0.3].
        // So body center at y = 64.9.
        let mut body = PhysicsBody::new_player([0.0, 64.9, 0.0]);
        body.velocity = [4.0, 0.0, 0.0];
        body.on_ground = true;
        // Use step_height=1.1 to clear a full 1-block step.
        let mut p = PhysicsParams::player_default();
        p.step_height = 1.1;
        // Tiny dt to avoid tunnelling.
        for _ in 0..200 {
            super::step_body(&mut body, 0.005, p, &world);
        }
        // Body should have stepped up past x=2.
        assert!(body.position[0] > 2.0,
            "body should have stepped up onto the full block; got x={}",
            body.position[0]);
        // And should be on top of the step (y center near 65.9).
        assert!(body.position[1] > 65.5,
            "body should be on top of the step; got y={}",
            body.position[1]);
    }

    /// Step-up should NOT engage when the body is in mid-air (jumping/falling).
    /// Otherwise the player could "auto-jump" up walls while falling.
    #[test]
    fn no_step_up_when_airborne() {
        let mut world = MapWorld::floor_at_y(63);
        for x in 2..16 {
            for z in -2..2 {
                world.set(x, 64, z, BlockId::Stone);
                world.set(x, 65, z, BlockId::Stone); // make it a 2-block wall
            }
        }
        let mut body = PhysicsBody::new_player([0.0, 70.0, 0.0]); // in mid-air
        body.velocity = [4.0, 0.0, 0.0];
        body.on_ground = false;
        let p = PhysicsParams::player_default();
        // Just one small step; body should still be in the air at end.
        super::step_body(&mut body, 0.05, p, &world);
        // Body should fall some, not step.
        assert!(body.position[1] < 70.0, "body should fall, not step");
    }

    /// Step-up requires step_height > 0. With it disabled (e.g. fly mode),
    /// a body walking into a step should bounce off as before.
    #[test]
    fn no_step_up_when_step_height_zero() {
        let mut world = MapWorld::floor_at_y(63);
        for x in 2..16 {
            for z in -2..2 {
                world.set(x, 64, z, BlockId::Stone);
            }
        }
        let mut body = PhysicsBody::new_player([0.0, 64.9, 0.0]);
        body.velocity = [4.0, 0.0, 0.0];
        body.on_ground = true;
        // Use player_default but disable step_height.
        let mut p = PhysicsParams::player_default();
        p.step_height = 0.0;
        // Tiny dt to avoid tunnelling; many substeps for total displacement.
        for _ in 0..100 {
            super::step_body(&mut body, 0.005, p, &world);
        }
        // Without step-up, body bumps the step and stops at x ≈ 1.7.
        assert!(body.position[0] < 1.8,
            "body should have been blocked by step (no auto-step); got x={}",
            body.position[0]);
    }

    /// Step-up animation: when the body auto-steps onto a ledge, the
    /// render_y_offset should become negative (visible body lags
    /// below physics) and decay back to 0 over the next several ticks.
    #[test]
    fn step_up_records_render_lag() {
        let mut world = MapWorld::floor_at_y(63);
        for x in 2..16 {
            for z in -2..2 {
                world.set(x, 64, z, BlockId::Stone);
            }
        }
        let mut body = PhysicsBody::new_player([0.0, 64.9, 0.0]);
        body.velocity = [4.0, 0.0, 0.0];
        body.on_ground = true;
        let p = PhysicsParams::player_default();
        // Step up onto the ledge — this should set a negative
        // render_y_offset.
        for _ in 0..200 {
            super::step_body(&mut body, 0.005, p, &world);
        }
        // Body has stepped up; physics Y is around 65.9 (top of step).
        // render_y_offset should be < 0 (visual is lagging) OR has
        // already partially decayed back toward 0.
        assert!(body.position[1] > 65.5,
            "body should have stepped up; physics Y={}",
            body.position[1]);
        // Render Y must be ≤ physics Y (lag is downward).
        assert!(body.render_y() <= body.position[1] + 1e-4,
            "render_y must lag physics: render={}, physics={}",
            body.render_y(), body.position[1]);
    }

    /// Step-up render lag should fully decay to zero given enough time.
    #[test]
    fn step_up_render_lag_decays_to_zero() {
        let world = MapWorld::floor_at_y(63);
        let mut body = PhysicsBody::new_player([0.0, 64.9, 0.0]);
        body.render_y_offset = -1.0; // pretend we just step-upped a full block
        body.on_ground = true;
        let p = PhysicsParams::player_default();
        // Decay rate is 8.0 per second, so 1 block of lag clears in 0.125s.
        // Run for 0.5s — should be fully decayed.
        for _ in 0..100 {
            super::step_body(&mut body, 0.005, p, &world);
        }
        assert!(body.render_y_offset.abs() < 1e-4,
            "render_y_offset should decay to 0, got {}", body.render_y_offset);
    }
}
