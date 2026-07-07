//! Blend a detail normal onto a base normal (UDN-style) by strength; returns a
//! unit vector. strength 0 ⇒ base, 1 ⇒ full detail influence. Pure, deterministic.

pub fn blend(base: [f32; 3], detail: [f32; 3], strength: f32) -> [f32; 3] {
    let s = strength.clamp(0.0, 1.0);
    let mut v = [base[0] + detail[0] * s, base[1] + detail[1] * s, base[2] + detail[2] * s];
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 1e-6 {
        v[0] /= len;
        v[1] /= len;
        v[2] /= len;
        v
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ulen(v: [f32; 3]) -> f32 { (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt() }
    #[test] fn strength0_is_base() { assert_eq!(blend([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], 0.0), [0.0, 0.0, 1.0]); }
    #[test] fn unit_out() { for &s in &[0.2f32, 0.5, 0.9] { assert!((ulen(blend([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], s)) - 1.0).abs() < 1e-5); } }
    #[test] fn more_strength_more_tilt() { assert!(blend([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], 0.8)[0] > blend([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], 0.2)[0]); }
    #[test] fn degenerate_guard() { let v = blend([0.0, 0.0, 0.0], [0.0, 0.0, 0.0], 0.5); assert!(v.iter().all(|c| c.is_finite())); }
    #[test] fn bounded_components() { let v = blend([0.0, 1.0, 0.0], [0.0, 0.0, 1.0], 0.5); for c in v { assert!(c.abs() <= 1.0 + 1e-6); } }
    #[test] fn deterministic() { assert_eq!(blend([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], 0.5), blend([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], 0.5)); }
}
