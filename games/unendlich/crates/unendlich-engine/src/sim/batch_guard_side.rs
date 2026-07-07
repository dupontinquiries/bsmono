//! Perf: batched guard-side decision, bit-identical to the scalar reference.
//! Mirrors guard_side: present the higher-HP side (within hysteresis ⇒ square).

pub fn present_one(left_hp: f32, right_hp: f32, hysteresis: f32) -> i8 {
    let h = hysteresis.max(0.0);
    let d = left_hp - right_hp;
    if d > h { -1 } else if d < -h { 1 } else { 0 }
}

pub fn present_batch(items: &[(f32, f32)], hysteresis: f32) -> Vec<i8> {
    items.iter().map(|&(l, r)| present_one(l, r, hysteresis)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(100.0f32, 40.0f32), (40.0, 100.0), (70.0, 70.0), (72.0, 70.0)];
        let b = present_batch(&items, 5.0);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i], present_one(it.0, it.1, 5.0)); }
    }
    #[test] fn empty_is_empty() { assert!(present_batch(&[], 5.0).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(present_batch(&[(90.0, 30.0)], 3.0)[0], present_one(90.0, 30.0, 3.0)); }
    #[test] fn covers_all_three() {
        let b = present_batch(&[(90.0, 10.0), (10.0, 90.0), (50.0, 50.0)], 1.0);
        assert_eq!(b, vec![-1, 1, 0]);
    }
    #[test] fn range_valid() { for v in present_batch(&[(1.0, 2.0), (3.0, 1.0)], 0.5) { assert!((-1..=1).contains(&(v as i32))); } }
    #[test] fn deterministic() { assert_eq!(present_batch(&[(5.0, 5.0)], 1.0), present_batch(&[(5.0, 5.0)], 1.0)); }
}
