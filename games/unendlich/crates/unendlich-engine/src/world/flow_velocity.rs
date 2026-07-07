//! Shallow-water flow speed between two cells from the height gradient (steeper ⇒
//! faster, ∝ √gradient). Downhill only (≥0). Pure, deterministic.

pub fn flow_velocity(height_high: f32, height_low: f32, distance: f32) -> f32 {
    let drop = (height_high - height_low).max(0.0);
    let dist = distance.max(1e-4);
    (drop / dist).max(0.0).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn no_drop_no_flow() { assert_eq!(flow_velocity(10.0, 10.0, 1.0), 0.0); }
    #[test] fn steeper_faster() { assert!(flow_velocity(20.0, 10.0, 1.0) > flow_velocity(12.0, 10.0, 1.0)); }
    #[test] fn uphill_zero() { assert_eq!(flow_velocity(5.0, 10.0, 1.0), 0.0); }
    #[test] fn farther_slower() { assert!(flow_velocity(20.0, 10.0, 5.0) < flow_velocity(20.0, 10.0, 1.0)); }
    #[test] fn nonneg() { assert!(flow_velocity(20.0, 10.0, 1.0) >= 0.0); }
    #[test] fn deterministic() { assert_eq!(flow_velocity(20.0, 10.0, 1.0).to_bits(), flow_velocity(20.0, 10.0, 1.0).to_bits()); }
}
