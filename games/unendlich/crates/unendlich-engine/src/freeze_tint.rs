//! Blue-white frost tint strength for a frozen entity: rises with freeze stacks,
//! saturating at full. In [0,1]. Pure, deterministic.

pub fn tint(stacks: u32, saturation_stacks: u32) -> f32 {
    let sat = saturation_stacks.max(1) as f32;
    ((stacks as f32) / sat).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn zero_clear() { assert_eq!(tint(0, 5), 0.0); }
    #[test] fn more_stronger() { assert!(tint(3, 5) > tint(1, 5)); }
    #[test] fn saturates() { assert_eq!(tint(100, 5), 1.0); }
    #[test] fn at_sat_full() { assert_eq!(tint(5, 5), 1.0); }
    #[test] fn bounded() { for s in [0u32, 2, 5, 100] { assert!((0.0..=1.0).contains(&tint(s, 5))); } }
    #[test] fn deterministic() { assert_eq!(tint(3, 5).to_bits(), tint(3, 5).to_bits()); }
}
