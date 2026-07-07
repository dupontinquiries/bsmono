//! `world::water_cells` — the per-column water cell step: **placement**, **drain**,
//! **fill**, and **physics**, composing the engine's tested water cores.
//!
//! One tick over a chunk's `CHUNK_SIZE*CHUNK_SIZE` columns:
//! 1. **Placement** — per-column `sources` inflow (a spring, a poured bucket, rain).
//!    [`pour_into`] paints a [`FieldBrush`](crate::world::field_brush::FieldBrush) pour
//!    into that buffer.
//! 2. **Drain + Fill** — each column advances by
//!    [`fill_level`](crate::world::lake_fill_level::fill_level)`(depth, inflow=source,
//!    outflow=evaporation+infiltration, capacity=spill−ground)`: inflow fills, the two
//!    outflow terms drain, and the capacity clamp caps the column at the spill height so
//!    it can't pile above where it would overflow.
//! 3. **Physics** — [`WaterField::settle_step`](crate::world::water_field::WaterField)
//!    spreads water laterally so surfaces equalize (a lake finds its level).
//!
//! Each chunk's step is independent (settle has closed boundaries), so [`step_cells`]
//! runs a whole set in parallel — a Class-P map over the rayon lane
//! ([`util::task_lanes::map_pure`](crate::util::task_lanes)).
//!
//! Inert groundwork (like `water_field`): the persistent per-chunk field + scheduler hook
//! is the blocked integration noted on the `CLIMATE_PATH` roadmap.

use crate::constants::CHUNK_SIZE;
use crate::util::task_lanes::map_pure;
use crate::world::field_brush::{brush_columns, FieldBrush};
use crate::world::lake_fill_level::fill_level;
use crate::world::water_field::WaterField;

/// Tuning for one water-cell tick.
#[derive(Clone, Copy, Debug)]
pub struct CellParams {
    /// Lateral settle rate `[0,1]` passed to `settle_step`.
    pub settle_rate: f32,
    /// Per-tick depth lost to evaporation (surface drain).
    pub evaporation: f32,
    /// Per-tick depth lost to infiltration (subsurface drain → aquifer).
    pub infiltration: f32,
    /// Maximum water *surface* height; a column caps at `spill − ground` so it can't
    /// rise past the point where it would overflow.
    pub spill: f32,
}

impl Default for CellParams {
    fn default() -> Self {
        CellParams { settle_rate: 0.5, evaporation: 0.01, infiltration: 0.05, spill: 64.0 }
    }
}

/// Placement: brush a pour of water into the per-column `sources` inflow buffer
/// (`CHUNK_SIZE*CHUNK_SIZE`, row-major) centered at column `(cx, cz)`. Out-of-bounds
/// cells are skipped; deltas are clamped non-negative (a pour adds water).
pub fn pour_into(sources: &mut [f32], cx: i32, cz: i32, brush: &FieldBrush) {
    let s = CHUNK_SIZE;
    for (dx, dz, delta) in brush_columns(brush) {
        let (x, z) = (cx + dx, cz + dz);
        if x >= 0 && x < s && z >= 0 && z < s {
            let i = (x + z * s) as usize;
            if i < sources.len() {
                sources[i] += delta.max(0.0);
            }
        }
    }
}

/// One water-cell tick over a chunk: placement + drain + fill (per-column `fill_level`)
/// then the settle physics. `sources` is the per-column inflow for this tick.
pub fn step_cell(field: &mut WaterField, ground: &[f32], sources: &[f32], p: &CellParams) {
    let outflow = p.evaporation.max(0.0) + p.infiltration.max(0.0);
    let n = field.depth.len().min(ground.len()).min(sources.len());
    for i in 0..n {
        let cap = (p.spill - ground[i]).max(0.0);
        field.depth[i] = fill_level(field.depth[i], sources[i], outflow, cap);
    }
    field.settle_step(ground, p.settle_rate);
}

/// One chunk's unit for the parallel batch.
#[derive(Clone, Debug)]
pub struct CellChunk {
    pub field: WaterField,
    pub ground: Vec<f32>,
    pub sources: Vec<f32>,
}

