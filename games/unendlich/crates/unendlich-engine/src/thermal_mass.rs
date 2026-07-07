//! Thermal mass — how strongly a cell resists temperature change.
//!
//! ## What this replaces
//!
//! Earlier worldgen used a height heuristic to decide which water
//! cells freeze: "if the cell is at the surface AND temperature ≤ 0°C,
//! place Ice instead of Water." That's a crude approximation of what
//! actually happens physically, and it's wrong in obvious ways:
//!
//!   - A 1-cell-deep puddle at 1°C above sea level should freeze
//!     entirely — but the height check excludes it.
//!   - A surface cell of a deep ocean at -50°C should freeze on top —
//!     the height check handles that, but only because of a coincidence
//!     of altitude, not the real reason (heat-loss-vs-thermal-mass).
//!   - A water cell embedded in stone (e.g. a sealed underground
//!     pocket) should resist freezing more than open water at the
//!     same depth — the height check ignores this entirely.
//!
//! ## The principle
//!
//! A cell's resistance to phase change is proportional to the
//! THERMAL MASS of the connected blob it belongs to, weighted by
//! the heat-loss surface area. A small isolated puddle freezes
//! instantly; a large deep lake freezes only on top; water trapped
//! in stone resists freezing because the stone insulates.
//!
//! Computing TRUE connected thermal mass requires a flood-fill that
//! crosses chunk boundaries. That's expensive and worth doing
//! eventually (in the water_field module's tick handler) but
//! overkill for worldgen.
//!
//! ## The cheap proxy
//!
//! For each candidate cell, look at its 6 immediate neighbors. Each
//! contributes to "thermal mass" by its material:
//!
//!   - Water/ice: high contribution (large heat reservoir).
//!   - Solid (stone/dirt/sand): moderate contribution (insulator).
//!   - Air: zero contribution (infinite heat sink — the cell
//!     LOSES heat through air faces).
//!
//! Direction matters too. The +Y face (top) loses heat fastest in
//! reality because warm air rises and cold air sinks. So an air
//! neighbor on the +Y face counts as MORE heat-loss than air on
//! a side face.
//!
//! The result is a unitless `thermal_mass_factor` in [0, 1]:
//!
//!   - 0.0 = single isolated cell with air on all 6 sides → freezes
//!     at any sub-zero temperature.
//!   - 1.0 = cell completely surrounded by water/ice → resists
//!     freezing strongly; only freezes well below zero.
//!
//! ## Putting it together
//!
//! The freezing rule combines local temperature with thermal mass:
//!
//!     freezes = temperature_C < THRESHOLD_C * thermal_mass_factor
//!
//! where THRESHOLD_C is a tuning constant (today: -8°C). At
//! mass=0, ANY sub-zero temperature freezes the cell. At mass=1,
//! the cell needs -8°C or colder to freeze.
//!
//! This produces visually correct behavior:
//!   - Ocean surfaces at cold latitudes freeze (top exposed to air).
//!   - Ocean depths stay liquid (mass=1, doesn't reach -8°C inland).
//!   - Inland puddles in the same biome freeze through (low mass).
//!   - Sealed water pockets stay liquid (mass=high, stone insulates).

use crate::world::blocks::BlockId;

/// Function signature for "what's at this world position?" Used to
/// avoid coupling this module to a specific chunk/block storage.
/// Worldgen passes a closure that reads from its in-progress block
/// array; runtime callers pass a closure that queries `ChunkManager`.
///
/// Returns BlockId::Air for any out-of-range cell. Out-of-chunk
/// reads during worldgen are treated conservatively as air — this
/// makes a chunk's edge cells appear MORE freezable than the
/// interior, which is acceptable: by the time the neighbor chunk
/// generates, this cell will already have been written. (For
/// production runtime queries that have access to neighbor chunks,
/// the closure can return real values.)
pub type BlockSampler<'a> = &'a dyn Fn(i32, i32, i32) -> BlockId;

/// Per-direction weights for heat-loss calculation. Order is
/// `[+X, -X, +Y, -Y, +Z, -Z]`. The +Y face is weighted heaviest
/// because warm air rises (a real thermodynamic effect): the
/// top of a body of water loses heat ~2x faster than the sides.
const FACE_HEAT_LOSS_WEIGHTS: [f32; 6] = [
    1.0, // +X
    1.0, // -X
    2.0, // +Y (top — fastest heat loss)
    0.5, // -Y (bottom — slowest heat loss; insulated by water/earth)
    1.0, // +Z
    1.0, // -Z
];

