//! Perf: batched river widths over a drainage network, bit-identical to ref.

pub fn width_one(flow_accumulation: f32, scale: f32, max_width: f32) -> f32 {
    let fa = flow_accumulation.max(0.0);
    let sc = scale.max(0.0);
    let mw = max_width.max(0.0);
    (sc * fa.sqrt()).min(mw)
}

pub fn width_batch(items: &[(f32, f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(fa, sc, mw)| width_one(fa, sc, mw)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(0.0f32, 0.5f32, 50.0f32), (100.0, 0.5, 50.0), (25.0, 0.5, 50.0), (1.0e6, 0.5, 50.0)];
        let b = width_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), width_one(it.0, it.1, it.2).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(width_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(width_batch(&[(100.0, 0.5, 50.0)])[0].to_bits(), width_one(100.0, 0.5, 50.0).to_bits()); }
    #[test] fn zero_flow_zero() { assert_eq!(width_batch(&[(0.0, 0.5, 50.0)])[0], 0.0); }
    #[test] fn capped() { assert_eq!(width_batch(&[(1.0e6, 0.5, 50.0)])[0], 50.0); }
    #[test] fn deterministic() { assert_eq!(width_batch(&[(100.0, 0.5, 50.0)]), width_batch(&[(100.0, 0.5, 50.0)])); }
}
