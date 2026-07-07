//! Smooth biome-boundary blend weight in [0,1]: 1 inside biome A (dist ≤ 0), 0 once
//! `band` into biome B, smoothstep across the band. `dist_past_boundary` is signed
//! distance past the A/B boundary. Pure, deterministic.

pub fn weight(dist_past_boundary: f32, band: f32) -> f32 {
    let t = (dist_past_boundary / band.max(1e-4)).clamp(0.0, 1.0);
    1.0 - t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn inside_a_full() { assert_eq!(weight(-5.0, 4.0), 1.0); }
    #[test] fn past_band_zero() { assert_eq!(weight(8.0, 4.0), 0.0); }
    #[test] fn mid_half() { assert!((weight(2.0, 4.0) - 0.5).abs() < 1e-6); }
    #[test] fn monotone_decreasing() {
        let (mut prev, mut d) = (f32::MAX, -1.0f32);
        while d <= 5.0 { let v = weight(d, 4.0); assert!(v <= prev + 1e-6); prev = v; d += 0.25; }
    }
    #[test] fn bounded() { let mut d = -2.0f32; while d <= 6.0 { assert!((0.0..=1.0).contains(&weight(d, 4.0))); d += 0.25; } }
    #[test] fn deterministic() { assert_eq!(weight(2.0, 4.0).to_bits(), weight(2.0, 4.0).to_bits()); }
}
