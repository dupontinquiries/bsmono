//! `world::aquifer_coupling` — close the surface/subsurface water cycle.
//!
//! The water-cell step ([`water_cells`](crate::world::water_cells)) drains surface water
//! by **infiltration**. That infiltration recharges the **groundwater table**
//! ([`aquifer::water_table`](crate::world::aquifer)): rain (here, infiltration) raises it,
//! drainage lowers it, bounded by bedrock and a saturation cap. Where the table rises
//! **above the ground surface**, water **seeps** back out as a spring — which becomes
//! placement inflow for the surface cell sim. So the loop closes:
//!
//! ```text
//! surface depth ──drain(infiltration)──▶ table ──seep where table>ground──▶ surface inflow
//! ```
//!
//! Per-column array ops, so they slot straight into a chunk's `CHUNK_SIZE*CHUNK_SIZE`
//! buffers and the existing parallel chunk batch. Pure; inert groundwork like the rest of
//! the field-driven water path.

use crate::world::aquifer::water_table;

/// Recharge the groundwater table per column from `infiltration` (the per-column water
/// that drained subsurface this tick) and a uniform natural `drainage`, over the
/// per-column `bedrock` floor. Writes the new table heights into `out`.
pub fn recharge_table(infiltration: &[f32], drainage: f32, bedrock: &[f32], out: &mut [f32]) {
    let n = infiltration.len().min(bedrock.len()).min(out.len());
    for i in 0..n {
        out[i] = water_table(infiltration[i], drainage, bedrock[i]);
    }
}

/// Spring/seep inflow per column: where the water `table` sits above the `ground` surface,
/// water seeps out at `rate * (table - ground)`. Where the table is at or below ground,
/// the column is dry (0). Writes inflow into `out` — feed this to the cell sim's `sources`.
pub fn seep_inflow(table: &[f32], ground: &[f32], rate: f32, out: &mut [f32]) {
    let r = rate.max(0.0);
    let n = table.len().min(ground.len()).min(out.len());
    for i in 0..n {
        out[i] = (table[i] - ground[i]).max(0.0) * r;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn more_infiltration_raises_table() {
        let bedrock = vec![30.0; 4];
        let mut wet = vec![0.0; 4];
        let mut dry = vec![0.0; 4];
        recharge_table(&[20.0; 4], 2.0, &bedrock, &mut wet);
        recharge_table(&[2.0; 4], 2.0, &bedrock, &mut dry);
        assert!(wet[0] > dry[0], "more infiltration ⇒ higher table");
        assert!(dry[0] >= 30.0, "table never falls below bedrock");
    }

    #[test]
    fn seep_only_where_table_above_ground() {
        let table = vec![35.0, 28.0, 30.0];
        let ground = vec![30.0, 30.0, 30.0];
        let mut inflow = vec![0.0; 3];
        seep_inflow(&table, &ground, 0.5, &mut inflow);
        assert!(inflow[0] > 0.0, "table above ground seeps out");
        assert_eq!(inflow[1], 0.0, "table below ground ⇒ dry");
        assert_eq!(inflow[2], 0.0, "table at ground ⇒ dry");
        assert!((inflow[0] - 2.5).abs() < 1e-4, "seep = rate*(table-ground) = 0.5*5");
    }

    #[test]
    fn cycle_infiltration_to_seep_produces_inflow() {
        // a low basin (bedrock high relative to ground) that infiltrates heavily ends up
        // with a table above its ground, seeping water back to the surface.
        let bedrock = vec![32.0; 2];
        let ground = vec![30.0; 2];
        let mut table = vec![0.0; 2];
        recharge_table(&[10.0, 10.0], 1.0, &bedrock, &mut table); // table = 32 + (10-1) capped
        let mut inflow = vec![0.0; 2];
        seep_inflow(&table, &ground, 0.25, &mut inflow);
        assert!(inflow[0] > 0.0, "recharged table seeps back to the surface — cycle closes");
    }

    #[test]
    fn length_mismatch_is_safe() {
        let mut out = vec![0.0; 2];
        seep_inflow(&[1.0, 2.0, 3.0], &[0.0], 1.0, &mut out); // shortest wins, no panic
        assert_eq!(out[1], 0.0);
    }
}
