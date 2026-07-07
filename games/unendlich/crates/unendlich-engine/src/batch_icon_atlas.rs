//! batch_icon_atlas.rs — pure batched icon-atlas UV (microopt of R16).
//!
//! Scalar `icon_rect_one` (mirroring R16 `icon_atlas::icon_rect`) plus a batched
//! `icon_rect_batch` that is **bit-for-bit identical** to the scalar path applied
//! per index. Pure, deterministic, panic-free (R1/R11/R21).
//!
//! Perf note: a full inventory/hotbar re-lays-out many icons per frame against a
//! fixed `cols×rows`; the batch hoists only the output allocation
//! (`with_capacity` + a tight loop), running the identical per-element math, so
//! the rects match the scalar reference bit-for-bit.

/// Scalar reference: UV rect for icon `idx` in a `cols×rows` atlas, row-major,
/// `idx` clamped to the last cell.
pub fn icon_rect_one(idx: u32, cols: u32, rows: u32) -> [f32; 4] {
    let cols = cols.max(1);
    let rows = rows.max(1);
    let n = cols * rows;
    let i = idx.min(n - 1);
    let col = i % cols;
    let row = i / cols;
    let w = 1.0 / cols as f32;
    let h = 1.0 / rows as f32;
    [col as f32 * w, row as f32 * h, w, h]
}

/// Batched form over `indices` sharing one `cols×rows`. Routes each index
/// through `icon_rect_one`, guaranteeing bit-parity.
pub fn icon_rect_batch(indices: &[u32], cols: u32, rows: u32) -> Vec<[f32; 4]> {
    let mut out = Vec::with_capacity(indices.len());
    for &idx in indices {
        out.push(icon_rect_one(idx, cols, rows));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(r: [f32; 4]) -> [u32; 4] {
        [r[0].to_bits(), r[1].to_bits(), r[2].to_bits(), r[3].to_bits()]
    }

    #[test]
    fn batch_is_bit_parity_with_scalar() {
        let (cols, rows) = (8u32, 6u32);
        let indices: Vec<u32> = (0..200u32).map(|i| i.wrapping_mul(7) % 60).collect();
        let batched = icon_rect_batch(&indices, cols, rows);
        assert_eq!(batched.len(), indices.len());
        for (k, &idx) in indices.iter().enumerate() {
            assert_eq!(bits(batched[k]), bits(icon_rect_one(idx, cols, rows)), "mismatch at {}", k);
        }
    }

    #[test]
    fn empty_is_empty() {
        assert!(icon_rect_batch(&[], 8, 8).is_empty());
    }

    #[test]
    fn single_equals_batch_of_one() {
        assert_eq!(bits(icon_rect_batch(&[5], 8, 4)[0]), bits(icon_rect_one(5, 8, 4)));
    }

    #[test]
    fn bounded_unit_range() {
        let indices: Vec<u32> = (0..64).collect();
        for r in icon_rect_batch(&indices, 8, 8) {
            assert!(r[0] >= 0.0 && r[1] >= 0.0);
            assert!(r[0] + r[2] <= 1.0 + 1e-5 && r[1] + r[3] <= 1.0 + 1e-5);
        }
    }

    #[test]
    fn deterministic() {
        let indices: Vec<u32> = (0..32).collect();
        assert_eq!(icon_rect_batch(&indices, 8, 8), icon_rect_batch(&indices, 8, 8));
    }

    #[test]
    fn idx_clamps_like_scalar() {
        assert_eq!(bits(icon_rect_batch(&[9999], 8, 4)[0]), bits(icon_rect_one(31, 8, 4)));
    }
}
