//! Reconstruct view-space Z (positive distance) from non-linear NDC depth [0,1]
//! for a perspective projection with `near`/`far` planes. Pure, deterministic.

pub fn view_z(ndc_depth: f32, near: f32, far: f32) -> f32 {
    let d = ndc_depth.clamp(0.0, 1.0);
    let n = near.max(1e-4);
    let f = far.max(n + 1e-4);
    (n * f) / (f - d * (f - n))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn depth0_is_near() { assert!((view_z(0.0, 0.1, 100.0) - 0.1).abs() < 1e-3); }
    #[test] fn depth1_is_far() { assert!((view_z(1.0, 0.1, 100.0) - 100.0).abs() < 1e-1); }
    #[test] fn monotone_increasing() {
        let (mut prev, mut d) = (f32::MIN, 0.0f32);
        while d <= 1.0 { let z = view_z(d, 0.1, 100.0); assert!(z >= prev - 1e-3); prev = z; d += 0.05; }
    }
    #[test] fn bounded() {
        let mut d = 0.0f32;
        while d <= 1.0 { let z = view_z(d, 0.1, 100.0); assert!((0.1 - 1e-2..=100.0 + 1e-1).contains(&z)); d += 0.05; }
    }
    #[test] fn guarded_ordering() { assert!(view_z(0.5, 100.0, 0.1).is_finite()); }
    #[test] fn deterministic() { assert_eq!(view_z(0.5, 0.1, 100.0).to_bits(), view_z(0.5, 0.1, 100.0).to_bits()); }
}
