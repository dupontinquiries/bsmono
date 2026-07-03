//! `world::flora_store` — persistent, sparse per-world flora & crop state.
//!
//! Planted crops (and, in time, saplings and other tended plants) are *entities
//! with permanent state*: a specific world cell holds a specific plant at a
//! specific point in its growth, and that must survive save/load like any other
//! world edit. Storing a growth value for *every* world cell would be absurd, so
//! this store is **sparse** — a map keyed by the few cells that actually hold a
//! tracked plant. Empty space costs nothing.
//!
//! On disk it serialises to a single `flora.json` under the world root (a flat
//! list of `(cell, state)` pairs — JSON object keys can't be arrays, so the map
//! is flattened for storage and rebuilt on load), written atomically via a
//! temp-file rename so a crash mid-save can't corrupt the live file. This mirrors
//! how `crate::storage` persists chunk edits.
//!
//! The runtime structure is a `HashMap<[i32; 3], FloraCell>` so plant / harvest /
//! lookup are all O(1) by position — the growth tick and the break/harvest path
//! both address a single cell directly instead of scanning a list.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

/// The permanent state of one tended plant occupying one world cell.
///
/// `block` is the `BlockId` discriminant (`as u8`) of the plant — enough to know
/// what grows here and what it drops. The growth fields mirror
/// [`crate::sim::plant_growth::PlantState`] so the store and the growth core
/// round-trip losslessly. `planted_tick` records when it was sown (frame count),
/// for age-based effects and debugging.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct FloraCell {
    pub block: u8,
    pub growth01: f32,
    pub stage: u8,
    pub element: u8,
    pub vigor: f32,
    pub planted_tick: u64,
    /// r522: this seed's genome — the 13-gene array from `sim::plant_tree_genetics::PlantGenome`
    /// (max_height, fruitfulness, growth_rate, hardiness, …). Persisted with the plant so its
    /// traits survive save/load and pass to harvested seed. Neutral (0.5) = an unbred plant.
    #[serde(default = "neutral_genome")]
    pub genome: [f32; 13],
}

fn neutral_genome() -> [f32; 13] {
    [0.5; 13]
}

impl FloraCell {
    /// A freshly-sown plant of `block`, just-planted at `tick`.
    pub fn sown(block: u8, tick: u64) -> Self {
        Self {
            block,
            growth01: 0.02,
            stage: 0,
            element: 0,
            vigor: 0.6,
            planted_tick: tick,
            genome: neutral_genome(),
        }
    }
    /// True once the plant has reached the final growth stage and can be harvested
    /// for a yield.
    pub fn is_ripe(&self) -> bool {
        self.stage >= crate::sim::plant_growth::FINAL_STAGE
    }
}

/// Sparse, serialisable map of every tracked plant in the world.
#[derive(Default)]
pub struct FloraStore {
    cells: HashMap<[i32; 3], FloraCell>,
    /// Set whenever the contents change; lets the host save only when needed.
    dirty: bool,
}

impl FloraStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }
    pub fn contains(&self, pos: [i32; 3]) -> bool {
        self.cells.contains_key(&pos)
    }
    pub fn get(&self, pos: [i32; 3]) -> Option<&FloraCell> {
        self.cells.get(&pos)
    }

    /// Sow / overwrite the plant at `pos`.
    pub fn plant(&mut self, pos: [i32; 3], cell: FloraCell) {
        self.cells.insert(pos, cell);
        self.dirty = true;
    }

    /// Remove and return the plant at `pos` (e.g. on harvest or destruction).
    pub fn remove(&mut self, pos: [i32; 3]) -> Option<FloraCell> {
        let r = self.cells.remove(&pos);
        if r.is_some() {
            self.dirty = true;
        }
        r
    }

    /// Mutable iteration for the growth tick.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&[i32; 3], &mut FloraCell)> {
        self.cells.iter_mut()
    }
    pub fn iter(&self) -> impl Iterator<Item = (&[i32; 3], &FloraCell)> {
        self.cells.iter()
    }

    /// Drop every cell for which `keep` returns false (e.g. the host block was
    /// mined or replaced). Marks dirty if anything was removed.
    pub fn retain(&mut self, mut keep: impl FnMut(&[i32; 3], &FloraCell) -> bool) {
        let before = self.cells.len();
        self.cells.retain(|k, v| keep(k, v));
        if self.cells.len() != before {
            self.dirty = true;
        }
    }

    /// The growth tick mutates state in place; call this so a later save persists
    /// the advanced growth.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }
    /// Consume the dirty flag — returns whether a save is warranted.
    pub fn take_dirty(&mut self) -> bool {
        let d = self.dirty;
        self.dirty = false;
        d
    }

    fn store_path(world_root: &Path) -> PathBuf {
        world_root.join("flora.json")
    }

    /// Persist atomically (temp file + rename) under `world_root`.
    pub fn save(&self, world_root: &Path) -> io::Result<()> {
        let flat: Vec<([i32; 3], FloraCell)> =
            self.cells.iter().map(|(k, v)| (*k, *v)).collect();
        let path = Self::store_path(world_root);
        let tmp = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec(&flat)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Load from `world_root`, or an empty store if absent / unreadable.
    pub fn load(world_root: &Path) -> Self {
        let path = Self::store_path(world_root);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(_) => return Self::new(),
        };
        match serde_json::from_slice::<Vec<([i32; 3], FloraCell)>>(&bytes) {
            Ok(flat) => Self { cells: flat.into_iter().collect(), dirty: false },
            Err(_) => Self::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plant_get_remove() {
        let mut s = FloraStore::new();
        assert!(s.is_empty());
        s.plant([1, 2, 3], FloraCell::sown(88, 10));
        assert_eq!(s.len(), 1);
        assert!(s.contains([1, 2, 3]));
        assert_eq!(s.get([1, 2, 3]).unwrap().block, 88);
        let removed = s.remove([1, 2, 3]).unwrap();
        assert_eq!(removed.block, 88);
        assert!(s.is_empty());
    }

    #[test]
    fn dirty_tracking() {
        let mut s = FloraStore::new();
        assert!(!s.take_dirty());
        s.plant([0, 0, 0], FloraCell::sown(89, 1));
        assert!(s.take_dirty());
        assert!(!s.take_dirty()); // consumed
    }

    #[test]
    fn roundtrip_through_disk() {
        let dir = std::env::temp_dir().join(format!("flora_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut s = FloraStore::new();
        s.plant([5, 64, -7], FloraCell::sown(88, 100));
        s.plant([6, 64, -7], FloraCell { block: 90, growth01: 0.5, stage: 2, element: 0, vigor: 0.7, planted_tick: 50, genome: [0.5; 13] });
        s.save(&dir).unwrap();

        let loaded = FloraStore::load(&dir);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.get([5, 64, -7]).unwrap().block, 88);
        let c = loaded.get([6, 64, -7]).unwrap();
        assert_eq!(c.stage, 2);
        assert!((c.growth01 - 0.5).abs() < 1e-6);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_absent_is_empty() {
        let dir = std::env::temp_dir().join("flora_test_does_not_exist_xyz");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(FloraStore::load(&dir).is_empty());
    }
}
