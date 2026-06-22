//! Aerodynamic drag: ½·ρ·v²·area·CD (K=0.6125). Pure, deterministic.

pub fn drag(airspeed: f32, area: f32, drag_coeff: f32) -> f32 {
    let v = airspeed.max(0.0);
    let a = area.max(0.0);
    let cd = drag_coeff.max(0.0);
    0.6125 * v * v * a * cd
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn zero_speed_no_drag() { assert_eq!(drag(0.0, 3.0, 0.4), 0.0); }
    #[test] fn quadratic_in_speed() { assert!((drag(20.0, 3.0, 0.4) - 4.0 * drag(10.0, 3.0, 0.4)).abs() < 1e-2); }
    #[test] fn bigger_area_more() { assert!(drag(15.0, 5.0, 0.4) > drag(15.0, 2.0, 0.4)); }
    #[test] fn more_cd_more() { assert!(drag(15.0, 3.0, 0.8) > drag(15.0, 3.0, 0.2)); }
    #[test] fn nonneg() { assert!(drag(-5.0, 3.0, 0.4) >= 0.0); }
    #[test] fn deterministic() { assert_eq!(drag(15.0, 3.0, 0.4).to_bits(), drag(15.0, 3.0, 0.4).to_bits()); }
}
