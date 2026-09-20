//! stabilize — warp stabilization in flavours, as path maths over measured motion.
//!
//! Stabilizing is two separate problems, and keeping them apart is what makes
//! "flavours" cheap:
//!
//! 1. **Measure** how the camera moved between frames ([`estimate`], and
//!    [`measure`] over a whole shot). That is the expensive part and it is the
//!    same whatever the result should look like.
//! 2. **Decide** where the camera SHOULD have been ([`solve`]). Every flavour is
//!    a different answer to that question over the same measured path, so
//!    switching flavour re-runs a few microseconds of arithmetic, not the
//!    motion search.
//!
//! The output is a per-frame correction (shift and zoom) — what the renderer
//! applies to each frame. Rendering it needs a per-frame warp, which is a
//! frame pipe, not this module.
//!
//! ## The flavours
//!
//! | flavour | the camera should have… | good for |
//! |---|---|---|
//! | [`Flavor::Still`] | not moved at all | a locked-off shot handheld by accident |
//! | [`Flavor::Smooth`] | moved along a low-passed version of its path | a walk-and-talk, a handheld push-in |
//! | [`Flavor::Dejerk`] | moved exactly as it did, minus sudden jerks | a deliberate pan with a bump in it; the smallest punch-in |
//! | [`Flavor::Track`] | kept a subject (a face) at a fixed place in frame | a face drifting in a handheld close-up |
//!
//! Every flavour spends the same currency: **crop**. A correction of 20 px
//! means 20 px of picture at the edge is gone, so the frame must be zoomed in
//! to hide the border. [`Budget`] caps that — the corrections are clamped so
//! the zoom never exceeds it. On a 4K release with wide compositions the
//! budget should be small; that is why `Dejerk` exists: it removes only the
//! jolts and so needs the least zoom.
//!
//! ## What is measured
//!
//! Translation only, by pyramid block matching on grey frames. Feed it
//! analysis-size frames (≈960 px wide), not 4K: the answer scales, the cost
//! does not. Rotation and
//! rolling shutter are not estimated. A shot whose shake is mostly roll needs
//! them, and the right tool until then is ffmpeg's `vidstabdetect` /
//! `vidstabtransform`, present in the ffmpeg this repo runs against.

use crate::slate_ocr::Gray;

/// Camera motion between two frames, in pixels of the frames given.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Motion {
    pub dx: f32,
    pub dy: f32,
    /// Mean absolute difference at the best match, 0–255. High means the
    /// frames did not really match (a cut, a flash, an object filling frame).
    pub residual: f32,
}

/// Half-size of a frame: the pyramid step.
fn half(g: &Gray) -> Gray {
    let (w, h) = ((g.w / 2).max(1), (g.h / 2).max(1));
    let mut o = Gray::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let s = g.at(2 * x, 2 * y) as u32 + g.at(2 * x + 1, 2 * y) as u32
                + g.at(2 * x, 2 * y + 1) as u32 + g.at(2 * x + 1, 2 * y + 1) as u32;
            o.px[y * w + x] = (s / 4) as u8;
        }
    }
    o
}

/// Mean absolute difference of `cur` shifted by (dx, dy) against `prev`,
/// over the central region (a margin so every shift compares the same count).
///
/// `sparse` samples every other pixel — used only for the coarse search.
/// At full resolution it must be off: on flat-ish texture, every-other-pixel
/// sampling scored shifts of 26 and 27 px both as a perfect 0 and the
/// sub-pixel fit landed halfway between them.
fn sad(prev: &Gray, cur: &Gray, dx: i32, dy: i32, margin: i32) -> f32 { sad_step(prev, cur, dx, dy, margin, 1) }

fn sad_step(prev: &Gray, cur: &Gray, dx: i32, dy: i32, margin: i32, step: i32) -> f32 {
    let (w, h) = (prev.w as i32, prev.h as i32);
    let mut sum = 0u64;
    let mut n = 0u64;
    let mut y = margin;
    while y < h - margin {
        let mut x = margin;
        while x < w - margin {
            let a = prev.px[(y * w + x) as usize] as i32;
            let b = cur.px[((y + dy) * w + (x + dx)) as usize] as i32;
            sum += (a - b).unsigned_abs() as u64;
            n += 1;
            x += step;
        }
        y += step;
    }
    if n == 0 { f32::MAX } else { sum as f32 / n as f32 }
}

