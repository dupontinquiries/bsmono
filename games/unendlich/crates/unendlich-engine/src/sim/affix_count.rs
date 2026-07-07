//! Number of affixes on an item: +1 per 20 item levels plus a luck contribution,
//! capped at max_affixes. Deterministic (the random roll is upstream). Pure.

pub fn affix_count(item_level: u32, luck: f32, max_affixes: u32) -> u32 {
    let lvl_term = item_level / 20;
    let luck_term = (luck.clamp(0.0, 1.0) * max_affixes as f32) as u32;
    (lvl_term + luck_term).min(max_affixes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn base_zero() { assert_eq!(affix_count(0, 0.0, 6), 0); }
    #[test] fn level_adds() { assert_eq!(affix_count(60, 0.0, 6), 3); }
    #[test] fn luck_adds() { assert_eq!(affix_count(0, 0.5, 6), 3); }
    #[test] fn capped() { assert_eq!(affix_count(200, 1.0, 6), 6); }
    #[test] fn never_exceeds_max() { for lvl in [0u32, 50, 200] { assert!(affix_count(lvl, 1.0, 4) <= 4); } }
    #[test] fn deterministic() { assert_eq!(affix_count(60, 0.5, 6), affix_count(60, 0.5, 6)); }
}
