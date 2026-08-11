//! Creature-mesh dirty tracking — the r247 fix, isolated and testable.
//!
//! `docs/design/PERF_FINDING_creature_mesh.md` traced the game_update p95
//! stutter to the per-frame creature mesh rebuild (unendlich_fullgame.rs,
//! the `creature_mesh` profiler zone): full multi-box geometry regenerated
//! into fresh `Vec`s and re-uploaded for every creature, every frame, even
//! when nothing about that creature changed. The documented fix is
//! dirty-flag caching — rebuild + re-upload only what actually moved.
//!
//! That integration touches a ~600-line block in the fullgame bin mixing
//! roamers, test creatures, LAN remote avatars, and the player's own
//! character — a hot render path with no way to visually verify here (no
//! GPU/display in this sandbox), and this project's own history
//! (r175b/r182b/r234/r245) shows blind edits to exactly this kind of code
//! landing wrong. So this module is deliberately scoped to the SAFE part:
//! a pure, fully-tested "does creature N need remeshing this frame"
//! decision. Wiring it into the real block (caching the built `Vec`s,
//! reusing them via `.clear()` instead of `Vec::new()`, skipping the
//! re-upload call) is mechanical once this exists, but needs a run to
//! confirm nothing visibly freezes — left as a sketch, not applied.
//!
//! Quantization matters here: comparing `f32` pose fields directly would
//! rebuild every frame from float noise (physics/anim never lands on
//! exact repeats). Positions and angles are quantized to a coarse grid —
//! sub-visual jitter collapses to "unchanged," matching the same
//! philosophy as the project's mob-band tick cadence (visually
//! indistinguishable work gets skipped, not almost-skipped).

use std::collections::HashMap;

/// Quantization step for position (world units). 1/16 block — finer than
/// any visible mesh seam, coarse enough to absorb physics-solver float
/// noise on a "resting" creature.
const POS_QUANT: f32 = 1.0 / 16.0;
/// Quantization step for angles (radians). ~0.3 degrees.
const ANGLE_QUANT: f32 = 0.005;
/// Quantization step for the animation clock (seconds). Matches the
/// "animated parts on a coarse 2-3 frame cadence" note in the finding doc
/// at 60fps (~1/24s ≈ every other frame skipped for anim-only drift).
const ANIM_QUANT: f32 = 1.0 / 24.0;

#[inline]
fn q(v: f32, step: f32) -> i64 {
    (v / step).round() as i64
}

/// A quantized snapshot of everything that can change a creature's
/// vertex geometry. Two `PoseKey`s that are `==` are guaranteed to
/// produce visually-identical meshes; this is intentionally coarser
/// than bit-exact so near-stationary creatures stop costing anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PoseKey {
    pos: (i64, i64, i64),
    yaw: i64,
    pitch: i64,
    roll: i64,
    gait_phase: i64,
    torso_bend: i64,
    anim_time: i64,
    tint: (i64, i64, i64),
}

impl PoseKey {
    pub fn new(
        pos: [f32; 3],
        yaw: f32,
        pitch: f32,
        roll: f32,
        gait_phase: f32,
        torso_bend: f32,
        anim_time: f32,
        tint: [f32; 3],
    ) -> Self {
        Self {
            pos: (q(pos[0], POS_QUANT), q(pos[1], POS_QUANT), q(pos[2], POS_QUANT)),
            yaw: q(yaw, ANGLE_QUANT),
            pitch: q(pitch, ANGLE_QUANT),
            roll: q(roll, ANGLE_QUANT),
            gait_phase: q(gait_phase, ANGLE_QUANT),
            torso_bend: q(torso_bend, ANGLE_QUANT),
            anim_time: q(anim_time, ANIM_QUANT),
            tint: (q(tint[0], ANGLE_QUANT), q(tint[1], ANGLE_QUANT), q(tint[2], ANGLE_QUANT)),
        }
    }
}

/// Per-creature dirty tracker. Caller owns one instance for the frame's
/// full creature set (roamers + test creatures + remote avatars can share
/// it if their ids don't collide, or each source can own its own).
#[derive(Debug, Default)]
pub struct MeshDirtyTracker<Id: std::hash::Hash + Eq + Copy> {
    last: HashMap<Id, PoseKey>,
}

impl<Id: std::hash::Hash + Eq + Copy> MeshDirtyTracker<Id> {
    pub fn new() -> Self {
        Self { last: HashMap::new() }
    }

    /// Returns true iff this creature's mesh must be rebuilt this frame
    /// (new id, or pose changed beyond quantization), and records the new
    /// key either way. Call once per creature per frame, in the same
    /// pass that would otherwise unconditionally rebuild it.
    pub fn should_rebuild(&mut self, id: Id, key: PoseKey) -> bool {
        match self.last.insert(id, key) {
            None => true,             // never seen -> must build
            Some(prev) => prev != key, // changed -> rebuild; same -> skip
        }
    }

    /// Drop a creature's cached key (call on despawn/death so a future
    /// id reuse — if the id space is ever recycled — doesn't compare
    /// against stale state). Safe to skip if ids are never reused.
    pub fn forget(&mut self, id: Id) {
        self.last.remove(&id);
    }

