//! icon_atlas.rs — pure inventory icon-atlas UV selection.
//!
//! Returns the `[x, y, w, h]` rectangle (all in `[0,1]`) of icon `idx` in an
//! atlas of `cols × rows` cells, laid out row-major. Sibling of `impostor_uv`
//! (R15) for the UI atlas. Pure, deterministic, panic-free (R1/R11/R21).

/// UV rect for icon `idx` in a `cols × rows` atlas. `cols`/`rows` are treated as
/// at least 1; `idx` is clamped to the last cell.
pub fn icon_rect(idx: u32, cols: u32, rows: u32) -> [f32; 4] {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn overlaps(a: [f32; 4], b: [f32; 4]) -> bool {
        let e = 1e-5;
        a[0] < b[0] + b[2] - e && b[0] < a[0] + a[2] - e && a[1] < b[1] + b[3] - e && b[1] < a[1] + a[3] - e
    }

    #[test]
    fn idx_zero_is_first_cell() {
        let r = icon_rect(0, 8, 4);
        assert_eq!([r[0], r[1]], [0.0, 0.0]);
        assert!((r[2] - 0.125).abs() < 1e-6 && (r[3] - 0.25).abs() < 1e-6);
    }

    #[test]
    fn tiles_cols_and_rows() {
        let c1 = icon_rect(1, 8, 4); // row0 col1
        assert!((c1[0] - 0.125).abs() < 1e-6 && c1[1].abs() < 1e-6);
        let r1 = icon_rect(8, 8, 4); // row1 col0
        assert!(r1[0].abs() < 1e-6 && (r1[1] - 0.25).abs() < 1e-6);
    }

    #[test]
    fn within_unit_range() {
        for &(cols, rows) in &[(8u32, 4u32), (3, 3), (1, 1), (5, 2), (16, 16)] {
            for idx in 0..(cols * rows) {
                let r = icon_rect(idx, cols, rows);
                assert!(r[0] >= 0.0 && r[1] >= 0.0);
                assert!(r[0] + r[2] <= 1.0 + 1e-5 && r[1] + r[3] <= 1.0 + 1e-5, "oob {:?}", r);
            }
        }
    }

    #[test]
    fn cells_non_overlapping() {
        let (cols, rows) = (5u32, 4u32);
        let n = cols * rows;
        let rects: Vec<[f32; 4]> = (0..n).map(|i| icon_rect(i, cols, rows)).collect();
        for i in 0..n as usize {
            for j in (i + 1)..n as usize {
                assert!(!overlaps(rects[i], rects[j]), "cells {} {} overlap", i, j);
            }
        }
    }

    #[test]
    fn idx_clamps_to_last() {
        let r = icon_rect(9999, 8, 4);
        assert_eq!(r, icon_rect(31, 8, 4));
        assert!(r[0] + r[2] <= 1.0 + 1e-5 && r[1] + r[3] <= 1.0 + 1e-5);
    }

    #[test]
    fn deterministic() {
        assert_eq!(icon_rect(7, 8, 8), icon_rect(7, 8, 8));
    }
}
