//! Sediment carrying capacity of flowing water: ∝ flow speed × slope (faster,
//! steeper water carries more), capped. In [0, max_capacity]. Pure, deterministic.

pub fn capacity(flow_speed: f32, slope: f32, max_capacity: f32) -> f32 {
    let v = flow_speed.max(0.0);
    let s = slope.max(0.0);
    let mc = max_capacity.max(0.0);
    (v * s).min(mc).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn no_flow_zero() { assert_eq!(capacity(0.0, 0.5, 10.0), 0.0); }
    #[test] fn faster_more() { assert!(capacity(2.0, 0.5, 10.0) > capacity(0.5, 0.5, 10.0)); }
    #[test] fn steeper_more() { assert!(capacity(1.0, 0.8, 10.0) > capacity(1.0, 0.2, 10.0)); }
    #[test] fn capped() { assert_eq!(capacity(100.0, 100.0, 5.0), 5.0); }
    #[test] fn nonneg() { assert!(capacity(1.0, 0.5, 10.0) >= 0.0); }
    #[test] fn deterministic() { assert_eq!(capacity(1.0, 0.5, 10.0).to_bits(), capacity(1.0, 0.5, 10.0).to_bits()); }
}
