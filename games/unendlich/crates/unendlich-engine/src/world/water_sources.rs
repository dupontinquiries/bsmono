//! `water_sources` — adds water to the sim from rain. Capped so it makes PUDDLES, never a
//! flood: a column only gains rain up to `cap`, and already-deeper water (lakes, oceans) is
//! left alone — so rain can't raise standing-water levels. Paired with the settle (spreads
//! rain to low spots) and evaporation (dries the sheets), this is a puddle cycle: wet during
//! rain, pooling in depressions, drying after.

/// Add `amount` of rain depth to each EXPOSED column (one with a real floor in the window,
/// i.e. `floor[i] != i32::MIN`) whose current depth is below `cap`, clamped at `cap`.
/// Columns already at/above `cap` (deep water) are untouched, so rain never floods them.
/// Returns total depth added (for diagnostics / keeping the sim awake).
pub fn apply_rain(depth: &mut [f32], floor: &[i32], amount: f32, cap: f32) -> f32 {
    if amount <= 0.0 {
        return 0.0;
    }
    let n = depth.len().min(floor.len());
    let mut total = 0.0;
    for i in 0..n {
        if floor[i] == i32::MIN {
            continue; // no floor in window → not open ground, skip
        }
        let d = depth[i];
        if d >= cap {
            continue; // already deep — don't raise standing water
        }
        let nd = (d + amount).min(cap);
        total += nd - d;
        depth[i] = nd;
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rains_onto_exposed_dry_ground() {
        let mut d = vec![0.0f32; 4];
        let floor = vec![10, 10, 10, 10];
        let added = apply_rain(&mut d, &floor, 0.3, 2.0);
        assert!((added - 1.2).abs() < 1e-6); // 4 * 0.3
        assert!(d.iter().all(|&v| (v - 0.3).abs() < 1e-6));
    }

    #[test]
    fn skips_columns_without_floor() {
        let mut d = vec![0.0f32; 2];
        let floor = vec![i32::MIN, 5];
        let added = apply_rain(&mut d, &floor, 0.5, 2.0);
        assert!((added - 0.5).abs() < 1e-6);
        assert_eq!(d[0], 0.0); // no floor → no rain
        assert!((d[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn does_not_raise_deep_water() {
        let mut d = vec![5.0f32, 0.0];
        let floor = vec![3, 3];
        let added = apply_rain(&mut d, &floor, 0.5, 2.0);
        assert!((added - 0.5).abs() < 1e-6); // only the dry one gained
        assert_eq!(d[0], 5.0); // deep water untouched
        assert!((d[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn clamps_at_cap() {
        let mut d = vec![1.9f32];
        let floor = vec![3];
        let added = apply_rain(&mut d, &floor, 0.5, 2.0);
        assert!((added - 0.1).abs() < 1e-6); // only up to cap
        assert!((d[0] - 2.0).abs() < 1e-6);
    }

    #[test]
    fn zero_amount_noop() {
        let mut d = vec![0.0f32; 4];
        let floor = vec![10; 4];
        assert_eq!(apply_rain(&mut d, &floor, 0.0, 2.0), 0.0);
    }
}
