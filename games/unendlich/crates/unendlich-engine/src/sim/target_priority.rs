//! Target-selection score: higher for higher-threat, closer, and lower-HP targets
//! (finish the wounded). Pure, deterministic.

pub fn priority(threat: f32, distance: f32, target_hp_frac: f32, low_hp_weight: f32) -> f32 {
    let t = threat.max(0.0);
    let prox = 1.0 / (1.0 + distance.max(0.0));
    let hp = target_hp_frac.clamp(0.0, 1.0);
    let finisher = 1.0 + low_hp_weight.max(0.0) * (1.0 - hp);
    t * prox * finisher
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn higher_threat_more() { assert!(priority(10.0, 5.0, 1.0, 1.0) > priority(2.0, 5.0, 1.0, 1.0)); }
    #[test] fn closer_more() { assert!(priority(10.0, 1.0, 1.0, 1.0) > priority(10.0, 10.0, 1.0, 1.0)); }
    #[test] fn wounded_more() { assert!(priority(10.0, 5.0, 0.1, 1.0) > priority(10.0, 5.0, 0.9, 1.0)); }
    #[test] fn zero_threat_zero() { assert_eq!(priority(0.0, 5.0, 0.5, 1.0), 0.0); }
    #[test] fn nonneg() { assert!(priority(10.0, 5.0, 0.5, 1.0) >= 0.0); }
    #[test] fn deterministic() { assert_eq!(priority(10.0, 5.0, 0.5, 1.0).to_bits(), priority(10.0, 5.0, 0.5, 1.0).to_bits()); }
}
