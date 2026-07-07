//! Combine a stat: (base + flat equipment bonus) × buff multiplier. Pure.

pub fn total(base: f32, equipment_bonus: f32, buff_mult: f32) -> f32 {
    (base.max(0.0) + equipment_bonus.max(0.0)) * buff_mult.max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn base_only() { assert_eq!(total(10.0, 0.0, 1.0), 10.0); }
    #[test] fn equipment_adds() { assert_eq!(total(10.0, 5.0, 1.0), 15.0); }
    #[test] fn buff_multiplies() { assert_eq!(total(10.0, 0.0, 2.0), 20.0); }
    #[test] fn zero_buff_zero() { assert_eq!(total(10.0, 5.0, 0.0), 0.0); }
    #[test] fn monotone_in_equipment() { assert!(total(10.0, 8.0, 1.0) > total(10.0, 2.0, 1.0)); }
    #[test] fn deterministic() { assert_eq!(total(10.0, 5.0, 1.5).to_bits(), total(10.0, 5.0, 1.5).to_bits()); }
}
