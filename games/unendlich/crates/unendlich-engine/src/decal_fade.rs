//! Decal alpha by distance from the projected surface: full on-surface, fading to
//! 0 at `thickness` away so decals don't float in air. In [0,1]. Pure.

pub fn fade(distance_to_surface: f32, thickness: f32) -> f32 {
    let t = thickness.max(1e-6);
    let d = distance_to_surface.abs();
    if d >= t {
        0.0
    } else {
        crate::util::mathx::clampf(1.0 - d / t, 0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn on_surface_full() { assert!((fade(0.0, 0.5) - 1.0).abs() < 1e-6); }
    #[test] fn at_thickness_zero() { assert_eq!(fade(0.5, 0.5), 0.0); }
    #[test] fn beyond_zero() { assert_eq!(fade(1.0, 0.5), 0.0); }
    #[test] fn symmetric() { assert_eq!(fade(0.2, 0.5), fade(-0.2, 0.5)); }
    #[test] fn closer_stronger() { assert!(fade(0.1, 0.5) > fade(0.4, 0.5)); }
    #[test] fn deterministic() { assert_eq!(fade(0.2, 0.5).to_bits(), fade(0.2, 0.5).to_bits()); }
}
