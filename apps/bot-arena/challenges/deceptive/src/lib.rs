//! Deceptive coordinated valley: the intermediate state is strictly WORSE.
//!
//! Both flags are wrong (`true`) but CONSISTENT, so the two consistency tests pass.
//! Flipping either flag alone breaks BOTH consistency tests AND still fails the
//! both-off test → the score drops. So neutral drift can't help (it only accepts
//! score-≥ moves, and every single flip is strictly worse). The ONLY improving move
//! is the atomic two-edit bundle that flips both flags at once. This is the case
//! that multi-edit bundling exists for.

pub fn left_flag() -> bool { true }    // BUG: should be false (coupled with right_flag)
pub fn right_flag() -> bool { true }   // BUG: should be false (coupled with left_flag)

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn t_consistent_a() { assert_eq!(left_flag(), right_flag()); }      // PASS (both true)
    #[test] fn t_consistent_b() { assert!(left_flag() == right_flag()); }       // PASS (both true)
    #[test] fn t_both_off()     { assert!(!left_flag() && !right_flag()); }     // FAIL → needs BOTH false
}
