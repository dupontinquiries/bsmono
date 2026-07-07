//! How aggressive a mob should be in [0,1]: rises with own health and ally
//! numerical advantage, falls when hurt or outnumbered. Pure, deterministic.

pub fn aggression(health_frac: f32, ally_count: u32, enemy_count: u32) -> f32 {
    let hp = health_frac.clamp(0.0, 1.0);
    let allies = ally_count as f32 + 1.0;
    let enemies = (enemy_count as f32).max(1.0);
    let advantage = allies / (allies + enemies);
    (hp * advantage * 2.0).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn hurt_lower() { assert!(aggression(0.3, 2, 2) < aggression(1.0, 2, 2)); }
    #[test] fn outnumbered_lower() { assert!(aggression(1.0, 0, 5) < aggression(1.0, 5, 0)); }
    #[test] fn more_allies_more() { assert!(aggression(0.5, 5, 2) > aggression(0.5, 0, 2)); }
    #[test] fn full_when_dominant() { assert_eq!(aggression(1.0, 5, 1), 1.0); }
    #[test] fn bounded() { for h in [0.0f32, 0.5, 1.0] { for e in [0u32, 3, 9] { assert!((0.0..=1.0).contains(&aggression(h, 2, e))); } } }
    #[test] fn deterministic() { assert_eq!(aggression(0.5, 2, 2).to_bits(), aggression(0.5, 2, 2).to_bits()); }
}
