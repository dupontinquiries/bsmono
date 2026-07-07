//! Nest-site quality in [0,1]: dragons favor high, sheltered crags (elevation
//! optimum ≈0.7) with prey nearby. Needs shelter AND prey. Pure, deterministic.

pub fn suitability(elevation_norm: f32, shelter: f32, prey_density: f32) -> f32 {
    let e = elevation_norm.clamp(0.0, 1.0);
    let sh = shelter.clamp(0.0, 1.0);
    let prey = prey_density.clamp(0.0, 1.0);
    let elev_fit = (-((e - 0.7) * (e - 0.7)) / 0.1).exp();
    (elev_fit * sh * prey).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn no_shelter_zero() { assert_eq!(suitability(0.7, 0.0, 1.0), 0.0); }
    #[test] fn no_prey_zero() { assert_eq!(suitability(0.7, 1.0, 0.0), 0.0); }
    #[test] fn ideal_high() { assert!(suitability(0.7, 1.0, 1.0) > 0.9); }
    #[test] fn wrong_elevation_lower() { assert!(suitability(0.0, 1.0, 1.0) < suitability(0.7, 1.0, 1.0)); }
    #[test] fn bounded() {
        for e in [0.0f32, 0.3, 0.7, 1.0] { for s in [0.0f32, 0.5, 1.0] { let v = suitability(e, s, 0.8); assert!((0.0..=1.0).contains(&v)); } }
    }
    #[test] fn deterministic() { assert_eq!(suitability(0.7, 1.0, 1.0).to_bits(), suitability(0.7, 1.0, 1.0).to_bits()); }
}
