//! Whether a bridge is required to cross a gap: true if the gap exceeds what fill
//! or a single span can cover (max_fillable). Pure, deterministic.

pub fn needed(gap_width: f32, max_fillable: f32) -> bool {
    gap_width.max(0.0) > max_fillable.max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn narrow_no() { assert!(!needed(5.0, 10.0)); }
    #[test] fn wide_yes() { assert!(needed(20.0, 10.0)); }
    #[test] fn exact_no() { assert!(!needed(10.0, 10.0)); }
    #[test] fn just_over_yes() { assert!(needed(10.1, 10.0)); }
    #[test] fn zero_gap_no() { assert!(!needed(0.0, 10.0)); }
    #[test] fn deterministic() { assert_eq!(needed(20.0, 10.0), needed(20.0, 10.0)); }
}