/// Per-material contribution to thermal mass. A water cell with a
/// water neighbor contributes the most (heat reservoir effect); a
/// solid neighbor contributes moderately (insulator); air contributes
/// nothing (heat sink).
fn material_contribution(b: BlockId) -> f32 {
    use BlockId::*;
    match b {
        Air => 0.0,
        // Water and ice: large reservoir. Note ice-on-water also
        // counts — a partially-frozen lake's interior cells still
        // benefit from the volume below.
        Water | Ice => 1.0,
        // Lava: tracks differently. Lava is a HEAT source, so
        // adjacent water shouldn't freeze even at cold temperatures.
        // We give lava a contribution of 1.0 here (acts as reservoir
        // resisting cooling). The actual heat input is via
        // heat_source.rs; this just prevents the freezing rule from
        // kicking in adjacent to lava.
        Lava => 1.0,
        // Solid blocks: insulators. They don't add reservoir mass
        // but they slow heat loss to the air beyond them. Treat
        // as moderate contribution.
        _ => 0.5,
    }
}

/// Sample the thermal mass factor at this world position, given a
/// block-sampling closure. Returns a value in [0, 1].
///
/// 0.0 = no thermal mass (cell freezes at any cold temperature).
/// 1.0 = full thermal mass (resists freezing strongly).
pub fn thermal_mass_factor(
    wx: i32, wy: i32, wz: i32,
    sample: BlockSampler,
) -> f32 {
    // Six immediate face neighbors.
    let neighbors = [
        sample(wx + 1, wy, wz),  // +X
        sample(wx - 1, wy, wz),  // -X
        sample(wx, wy + 1, wz),  // +Y (top)
        sample(wx, wy - 1, wz),  // -Y (bottom)
        sample(wx, wy, wz + 1),  // +Z
        sample(wx, wy, wz - 1),  // -Z
    ];

    // Weighted contribution. We compute a weighted SUM and divide by
    // the total possible weighted sum to get a value in [0, 1].
    //
    //     factor = Σ(weight_i * contribution_i) / Σ(weight_i)
    //
    // Maximum is achieved when every neighbor has contribution 1.0
    // (water/ice all around). Minimum (0) is when every neighbor is
    // air.
    let mut weighted_contribution = 0.0_f32;
    let mut total_weight = 0.0_f32;
    for i in 0..6 {
        let weight = FACE_HEAT_LOSS_WEIGHTS[i];
        let contribution = material_contribution(neighbors[i]);
        weighted_contribution += weight * contribution;
        total_weight += weight;
    }
    (weighted_contribution / total_weight).clamp(0.0, 1.0)
}

