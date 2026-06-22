//! Geometric specular anti-aliasing: widen roughness where surface normals vary
//! (Toksvig-style) to kill spec shimmer on merged/LOD'd meshes
//! (handoff/BODY_PART_MESHING.md). Never decreases roughness; output in [0,1].

pub fn clamp_roughness(roughness: f32, normal_variance: f32) -> f32 {
    let r = roughness.clamp(0.0, 1.0);
    let v = normal_variance.max(0.0);
    let widened = (r * r + v).sqrt().clamp(0.0, 1.0);
    widened.max(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn no_variance_unchanged() { assert!((clamp_roughness(0.3, 0.0) - 0.3).abs() < 1e-6); }
    #[test] fn more_variance_rougher() { assert!(clamp_roughness(0.3, 0.5) > clamp_roughness(0.3, 0.0)); }
    #[test] fn bounded() {
        let mut v = 0.0f32;
        while v < 5.0 { let r = clamp_roughness(0.4, v); assert!((0.0..=1.0).contains(&r)); v += 0.1; }
    }
    #[test] fn monotone_in_variance() {
        let (mut prev, mut v) = (f32::MIN, 0.0f32);
        while v < 2.0 { let r = clamp_roughness(0.2, v); assert!(r >= prev); prev = r; v += 0.05; }
    }
    #[test] fn never_decreases() {
        for &(r, v) in &[(0.1f32, 0.0f32), (0.5, 0.01), (0.9, 0.2)] { assert!(clamp_roughness(r, v) >= r - 1e-6); }
    }
    #[test] fn deterministic() { assert_eq!(clamp_roughness(0.3, 0.2).to_bits(), clamp_roughness(0.3, 0.2).to_bits()); }
}
