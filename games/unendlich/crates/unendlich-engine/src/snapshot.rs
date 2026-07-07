//! Snapshot / restore API for rollback netcode.
//!
//! For rollback to work, every system that participates in simulation
//! must support capturing its state to a value AND restoring from one.
//! `Snapshottable` is the trait that says "I know how to do that."
//!
//! ## Contract
//!
//! ```ignore
//! impl Snapshottable for Foo {
//!     type Snapshot = FooSnap;
//!     fn capture(&self) -> Self::Snapshot { ... }
//!     fn restore(&mut self, s: &Self::Snapshot) { ... }
//! }
//! ```
//!
//! Required invariant: for ANY `state` and ANY tick `T`,
//!
//! ```ignore
//! let snap = state.capture();
//! // ... time passes, state mutates ...
//! state.restore(&snap);
//! // state is now bit-exactly equivalent to what it was at capture time
//! ```
//!
//! "Bit-exactly equivalent" is strong. Float NaN handling, HashMap
//! ordering, anything that affects observable output must be the same.
//!
//! ## What snapshotted state should NOT include
//!
//! - Caches that can be re-derived (mesh data, field overlays).
//!   Restore should INVALIDATE these, not capture them. Saves snapshot
//!   space and avoids stale-cache risks.
//! - Render-only data (particle positions, camera shake state, etc.)
//!   These don't participate in the simulation; their values at any
//!   given tick aren't load-bearing.
//! - Wall-clock timestamps. Wall clock isn't part of the simulation.
//!
//! ## Today's status
//!
//! No system is fully Snapshottable yet. The trait is defined here as
//! the API contract; concrete implementations land alongside the
//! relevant systems as they migrate to rollback-safe form.
//!
//! Provisional snapshot strategies per system:
//!
//! - `Chunk`: `Arc<Chunk>` is already cheap to clone (block array +
//!   instance_state map). Snapshot = the Arc. Restore = swap Arc back.
//! - `PhysicsBody`: 6 floats + flags + AABB. Trivially Copy. Snapshot
//!   is the body itself.
//! - `TickScheduler`: TickId + accumulator. Trivially Copy.
//! - `Sky`: time_seconds + time_scale. Trivially Copy.
//! - World fields: re-derived from chunks; restore = invalidate.
//!
//! ## Future shape
//!
//! `WorldSnapshot` will aggregate all of these:
//!
//! ```ignore
//! struct WorldSnapshot {
//!     tick_id: TickId,
//!     chunks: HashMap<ChunkCoord, Arc<Chunk>>,  // Arcs, cheap
//!     player_bodies: Vec<PhysicsBody>,
//!     scheduler: SchedulerState,
//!     // entities: Vec<EntitySnap>, when entities exist
//! }
//! ```
//!
//! The rollback driver keeps a ring buffer of recent WorldSnapshots
//! parallel to the InputBuffer. On a server correction:
//!
//! 1. Restore from the snapshot at the corrected tick.
//! 2. Apply the corrected authoritative state.
//! 3. Re-simulate forward through cached InputFrames to current tick.
//!
//! Step 3 produces a corrected current state; rendering interpolates
//! visually so the player sees the correction smoothly rather than as
//! a jarring snap.

/// Capture and restore state for rollback. Implementors expose a
/// snapshot type that's cheap to clone and contains exactly the
/// information needed to restore the simulation to a previous state.
pub trait Snapshottable {
    /// The captured-state value. Should be `Clone` (so we can keep
    /// snapshots in a ring buffer) and ideally compact.
    type Snapshot: Clone;

    /// Capture current state. Pure read — must not mutate.
    fn capture(&self) -> Self::Snapshot;

    /// Restore from a previous snapshot. After this, observable state
    /// must be bit-exactly equivalent to what it was when the
    /// snapshot was taken.
    fn restore(&mut self, snapshot: &Self::Snapshot);
}

// ---------------------------------------------------------------------------
// Concrete impls — start small.
// ---------------------------------------------------------------------------

impl Snapshottable for crate::physics::PhysicsBody {
    /// PhysicsBody is `Copy` — snapshot IS the body. No allocation.
    /// This is the cheapest case; player bodies snapshot effectively
    /// for free.
    type Snapshot = Self;

    fn capture(&self) -> Self::Snapshot { *self }
    fn restore(&mut self, snapshot: &Self::Snapshot) { *self = *snapshot; }
}

