//! `world::water_cycle` — the full per-chunk water cycle in one tick, composing every
//! tested core from the field-driven water path into a single entry point.
//!
//! One `step_cycle` over a chunk does, in order:
//! 1. **Seep (placement from springs)** — where the groundwater `table` sits above the
//!    `ground`, water seeps out ([`aquifer_coupling::seep_inflow`]) and is added to the
//!    inflow alongside any external sources (rain, a poured bucket).
//! 2. **Infiltration (drain → aquifer)** — a little standing water drains subsurface;
//!    the amount is tracked to recharge the table.
//! 3. **Placement + evaporation + fill-cap** — [`lake_fill_level::fill_level`] adds the
//!    inflow, removes evaporation, and clamps each column at the spill height.
//! 4. **Physics** — [`water_field::WaterField::settle_step`] spreads water laterally so
//!    surfaces equalize.
//! 5. **Recharge** — [`aquifer_coupling::recharge_table`] sets the table from this tick's
//!    infiltration (wet columns hold a higher table, which seeps back in step 1 next tick).
//!
//! Cross-chunk flow ([`water_edge_flow`](crate::world::water_edge_flow)) is the one part
//! left to the caller — it couples *adjacent* chunks, so it's applied between cycle ticks
//! over the chunk grid, not inside a single chunk's step. Everything here is per-chunk and
//! independent, so [`step_cycles`] runs a whole set in parallel (Class-P over the rayon
//! lane). Inert groundwork pending the chunk-manager field hook, like the rest of the path.

use crate::util::task_lanes::map_pure;
use crate::world::aquifer_coupling::{recharge_table, seep_inflow};
use crate::world::lake_fill_level::fill_level;
use crate::world::water_field::WaterField;

/// One chunk's full water state: surface water field, terrain surface heights, aquifer
/// bedrock floor, and the current groundwater table — all `CHUNK_SIZE*CHUNK_SIZE`,
/// row-major.
#[derive(Clone, Debug)]
pub struct ChunkWater {
    pub field: WaterField,
    pub ground: Vec<f32>,
    pub bedrock: Vec<f32>,
    pub table: Vec<f32>,
}

/// Tuning for one full cycle tick.
#[derive(Clone, Copy, Debug)]
pub struct CycleParams {
    pub settle_rate: f32,
    pub evaporation: f32,
    pub infiltration: f32,
    pub spill: f32,
    pub drainage: f32,
    pub seep_rate: f32,
}

impl Default for CycleParams {
    fn default() -> Self {
        CycleParams {
            settle_rate: 0.5,
            evaporation: 0.01,
            infiltration: 0.05,
            spill: 64.0,
            // drainage is the groundwater loss rate; keep it below typical per-tick
            // infiltration so sustained surface water actually lifts the table (set
            // higher for drier climates where the table drains away).
            drainage: 0.0,
            seep_rate: 0.25,
        }
    }
}

/// Advance one chunk's water cycle by a tick. `ext_sources` is per-column external inflow
/// (rain / pour); springs from the aquifer are added on top.
pub fn step_cycle(cw: &mut ChunkWater, ext_sources: &[f32], p: &CycleParams) {
    let n = cw
        .field
        .depth
        .len()
        .min(cw.ground.len())
        .min(cw.bedrock.len())
        .min(cw.table.len());

    // 1. seep (springs) → total inflow = external + seep
    let mut seep = vec![0.0f32; n];
    seep_inflow(&cw.table[..n], &cw.ground[..n], p.seep_rate, &mut seep);

    // 2. infiltration drain (tracked for recharge) + 3. placement/evap/fill-cap
    let mut infil = vec![0.0f32; n];
    let infil_rate = p.infiltration.max(0.0);
    for i in 0..n {
        let d = cw.field.depth[i];
        let drained = infil_rate.min(d.max(0.0));
        infil[i] = drained;
        let after_infil = (d - drained).max(0.0);
        let inflow = seep[i] + ext_sources.get(i).copied().unwrap_or(0.0);
        let cap = (p.spill - cw.ground[i]).max(0.0);
        cw.field.depth[i] = fill_level(after_infil, inflow, p.evaporation, cap);
    }

    // 4. physics: lateral settle within the chunk
    cw.field.settle_step(&cw.ground, p.settle_rate);

    // 5. recharge the table from this tick's infiltration
    let mut new_table = vec![0.0f32; n];
    recharge_table(&infil, p.drainage, &cw.bedrock[..n], &mut new_table);
    cw.table[..n].copy_from_slice(&new_table);
}

