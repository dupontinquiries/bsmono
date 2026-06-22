//! Perf: batched corridor lengths over all room-pair connections, bit-identical to ref.

pub fn len_one(ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    (bx - ax).abs() + (by - ay).abs()
}

pub fn len_batch(items: &[(f32, f32, f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(ax, ay, bx, by)| len_one(ax, ay, bx, by)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(0.0f32, 0.0f32, 5.0f32, 0.0f32), (0.0, 0.0, 0.0, 3.0), (0.0, 0.0, 3.0, 4.0), (1.0, 2.0, 5.0, 6.0)];
        let b = len_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), len_one(it.0, it.1, it.2, it.3).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(len_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(len_batch(&[(0.0, 0.0, 3.0, 4.0)])[0].to_bits(), len_one(0.0, 0.0, 3.0, 4.0).to_bits()); }
    #[test] fn l_shape() { assert_eq!(len_batch(&[(0.0, 0.0, 3.0, 4.0)])[0], 7.0); }
    #[test] fn same_zero() { assert_eq!(len_batch(&[(0.0, 0.0, 0.0, 0.0)])[0], 0.0); }
    #[test] fn deterministic() { assert_eq!(len_batch(&[(1.0, 2.0, 5.0, 6.0)]), len_batch(&[(1.0, 2.0, 5.0, 6.0)])); }
}
