//! Warm-up challenge for the arena's Local backend.
//!
//! Every defect below is a SINGLE one-token error that a basic mutation operator
//! can repair — a wrong number (tweak-num), a flipped comparison (flip-op), a
//! flipped boolean (flip-bool), or a wrong arithmetic op (math-op). The code
//! COMPILES cleanly; the failures are logic errors caught by tests, not build
//! errors. Two tests pass on a clean checkout and three fail, so the baseline is
//! 2/5 = 0.40 — proof the harness scores real Rust as a fraction. The arena's job
//! is to mutate these toward 1.0 without touching the tests.

pub fn double(x: i64) -> i64 { x * 2 }              // correct
pub fn answer() -> i64 { 41 }                        // BUG: should be 42      (tweak-num)
pub fn greater(a: i64, b: i64) -> bool { a < b }     // BUG: should be a > b    (flip-op)
pub fn always_true() -> bool { false }               // BUG: should be true     (flip-bool)

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn t_double_pos() { assert_eq!(double(21), 42); }   // PASS
    #[test] fn t_double_neg() { assert_eq!(double(-3), -6); }   // PASS
    #[test] fn t_answer()     { assert_eq!(answer(), 42); }     // FAIL → tweak-num can fix
    #[test] fn t_greater()    { assert!(greater(5, 2)); }       // FAIL → flip-op can fix
    #[test] fn t_always()     { assert!(always_true()); }       // FAIL → flip-bool can fix
}
