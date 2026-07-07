//! Ballistic vertical-velocity update for one step: vy' = vy − g·dt. Pure.

pub fn drop_velocity(velocity_y: f32, gravity: f32, dt: f32) -> f32 {
    velocity_y - gravity.max(0.0) * dt.max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn no_gravity_unchanged() { assert_eq!(drop_velocity(5.0, 0.0, 0.1), 5.0); }
    #[test] fn gravity_reduces() { assert!(drop_velocity(5.0, 9.8, 0.1) < 5.0); }
    #[test] fn longer_dt_more_drop() { assert!(drop_velocity(5.0, 9.8, 0.2) < drop_velocity(5.0, 9.8, 0.1)); }
    #[test] fn becomes_negative() { assert!(drop_velocity(1.0, 9.8, 1.0) < 0.0); }
    #[test] fn linear_in_dt() {
        let d1 = drop_velocity(5.0, 9.8, 0.1) - drop_velocity(5.0, 9.8, 0.2);
        let d2 = drop_velocity(5.0, 9.8, 0.0) - drop_velocity(5.0, 9.8, 0.1);
        assert!((d1 - d2).abs() < 1e-4);
    }
    #[test] fn deterministic() { assert_eq!(drop_velocity(5.0, 9.8, 0.1).to_bits(), drop_velocity(5.0, 9.8, 0.1).to_bits()); }
}
