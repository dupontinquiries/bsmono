//! A cheap tracker for the camera pane: corners found on a small grayscale
//! frame, followed into the next frame by block matching, and the robust
//! (median) image motion between frames. Enough to tell the world model
//! how the picture actually moved, which is what keeps the anchors from
//! sliding when the gyro is off; not a feature pipeline for anything else.
//!
//! Cost is bounded by construction: frames are small (the frontend scales
//! to ~240x320), at most MAX_POINTS corners spread over a grid so they do
//! not all sit on one edge, and matching searches a ±SEARCH px window with
//! a PATCH×PATCH sum of absolute differences.

pub const MAX_POINTS: usize = 48;
pub const CELL: usize = 32;
pub const PATCH: usize = 7;
pub const SEARCH: i32 = 8;
/// Minimum corner response (min eigenvalue of the 5x5 structure tensor).
pub const MIN_RESPONSE: f32 = 40.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub w: usize,
    pub h: usize,
    pub gray: Vec<u8>,
}

impl Frame {
    pub fn new(w: usize, h: usize, gray: Vec<u8>) -> Frame {
        assert_eq!(gray.len(), w * h);
        Frame { w, h, gray }
    }

    /// From RGBA bytes (luma only), the frontend's texture format.
    pub fn from_rgba(w: usize, h: usize, rgba: &[u8]) -> Frame {
        let gray = rgba.chunks_exact(4).map(|p| ((p[0] as u32 * 77 + p[1] as u32 * 150 + p[2] as u32 * 29) >> 8) as u8).collect();
        Frame::new(w, h, gray)
    }

