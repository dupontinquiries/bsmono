//! Vertical climb rate from excess thrust over drag per unit weight: (thrust −
//! drag)/weight. Positive ⇒ climb, negative ⇒ descend. Pure, deterministic.

pub fn climb_rate(thrust: f32, drag: f32, weight: f32) -> f32 {
    (thrust - drag) / weight.max(1e-4)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn excess_climbs() { assert!(climb_rate(100.0, 50.0, 10.0) > 0.0); }
    #[test] fn deficit_descends() { assert!(climb_rate(30.0, 50.0, 10.0) < 0.0); }
    #[test] fn balanced_level() { assert_eq!(climb_rate(50.0, 50.0, 10.0), 0.0); }
    #[test] fn heavier_less() { assert!(climb_rate(100.0, 50.0, 20.0) < climb_rate(100.0, 50.0, 10.0)); }
    #[test] fn more_thrust_more() { assert!(climb_rate(120.0, 50.0, 10.0) > climb_rate(80.0, 50.0, 10.0)); }
    #[test] fn deterministic() { assert_eq!(climb_rate(100.0, 50.0, 10.0).to_bits(), climb_rate(100.0, 50.0, 10.0).to_bits()); }
}