    /// Force every creature to rebuild next frame (e.g. a debug-render
    /// setting changed and would otherwise never be picked up since it's
    /// not part of `PoseKey`).
    pub fn invalidate_all(&mut self) {
        self.last.clear();
    }

    pub fn tracked_count(&self) -> usize {
        self.last.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(pos: [f32; 3], anim_time: f32) -> PoseKey {
        PoseKey::new(pos, 0.0, 0.0, 0.0, 0.0, 0.0, anim_time, [1.0, 1.0, 1.0])
    }

    #[test]
    fn first_sighting_always_rebuilds() {
        let mut t: MeshDirtyTracker<u32> = MeshDirtyTracker::new();
        assert!(t.should_rebuild(1, key([0.0, 0.0, 0.0], 0.0)));
    }

    #[test]
    fn unchanged_pose_skips_second_frame() {
        let mut t: MeshDirtyTracker<u32> = MeshDirtyTracker::new();
        let k = key([10.0, 5.0, -3.0], 1.2);
        assert!(t.should_rebuild(1, k)); // frame 1: new
        assert!(!t.should_rebuild(1, k)); // frame 2: identical -> skip
        assert!(!t.should_rebuild(1, k)); // frame 3: still identical -> skip
    }

    #[test]
    fn real_movement_triggers_rebuild() {
        let mut t: MeshDirtyTracker<u32> = MeshDirtyTracker::new();
        t.should_rebuild(1, key([0.0, 0.0, 0.0], 0.0));
        assert!(t.should_rebuild(1, key([1.0, 0.0, 0.0], 0.0)), "a full block of movement must rebuild");
    }

    #[test]
    fn sub_visual_jitter_does_not_trigger_rebuild() {
        let mut t: MeshDirtyTracker<u32> = MeshDirtyTracker::new();
        t.should_rebuild(1, key([0.0, 0.0, 0.0], 0.0));
        // Physics-solver float noise: a resting creature's position
        // drifts by a fraction of a millimeter-scale unit between frames.
        let jitter = key([0.0001, -0.0002, 0.00005], 0.0);
        assert!(!t.should_rebuild(1, jitter), "float noise under the quantization step must not force a rebuild");
    }

    #[test]
    fn animation_clock_advancing_triggers_rebuild_at_the_configured_cadence() {
        let mut t: MeshDirtyTracker<u32> = MeshDirtyTracker::new();
        t.should_rebuild(1, key([0.0, 0.0, 0.0], 0.0));
        // Half an ANIM_QUANT step: still the same quantized bucket.
        assert!(!t.should_rebuild(1, key([0.0, 0.0, 0.0], ANIM_QUANT * 0.4)));
        // A full step: crosses into the next bucket -> rebuild.
        assert!(t.should_rebuild(1, key([0.0, 0.0, 0.0], ANIM_QUANT * 1.1)));
    }

    #[test]
    fn independent_creatures_tracked_independently() {
        let mut t: MeshDirtyTracker<u32> = MeshDirtyTracker::new();
        t.should_rebuild(1, key([0.0, 0.0, 0.0], 0.0));
        t.should_rebuild(2, key([0.0, 0.0, 0.0], 0.0));
        // Moving creature 1 must not mark creature 2 dirty.
        assert!(t.should_rebuild(1, key([5.0, 0.0, 0.0], 0.0)));
        assert!(!t.should_rebuild(2, key([0.0, 0.0, 0.0], 0.0)));
        assert_eq!(t.tracked_count(), 2);
    }

    #[test]
    fn forget_then_resight_rebuilds() {
        let mut t: MeshDirtyTracker<u32> = MeshDirtyTracker::new();
        let k = key([0.0, 0.0, 0.0], 0.0);
        t.should_rebuild(1, k);
        assert!(!t.should_rebuild(1, k));
        t.forget(1);
        assert!(t.should_rebuild(1, k), "after forget, even the same pose must rebuild once");
    }

    #[test]
    fn invalidate_all_forces_every_creature_to_rebuild_once() {
        let mut t: MeshDirtyTracker<u32> = MeshDirtyTracker::new();
        let k = key([0.0, 0.0, 0.0], 0.0);
        t.should_rebuild(1, k);
        t.should_rebuild(2, k);
        t.invalidate_all();
        assert!(t.should_rebuild(1, k));
        assert!(t.should_rebuild(2, k));
    }

    #[test]
    fn realistic_population_mostly_skips_a_steady_frame() {
        // Models the r247 scenario: ~200 creatures, most idle/resting for
        // a stretch, a few moving. Confirms the tracker's actual payoff:
        // steady-state frames should rebuild only the moving minority.
        let mut t: MeshDirtyTracker<u32> = MeshDirtyTracker::new();
        for id in 0..200u32 {
            t.should_rebuild(id, key([id as f32, 0.0, 0.0], 0.0)); // seed frame 1
        }
        let mut rebuilds = 0;
        for id in 0..200u32 {
            let moving = id % 20 == 0; // 10 of 200 moving this frame
            let pos = if moving { [id as f32 + 2.0, 0.0, 0.0] } else { [id as f32, 0.0, 0.0] };
            if t.should_rebuild(id, key(pos, 0.0)) {
                rebuilds += 1;
            }
        }
        assert_eq!(rebuilds, 10, "only the moving minority should trigger a rebuild");
    }
}