    #[inline]
    fn at(&self, x: i32, y: i32) -> f32 {
        let x = x.clamp(0, self.w as i32 - 1) as usize;
        let y = y.clamp(0, self.h as i32 - 1) as usize;
        self.gray[y * self.w + x] as f32
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
    pub response: f32,
}

/// Corner response per pixel (min eigenvalue of the summed gradient
/// products over a 5x5 window), sampled on a 2 px grid for speed.
fn response(f: &Frame, x: i32, y: i32) -> f32 {
    let (mut a, mut b, mut c) = (0.0f32, 0.0f32, 0.0f32);
    for dy in -2..=2 {
        for dx in -2..=2 {
            let ix = f.at(x + dx + 1, y + dy) - f.at(x + dx - 1, y + dy);
            let iy = f.at(x + dx, y + dy + 1) - f.at(x + dx, y + dy - 1);
            a += ix * ix;
            b += ix * iy;
            c += iy * iy;
        }
    }
    let (a, b, c) = (a / 25.0, b / 25.0, c / 25.0);
    ((a + c) - ((a - c) * (a - c) + 4.0 * b * b).sqrt()) / 2.0
}

/// The strongest corner in each CELL×CELL cell, up to MAX_POINTS.
pub fn find_corners(f: &Frame) -> Vec<Point> {
    let margin = (PATCH / 2 + SEARCH as usize + 2) as i32;
    let mut best: Vec<Option<Point>> = vec![None; (f.w / CELL + 1) * (f.h / CELL + 1)];
    let cols = f.w / CELL + 1;
    let mut y = margin;
    while y < f.h as i32 - margin {
        let mut x = margin;
        while x < f.w as i32 - margin {
            let r = response(f, x, y);
            if r >= MIN_RESPONSE {
                let cell = (y as usize / CELL) * cols + x as usize / CELL;
                if best[cell].is_none_or(|p| r > p.response) {
                    best[cell] = Some(Point { x, y, response: r });
                }
            }
            x += 2;
        }
        y += 2;
    }
    let mut pts: Vec<Point> = best.into_iter().flatten().collect();
    pts.sort_by(|a, b| b.response.partial_cmp(&a.response).unwrap());
    pts.truncate(MAX_POINTS);
    pts
}

fn sad(a: &Frame, ax: i32, ay: i32, b: &Frame, bx: i32, by: i32) -> f32 {
    let h = PATCH as i32 / 2;
    let mut s = 0.0;
    for dy in -h..=h {
        for dx in -h..=h {
            s += (a.at(ax + dx, ay + dy) - b.at(bx + dx, by + dy)).abs();
        }
    }
    s
}

/// One point's motion into `next`: (dx, dy) minimizing the patch SAD within
/// ±SEARCH, or None when the best match is not clearly better than staying.
pub fn track_point(prev: &Frame, p: Point, next: &Frame) -> Option<(i32, i32)> {
    let mut best = (f32::MAX, 0, 0);
    let mut sum = 0.0;
    let mut n = 0.0;
    for dy in -SEARCH..=SEARCH {
        for dx in -SEARCH..=SEARCH {
            let s = sad(prev, p.x, p.y, next, p.x + dx, p.y + dy);
            sum += s;
            n += 1.0;
            if s < best.0 {
                best = (s, dx, dy);
            }
        }
    }
    // Distinctiveness: the best match must be clearly better than the
    // average offset. On a flat or vanished picture every offset scores
    // alike, and that is "not tracked", not "did not move" (the first
    // version reported 48/48 tracked on a blank frame).
    let mean = sum / n;
    if mean - best.0 < 0.25 * mean {
        return None;
    }
    Some((best.1, best.2))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Motion {
    pub dx: f32,
    pub dy: f32,
    pub tracked: usize,
    pub total: usize,
}

/// Robust global motion prev -> next: the median of the point flows.
pub fn estimate_motion(prev: &Frame, points: &[Point], next: &Frame) -> Motion {
    let flows: Vec<(i32, i32)> = points.iter().filter_map(|p| track_point(prev, *p, next)).collect();
    let n = flows.len();
    if n == 0 {
        return Motion { dx: 0.0, dy: 0.0, tracked: 0, total: points.len() };
    }
    let mut xs: Vec<i32> = flows.iter().map(|f| f.0).collect();
    let mut ys: Vec<i32> = flows.iter().map(|f| f.1).collect();
    xs.sort_unstable();
    ys.sort_unstable();
    Motion { dx: xs[n / 2] as f32, dy: ys[n / 2] as f32, tracked: n, total: points.len() }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A textured frame: smoothed deterministic noise with a few bright discs.
    fn textured(w: usize, h: usize, shift: (i32, i32)) -> Frame {
        let mut seed = 12345u32;
        let mut noise = vec![0u8; w * h];
        for v in noise.iter_mut() {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            *v = (seed >> 24) as u8;
        }
        let px = |x: i32, y: i32| -> u32 {
            let xs = (x - shift.0).rem_euclid(w as i32) as usize;
            let ys = (y - shift.1).rem_euclid(h as i32) as usize;
            noise[ys * w + xs] as u32
        };
        let mut g = vec![0u8; w * h];
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                // 3x3 box blur of the shifted noise, plus discs.
                let mut s = 0;
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        s += px(x + dx, y + dy);
                    }
                }
                let mut v = (s / 9) as i32 / 2;
                for (cx, cy) in [(60, 80), (170, 200), (100, 260)] {
                    let (ddx, ddy) = ((x - shift.0 - cx) as f32, (y - shift.1 - cy) as f32);
                    if ddx * ddx + ddy * ddy < 100.0 {
                        v += 120;
                    }
                }
                g[y as usize * w + x as usize] = v.clamp(0, 255) as u8;
            }
        }
        Frame::new(w, h, g)
    }

    #[test]
    fn corners_are_found_and_spread_over_the_frame() {
        let f = textured(240, 320, (0, 0));
        let pts = find_corners(&f);
        assert!(pts.len() >= 20 && pts.len() <= MAX_POINTS, "{} corners", pts.len());
        let left = pts.iter().filter(|p| p.x < 120).count();
        assert!(left > 3 && left < pts.len() - 3, "spread across both halves: {left} of {}", pts.len());
        let flat = Frame::new(64, 64, vec![128; 64 * 64]);
        assert!(find_corners(&flat).is_empty(), "a flat frame has no corners");
    }

    #[test]
    fn a_shifted_frame_is_tracked_to_the_pixel() {
        let a = textured(240, 320, (0, 0));
        let pts = find_corners(&a);
        for shift in [(3, -2), (-6, 5), (0, 7)] {
            let b = textured(240, 320, shift);
            let m = estimate_motion(&a, &pts, &b);
            assert!(m.tracked * 2 >= m.total, "most points tracked: {}/{}", m.tracked, m.total);
            assert_eq!((m.dx as i32, m.dy as i32), shift, "{m:?}");
        }
    }

    #[test]
    fn no_motion_is_reported_as_zero_and_a_flat_next_frame_tracks_nothing() {
        let a = textured(240, 320, (0, 0));
        let pts = find_corners(&a);
        let m = estimate_motion(&a, &pts, &a);
        assert_eq!((m.dx, m.dy), (0.0, 0.0));
        let flat = Frame::new(240, 320, vec![100; 240 * 320]);
        let m = estimate_motion(&a, &pts, &flat);
        assert!(m.tracked <= m.total / 4, "the picture is gone: {m:?}");
    }

    #[test]
    fn rgba_to_luma() {
        let f = Frame::from_rgba(2, 1, &[255, 255, 255, 255, 0, 0, 0, 255]);
        assert!(f.gray[0] >= 250 && f.gray[1] == 0);
    }
}
