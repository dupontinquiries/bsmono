//! impostor_uv.rs — pure impostor-atlas UV selection.
//!
//! Given a captured angle index, returns the `[x, y, w, h]` rectangle (all in
//! `[0,1]`) of that capture's cell in the impostor atlas, laid out row-major
//! over `atlas_cols` columns. Pairs with `impostor_card` (R10), which picks the
//! angle index. Pure, deterministic, panic-free (R1/R11/R21).

/// UV rectangle for capture `angle_idx` of `n_angles` total, in an atlas with
/// `atlas_cols` columns (rows derived to fit all captures). Returns
/// `[x, y, w, h]`, each in `[0,1]`. `angle_idx` is clamped to `< n_angles`;
/// `n_angles` and `atlas_cols` are treated as at least 1.
pub fn uv_rect(angle_idx: u32, n_angles: u32, atlas_cols: u32) -> [f32; 4] {
    let n = n_angles.max(1);
    let cols = atlas_cols.max(1);
    let rows = (n + cols - 1) / cols; // ceil(n / cols)
    let idx = angle_idx.min(n - 1);

    let col = idx % cols;
    let row = idx / cols;
    let w = 1.0 / cols as f32;
    let h = 1.0 / rows as f32;
    [col as f32 * w, row as f32 * h, w, h]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlaps(a: [f32; 4], b: [f32; 4]) -> bool {
        // interiors overlap if x and y spans both overlap (with a small epsilon
        // so shared edges don't count)
        let eps = 1e-5;
        let x_ov = a[0] < b[0] + b[2] - eps && b[0] < a[0] + a[2] - eps;
        let y_ov = a[1] < b[1] + b[3] - eps && b[1] < a[1] + a[3] - eps;
        x_ov && y_ov
    }

    #[test]
    fn idx_zero_is_first_cell() {
        let r = uv_rect(0, 8, 4);
        assert_eq!(r[0], 0.0);
        assert_eq!(r[1], 0.0);
        assert!((r[2] - 0.25).abs() < 1e-6); // 1/4 cols
        assert!((r[3] - 0.5).abs() < 1e-6); // 8/4 = 2 rows -> 1/2
    }

    #[test]
    fn tiles_across_cols_and_rows() {
        // 8 captures, 4 cols -> 2 rows
        let c1 = uv_rect(1, 8, 4); // row 0, col 1
        assert!((c1[0] - 0.25).abs() < 1e-6 && c1[1].abs() < 1e-6);
        let r1 = uv_rect(4, 8, 4); // row 1, col 0
        assert!(r1[0].abs() < 1e-6 && (r1[1] - 0.5).abs() < 1e-6);
        let last = uv_rect(7, 8, 4); // row 1, col 3
        assert!((last[0] - 0.75).abs() < 1e-6 && (last[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn within_unit_range() {
        for &(n, cols) in &[(8u32, 4u32), (16, 4), (5, 3), (1, 1), (7, 8), (12, 5)] {
            for idx in 0..n {
                let r = uv_rect(idx, n, cols);
                assert!(r[0] >= 0.0 && r[1] >= 0.0, "negative origin");
                assert!(r[0] + r[2] <= 1.0 + 1e-5, "x+w > 1: {:?}", r);
                assert!(r[1] + r[3] <= 1.0 + 1e-5, "y+h > 1: {:?}", r);
            }
        }
    }

    #[test]
    fn cells_non_overlapping() {
        let (n, cols) = (12u32, 5u32);
        let rects: Vec<[f32; 4]> = (0..n).map(|i| uv_rect(i, n, cols)).collect();
        for i in 0..n as usize {
            for j in (i + 1)..n as usize {
                assert!(!overlaps(rects[i], rects[j]), "cells {} and {} overlap", i, j);
            }
        }
    }

    #[test]
    fn idx_clamped_into_range() {
        // out-of-range idx clamps to the last cell rather than escaping [0,1]
        let r = uv_rect(999, 8, 4);
        let last = uv_rect(7, 8, 4);
        assert_eq!(r, last);
        assert!(r[0] + r[2] <= 1.0 + 1e-5 && r[1] + r[3] <= 1.0 + 1e-5);
    }

    #[test]
    fn deterministic() {
        assert_eq!(uv_rect(3, 16, 4), uv_rect(3, 16, 4));
    }
}
