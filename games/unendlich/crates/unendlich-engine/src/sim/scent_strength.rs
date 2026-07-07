//! Scent detection in [0,1]: falls with distance, boosted when the AI is downwind
//! (wind_alignment in [-1,1]: +1 = scent blows toward the AI). Pure, deterministic.

pub fn strength(distance: f32, wind_alignment: f32, decay: f32, max_range: f32) -> f32 {
    let mr = max_range.max(1e-6);
    let d = distance.max(0.0);
    if d >= mr {
        return 0.0;
    }
    let base = (-(d * decay.max(0.0))).exp();
    let wind = (0.5 + 0.5 * wind_alignment.clamp(-1.0, 1.0)).clamp(0.0, 1.0);
    (base * wind).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn close_downwind_strong() { assert!(strength(0.0, 1.0, 0.1, 100.0) > 0.99); }
    #[test] fn upwind_zero() { assert_eq!(strength(5.0, -1.0, 0.1, 100.0), 0.0); }
    #[test] fn far_zero() { assert_eq!(strength(200.0, 1.0, 0.1, 100.0), 0.0); }
    #[test] fn closer_stronger() { assert!(strength(2.0, 1.0, 0.1, 100.0) > strength(40.0, 1.0, 0.1, 100.0)); }
    #[test] fn downwind_more_than_upwind() { assert!(strength(5.0, 1.0, 0.1, 100.0) > strength(5.0, -0.5, 0.1, 100.0)); }
    #[test] fn deterministic() { assert_eq!(strength(5.0, 1.0, 0.1, 100.0).to_bits(), strength(5.0, 1.0, 0.1, 100.0).to_bits()); }
}