/// Step many chunks' cycles in parallel (each chunk independent → Class-P). External
/// inflow is shared across chunks here (e.g. uniform rain); per-chunk inflow can be
/// folded into each chunk before calling.
pub fn step_cycles(chunks: &[ChunkWater], ext_sources: &[f32], p: &CycleParams) -> Vec<ChunkWater> {
    map_pure(chunks, |c| {
        let mut cw = c.clone();
        step_cycle(&mut cw, ext_sources, p);
        cw
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::water_field::WATER_AREA;

    fn chunk(table_h: f32, ground_h: f32, bedrock_h: f32, depth_h: f32) -> ChunkWater {
        ChunkWater {
            field: WaterField { depth: vec![depth_h; WATER_AREA] },
            ground: vec![ground_h; WATER_AREA],
            bedrock: vec![bedrock_h; WATER_AREA],
            table: vec![table_h; WATER_AREA],
        }
    }

    #[test]
    fn spring_seeps_water_to_surface() {
        // table above ground, no surface water, no external inflow → a spring fills in
        let mut cw = chunk(35.0, 30.0, 30.0, 0.0);
        let before: f32 = cw.field.depth.iter().sum();
        step_cycle(&mut cw, &[], &CycleParams { evaporation: 0.0, infiltration: 0.0, ..Default::default() });
        let after: f32 = cw.field.depth.iter().sum();
        assert!(after > before, "a high water table seeps a spring to the surface");
    }

    #[test]
    fn infiltration_recharges_table() {
        // surface water present, table starts at bedrock → infiltration lifts the table
        let mut cw = chunk(30.0, 30.0, 30.0, 5.0);
        step_cycle(&mut cw, &[], &CycleParams { seep_rate: 0.0, ..Default::default() });
        assert!(cw.table.iter().any(|&t| t > 30.0), "infiltrating surface water raises the table");
    }

    #[test]
    fn cycle_runs_and_stays_finite_bounded() {
        // run the full cycle many ticks with steady rain; depths stay finite + capped
        let mut cw = chunk(30.0, 30.0, 30.0, 0.0);
        let rain = vec![0.1f32; WATER_AREA];
        let p = CycleParams { spill: 33.0, ..Default::default() }; // cap = 3 above ground
        for _ in 0..500 {
            step_cycle(&mut cw, &rain, &p);
        }
        assert!(cw.field.depth.iter().all(|&d| d.is_finite() && (0.0..=3.0 + 1e-3).contains(&d)),
            "the full cycle stays finite and respects the spill cap");
    }

    #[test]
    fn parallel_equals_serial() {
        let p = CycleParams::default();
        let rain = vec![0.05f32; WATER_AREA];
        let chunks: Vec<ChunkWater> = (0..8)
            .map(|i| chunk(30.0 + i as f32 * 0.5, 30.0, 30.0, i as f32))
            .collect();
        let par = step_cycles(&chunks, &rain, &p);
        let ser: Vec<ChunkWater> = chunks
            .iter()
            .map(|c| {
                let mut cw = c.clone();
                step_cycle(&mut cw, &rain, &p);
                cw
            })
            .collect();
        for (a, b) in par.iter().zip(ser.iter()) {
            assert_eq!(a.field.depth, b.field.depth);
            assert_eq!(a.table, b.table);
        }
    }
}
