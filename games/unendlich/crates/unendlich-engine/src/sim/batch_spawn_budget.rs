//! Perf: batched spawn-budget evaluation, bit-identical to the scalar reference.
//! Mirrors the spawn_budget rule (allowed = min(pending, budget, cap-active)).

pub fn allowed_one(pending: u32, budget: u32, active: u32, cap: u32) -> u32 {
    let headroom = cap.saturating_sub(active);
    pending.min(budget).min(headroom)
}

pub fn allowed_batch(items: &[(u32, u32, u32, u32)]) -> Vec<u32> {
    items.iter().map(|&(p, b, a, c)| allowed_one(p, b, a, c)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn bit_parity() {
        let items = [(10u32, 5u32, 2u32, 8u32), (3, 9, 0, 4), (100, 100, 90, 95), (0, 5, 0, 5)];
        let b = allowed_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i], allowed_one(it.0, it.1, it.2, it.3)); }
    }
    #[test] fn empty_is_empty() { assert!(allowed_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(allowed_batch(&[(7, 4, 1, 6)])[0], allowed_one(7, 4, 1, 6)); }
    #[test] fn respects_cap() { assert_eq!(allowed_one(10, 10, 9, 10), 1); }
    #[test] fn bounded_by_budget() { assert!(allowed_one(100, 5, 0, 100) <= 5); }
    #[test] fn deterministic() { assert_eq!(allowed_batch(&[(5, 5, 5, 5)]), allowed_batch(&[(5, 5, 5, 5)])); }
}
