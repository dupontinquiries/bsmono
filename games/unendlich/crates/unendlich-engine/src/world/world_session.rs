//! `world::world_session` (r467) — owns the per-world SIM accumulator state so that switching worlds
//! is a single STRUCTURAL reset instead of a hand-maintained list of `.clear()` calls.
//!
//! THE BUG CLASS THIS KILLS: per-world state used to live as ~20 loose `let mut` variables in the
//! 11k-line fullgame main(), reset by a long manual block. Every new per-world feature had to
//! remember to add a `.clear()` there; forgetting one leaks the old world's data into the new world
//! (a multi-world bug). Here, `reset()` does `*self = Self::new(seed)` — the compiler forces `new()`
//! to construct EVERY field, so adding a field can never be silently missed on reset.
//!
//! Migration is incremental: the newest per-world accumulators live here now; the water/sediment/lava
//! state will move in over subsequent passes until the manual block is gone.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::sim::dragon_activity::ActivityState;
use crate::sim::dragon_bending_ai::BendIntent;
use crate::sim::dragon_hunt::HuntState;
use crate::sim::world_authority::WorldCmd;
use crate::world::chunk::ChunkCoord;
use crate::world::named_locations::LocationRegistry;

pub struct WorldSession {
    /// Seed of the world this session belongs to.
    pub seed: u64,
    /// r514/r519: persistent, sparse per-world crop AND sapling growth state — the single
    /// flora layer. Round-trips to flora.json under the world root (crops and saplings are
    /// distinguished by their block type).
    pub flora: crate::world::flora_store::FloraStore,
    /// Hot-metal thermodynamics: cell → temperature (°C).
    pub hot_metal: HashMap<[i32; 3], f32>,
    /// Discovered named locations (the Collections backing store).
    pub locations: LocationRegistry,
    /// Region the player was last in, for discovery edge-detection.
    pub current_region: Option<(i32, i32)>,
    /// Regions where wild creatures have already been spawned (so we don't respawn them).
    pub spawned_regions: HashSet<(i32, i32)>,
    /// Active fire/ember positions (smoke sources).
    pub fire_positions: Vec<[f32; 3]>,
    /// Per-dragon activity / hunt / bending-intent state, keyed by creature id.
    pub dragon_activities: HashMap<u64, ActivityState>,
    pub dragon_hunts: HashMap<u64, HuntState>,
    pub dragon_bend_intents: HashMap<u64, BendIntent>,
    /// Chunk coords already registered as lava sources (skip-guard).
    pub lava_registered: HashSet<ChunkCoord>,
    /// Outgoing world edits queued for LAN peers.
    pub lan_outgoing_edits: Vec<WorldCmd>,
    /// Chunks queued for eviction.
    pub fg_evict_queue: VecDeque<ChunkCoord>,
}

impl WorldSession {
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            flora: crate::world::flora_store::FloraStore::new(),
            hot_metal: HashMap::new(),
            locations: LocationRegistry::new(),
            current_region: None,
            spawned_regions: HashSet::new(),
            fire_positions: Vec::new(),
            dragon_activities: HashMap::new(),
            dragon_hunts: HashMap::new(),
            dragon_bend_intents: HashMap::new(),
            lava_registered: HashSet::new(),
            lan_outgoing_edits: Vec::new(),
            fg_evict_queue: VecDeque::new(),
        }
    }

    /// Reset ALL per-world state for a switch to `seed`. Implemented as a full rebuild so no field can
    /// ever be forgotten — add a field to the struct + `new()` and it is automatically reset here.
    pub fn reset(&mut self, seed: u64) {
        *self = Self::new(seed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reset_clears_everything_and_reseeds() {
        let mut s = WorldSession::new(1);
        // Populate the trivially-constructable fields; the `*self = new()` reset covers ALL fields
        // structurally, so proving these clear proves the mechanism for the rest too.
        s.hot_metal.insert([1, 2, 3], 900.0);
        s.locations.discover(0, 0, "The Test Reach".into());
        s.current_region = Some((4, 5));
        s.spawned_regions.insert((7, 8));
        s.fire_positions.push([1.0, 2.0, 3.0]);
        s.reset(42);
        assert_eq!(s.seed, 42);
        assert!(s.hot_metal.is_empty());
        assert_eq!(s.locations.count(), 0);
        assert_eq!(s.current_region, None);
        assert!(s.spawned_regions.is_empty());
        assert!(s.fire_positions.is_empty());
        // remaining accumulators start empty and are rebuilt by reset (same mechanism)
        assert!(s.dragon_hunts.is_empty() && s.dragon_activities.is_empty());
        assert!(s.dragon_bend_intents.is_empty() && s.lava_registered.is_empty());
        assert!(s.lan_outgoing_edits.is_empty() && s.fg_evict_queue.is_empty());
    }
}
