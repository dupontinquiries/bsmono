//! Perf: batched burn DoT over many afflicted entities, bit-identical to ref.

pub fn tick_one(stacks: u8, base_per_stack: f32, dt: f32) -> f32 {
    (stacks as f32) * base_per_stack.max(0.0) * dt.max(0.0)
}

pub fn tick_batch(items: &[(u8, f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(s, b, t)| tick_one(s, b, t)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(3u8, 2.0f32, 0.1f32), (0, 5.0, 0.2), (5, 1.0, 0.5), (10, 0.5, 0.05)];
        let b = tick_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), tick_one(it.0, it.1, it.2).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(tick_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(tick_batch(&[(3, 2.0, 0.1)])[0].to_bits(), tick_one(3, 2.0, 0.1).to_bits()); }
    #[test] fn zero_stacks_zero() { assert_eq!(tick_one(0, 5.0, 0.2), 0.0); }
    #[test] fn more_stacks_more() { assert!(tick_one(5, 2.0, 0.1) > tick_one(1, 2.0, 0.1)); }
    #[test] fn deterministic() { assert_eq!(tick_batch(&[(3, 2.0, 0.1)]), tick_batch(&[(3, 2.0, 0.1)])); }
}
