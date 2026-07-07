//! Perf: batched Verlet integration over all cloth particles, bit-identical to ref.

pub fn step_one(pos: f32, prev_pos: f32, accel: f32, dt: f32) -> f32 {
    let t = dt.max(0.0);
    pos + (pos - prev_pos) + accel * t * t
}

pub fn step_batch(items: &[(f32, f32, f32)], dt: f32) -> Vec<f32> {
    items.iter().map(|&(p, pp, a)| step_one(p, pp, a, dt)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(5.0f32, 5.0f32, 0.0f32), (6.0, 5.0, 0.0), (5.0, 5.0, 10.0), (3.0, 4.0, -9.8)];
        let b = step_batch(&items, 0.1);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), step_one(it.0, it.1, it.2, 0.1).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(step_batch(&[], 0.1).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(step_batch(&[(6.0, 5.0, 0.0)], 0.1)[0].to_bits(), step_one(6.0, 5.0, 0.0, 0.1).to_bits()); }
    #[test] fn at_rest_stays() { assert_eq!(step_batch(&[(5.0, 5.0, 0.0)], 0.1)[0], 5.0); }
    #[test] fn momentum_continues() { assert!((step_batch(&[(6.0, 5.0, 0.0)], 0.1)[0] - 7.0).abs() < 1e-6); }
    #[test] fn deterministic() { assert_eq!(step_batch(&[(6.0, 5.0, 0.0)], 0.1), step_batch(&[(6.0, 5.0, 0.0)], 0.1)); }
}
