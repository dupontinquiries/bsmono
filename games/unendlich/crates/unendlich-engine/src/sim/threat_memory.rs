//! Update remembered threat toward a fresh observation by learn_rate (EMA).
//! In [0,1]. Pure, deterministic.

pub fn update(current: f32, observed: f32, learn_rate: f32) -> f32 {
    let c = current.clamp(0.0, 1.0);
    let o = observed.clamp(0.0, 1.0);
    let lr = learn_rate.clamp(0.0, 1.0);
    (c + (o - c) * lr).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn lr0_unchanged() { assert_eq!(update(0.3, 0.9, 0.0), 0.3); }
    #[test] fn lr1_takes_observed() { assert!((update(0.3, 0.9, 1.0) - 0.9).abs() < 1e-6); }
    #[test] fn moves_toward_observed() { let v = update(0.3, 0.9, 0.5); assert!(v > 0.3 && v < 0.9); }
    #[test] fn observed_higher_raises() { assert!(update(0.2, 0.8, 0.5) > 0.2); }
    #[test] fn bounded() { for c in [0.0f32, 0.5, 1.0] { for o in [0.0f32, 0.5, 1.0] { assert!((0.0..=1.0).contains(&update(c, o, 0.5))); } } }
    #[test] fn deterministic() { assert_eq!(update(0.3, 0.9, 0.5).to_bits(), update(0.3, 0.9, 0.5).to_bits()); }
}
