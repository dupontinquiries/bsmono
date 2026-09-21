//! Dirty tiles → a handful of update rectangles.
//!
//! Every update sent to an e-ink controller has fixed overhead, and a
//! region update redraws the whole rect, so the goal is few rects that do
//! not cover much clean area. Horizontal runs of dirty tiles become rects;
//! runs in consecutive rows with the same span merge vertically. If that
//! still leaves more than `max_rects`, everything collapses to one bounding
//! box: at that point the extra area is cheaper than the extra updates.

use crate::geom::IRect;
use crate::raster::DirtyTiles;

/// One region to refresh, with whether its new content has colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Damage {
    pub rect: IRect,
    pub color: bool,
}

pub fn coalesce(d: &DirtyTiles, max_rects: usize) -> Vec<Damage> {
    // (row, c0, c1_exclusive, color) runs
    let mut open: Vec<(usize, usize, usize, usize, bool)> = Vec::new(); // (r0, r1_excl, c0, c1, color)
    let mut done: Vec<(usize, usize, usize, usize, bool)> = Vec::new();
    for r in 0..d.rows {
        let mut runs = Vec::new();
        let mut c = 0;
        while c < d.cols {
            if d.dirty[r * d.cols + c] {
                let start = c;
                let mut color = false;
                while c < d.cols && d.dirty[r * d.cols + c] {
                    color |= d.chroma[r * d.cols + c];
                    c += 1;
                }
                runs.push((start, c, color));
            } else {
                c += 1;
            }
        }
        let mut next_open = Vec::new();
        for (c0, c1, color) in runs {
            if let Some(i) = open.iter().position(|o| o.1 == r && o.2 == c0 && o.3 == c1) {
                let mut o = open.remove(i);
                o.1 = r + 1;
                o.4 |= color;
                next_open.push(o);
            } else {
                next_open.push((r, r + 1, c0, c1, color));
            }
        }
        done.append(&mut open);
        open = next_open;
    }
    done.append(&mut open);

    let to_rect = |(r0, r1, c0, c1, _): &(usize, usize, usize, usize, bool)| {
        d.tile_rect(*c0, *r0).union(&d.tile_rect(c1 - 1, r1 - 1))
    };
    if done.len() > max_rects {
        let rect = done.iter().map(to_rect).fold(IRect::default(), |a, b| a.union(&b));
        let color = done.iter().any(|o| o.4);
        return vec![Damage { rect, color }];
    }
    done.iter().map(|o| Damage { rect: to_rect(o), color: o.4 }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster::TILE;

    fn tiles(cols: usize, rows: usize, dirty: &[(usize, usize)], chroma: &[(usize, usize)]) -> DirtyTiles {
        let mut d = DirtyTiles {
            cols, rows, dirty: vec![false; cols * rows], chroma: vec![false; cols * rows],
            width: (cols * TILE) as u32, height: (rows * TILE) as u32,
        };
        for &(c, r) in dirty { d.dirty[r * cols + c] = true; }
        for &(c, r) in chroma { d.chroma[r * cols + c] = true; }
        d
    }

    #[test]
    fn a_block_of_tiles_is_one_rect() {
        let d = tiles(8, 8, &[(2, 2), (3, 2), (2, 3), (3, 3)], &[(3, 3)]);
        let r = coalesce(&d, 8);
        assert_eq!(r.len(), 1);
        let t = TILE as i32;
        assert_eq!(r[0].rect, IRect::new(2 * t, 2 * t, 4 * t, 4 * t));
        assert!(r[0].color);
    }

    #[test]
    fn separate_regions_stay_separate_and_keep_their_own_colour() {
        let d = tiles(8, 8, &[(0, 0), (7, 7)], &[(7, 7)]);
        let r = coalesce(&d, 8);
        assert_eq!(r.len(), 2);
        assert!(!r.iter().find(|x| x.rect.x0 == 0).unwrap().color);
        assert!(r.iter().find(|x| x.rect.x0 > 0).unwrap().color);
    }

    #[test]
    fn too_many_rects_collapse_to_the_bounding_box() {
        let d = tiles(8, 8, &[(0, 0), (2, 0), (4, 0), (6, 0)], &[]);
        let r = coalesce(&d, 3);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].rect.x0, 0);
        assert_eq!(r[0].rect.x1, 7 * TILE as i32);
    }

    #[test]
    fn partial_edge_tiles_are_clamped_to_the_screen() {
        let mut d = tiles(2, 2, &[(1, 1)], &[]);
        d.width = TILE as u32 + 10;
        d.height = TILE as u32 + 5;
        let r = coalesce(&d, 4);
        assert_eq!(r[0].rect, IRect::new(TILE as i32, TILE as i32, TILE as i32 + 10, TILE as i32 + 5));
    }
}