/// How the camera moved from `prev` to `cur`: content that was at `p` in
/// `prev` is at `p + (dx, dy)` in `cur`. `max_shift` bounds the search in
/// pixels. Coarse-to-fine: exhaustive at the smallest level, ±1 refinement
/// on the way up, then an equiangular sub-pixel fit.
pub fn estimate(prev: &Gray, cur: &Gray, max_shift: u32) -> Motion {
    assert_eq!((prev.w, prev.h), (cur.w, cur.h), "frames must match in size");
    let mut levels = vec![(prev.clone(), cur.clone())];
    while levels.last().unwrap().0.w > 64 && levels.len() < 6 {
        let (a, b) = levels.last().unwrap();
        levels.push((half(a), half(b)));
    }
    let top = levels.len() - 1;
    let r = ((max_shift >> top).max(2)) as i32;
    let (mut bx, mut by) = (0i32, 0i32);
    for (li, (a, b)) in levels.iter().enumerate().rev() {
        let (range, cx, cy) = if li == top { (r, 0, 0) } else { (1, bx * 2, by * 2) };
        let margin = if li == top { r + 1 } else { (r << (top - li)).min(a.w as i32 / 4) + 2 };
        let mut best = (f32::MAX, cx, cy);
        for dy in cy - range..=cy + range {
            for dx in cx - range..=cx + range {
                if dx.abs() >= margin || dy.abs() >= margin { continue; }
                let step = if li > 0 && a.w * a.h > 40_000 { 2 } else { 1 };
                let s = sad_step(a, b, dx, dy, margin, step);
                if s < best.0 { best = (s, dx, dy); }
            }
        }
        bx = best.1;
        by = best.2;
    }
    let (a, b) = &levels[0];
    let margin = (max_shift as i32 + 2).min(a.w as i32 / 4);
    let c = sad(a, b, bx, by, margin);
    // Equiangular (V) fit, not a parabola: SAD is V-shaped around its
    // minimum, and a parabola through three points of a V snaps an exact
    // integer match to ±0.5 (it put a 27 px move at 26.5).
    let sub = |m: f32, p: f32| {
        let d = 2.0 * (m.max(p) - c);
        if d > 1e-6 { ((m - p) / d).clamp(-0.5, 0.5) } else { 0.0 }
    };
    let fx = sub(sad(a, b, bx - 1, by, margin), sad(a, b, bx + 1, by, margin));
    let fy = sub(sad(a, b, bx, by - 1, margin), sad(a, b, bx, by + 1, margin));
    Motion { dx: bx as f32 + fx, dy: by as f32 + fy, residual: c }
}

/// The camera's path over a shot: position of frame i relative to frame 0
/// (the running sum of frame-to-frame motion).
pub fn measure(frames: &[Gray], max_shift: u32) -> Vec<(f32, f32)> {
    let mut path = vec![(0.0f32, 0.0f32)];
    for w in frames.windows(2) {
        let m = estimate(&w[0], &w[1], max_shift);
        let (x, y) = *path.last().unwrap();
        path.push((x + m.dx, y + m.dy));
    }
    path
}

/// What the stabilized camera should have done.
#[derive(Debug, Clone, PartialEq)]
pub enum Flavor {
    /// Lock to the shot's average position.
    Still,
    /// Follow the path low-passed with a Gaussian of this many frames'
    /// standard deviation (12 at 24 fps is about half a second).
    Smooth { sigma: f32 },
    /// Keep the path, but remove any frame-to-frame jump larger than this
    /// many pixels beyond the local median velocity (a bump in a pan).
    Dejerk { threshold: f32 },
    /// Keep `subject[i]` (its position in frame i, in pixels) at `target`,
    /// smoothed with `sigma` frames so the tracker's own noise isn't copied.
    Track { subject: Vec<(f32, f32)>, target: (f32, f32), sigma: f32 },
}

/// How much picture may be given up to hide the moving border.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Budget {
    /// Largest zoom allowed; 1.05 gives up 5% of width and height.
    pub max_zoom: f32,
}

/// Per-frame correction: shift the frame by (dx, dy) pixels, then zoom by
/// `zoom` about the centre so no border shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Correction {
    pub dx: f32,
    pub dy: f32,
    pub zoom: f32,
}

