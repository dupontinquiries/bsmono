//! `world::water_field_batch` — thread the lake-settling water field across chunks, and
//! the evaporation/rainfall cycle on top of it (the next pure steps after the
//! `water_field` foundation on the `CLIMATE_PATH` roadmap; still inert pending the
//! chunk-manager/sim-scheduler integration the roadmap notes is blocked).
//!
//! Within-chunk `settle_step` has closed boundaries (no flux leaves a chunk), so each
//! chunk's settle is independent of the others — settling a whole set of chunk fields is
//! a **Class-P** parallel map over the rayon lane
//! ([`util::task_lanes::map_pure`](crate::util::task_lanes)), order-preserving and
//! identical to the serial result. [`evaporate_rain`] is the pure per-column cycle step
//! (evaporate where wet, rain everywhere) that composes with settling.

use crate::util::task_lanes::map_pure;
use crate::world::water_field::WaterField;

/// One chunk's settle unit: its water field plus the ground heightfield the water
/// settles over (both `CHUNK_SIZE*CHUNK_SIZE`, row-major).
#[derive(Clone, Debug)]
pub struct FieldChunk {
    pub field: WaterField,
    pub ground: Vec<f32>,
}

/// Settle every chunk's field `iters` times in parallel (`settle_step` conserves volume
/// within each chunk). Returns the settled fields in input order — identical to settling
/// each chunk serially, just spread across workers.
pub fn settle_chunks(chunks: &[FieldChunk], rate: f32, iters: u32) -> Vec<WaterField> {
    map_pure(chunks, |c| {
        let mut f = c.field.clone();
        for _ in 0..iters {
            f.settle_step(&c.ground, rate);
        }
        f
    })
}

/// One evaporation + rainfall step on a field, in place. Each column first loses up to
/// `evap` depth (never below zero — you can't evaporate dry ground), then gains `rain`
/// depth (rain pools everywhere). Both inputs are clamped non-negative.
pub fn evaporate_rain(field: &mut WaterField, evap: f32, rain: f32) {
    let e = evap.max(0.0);
    let r = rain.max(0.0);
    for d in field.depth.iter_mut() {
        *d = (*d - e).max(0.0) + r;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::water_field::WATER_AREA;

    fn flat_ground() -> Vec<f32> {
        vec![0.0f32; WATER_AREA]
    }

    fn field_with(depths: &[(usize, f32)]) -> WaterField {
        let mut f = WaterField::empty();
        for &(i, d) in depths {
            f.depth[i] = d;
        }
        f
    }

    #[test]
    fn parallel_settle_equals_serial_and_conserves_volume() {
        // a pile of water in one column per chunk, on flat ground → settles outward
        let chunks: Vec<FieldChunk> = (0..12)
            .map(|i| FieldChunk { field: field_with(&[(0, 50.0 + i as f32)]), ground: flat_ground() })
            .collect();
        let par = settle_chunks(&chunks, 0.5, 4);
        let ser: Vec<WaterField> = chunks
            .iter()
            .map(|c| {
                let mut f = c.field.clone();
                for _ in 0..4 {
                    f.settle_step(&c.ground, 0.5);
                }
                f
            })
            .collect();
        for (p, s) in par.iter().zip(ser.iter()) {
            assert_eq!(p.depth, s.depth, "parallel settle matches serial per chunk");
        }
        // volume conserved through the parallel settle
        for (c, p) in chunks.iter().zip(par.iter()) {
            assert!(
                (c.field.total_volume() - p.total_volume()).abs() < 1e-3,
                "settle conserves volume"
            );
        }
    }

    #[test]
    fn evaporation_lowers_rain_raises() {
        let mut f = field_with(&[(0, 1.0), (1, 0.0)]);
        let before = f.total_volume();
        evaporate_rain(&mut f, 0.3, 0.0); // pure evaporation
        assert!(f.total_volume() < before, "evaporation removes water");
        assert_eq!(f.depth[1], 0.0, "dry column stays dry under evaporation");

        let mut g = WaterField::empty();
        evaporate_rain(&mut g, 0.0, 0.2); // pure rain
        assert!(g.depth.iter().all(|&d| (d - 0.2).abs() < 1e-6), "rain pools on every column");
    }

    #[test]
    fn depth_never_negative() {
        let mut f = field_with(&[(5, 0.1)]);
        evaporate_rain(&mut f, 10.0, 0.0); // over-evaporate
        assert!(f.depth.iter().all(|&d| d >= 0.0), "depth clamps at zero");
    }

    #[test]
    fn empty_batch() {
        assert!(settle_chunks(&[], 0.5, 1).is_empty());
    }
}
