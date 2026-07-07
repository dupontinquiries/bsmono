//! Water field — 3D scalar moisture-volume field that's the
//! authoritative source for water in the world.
//!
//! ## Architectural intent
//!
//! Today the water visible in the world comes from `BlockId::Water`
//! cells placed at worldgen. That works for static seas but can't
//! represent flow, erosion, conservation, or pressure. Those are
//! ALL field-level concerns: continuous rather than discrete.
//!
//! This module introduces water as a SCALAR FIELD: per-cell volume
//! in [0, 1], stored in a sparse 3D grid keyed by world block coord.
//! The field is the simulation layer:
//!   - Flow: water settles to lower-volume neighbors over time.
//!   - Erosion: cells the water passes through accumulate weathering.
//!   - Conservation: total volume is preserved across moves.
//!   - Pressure: hydrostatic (deeper cells push harder).
//!
//! The block representation is kept TEMPORARILY for collision/physics
//! interaction. The field and block layers are SYNCED at worldgen
//! time (any BlockId::Water cell gets field value 1.0). Eventually:
//!   - Mesher renders water surface from the FIELD, not from blocks.
//!   - Physics queries the field for "is the player in water" rather
//!     than the block.
//!   - BlockId::Water remains as a render-only marker (or is removed
//!     entirely — the field is enough).
//!
//! The migration is incremental. Today: field is populated alongside
//! blocks but unused by the mesher. Future sessions wire it in.
//!
//! ## Why a sparse grid
//!
//! Most of the world has zero water. A dense per-chunk array would
//! waste 32×32×32 floats (128KB) per chunk × thousands of chunks =
//! gigabytes for nothing. Sparse: only cells with non-zero water
//! consume memory. A surface ocean at a flat seabed has perhaps
//! 32×32 = 1024 entries per chunk slice — manageable.
//!
//! The sparse representation is `BTreeMap<u16, u16>`: same as
//! `BlockInstanceState`'s `instance_state` map. Key is the local
//! cell index (0..32768), value is volume × 65535 (16-bit fixed
//! point: gives 0.000015 resolution, plenty).
//!
//! ## What's NOT here yet
//!
//! - The simulation tick (flow / erosion / conservation). Skeleton
//!   only with TODO comments. Coming in a follow-up session.
//! - The mesher integration. Mesher still reads from BlockId::Water.
//! - Physics integration. Player still queries blocks.

use std::collections::BTreeMap;

/// Scaling factor for fixed-point storage: f32 in [0,1] becomes u16
/// in [0, FIXED_POINT_SCALE]. Gives 1/65535 ≈ 0.000015 resolution,
/// well below any visible / gameplay-relevant difference.
pub const FIXED_POINT_SCALE: u16 = 65535;

/// Per-chunk sparse water volume map. Keyed by chunk-local block index
/// (matches the `BlockInstanceState` indexing convention).
///
/// Values are 16-bit fixed point in [0, FIXED_POINT_SCALE].
/// Cells not present in the map have volume 0 (dry).
#[derive(Debug, Clone, Default)]
pub struct ChunkWaterField {
    cells: BTreeMap<u16, u16>,
}

impl ChunkWaterField {
    pub fn empty() -> Self { Self::default() }

    /// Number of cells with non-zero water in this chunk.
    pub fn cell_count(&self) -> usize { self.cells.len() }

    /// Read the water volume at this local cell index. Returns 0.0 for
    /// cells not in the sparse map.
    pub fn get(&self, idx: u16) -> f32 {
        match self.cells.get(&idx) {
            Some(&v) => v as f32 / FIXED_POINT_SCALE as f32,
            None => 0.0,
        }
    }

    /// Write water volume at this local cell index. Volume is clamped to
    /// [0, 1]. Setting to 0 removes the entry from the sparse map (keeps
    /// memory tight — empty cells don't consume storage).
    pub fn set(&mut self, idx: u16, volume: f32) {
        let clamped = volume.clamp(0.0, 1.0);
        if clamped == 0.0 {
            self.cells.remove(&idx);
        } else {
            let v = (clamped * FIXED_POINT_SCALE as f32).round() as u16;
            self.cells.insert(idx, v);
        }
    }

    /// Total water volume across all cells in this chunk. Useful for
    /// conservation tests (volume in == volume out + volume retained).
    pub fn total_volume(&self) -> f32 {
        let sum: u64 = self.cells.values().map(|&v| v as u64).sum();
        sum as f32 / FIXED_POINT_SCALE as f32
    }