/// The stabilized result: per-frame corrections, one zoom for the whole shot
/// (a zoom that changes per frame reads as breathing), and how much of each
/// wanted correction the budget forced to be dropped.
#[derive(Debug, Clone, PartialEq)]
pub struct Solution {
    pub corrections: Vec<Correction>,
    pub zoom: f32,
    /// Largest |wanted − applied| over the shot, in pixels. Zero means the
    /// flavour got everything it asked for.
    pub clamped_px: f32,
}

fn gaussian(v: &[f32], sigma: f32) -> Vec<f32> {
    if sigma <= 0.0 || v.len() < 2 { return v.to_vec(); }
    let r = (3.0 * sigma).ceil() as isize;
    let k: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    (0..v.len() as isize).map(|i| {
        let (mut s, mut ws) = (0.0, 0.0);
        for (j, kw) in (-r..=r).zip(&k) {
            // mirror at the ends so the smooth path does not sag toward zero
            let mut q = i + j;
            if q < 0 { q = -q; }
            if q >= v.len() as isize { q = 2 * (v.len() as isize - 1) - q; }
            let q = q.clamp(0, v.len() as isize - 1) as usize;
            s += v[q] * kw;
            ws += kw;
        }
        s / ws
    }).collect()
}

fn median(mut v: Vec<f32>) -> f32 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v[v.len() / 2]
}

/// Remove velocity spikes: any step differing from the local median step by
/// more than `threshold` is replaced by that median; the path is re-integrated.
fn dejerk(v: &[f32], threshold: f32) -> Vec<f32> {
    if v.len() < 3 { return v.to_vec(); }
    let steps: Vec<f32> = v.windows(2).map(|w| w[1] - w[0]).collect();
    let mut out = vec![v[0]];
    for i in 0..steps.len() {
        let lo = i.saturating_sub(4);
        let hi = (i + 5).min(steps.len());
        let m = median(steps[lo..hi].to_vec());
        let s = if (steps[i] - m).abs() > threshold { m } else { steps[i] };
        out.push(out.last().unwrap() + s);
    }
    out
}

