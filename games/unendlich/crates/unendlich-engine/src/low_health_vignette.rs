//! Red screen-vignette intensity as HP drops: 0 above the onset HP fraction, rising
//! toward 1 as HP approaches 0. In [0,1]. Pure, deterministic.

pub fn vignette(hp_frac: f32, onset: f32) -> f32 {
    let hp = hp_frac.clamp(0.0, 1.0);
    let on = onset.clamp(1e-4, 1.0);
    if hp >= on { 0.0 } else { ((on - hp) / on).clamp(0.0, 1.0) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn full_none() { assert_eq!(vignette(1.0, 0.3), 0.0); }
    #[test] fn above_onset_none() { assert_eq!(vignette(0.5, 0.3), 0.0); }
    #[test] fn empty_full() { assert!((vignette(0.0, 0.3) - 1.0).abs() < 1e-6); }
    #[test] fn lower_more() { assert!(vignette(0.1, 0.3) > vignette(0.2, 0.3)); }
    #[test] fn bounded() { let mut h = 0.0f32; while h <= 1.0 { assert!((0.0..=1.0).contains(&vignette(h, 0.3))); h += 0.05; } }
    #[test] fn deterministic() { assert_eq!(vignette(0.1, 0.3).to_bits(), vignette(0.1, 0.3).to_bits()); }
}
