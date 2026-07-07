//! Damage remaining after a block absorbs up to block_value: max(incoming−block,0).
//! Pure, deterministic.

pub fn after_block(incoming: f32, block_value: f32) -> f32 {
    (incoming.max(0.0) - block_value.max(0.0)).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn partial_block_reduces() { assert_eq!(after_block(100.0, 30.0), 70.0); }
    #[test] fn full_block_zero() { assert_eq!(after_block(100.0, 200.0), 0.0); }
    #[test] fn no_block_full() { assert_eq!(after_block(100.0, 0.0), 100.0); }
    #[test] fn exact_block_zero() { assert_eq!(after_block(100.0, 100.0), 0.0); }
    #[test] fn nonneg() { assert!(after_block(50.0, 80.0) >= 0.0); }
    #[test] fn deterministic() { assert_eq!(after_block(100.0, 30.0).to_bits(), after_block(100.0, 30.0).to_bits()); }
}
