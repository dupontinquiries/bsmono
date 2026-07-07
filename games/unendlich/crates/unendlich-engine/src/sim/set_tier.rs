//! Active set-bonus tier = how many `step`-piece thresholds the equipped count
//! meets (step=2 ⇒ bonuses unlock at 2/4/6 pieces). Pure, deterministic.

pub fn set_tier(pieces_equipped: u32, step: u32) -> u32 {
    pieces_equipped / step.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn below_first_zero() { assert_eq!(set_tier(1, 2), 0); }
    #[test] fn at_step_one() { assert_eq!(set_tier(2, 2), 1); }
    #[test] fn two_tiers() { assert_eq!(set_tier(4, 2), 2); }
    #[test] fn step1_equals_pieces() { assert_eq!(set_tier(5, 1), 5); }
    #[test] fn zero_pieces_zero() { assert_eq!(set_tier(0, 2), 0); }
    #[test] fn deterministic() { assert_eq!(set_tier(4, 2), set_tier(4, 2)); }
}
