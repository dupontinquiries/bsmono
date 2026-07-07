//! Season phase as a smooth annual cycle in [-1,1]: +1 peak summer, -1 peak
//! winter, 0 at the equinoxes. Day 0 = deep winter. Pure, deterministic.

use std::f32::consts::PI;

pub fn phase(day_of_year: f32, year_length: f32) -> f32 {
    let yl = year_length.max(1.0);
    let d = day_of_year.rem_euclid(yl);
    -((2.0 * PI * d / yl).cos())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn day0_winter() { assert!((phase(0.0, 360.0) + 1.0).abs() < 1e-5); }
    #[test] fn midyear_summer() { assert!((phase(180.0, 360.0) - 1.0).abs() < 1e-5); }
    #[test] fn quarter_equinox() { assert!(phase(90.0, 360.0).abs() < 1e-4); }
    #[test] fn bounded() {
        let mut d = 0.0f32;
        while d < 360.0 { assert!((-1.0..=1.0).contains(&phase(d, 360.0))); d += 5.0; }
    }
    #[test] fn periodic() { assert!((phase(10.0, 360.0) - phase(370.0, 360.0)).abs() < 1e-5); }
    #[test] fn deterministic() { assert_eq!(phase(100.0, 360.0).to_bits(), phase(100.0, 360.0).to_bits()); }
}