    /// Iterator over all non-zero (idx, volume) pairs.
    pub fn iter(&self) -> impl Iterator<Item = (u16, f32)> + '_ {
        self.cells.iter()
            .map(|(&idx, &v)| (idx, v as f32 / FIXED_POINT_SCALE as f32))
    }
}

/// Stub for the per-tick water simulation.
///
/// Future logic (in priority order):
///
/// 1. **Settle / flow**: any cell with volume > 0 and a lower-volume
///    neighbor BELOW it transfers some volume downward (gravity).
///    Then any cell with no air-or-equal-or-greater-volume neighbor
///    above transfers laterally to the lowest adjacent neighbor.
///    Conservation: total volume change across all cells == 0.
///
/// 2. **Erosion**: each tick, cells with non-zero water "passing
///    through" them (volume changing across the tick) increment
///    weathering of any adjacent solid block. Wet stone weathers
///    faster than dry stone. Eventually stone → gravel → sand.
///
/// 3. **Evaporation**: cells with high temperature and exposed sky
///    lose a tiny fraction of volume per tick, returned to the
///    atmospheric moisture field (cloud module).
///
/// 4. **Source / sink**: cells with rain accumulate volume from the
///    cloud system; cells with sun on dry soil lose to it. Water
///    cycle closes.
///
/// For NOW this function is a no-op. Just establishes the API
/// signature so the tick framework can call it without changing
/// when the real implementation lands.
pub fn tick_chunk_water(_field: &mut ChunkWaterField, _dt_seconds: f32) {
    // Intentionally empty. See doc comment for what goes here.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_field_has_zero_volume() {
        let f = ChunkWaterField::empty();
        assert_eq!(f.total_volume(), 0.0);
        assert_eq!(f.cell_count(), 0);
    }

    #[test]
    fn set_and_get_round_trips() {
        let mut f = ChunkWaterField::empty();
        f.set(100, 0.5);
        let v = f.get(100);
        assert!((v - 0.5).abs() < 1e-4, "set 0.5, got {}", v);
    }

    #[test]
    fn unset_cells_read_as_zero() {
        let f = ChunkWaterField::empty();
        for idx in [0, 1, 100, 32767] {
            assert_eq!(f.get(idx), 0.0);
        }
    }

    #[test]
    fn setting_zero_removes_entry() {
        let mut f = ChunkWaterField::empty();
        f.set(50, 0.7);
        assert_eq!(f.cell_count(), 1);
        f.set(50, 0.0);
        assert_eq!(f.cell_count(), 0);
    }

    #[test]
    fn volume_clamps_to_unit_range() {
        let mut f = ChunkWaterField::empty();
        f.set(0, 5.0);
        assert_eq!(f.get(0), 1.0); // clamped up
        f.set(1, -3.0);
        assert_eq!(f.get(1), 0.0); // clamped down (and removed)
        assert_eq!(f.cell_count(), 1); // only cell 0 remains
    }

    /// Total volume == sum of cell volumes. This is the invariant the
    /// future simulation must preserve (mass conservation).
    #[test]
    fn total_volume_sums_cells() {
        let mut f = ChunkWaterField::empty();
        f.set(0, 1.0);
        f.set(1, 0.5);
        f.set(2, 0.25);
        let total = f.total_volume();
        assert!((total - 1.75).abs() < 1e-3,
            "expected total 1.75, got {}", total);
    }

    /// The tick stub is a no-op today: total volume must be unchanged
    /// after a tick. Once the real simulation lands, the volume MIGHT
    /// change due to evaporation/rain — that test will need updating.
    /// For now it verifies the stub doesn't accidentally lose mass.
    #[test]
    fn tick_stub_preserves_volume() {
        let mut f = ChunkWaterField::empty();
        f.set(10, 0.3);
        f.set(20, 0.7);
        let before = f.total_volume();
        tick_chunk_water(&mut f, 0.5);
        let after = f.total_volume();
        assert!((before - after).abs() < 1e-6,
            "tick stub leaked volume: before={}, after={}", before, after);
    }

    /// Iteration yields exactly the non-zero cells, in sorted order
    /// (BTreeMap's contract). Used by future flow code that needs
    /// deterministic iteration order for reproducibility.
    #[test]
    fn iter_yields_sorted_nonzero_cells() {
        let mut f = ChunkWaterField::empty();
        f.set(50, 0.5);
        f.set(10, 0.1);
        f.set(100, 1.0);
        let collected: Vec<_> = f.iter().collect();
        let indices: Vec<u16> = collected.iter().map(|&(i, _)| i).collect();
        assert_eq!(indices, vec![10, 50, 100], "iteration must be sorted");
    }
}