/// Step many chunks' water cells in parallel (each chunk independent → Class-P).
/// Order-preserving ⇒ identical to stepping each chunk serially.
pub fn step_cells(chunks: &[CellChunk], p: &CellParams) -> Vec<WaterField> {
    map_pure(chunks, |c| {
        let mut f = c.field.clone();
        step_cell(&mut f, &c.ground, &c.sources, p);
        f
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::field_brush::Falloff;
    use crate::world::water_field::WATER_AREA;

    fn ground_flat(h: f32) -> Vec<f32> {
        vec![h; WATER_AREA]
    }
    fn zeros() -> Vec<f32> {
        vec![0.0f32; WATER_AREA]
    }

    #[test]
    fn placement_raises_depth() {
        let mut f = WaterField::empty();
        let mut src = zeros();
        src[10] = 2.0; // a spring at column 10
        let before: f32 = f.depth.iter().sum();
        step_cell(&mut f, &ground_flat(0.0), &src, &CellParams { evaporation: 0.0, infiltration: 0.0, ..Default::default() });
        let after: f32 = f.depth.iter().sum();
        assert!(after > before, "placement adds water ({after} > {before})");
    }

    #[test]
    fn drain_lowers_depth() {
        let mut f = WaterField { depth: vec![5.0; WATER_AREA] };
        let before: f32 = f.depth.iter().sum();
        // no inflow, only drain; high spill so the cap doesn't interfere
        step_cell(&mut f, &ground_flat(0.0), &zeros(), &CellParams { settle_rate: 0.0, evaporation: 0.5, infiltration: 0.5, spill: 1000.0 });
        let after: f32 = f.depth.iter().sum();
        assert!(after < before, "evaporation + infiltration drain water ({after} < {before})");
    }

    #[test]
    fn fill_caps_at_spill() {
        let mut f = WaterField::empty();
        let mut src = zeros();
        for s in src.iter_mut() {
            *s = 100.0; // flood every column
        }
        // ground 2, spill 5 → capacity 3 per column
        step_cell(&mut f, &ground_flat(2.0), &src, &CellParams { settle_rate: 0.0, evaporation: 0.0, infiltration: 0.0, spill: 5.0 });
        assert!(f.depth.iter().all(|&d| d <= 3.0 + 1e-4), "no column exceeds spill capacity");
    }

    #[test]
    fn physics_spreads_a_pile() {
        // a tall pile in one column on flat ground settles outward to its neighbors
        let mut f = WaterField::empty();
        f.depth[WATER_AREA / 2] = 40.0;
        let center_before = f.depth[WATER_AREA / 2];
        step_cell(&mut f, &ground_flat(0.0), &zeros(), &CellParams { settle_rate: 0.5, evaporation: 0.0, infiltration: 0.0, spill: 1000.0 });
        assert!(f.depth[WATER_AREA / 2] < center_before, "settle physics lowers the pile center");
    }

    #[test]
    fn pour_brush_places_water() {
        let mut src = zeros();
        let center = CHUNK_SIZE / 2;
        pour_into(&mut src, center, center, &FieldBrush::new(2.0, 3.0, Falloff::Linear));
        let i = (center + center * CHUNK_SIZE) as usize;
        assert!(src[i] > 0.0, "brush pours water at its center column");
        assert!(src.iter().sum::<f32>() > 0.0);
    }

    #[test]
    fn parallel_equals_serial() {
        let p = CellParams::default();
        let chunks: Vec<CellChunk> = (0..10)
            .map(|i| {
                let mut field = WaterField::empty();
                field.depth[i] = 20.0 + i as f32;
                CellChunk { field, ground: ground_flat(0.0), sources: zeros() }
            })
            .collect();
        let par = step_cells(&chunks, &p);
        let ser: Vec<WaterField> = chunks
            .iter()
            .map(|c| {
                let mut f = c.field.clone();
                step_cell(&mut f, &c.ground, &c.sources, &p);
                f
            })
            .collect();
        for (a, b) in par.iter().zip(ser.iter()) {
            assert_eq!(a.depth, b.depth);
        }
    }
}
