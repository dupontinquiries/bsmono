//! Resource-node regrowth toward max: logistic — faster when partly depleted,
//! slowing near full, with a tiny seed so empty nodes recover. Returns new amount.

pub fn regen(current: f32, max: f32, rate: f32, dt: f32) -> f32 {
    let m = max.max(0.0);
    let cur = current.clamp(0.0, m);
    let r = rate.max(0.0);
    let t = dt.max(0.0);
    let growth = r * cur.max(0.001) * (1.0 - cur / m.max(1e-6)) * t;
    (cur + growth).clamp(0.0, m)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn full_stays_full() { assert!((regen(100.0, 100.0, 0.1, 1.0) - 100.0).abs() < 1e-4); }
    #[test] fn depleted_grows() { assert!(regen(50.0, 100.0, 0.1, 1.0) > 50.0); }
    #[test] fn empty_seeds_growth() { assert!(regen(0.0, 100.0, 0.5, 1.0) > 0.0); }
    #[test] fn capped_at_max() { assert!((regen(99.0, 100.0, 100.0, 10.0) - 100.0).abs() < 1e-4); }
    #[test] fn more_dt_more() { assert!(regen(50.0, 100.0, 0.1, 2.0) > regen(50.0, 100.0, 0.1, 1.0)); }
    #[test] fn deterministic() { assert_eq!(regen(50.0, 100.0, 0.1, 1.0).to_bits(), regen(50.0, 100.0, 0.1, 1.0).to_bits()); }
}
