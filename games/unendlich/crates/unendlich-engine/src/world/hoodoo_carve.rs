//! Hoodoo column radius vs. height: wide base, narrow neck at mid-height, wide
//! cap on top (worldgen). `height_frac` in [0,1]. Pure, deterministic.

pub fn radius(height_frac: f32, base_r: f32, cap_r: f32) -> f32 {
    let h = height_frac.clamp(0.0, 1.0);
    let b = base_r.max(0.0);
    let c = cap_r.max(0.0);
    let neck = 0.5;
    let narrow = b.min(c) * 0.5;
    if h <= neck {
        let t = h / neck;
        b * (1.0 - t) + narrow * t
    } else {
        let t = (h - neck) / (1.0 - neck);
        narrow * (1.0 - t) + c * t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn base_is_base_r() { assert_eq!(radius(0.0, 10.0, 6.0), 10.0); }
    #[test] fn top_is_cap_r() { assert!((radius(1.0, 10.0, 6.0) - 6.0).abs() < 1e-5); }
    #[test] fn mid_narrows() { let m = radius(0.5, 10.0, 6.0); assert!(m < 10.0 && m < 6.0); }
    #[test] fn clamped_above_one() { assert_eq!(radius(2.0, 10.0, 6.0), radius(1.0, 10.0, 6.0)); }
    #[test] fn bounded_nonneg() {
        let mut h = 0.0f32;
        while h <= 1.0 { assert!(radius(h, 10.0, 6.0) >= 0.0); h += 0.05; }
    }
    #[test] fn deterministic() { assert_eq!(radius(0.5, 10.0, 6.0).to_bits(), radius(0.5, 10.0, 6.0).to_bits()); }
}
