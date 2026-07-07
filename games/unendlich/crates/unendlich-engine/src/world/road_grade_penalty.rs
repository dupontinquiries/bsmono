//! Movement-cost penalty multiplier for a road segment by slope (grade): flat = 1,
//! rising with grade (steep roads cost more), capped. Direction-agnostic. Pure.

pub fn penalty(grade: f32, steepness_cost: f32, max_penalty: f32) -> f32 {
    let g = grade.abs();
    let sc = steepness_cost.max(0.0);
    let mp = max_penalty.max(1.0);
    (1.0 + g * sc).min(mp)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn flat_one() { assert_eq!(penalty(0.0, 5.0, 10.0), 1.0); }
    #[test] fn steeper_more() { assert!(penalty(0.5, 5.0, 10.0) > penalty(0.1, 5.0, 10.0)); }
    #[test] fn symmetric() { assert_eq!(penalty(-0.3, 5.0, 10.0), penalty(0.3, 5.0, 10.0)); }
    #[test] fn capped() { assert_eq!(penalty(100.0, 5.0, 10.0), 10.0); }
    #[test] fn never_below_one() { assert!(penalty(0.0, 5.0, 10.0) >= 1.0); }
    #[test] fn deterministic() { assert_eq!(penalty(0.3, 5.0, 10.0).to_bits(), penalty(0.3, 5.0, 10.0).to_bits()); }
}
