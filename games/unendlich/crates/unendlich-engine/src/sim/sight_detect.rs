//! Visual detection strength in [0,1]: the target must be within the FOV (its
//! direction dot ≥ fov_cos) and within max_range; closer ⇒ stronger. Pure.

pub fn detect(distance: f32, target_dir_dot: f32, fov_cos: f32, max_range: f32) -> f32 {
    let mr = max_range.max(1e-6);
    let d = distance.max(0.0);
    if target_dir_dot < fov_cos || d >= mr {
        0.0
    } else {
        (1.0 - d / mr).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn front_near_strong() { assert!(detect(1.0, 0.9, 0.5, 50.0) > 0.9); }
    #[test] fn behind_zero() { assert_eq!(detect(1.0, 0.0, 0.5, 50.0), 0.0); }
    #[test] fn too_far_zero() { assert_eq!(detect(60.0, 0.9, 0.5, 50.0), 0.0); }
    #[test] fn closer_stronger() { assert!(detect(5.0, 0.9, 0.5, 50.0) > detect(40.0, 0.9, 0.5, 50.0)); }
    #[test] fn fov_edge_detected() { assert!(detect(5.0, 0.5, 0.5, 50.0) > 0.0); }
    #[test] fn deterministic() { assert_eq!(detect(5.0, 0.9, 0.5, 50.0).to_bits(), detect(5.0, 0.9, 0.5, 50.0).to_bits()); }
}
