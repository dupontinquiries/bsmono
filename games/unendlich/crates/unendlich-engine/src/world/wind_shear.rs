//! Wind speed at altitude: surface wind strengthened by a shear gradient with
//! height (wind generally picks up aloft), saturating. Pure, deterministic.

pub fn wind_at(base_wind: f32, altitude: f32, shear: f32) -> f32 {
    let w = base_wind.max(0.0);
    let alt = altitude.max(0.0);
    let s = shear.max(0.0);
    w * (1.0 + s * (1.0 - (-(alt / 1000.0)).exp()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn surface_is_base() { assert!((wind_at(10.0, 0.0, 0.5) - 10.0).abs() < 1e-6); }
    #[test] fn higher_stronger() { assert!(wind_at(10.0, 3000.0, 0.5) > wind_at(10.0, 200.0, 0.5)); }
    #[test] fn more_shear_stronger() { assert!(wind_at(10.0, 2000.0, 1.0) > wind_at(10.0, 2000.0, 0.2)); }
    #[test] fn zero_base_zero() { assert_eq!(wind_at(0.0, 2000.0, 0.5), 0.0); }
    #[test] fn monotone_in_altitude() {
        let (mut prev, mut a) = (f32::MIN, 0.0f32);
        while a < 5000.0 { let w = wind_at(10.0, a, 0.5); assert!(w >= prev - 1e-6); prev = w; a += 100.0; }
    }
    #[test] fn deterministic() { assert_eq!(wind_at(10.0, 2000.0, 0.5).to_bits(), wind_at(10.0, 2000.0, 0.5).to_bits()); }
}
