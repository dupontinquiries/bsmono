//! Q-learning-style value update: prior + learn_rate·(reward − prior). Pure.

pub fn update(prior: f32, reward: f32, learn_rate: f32) -> f32 {
    let lr = learn_rate.clamp(0.0, 1.0);
    prior + (reward - prior) * lr
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn lr0_unchanged() { assert_eq!(update(0.5, 2.0, 0.0), 0.5); }
    #[test] fn lr1_takes_reward() { assert!((update(0.5, 2.0, 1.0) - 2.0).abs() < 1e-6); }
    #[test] fn moves_toward_reward() { let v = update(0.5, 2.0, 0.5); assert!(v > 0.5 && v < 2.0); }
    #[test] fn positive_reward_raises() { assert!(update(0.5, 2.0, 0.3) > 0.5); }
    #[test] fn negative_reward_lowers() { assert!(update(0.5, -1.0, 0.3) < 0.5); }
    #[test] fn deterministic() { assert_eq!(update(0.5, 2.0, 0.3).to_bits(), update(0.5, 2.0, 0.3).to_bits()); }
}
