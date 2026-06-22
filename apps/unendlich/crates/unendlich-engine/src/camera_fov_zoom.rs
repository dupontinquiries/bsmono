//! Zoom-adjusted field of view (radians): zoom>1 narrows (telephoto), zoom<1
//! widens. Clamped to a sane range. Pure, deterministic.

pub fn fov(base_fov: f32, zoom: f32) -> f32 {
    let bf = base_fov.clamp(0.1, 3.0);
    let z = zoom.max(0.05);
    (bf / z).clamp(0.1, 3.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn zoom1_is_base() { assert!((fov(1.0, 1.0) - 1.0).abs() < 1e-6); }
    #[test] fn zoom_in_narrower() { assert!(fov(1.0, 2.0) < 1.0); }
    #[test] fn zoom_out_wider() { assert!(fov(1.0, 0.5) > 1.0); }
    #[test] fn clamped() { assert!(fov(1.0, 0.001) <= 3.0 && fov(1.0, 100.0) >= 0.1); }
    #[test] fn monotone() { assert!(fov(1.0, 3.0) < fov(1.0, 1.5)); }
    #[test] fn deterministic() { assert_eq!(fov(1.0, 2.0).to_bits(), fov(1.0, 2.0).to_bits()); }
}
