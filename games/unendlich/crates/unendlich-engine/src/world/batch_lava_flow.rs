//! Perf: batched lava-flow rate, bit-identical to the scalar reference.
//! Mirrors lava_flow: clamp01(slope * amount / (1 + viscosity)).

pub fn flow_one(slope: f32, viscosity: f32, amount: f32) -> f32 {
    let s = slope.max(0.0);
    let a = amount.max(0.0);
    let v = viscosity.max(0.0);
    (s * a / (1.0 + v)).clamp(0.0, 1.0)
}

pub fn flow_batch(items: &[(f32, f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(s, v, a)| flow_one(s, v, a)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn bit_parity() {
        let items = [(0.5f32, 0.2f32, 0.8f32), (0.0, 1.0, 1.0), (2.0, 0.0, 1.0), (0.3, 3.0, 0.5)];
        let b = flow_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), flow_one(it.0, it.1, it.2).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(flow_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(flow_batch(&[(0.4, 0.5, 0.6)])[0].to_bits(), flow_one(0.4, 0.5, 0.6).to_bits()); }
    #[test] fn flat_no_flow() { assert_eq!(flow_one(0.0, 0.1, 1.0), 0.0); }
    #[test] fn viscous_less() { assert!(flow_one(0.5, 5.0, 1.0) < flow_one(0.5, 0.0, 1.0)); }
    #[test] fn deterministic() { assert_eq!(flow_batch(&[(0.5, 0.5, 0.5)]), flow_batch(&[(0.5, 0.5, 0.5)])); }
}
