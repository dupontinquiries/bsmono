//! Arithmetic challenge: each defect is one math-op or tweak-num away from
//! correct. Compiles cleanly; one test passes on a clean checkout (baseline
//! 1/4 = 0.25).

pub fn add(a: i64, b: i64) -> i64 { a - b }   // BUG: should be a + b   (math-op)
pub fn triple(x: i64) -> i64 { x * 2 }        // BUG: should be x * 3   (tweak-num)
pub fn square(x: i64) -> i64 { x * x }        // correct
pub fn half(x: i64) -> i64 { x + 2 }          // BUG: should be x / 2   (math-op)

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn t_add()    { assert_eq!(add(2, 3), 5); }    // FAIL → math-op
    #[test] fn t_triple() { assert_eq!(triple(4), 12); }   // FAIL → tweak-num
    #[test] fn t_square() { assert_eq!(square(5), 25); }   // PASS
    #[test] fn t_half()   { assert_eq!(half(10), 5); }     // FAIL → math-op
}
