//! Number of loot items dropped: +1 base per enemy tier plus a luck contribution,
//! capped at max_drops. Deterministic (random roll is upstream). Pure.

pub fn quantity(enemy_tier: u32, luck: f32, max_drops: u32) -> u32 {
    let luck_term = (luck.clamp(0.0, 1.0) * max_drops as f32) as u32;
    (enemy_tier + luck_term).min(max_drops)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn base_zero() { assert_eq!(quantity(0, 0.0, 5), 0); }
    #[test] fn tier_adds() { assert_eq!(quantity(3, 0.0, 10), 3); }
    #[test] fn luck_adds() { assert_eq!(quantity(0, 0.5, 10), 5); }
    #[test] fn capped() { assert_eq!(quantity(100, 1.0, 5), 5); }
    #[test] fn never_exceeds() { for t in [0u32, 3, 100] { assert!(quantity(t, 1.0, 4) <= 4); } }
    #[test] fn deterministic() { assert_eq!(quantity(3, 0.5, 10), quantity(3, 0.5, 10)); }
}