/// Combine local temperature with thermal mass to decide whether
/// this water cell would freeze. Returns true if it freezes (place
/// Ice), false otherwise (place Water).
///
/// Mathematical model:
///
///     freezes = temperature_C < FREEZE_BASELINE_C - mass * MASS_BUFFER_C
///
/// where:
///   - FREEZE_BASELINE_C = 0°C (water's freezing point at 1 atm).
///   - MASS_BUFFER_C = depth of cold required for full-mass cell to
///     freeze. Higher = more reluctance from thermal-mass-protected
///     cells. Tuned to 8°C: a cell with mass=1 (deep interior of a
///     lake) needs -8°C to freeze, which only happens at very cold
///     latitudes/altitudes; a cell with mass=0 (isolated puddle)
///     freezes at any sub-zero temperature.
pub fn freezes_at(
    wx: i32, wy: i32, wz: i32,
    sample: BlockSampler,
) -> bool {
    const FREEZE_BASELINE_C: f32 = 0.0;
    const MASS_BUFFER_C: f32 = 8.0;
    let temp = crate::temperature::temperature_at(wx, wy, wz);
    let mass = thermal_mass_factor(wx, wy, wz, sample);
    let freeze_threshold = FREEZE_BASELINE_C - mass * MASS_BUFFER_C;
    temp < freeze_threshold
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: a sampler that always returns the same block. Useful
    /// for testing "what if everything around the cell is X?"
    fn const_sampler(b: BlockId) -> impl Fn(i32, i32, i32) -> BlockId {
        move |_, _, _| b
    }

    /// Helper: sampler from a small HashMap of (pos -> BlockId);
    /// missing positions return Air.
    fn hash_sampler(
        cells: std::collections::HashMap<(i32, i32, i32), BlockId>,
    ) -> impl Fn(i32, i32, i32) -> BlockId {
        move |x, y, z| cells.get(&(x, y, z)).copied().unwrap_or(BlockId::Air)
    }

    /// Cell with all-air neighbors → factor 0.0 (no thermal mass).
    #[test]
    fn isolated_cell_has_no_mass() {
        let sample = const_sampler(BlockId::Air);
        let m = thermal_mass_factor(0, 0, 0, &sample);
        assert!(m < 0.01, "isolated cell should have ~0 mass, got {}", m);
    }

    /// Cell surrounded by water → factor 1.0 (full thermal mass).
    #[test]
    fn fully_submerged_cell_has_full_mass() {
        let sample = const_sampler(BlockId::Water);
        let m = thermal_mass_factor(0, 0, 0, &sample);
        assert!((m - 1.0).abs() < 0.01,
            "fully submerged cell should have ~1.0 mass, got {}", m);
    }

    /// Cell surrounded by stone → moderate mass (insulator effect).
    #[test]
    fn stone_walled_cell_has_moderate_mass() {
        let sample = const_sampler(BlockId::Stone);
        let m = thermal_mass_factor(0, 0, 0, &sample);
        assert!(m > 0.4 && m < 0.6,
            "stone-walled cell should have moderate mass, got {}", m);
    }

    /// Top-air-only cell (water with air ONLY above) — should have
    /// reduced mass because the +Y face weights heavier in heat loss.
    #[test]
    fn top_air_reduces_mass_more_than_side_air() {
        // Cell at (0,0,0) with water everywhere except +Y (air above).
        let mut cells = std::collections::HashMap::new();
        cells.insert((1, 0, 0), BlockId::Water);
        cells.insert((-1, 0, 0), BlockId::Water);
        // (0, 1, 0) NOT inserted → defaults to Air.
        cells.insert((0, -1, 0), BlockId::Water);
        cells.insert((0, 0, 1), BlockId::Water);
        cells.insert((0, 0, -1), BlockId::Water);
        let sample_top_air = hash_sampler(cells);
        let m_top = thermal_mass_factor(0, 0, 0, &sample_top_air);

        // Same setup but air on +X side instead of +Y.
        let mut cells2 = std::collections::HashMap::new();
        // (1, 0, 0) NOT inserted → defaults to Air.
        cells2.insert((-1, 0, 0), BlockId::Water);
        cells2.insert((0, 1, 0), BlockId::Water);
        cells2.insert((0, -1, 0), BlockId::Water);
        cells2.insert((0, 0, 1), BlockId::Water);
        cells2.insert((0, 0, -1), BlockId::Water);
        let sample_side_air = hash_sampler(cells2);
        let m_side = thermal_mass_factor(0, 0, 0, &sample_side_air);

        // Top-air should give LESS mass (more heat loss) than side-air.
        assert!(m_top < m_side,
            "top-air ({}) should reduce mass more than side-air ({})",
            m_top, m_side);
    }

    /// Freezing rule: an isolated puddle (no neighbors) freezes at
    /// any sub-zero temperature.
    #[test]
    fn isolated_puddle_freezes_at_any_subzero() {
        // Ensure we're at a position where temperature_at is around -1°C.
        // We don't control temperature_at precisely, so we test the
        // RELATIVE behavior — at the polar Z, an isolated cell freezes
        // and a fully-submerged one doesn't.
        let polar_z = (crate::temperature::LATITUDE_SPAN as i32) / 2;
        let sample_isolated = const_sampler(BlockId::Air);
        let sample_submerged = const_sampler(BlockId::Water);
        // Pick a moderately-cold latitude, slightly inland.
        // Surface Y=64 at polar_z is around -3°C.
        let cold_freezes = freezes_at(0, 64, polar_z, &sample_isolated);
        let cold_resists = freezes_at(0, 64, polar_z, &sample_submerged);
        assert!(cold_freezes,
            "isolated cell at polar latitude should freeze");
        // Submerged at moderately-cold (-3°C) should NOT freeze yet
        // because the threshold is -8°C for full mass.
        assert!(!cold_resists,
            "fully-submerged cell at moderately-cold latitude should NOT freeze");
    }

    /// Both cells freeze at very cold temperatures (high altitude
    /// where lapse rate brings temp well below the deep-water
    /// threshold).
    #[test]
    fn very_cold_freezes_even_with_full_mass() {
        // High altitude + polar latitude → very cold.
        let polar_z = (crate::temperature::LATITUDE_SPAN as i32) / 2;
        let sample_submerged = const_sampler(BlockId::Water);
        // At Y=2000 (way above sea level), lapse rate gives ~-13°C
        // additional drop. At polar Z, base is already -3°C. Total
        // around -16°C, below the -8°C threshold for full mass.
        let frozen = freezes_at(0, 2000, polar_z, &sample_submerged);
        assert!(frozen, "very-cold full-mass cell should freeze");
    }

    /// Warm temperatures don't freeze ANY cell, regardless of mass.
    #[test]
    fn warm_doesnt_freeze_anything() {
        let sample_isolated = const_sampler(BlockId::Air);
        let sample_submerged = const_sampler(BlockId::Water);
        // Equator at sea level → ~22°C.
        assert!(!freezes_at(0, 64, 0, &sample_isolated),
            "isolated cell at equator should NOT freeze");
        assert!(!freezes_at(0, 64, 0, &sample_submerged),
            "submerged cell at equator should NOT freeze");
    }

    /// Mass factor is monotonic: more water neighbors → more mass.
    /// 1 water neighbor < 3 water neighbors < 5 water neighbors.
    #[test]
    fn mass_grows_with_water_neighbor_count() {
        fn make_sampler(water_dirs: &[(i32, i32, i32)]) -> impl Fn(i32, i32, i32) -> BlockId + '_ {
            move |x, y, z| {
                if water_dirs.iter().any(|&(dx, dy, dz)| (dx, dy, dz) == (x, y, z)) {
                    BlockId::Water
                } else {
                    BlockId::Air
                }
            }
        }
        let one = make_sampler(&[(1, 0, 0)]);
        let three = make_sampler(&[(1, 0, 0), (-1, 0, 0), (0, 0, 1)]);
        let five = make_sampler(&[(1, 0, 0), (-1, 0, 0), (0, 0, 1), (0, 0, -1), (0, -1, 0)]);
        let m1 = thermal_mass_factor(0, 0, 0, &one);
        let m3 = thermal_mass_factor(0, 0, 0, &three);
        let m5 = thermal_mass_factor(0, 0, 0, &five);
        assert!(m1 < m3, "1 neighbor ({}) should be less than 3 ({})", m1, m3);
        assert!(m3 < m5, "3 neighbors ({}) should be less than 5 ({})", m3, m5);
    }

    /// Lava neighbors are treated as full reservoir contribution
    /// (resists freezing), so a water cell next to lava doesn't
    /// freeze even at moderately-cold temperatures. Heat sources
    /// add to that on top via heat_source.rs.
    #[test]
    fn lava_neighbor_contributes_to_thermal_mass() {
        let mut cells = std::collections::HashMap::new();
        cells.insert((1, 0, 0), BlockId::Lava);
        cells.insert((-1, 0, 0), BlockId::Stone);
        cells.insert((0, 1, 0), BlockId::Stone);
        cells.insert((0, -1, 0), BlockId::Stone);
        cells.insert((0, 0, 1), BlockId::Stone);
        cells.insert((0, 0, -1), BlockId::Stone);
        let sample_with_lava = hash_sampler(cells);
        let m_with = thermal_mass_factor(0, 0, 0, &sample_with_lava);

        // Same setup but lava replaced with stone.
        let mut cells2 = std::collections::HashMap::new();
        cells2.insert((1, 0, 0), BlockId::Stone);
        cells2.insert((-1, 0, 0), BlockId::Stone);
        cells2.insert((0, 1, 0), BlockId::Stone);
        cells2.insert((0, -1, 0), BlockId::Stone);
        cells2.insert((0, 0, 1), BlockId::Stone);
        cells2.insert((0, 0, -1), BlockId::Stone);
        let sample_no_lava = hash_sampler(cells2);
        let m_without = thermal_mass_factor(0, 0, 0, &sample_no_lava);

        // Lava (contribution=1) should give MORE mass than stone (contribution=0.5).
        assert!(m_with > m_without,
            "lava-adjacent ({}) should have more mass than stone-only ({})",
            m_with, m_without);
    }
}
