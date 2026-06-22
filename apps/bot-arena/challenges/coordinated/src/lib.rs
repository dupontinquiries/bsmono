//! Coordinated-edit challenge: a FITNESS VALLEY across the call graph.
//!
//! `t_checkout` only passes when BOTH `discount` and `checkout` are fixed together
//! — fixing either alone leaves the test failing, so a greedy single-edit climb
//! (which only accepts an edit that immediately improves the score) can never
//! accept the first half and gets stuck. Crossing it needs a multi-edit step that
//! flips both before scoring — and call-graph-aware localization (the test names
//! `checkout`, which calls `discount`) is what makes the coupled callee visible.
//! `t_enabled` is an ordinary single-edit defect so the baseline is > 0.

pub fn discount() -> i64 { 6 + 2 }            // BUG: should be 6 - 2     (flip-op); coupled
pub fn checkout() -> i64 { discount() + 10 }  // BUG: should be discount() - 10 (flip-op); coupled
pub fn enabled() -> bool { false }            // BUG: should be true       (flip-bool); independent

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn t_pass()     { assert_eq!(2 + 2, 4); }      // always passes
    #[test] fn t_enabled()  { assert!(enabled()); }         // single-edit fixable
    #[test] fn t_checkout() { assert_eq!(checkout(), -6); } // needs BOTH discount and checkout fixed
}
