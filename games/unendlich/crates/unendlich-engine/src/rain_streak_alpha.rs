//! Rain-streak opacity [0,1]: denser rain ⇒ more opaque, fading with distance from
//! the camera to 0 at fade_dist. Pure, deterministic.

pub fn alpha(intensity: f32, distance: f32, fade_dist: f32) -> f32 {
    let i = intensity.clamp(0.0, 1.0);
    let d = distance.max(0.0);
    let fd = fade_dist.max(1e-4);
    (i * (1.0 - d / fd).clamp(0.0, 1.0)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn no_rain_clear() { assert_eq!(alpha(0.0, 1.0, 50.0), 0.0); }
    #[test] fn near_heavy_high() { assert!(alpha(1.0, 0.0, 50.0) > 0.9); }
    #[test] fn far_faded() { assert!(alpha(1.0, 40.0, 50.0) < alpha(1.0, 5.0, 50.0)); }
    #[test] fn beyond_fade_zero() { assert_eq!(alpha(1.0, 60.0, 50.0), 0.0); }
    #[test] fn more_intensity_more() { assert!(alpha(0.8, 10.0, 50.0) > alpha(0.2, 10.0, 50.0)); }
    #[test] fn deterministic() { assert_eq!(alpha(0.5, 10.0, 50.0).to_bits(), alpha(0.5, 10.0, 50.0).to_bits()); }
}
