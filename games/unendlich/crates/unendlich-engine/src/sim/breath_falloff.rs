//! Breath-weapon damage multiplier by distance: full at the mouth, linearly
//! falling to 0 at max_range. In [0,1]. Pure, deterministic.

pub fn falloff(distance: f32, max_range: f32) -> f32 {
    let d = distance.max(0.0);
    let r = max_range.max(1e-4);
    (1.0 - d / r).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn mouth_full() { assert_eq!(falloff(0.0, 10.0), 1.0); }
    #[test] fn edge_zero() { assert_eq!(falloff(10.0, 10.0), 0.0); }
    #[test] fn beyond_zero() { assert_eq!(falloff(20.0, 10.0), 0.0); }
    #[test] fn mid_half() { assert!((falloff(5.0, 10.0) - 0.5).abs() < 1e-6); }
    #[test] fn monotone_decreasing() {
        let (mut prev, mut d) = (f32::MAX, 0.0f32);
        while d <= 10.0 { let v = falloff(d, 10.0); assert!(v <= prev + 1e-6); prev = v; d += 0.5; }
    }
    #[test] fn deterministic() { assert_eq!(falloff(5.0, 10.0).to_bits(), falloff(5.0, 10.0).to_bits()); }
}
