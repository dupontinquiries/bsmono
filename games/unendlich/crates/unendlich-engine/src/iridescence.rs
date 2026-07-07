//! Thin-film iridescence hue shift for dragon scales: a view-angle-dependent hue
//! in [0,1) (wraps). Grazing angles shift more; `shift` scales the cycle rate.

pub fn hue_shift(cos_theta: f32, shift: f32) -> f32 {
    let c = cos_theta.clamp(0.0, 1.0);
    let s = shift.max(0.0);
    let raw = (1.0 - c) * s;
    raw - raw.floor()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn head_on_no_shift() { assert_eq!(hue_shift(1.0, 2.0), 0.0); }
    #[test] fn in_unit_range() {
        let mut c = 0.0f32;
        while c <= 1.0 { let h = hue_shift(c, 3.5); assert!((0.0..1.0).contains(&h)); c += 0.03; }
    }
    #[test] fn grazing_shifts() { assert!((hue_shift(0.2, 2.0) - hue_shift(0.9, 2.0)).abs() > 1e-3); }
    #[test] fn shift0_zero() { assert_eq!(hue_shift(0.3, 0.0), 0.0); }
    #[test] fn wraps_large_shift() { let h = hue_shift(0.1, 10.0); assert!((0.0..1.0).contains(&h)); }
    #[test] fn deterministic() { assert_eq!(hue_shift(0.3, 2.0).to_bits(), hue_shift(0.3, 2.0).to_bits()); }
}