impl Snapshottable for crate::sky::Sky {
    /// Sky has a time_seconds and time_scale — both `Copy`.
    type Snapshot = Self;

    fn capture(&self) -> Self::Snapshot { *self }
    fn restore(&mut self, snapshot: &Self::Snapshot) { *self = *snapshot; }
}

#[cfg(feature = "render-core")]
impl Snapshottable for crate::render::camera::View {
    /// View is `Copy` (just six floats and a Vec3) — snapshot is
    /// identical to the View. This makes per-tick view capture for
    /// rollback / replay essentially free.
    ///
    /// Multiplayer notes:
    ///   - In a rollback-style multiplayer system (CS-style or
    ///     GGPO-style), each tick's snapshot includes every local
    ///     player's view. Restoring the world to a past tick also
    ///     restores their view orientation, so re-simulation
    ///     reproduces the same camera frustums and same
    ///     player-issued raycasts.
    ///   - The `FlyController` is intentionally NOT snapshotted:
    ///     move_speed and mouse_sensitivity are settings, not state.
    ///     They don't change during simulation, so they don't need
    ///     to be in the rollback ring buffer.
    type Snapshot = Self;

    fn capture(&self) -> Self::Snapshot { *self }
    fn restore(&mut self, snapshot: &Self::Snapshot) { *self = *snapshot; }
}

impl Snapshottable for crate::sim::mob::Mob {
    /// A mob is `Copy` (body + kind + health + AI + commanded heading), so
    /// the entity-level snapshot is the mob itself. This is the `EntitySnap`
    /// the module header anticipated: capturing a mob captures everything the
    /// simulation reads back when it re-steps that mob — including the
    /// `commanded` heading, so a replayed tick reproduces the same AI branch.
    type Snapshot = Self;

    fn capture(&self) -> Self::Snapshot { *self }
    fn restore(&mut self, snapshot: &Self::Snapshot) { *self = *snapshot; }
}

impl Snapshottable for crate::sim::state::SimState {
    /// The world aggregate — the `WorldSnapshot` the header sketched, now
    /// concrete. `SimState` is `Clone` (tick + `Vec<Mob>` + world_seed), and
    /// every field is load-bearing for the deterministic `step`, so the clone
    /// is a complete, restore-able capture. Mesh/field caches are not part of
    /// `SimState`, so this correctly snapshots only simulation state and
    /// leaves re-derivable data out, exactly as the contract requires.
    ///
    /// This is the unit the rollback driver rings-buffers: restore to the
    /// corrected tick, then re-`step` forward through cached inputs. The
    /// round-trip test proves that re-simulation reproduces the checksum
    /// trajectory bit-for-bit.
    type Snapshot = Self;

    fn capture(&self) -> Self::Snapshot { self.clone() }
    fn restore(&mut self, snapshot: &Self::Snapshot) { *self = snapshot.clone(); }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physics_body_snapshot_round_trip() {
        let mut a = crate::physics::PhysicsBody::new_player([1.0, 2.0, 3.0]);
        let snap = a.capture();
        // Mutate the body.
        a.position[0] = 99.0;
        a.velocity[1] = -50.0;
        // Restore — should match original.
        a.restore(&snap);
        assert_eq!(a.position, [1.0, 2.0, 3.0]);
        assert_eq!(a.velocity[1], 0.0); // initial velocity is zero
    }

    #[test]
    fn sky_snapshot_round_trip() {
        let mut sky = crate::sky::Sky::default();
        let snap = sky.capture();
        sky.tick(60.0); // advance 60s
        let advanced_time = sky.time_seconds;
        sky.restore(&snap);
        assert_ne!(sky.time_seconds, advanced_time);
    }

    #[cfg(feature = "render-core")]
    #[test]
    fn view_snapshot_round_trip() {
        use crate::render::camera::View;
        use nalgebra_glm as glm;
        let mut v = View::new(glm::vec3(10.0, 20.0, 30.0));
        v.yaw = 1.5;
        v.pitch = -0.3;
        let snap = v.capture();
        // Mutate the view (simulating a player turning + walking).
        v.yaw = 4.0;
        v.pitch = 0.5;
        v.position = glm::vec3(99.0, 99.0, 99.0);
        // Restore — should match original.
        v.restore(&snap);
        assert!((v.yaw - 1.5).abs() < 1e-5, "yaw not restored: {}", v.yaw);
        assert!((v.pitch + 0.3).abs() < 1e-5, "pitch not restored: {}", v.pitch);
        assert!((v.position - glm::vec3(10.0, 20.0, 30.0)).norm() < 1e-5,
            "position not restored: {:?}", v.position);
    }

