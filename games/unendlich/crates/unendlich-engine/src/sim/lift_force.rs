//! Aerodynamic lift on a wing: ½·ρ·v²·area·CL, with air density folded into
//! K=0.6125 (ρ≈1.225 kg/m³). Drives dragon flight. Pure, deterministic.

pub fn lift(airspeed: f32, wing_area: f32, lift_coeff: f32) -> f32 {
    let v = airspeed.max(0.0);
    let a = wing_area.max(0.0);
    let cl = lift_coeff.max(0.0);
    0.6125 * v * v * a * cl
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn zero_speed_no_lift() { assert_eq!(lift(0.0, 12.0, 1.2), 0.0); }
    #[test] fn quadratic_in_speed() { assert!((lift(20.0, 12.0, 1.2) - 4.0 * lift(10.0, 12.0, 1.2)).abs() < 1e-2); }
    #[test] fn bigger_wing_more() { assert!(lift(15.0, 20.0, 1.0) > lift(15.0, 10.0, 1.0)); }
    #[test] fn more_cl_more() { assert!(lift(15.0, 12.0, 1.5) > lift(15.0, 12.0, 0.8)); }
    #[test] fn nonneg() { assert!(lift(-5.0, 12.0, 1.0) >= 0.0); }
    #[test] fn deterministic() { assert_eq!(lift(15.0, 12.0, 1.2).to_bits(), lift(15.0, 12.0, 1.2).to_bits()); }
}
