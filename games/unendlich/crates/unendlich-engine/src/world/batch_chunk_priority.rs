//! `batch_chunk_priority` — batched chunk-priority compute + sort.
//!
//! Worker-03 / WORLD pure core, round 15 (perf bonus). A categorical microopt:
//! compute many chunk priorities in one pass and sort them, while guaranteeing
//! the batch path produces **bit-identical** scores to the scalar reference
//! (which mirrors `chunk_priority` R11). Bit-parity matters because the streamer
//! compares scores for equality; any drift between a scalar fallback and a
//! batched path would reorder work nondeterministically.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Added when a chunk is inside the view frustum (mirrors R11).
pub const FRUSTUM_BOOST: f32 = 1.0;
/// Added when a chunk has pending edits (mirrors R11).
pub const DIRTY_BOOST: f32 = 0.5;

/// Scalar reference priority — identical formula to `chunk_priority::priority`.
pub fn priority_one(dist: f32, in_frustum: bool, dirty: bool) -> f32 {
    let d = if dist.is_nan() || dist < 0.0 { 0.0 } else { dist };
    let mut p = 1.0 / (1.0 + d);
    if in_frustum {
        p += FRUSTUM_BOOST;
    }
    if dirty {
        p += DIRTY_BOOST;
    }
    p
}

/// Batched priority over `(dist, in_frustum, dirty)` inputs. Each element goes
/// through the exact scalar `priority_one`, so the result is bit-for-bit equal
/// to calling it per item — no fast-math reassociation.
pub fn priority_batch(items: &[(f32, bool, bool)]) -> Vec<f32> {
    items
        .iter()
        .map(|&(d, f, dy)| priority_one(d, f, dy))
        .collect()
}

/// Stable descending sort of `(id, priority)` (mirrors R11 `order`).
pub fn order(items: &mut Vec<(usize, f32)>) {
    items.sort_by(|a, b| {
        if a.1 > b.1 {
            std::cmp::Ordering::Less
        } else if a.1 < b.1 {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_is_bit_parity_with_scalar() {
        let items = [
            (0.0f32, false, false),
            (5.0, true, false),
            (12.5, false, true),
            (-3.0, true, true), // negative clamps
            (f32::NAN, false, false),
            (1e6, true, true),
        ];
        let batch = priority_batch(&items);
        for (i, &(d, f, dy)) in items.iter().enumerate() {
            let scalar = priority_one(d, f, dy);
            assert_eq!(batch[i].to_bits(), scalar.to_bits(), "bit drift at {i}");
        }
    }

    #[test]
    fn empty_is_empty() {
        assert!(priority_batch(&[]).is_empty());
        let mut v: Vec<(usize, f32)> = Vec::new();
        order(&mut v);
        assert!(v.is_empty());
    }

    #[test]
    fn single_item() {
        let b = priority_batch(&[(4.0, true, false)]);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].to_bits(), priority_one(4.0, true, false).to_bits());
    }

    #[test]
    fn bounded() {
        let max = 1.0 + FRUSTUM_BOOST + DIRTY_BOOST;
        for &(d, f, dy) in &[(0.0f32, true, true), (50.0, false, false), (1e9, true, true)] {
            let p = priority_one(d, f, dy);
            assert!(p >= 0.0 && p <= max + 1e-6);
        }
    }

    #[test]
    fn order_is_descending_and_stable() {
        let mut v = vec![(5, 1.0f32), (3, 1.0), (8, 1.0), (2, 2.0), (9, 0.5)];
        order(&mut v);
        assert_eq!(
            v.iter().map(|x| x.0).collect::<Vec<_>>(),
            vec![2, 5, 3, 8, 9] // 2 highest; the three 1.0s keep insertion order
        );
    }

    #[test]
    fn deterministic() {
        let items = [(1.0f32, true, false), (1.0, true, false)];
        assert_eq!(priority_batch(&items), priority_batch(&items));
        let mut a = vec![(0, 0.3f32), (1, 0.3), (2, 0.9)];
        let mut b = a.clone();
        order(&mut a);
        order(&mut b);
        assert_eq!(a, b);
    }
}