    /// Multiple Views can be captured and restored independently —
    /// what a rollback ring buffer needs to do for split-screen.
    #[cfg(feature = "render-core")]
    #[test]
    fn multiple_views_snapshot_independently() {
        use crate::render::camera::View;
        use nalgebra_glm as glm;
        let mut v1 = View::new(glm::vec3(0.0, 0.0, 0.0));
        let mut v2 = View::new(glm::vec3(100.0, 0.0, 0.0));
        v1.yaw = 1.0;
        v2.yaw = 2.0;
        let s1 = v1.capture();
        let s2 = v2.capture();
        // Mutate both.
        v1.yaw = 999.0;
        v2.yaw = 999.0;
        // Restore each from its OWN snapshot.
        v1.restore(&s1);
        v2.restore(&s2);
        assert!((v1.yaw - 1.0).abs() < 1e-5);
        assert!((v2.yaw - 2.0).abs() < 1e-5);
    }

    #[test]
    fn mob_snapshot_round_trip() {
        use crate::sim::fixed::{Fixed, Vec3};
        use crate::sim::mob::Mob;
        let mut m = Mob::new_walker(
            Vec3 { x: Fixed::ZERO, y: Fixed::from_int(6), z: Fixed::ZERO }, 20);
        let snap = m.capture();
        // Mutate as a tick would: take damage, move, get an AI command.
        m.health = 3;
        m.body.pos.x = Fixed::from_int(99);
        m.commanded = Some(4);
        m.restore(&snap);
        assert_eq!(m.health, 20, "health restored");
        assert_eq!(m.body.pos.x, Fixed::ZERO, "position restored");
        assert_eq!(m.commanded, None, "commanded heading restored");
    }

    /// The reason the snapshot module exists: restore to a past tick, then
    /// re-simulate forward, and the result must be bit-exactly the same as
    /// the original forward run. This exercises the full rollback loop
    /// against the real deterministic `step` + `checksum`.
    #[test]
    fn sim_state_rollback_resim_is_bit_exact() {
        use crate::sim::fixed::{Fixed, Vec3};
        use crate::sim::mob::Mob;
        use crate::sim::physics::FlatWorld;
        use crate::sim::state::{checksum, step, SimState};

        let w = FlatWorld::new();
        let mut s = SimState::new_seeded(0, 0x00AB_CDEF);
        // A little ecosystem so the trajectory is non-trivial (hunt + flee + wander).
        s.mobs.push(Mob::new_seeker(
            Vec3 { x: Fixed::ZERO, y: Fixed::from_int(6), z: Fixed::ZERO }, 30));
        s.mobs.push(Mob::new_grazer(
            Vec3 { x: Fixed::from_int(3), y: Fixed::from_int(6), z: Fixed::ZERO }, 30));
        s.mobs.push(Mob::new_walker(
            Vec3 { x: Fixed::from_int(-2), y: Fixed::from_int(6), z: Fixed::from_int(1) }, 20));

        // Warm up so we snapshot a mid-simulation state, not the initial one.
        for _ in 0..5 {
            s = step(&s, &[], &w);
        }

        // Capture at tick T.
        let snap = s.capture();
        let checksum_at_t = checksum(&s);

        // Forward run from T: record the per-tick checksum trajectory.
        let mut forward = s.clone();
        let mut trajectory_a = Vec::new();
        for _ in 0..40 {
            forward = step(&forward, &[], &w);
            trajectory_a.push(checksum(&forward));
        }

        // Let `s` diverge far ahead (simulate having played on), then roll back.
        s = forward.clone();
        assert_ne!(checksum(&s), checksum_at_t, "state moved on before rollback");
        s.restore(&snap);
        assert_eq!(checksum(&s), checksum_at_t, "restore returns exactly to tick T");

        // Re-simulate the same 40 ticks. The trajectory must match bit-for-bit.
        let mut trajectory_b = Vec::new();
        for _ in 0..40 {
            s = step(&s, &[], &w);
            trajectory_b.push(checksum(&s));
        }
        assert_eq!(
            trajectory_a, trajectory_b,
            "rollback + re-sim must reproduce the exact checksum trajectory"
        );
    }
}