/// Where the camera should have been, and the corrections that get it there,
/// for a frame of `w`×`h` pixels.
pub fn solve(path: &[(f32, f32)], flavor: &Flavor, budget: Budget, w: u32, h: u32) -> Solution {
    let xs: Vec<f32> = path.iter().map(|p| p.0).collect();
    let ys: Vec<f32> = path.iter().map(|p| p.1).collect();
    let n = path.len();
    // wanted: the correction that moves the real camera onto the ideal one.
    let (wx, wy): (Vec<f32>, Vec<f32>) = match flavor {
        Flavor::Still => {
            let (mx, my) = (xs.iter().sum::<f32>() / n.max(1) as f32, ys.iter().sum::<f32>() / n.max(1) as f32);
            (xs.iter().map(|x| mx - x).collect(), ys.iter().map(|y| my - y).collect())
        }
        Flavor::Smooth { sigma } => {
            let (sx, sy) = (gaussian(&xs, *sigma), gaussian(&ys, *sigma));
            (sx.iter().zip(&xs).map(|(s, x)| s - x).collect(), sy.iter().zip(&ys).map(|(s, y)| s - y).collect())
        }
        Flavor::Dejerk { threshold } => {
            let (sx, sy) = (dejerk(&xs, *threshold), dejerk(&ys, *threshold));
            (sx.iter().zip(&xs).map(|(s, x)| s - x).collect(), sy.iter().zip(&ys).map(|(s, y)| s - y).collect())
        }
        Flavor::Track { subject, target, sigma } => {
            let sx: Vec<f32> = gaussian(&subject.iter().map(|p| p.0).collect::<Vec<_>>(), *sigma);
            let sy: Vec<f32> = gaussian(&subject.iter().map(|p| p.1).collect::<Vec<_>>(), *sigma);
            ((0..n).map(|i| target.0 - sx.get(i).copied().unwrap_or(target.0)).collect(),
             (0..n).map(|i| target.1 - sy.get(i).copied().unwrap_or(target.1)).collect())
        }
    };
    // A shift of s px at zoom z stays border-free while |s| <= (z − 1)·W/2.
    let lim_x = (budget.max_zoom.max(1.0) - 1.0) * w as f32 / 2.0;
    let lim_y = (budget.max_zoom.max(1.0) - 1.0) * h as f32 / 2.0;
    let mut clamped: f32 = 0.0;
    let corr: Vec<(f32, f32)> = wx.iter().zip(&wy).map(|(&x, &y)| {
        let (cx, cy) = (x.clamp(-lim_x, lim_x), y.clamp(-lim_y, lim_y));
        clamped = clamped.max((x - cx).abs()).max((y - cy).abs());
        (cx, cy)
    }).collect();
    // one zoom for the shot: just enough for its largest applied shift
    let need_x = corr.iter().map(|c| c.0.abs()).fold(0.0, f32::max) * 2.0 / w as f32;
    let need_y = corr.iter().map(|c| c.1.abs()).fold(0.0, f32::max) * 2.0 / h as f32;
    let zoom = 1.0 + need_x.max(need_y);
    Solution {
        corrections: corr.into_iter().map(|(dx, dy)| Correction { dx, dy, zoom }).collect(),
        zoom,
        clamped_px: clamped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::Rng;

    /// A textured frame of the scene, viewed with the camera at (cx, cy):
    /// the scene is a fixed pseudo-random texture, blurred so it has the
    /// low-frequency structure real pictures have.
    fn view(w: usize, h: usize, cx: f32, cy: f32) -> Gray {
        let scene = |x: i32, y: i32| -> f32 {
            let mut r = Rng::new(((x.div_euclid(6) as i64 * 7919 + y.div_euclid(6) as i64 * 104_729) as u64) ^ 0xABCD);
            (r.next() * 255.0) as f32
        };
        let mut g = Gray::new(w, h);
        for y in 0..h {
            for x in 0..w {
                // sample the scene at (x + cx, y + cy), bilinear on the 6-px cells
                let (sx, sy) = (x as f32 + cx, y as f32 + cy);
                let (x0, y0) = (sx.floor(), sy.floor());
                let (fx, fy) = (sx - x0, sy - y0);
                let v = |dx: i32, dy: i32| scene(x0 as i32 + dx, y0 as i32 + dy);
                let top = v(0, 0) * (1.0 - fx) + v(1, 0) * fx;
                let bot = v(0, 1) * (1.0 - fx) + v(1, 1) * fx;
                g.px[y * w + x] = (top * (1.0 - fy) + bot * fy) as u8;
            }
        }
        g
    }

    #[test]
    fn estimate_finds_integer_and_subpixel_shifts() {
        let a = view(160, 90, 0.0, 0.0);
        for &(dx, dy) in &[(3.0f32, -2.0f32), (-11.0, 6.0), (2.5, 0.5), (0.0, 0.0)] {
            // camera moved by (dx, dy): content moves the other way in the frame
            let b = view(160, 90, dx, dy);
            let m = estimate(&a, &b, 16);
            assert!((m.dx + dx).abs() < 0.35 && (m.dy + dy).abs() < 0.35, "want ({}, {}), got {m:?}", -dx, -dy);
        }
    }

    #[test]
    fn a_large_move_is_found_through_the_pyramid() {
        let a = view(320, 180, 0.0, 0.0);
        let b = view(320, 180, 27.0, -14.0);
        let m = estimate(&a, &b, 40);
        assert!((m.dx + 27.0).abs() < 0.5 && (m.dy - 14.0).abs() < 0.5, "{m:?}");
    }

    fn shaky_pan(n: usize) -> Vec<(f32, f32)> {
        // a slow pan to the right (0.8 px/frame) with handheld jitter, and one bump at frame 30
        let mut r = Rng::new(3);
        (0..n).map(|i| {
            let bump = if i >= 30 { 12.0 } else { 0.0 };
            (0.8 * i as f32 + (r.next() as f32 - 0.5) * 3.0 + bump, (r.next() as f32 - 0.5) * 3.0)
        }).collect()
    }

    fn jitter(p: &[(f32, f32)]) -> f32 {
        // mean |second difference|: what the eye reads as shake
        p.windows(3).map(|w| (w[2].0 - 2.0 * w[1].0 + w[0].0).abs() + (w[2].1 - 2.0 * w[1].1 + w[0].1).abs())
            .sum::<f32>() / (p.len() - 2) as f32
    }

    fn apply(path: &[(f32, f32)], s: &Solution) -> Vec<(f32, f32)> {
        path.iter().zip(&s.corrections).map(|(p, c)| (p.0 + c.dx, p.1 + c.dy)).collect()
    }

    const ROOMY: Budget = Budget { max_zoom: 1.3 };

    #[test]
    fn still_removes_all_motion() {
        let path = shaky_pan(60);
        let s = solve(&path, &Flavor::Still, ROOMY, 1920, 1080);
        let out = apply(&path, &s);
        let spread = out.iter().map(|p| p.0).fold(f32::MIN, f32::max) - out.iter().map(|p| p.0).fold(f32::MAX, f32::min);
        assert!(spread < 1e-3, "locked: {spread}");
        assert!(s.zoom > 1.0);
    }

    #[test]
    fn smooth_keeps_the_pan_and_removes_the_shake() {
        let path = shaky_pan(90);
        let s = solve(&path, &Flavor::Smooth { sigma: 8.0 }, ROOMY, 1920, 1080);
        let out = apply(&path, &s);
        assert!(jitter(&out) < jitter(&path) * 0.25, "{} vs {}", jitter(&out), jitter(&path));
        // the pan survives: still travelling right at about 0.8 px/frame mid-shot
        let v = (out[60].0 - out[20].0) / 40.0;
        assert!((0.6..=1.4).contains(&v), "pan speed {v}");
    }

    #[test]
    fn dejerk_removes_the_bump_and_leaves_the_rest_alone() {
        // clean pan plus a single 12 px bump: only the bump should be corrected
        let path: Vec<(f32, f32)> = (0..60).map(|i| (0.8 * i as f32 + if i >= 30 { 12.0 } else { 0.0 }, 0.0)).collect();
        let s = solve(&path, &Flavor::Dejerk { threshold: 3.0 }, ROOMY, 1920, 1080);
        let out = apply(&path, &s);
        let steps: Vec<f32> = out.windows(2).map(|w| w[1].0 - w[0].0).collect();
        assert!(steps.iter().all(|d| (d - 0.8).abs() < 1e-3), "{steps:?}");
        // before the bump nothing was touched at all: least crop of any flavour
        assert!(s.corrections[..30].iter().all(|c| c.dx.abs() < 1e-4));
    }

    #[test]
    fn track_holds_the_subject_on_its_mark() {
        let n = 50;
        let subject: Vec<(f32, f32)> = (0..n).map(|i| (900.0 + 3.0 * i as f32, 500.0 - i as f32)).collect();
        let s = solve(&vec![(0.0, 0.0); n], &Flavor::Track { subject: subject.clone(), target: (960.0, 540.0), sigma: 0.0 },
                      Budget { max_zoom: 1.5 }, 1920, 1080);
        for (p, c) in subject.iter().zip(&s.corrections) {
            assert!((p.0 + c.dx - 960.0).abs() < 1e-3 && (p.1 + c.dy - 540.0).abs() < 1e-3);
        }
    }

    #[test]
    fn the_budget_caps_the_zoom_and_reports_what_it_gave_up() {
        let path = shaky_pan(90);  // a pan of ~70 px + bump: Still wants a big correction
        let tight = Budget { max_zoom: 1.02 };  // 19.2 px of room at 1920 wide
        let s = solve(&path, &Flavor::Still, tight, 1920, 1080);
        assert!(s.zoom <= 1.02 + 1e-5, "zoom {}", s.zoom);
        assert!(s.clamped_px > 10.0, "it must say it could not lock the shot: {}", s.clamped_px);
        let d = solve(&path, &Flavor::Dejerk { threshold: 3.0 }, tight, 1920, 1080);
        assert!(d.zoom < s.zoom, "dejerk spends less crop than still: {} vs {}", d.zoom, s.zoom);
    }

    #[test]
    fn measure_then_smooth_end_to_end_on_rendered_frames() {
        let cams: Vec<(f32, f32)> = (0..24).map(|i| (1.5 * i as f32 + if i % 5 == 0 { 2.0 } else { 0.0 }, 0.0)).collect();
        let frames: Vec<Gray> = cams.iter().map(|c| view(160, 90, c.0, c.1)).collect();
        let path = measure(&frames, 12);
        // measured path is the negative of the camera path (content moves opposite)
        for (m, c) in path.iter().zip(&cams) {
            assert!((m.0 + (c.0 - cams[0].0)).abs() < 0.8, "measured {m:?} for camera {c:?}");
        }
    }
}
