//! `world::mesh_epoch` — the world-epoch stamp that stops stale mesh uploads
//! across a world reset (B1, r102 bughunt).
//!
//! ## The bug
//! `ChunkManager::reset_with_generator` (and `rebuild_for_seed`, the F4 launch
//! path) swaps in a new generator and clears all loaded/meshed chunk state so
//! the world regenerates. But meshing is asynchronous: jobs issued for the
//! *previous* world can still be in flight when the reset lands. When such a
//! job completes, its result is geometry for a world that no longer exists —
//! yet nothing distinguished it from a fresh result, so the renderer could
//! briefly upload stale geometry into the freshly-regenerated world (a flash of
//! old terrain at a chunk coordinate that now holds something else).
//!
//! ## The fix
//! `ChunkManager` now carries a monotonic [`world_epoch`](crate::world::chunk_manager)
//! counter that is bumped on every reset. The async mesher records the epoch at
//! the moment a job is *issued*; when the result returns, the consumer compares
//! the job's epoch against the manager's current epoch with
//! [`mesh_result_is_current`] and drops the result if they differ. This module
//! is the pure, GPU-free home for that predicate (and a tiny newtype) so the
//! logic can be unit-tested without a Vulkan context.
//!
//! Pure + deterministic: integer comparison only. No `unwrap`/`panic`, no IO,
//! no clock, no RNG.

/// A world generation stamp. Equal stamps describe the same world; a reset
/// produces a strictly greater stamp (see `ChunkManager::reset_with_generator`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct WorldEpoch(pub u64);

impl WorldEpoch {
    /// The initial epoch of a freshly constructed world.
    pub const ZERO: WorldEpoch = WorldEpoch(0);

    /// The next epoch after a world reset. Saturating so a (practically
    /// impossible) overflow can never wrap a fresh world back onto an old
    /// epoch and silently re-accept stale geometry.
    pub fn bumped(self) -> WorldEpoch {
        WorldEpoch(self.0.saturating_add(1))
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

/// Should a mesh result tagged with `result_epoch` be applied, given the world's
/// `current_epoch`? Only when they match: a result from a prior epoch was meshed
/// against a world that has since been reset, so it is stale and must be dropped.
///
/// This is the single decision point the renderer/mesher uses when an async mesh
/// result returns. Pre-fix this predicate did not exist and every result was
/// accepted, which is the stale-upload bug.
#[inline]
pub fn mesh_result_is_current(result_epoch: u64, current_epoch: u64) -> bool {
    result_epoch == current_epoch
}

/// Typed convenience over [`mesh_result_is_current`].
#[inline]
pub fn epoch_accepts(result: WorldEpoch, current: WorldEpoch) -> bool {
    result == current
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_current_rejects_stale() {
        // current-epoch result is applied
        assert!(mesh_result_is_current(0, 0));
        assert!(mesh_result_is_current(7, 7));
        // a result from a prior epoch (world was reset under it) is rejected
        assert!(!mesh_result_is_current(0, 1));
        assert!(!mesh_result_is_current(3, 5));
        // even a result from a *future* epoch (shouldn't happen) is not "current"
        assert!(!mesh_result_is_current(2, 1));
    }

    #[test]
    fn bumped_is_strictly_increasing_and_saturating() {
        let e0 = WorldEpoch::ZERO;
        let e1 = e0.bumped();
        let e2 = e1.bumped();
        assert!(e1 > e0 && e2 > e1, "each reset advances the epoch");
        // saturating: never wraps back to an old epoch
        let max = WorldEpoch(u64::MAX);
        assert_eq!(max.bumped(), WorldEpoch(u64::MAX), "saturates, never wraps to 0");
        assert!(!epoch_accepts(WorldEpoch(u64::MAX - 1), max.bumped()) || max.bumped() == WorldEpoch(u64::MAX));
    }

    #[test]
    fn typed_gate_matches_raw() {
        for r in 0u64..6 {
            for c in 0u64..6 {
                assert_eq!(
                    epoch_accepts(WorldEpoch(r), WorldEpoch(c)),
                    mesh_result_is_current(r, c)
                );
            }
        }
    }

    #[test]
    fn models_the_defect_a_late_result_after_reset_is_dropped() {
        // epoch at the time a mesh job was issued
        let issued_at = WorldEpoch(4);
        // a world reset happens while the job is in flight
        let after_reset = issued_at.bumped();
        // when the late result returns, the gate rejects it (pre-fix: accepted → stale upload)
        assert!(!epoch_accepts(issued_at, after_reset), "stale result must be dropped");
        // a job issued after the reset is accepted
        assert!(epoch_accepts(after_reset, after_reset));
    }
}
