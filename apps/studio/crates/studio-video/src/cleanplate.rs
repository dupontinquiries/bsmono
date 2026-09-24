//! cleanplate — removing something that passes THROUGH a shot.
//!
//! The case this exists for: an actor stands against a soft bokeh street and
//! a car crosses fast behind them. The background is nearly static and the
//! car covers any given pixel for only a few frames, so the background is
//! still there — in the neighbouring frames. Rotoscoping the car is the
//! expensive way to use that fact; taking each pixel's value from the frames
//! where it was not covered is the cheap one.
//!
//! ## How
//!
//! For each output frame, [`clean`] looks at a window of frames around it
//! (from [`crate::framepipe::Pipe::map_window`]) and, per pixel:
//!
//! 1. takes the temporal MEDIAN across the window — a fast occluder is a
//!    minority of the window, so the median is the background;
//! 2. compares the current frame with it. A difference above
//!    [`Settings::threshold`] means the pixel is covered;
//! 3. replaces covered pixels with the median, feathered at the edge of the
//!    covered region so the patch has no hard border;
//! 4. never touches a pixel the protect mask covers — an actor who holds
//!    still IS the minority-free background by this maths, and would be
//!    "cleaned" away. Protection is the difference between a tool and an
//!    eraser;
//! 5. reports [`Cleaned::confidence`]: the share of the window that agreed
//!    with the median. Where the occluder covered a pixel for most of the
//!    window (a slow car, a parked bus) confidence collapses, and the pixel
//!    is left alone rather than smeared with whatever the median happened to
//!    be.
//!
//! ## What it does not do
//!
//! A moving camera must be aligned first: pass `offsets` (from
//! [`crate::stabilize::estimate`]) so neighbours are warped onto the current
//! frame before the median. Nothing here estimates them, because the pass
//! that needs them has usually measured the shot already.
//!
//! When the occluder is present for most of the shot, no window is wide
//! enough and this is the wrong tool: that is a paint-out or an inpainting
//! model, and the low-confidence report is how the editor is told so rather
//! than shipping a smear.

use crate::framepipe::Frame;
use crate::slate_ocr::Gray;

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// How far a pixel must differ from the median (0–255, per channel mean)
    /// before it counts as covered.
    pub threshold: f32,
    /// Feather radius in pixels at the edge of a covered region.
    pub feather: u32,
    /// Pixels to leave untouched: 0 = free, anything above 127 = protected.
    /// Usually the actor, from a roto shape.
    pub protect: Option<Gray>,
    /// Below this share of the window agreeing, the pixel is left as shot
    /// and reported instead of filled.
    pub min_confidence: f32,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { threshold: 18.0, feather: 2, protect: None, min_confidence: 0.55 }
    }
}

/// The filled frame and what it is worth.
#[derive(Debug, Clone, PartialEq)]
pub struct Cleaned {
    pub frame: Frame,
    /// Per pixel, 0–255: how much of the window agreed with the value used.
    pub confidence: Gray,
    /// Pixels that were covered but could not be filled confidently.
    pub unresolved: u32,
    /// Pixels actually replaced.
    pub filled: u32,
}

fn median_of(v: &mut [u8]) -> u8 {
    v.sort_unstable();
    v[v.len() / 2]
}

/// Clean frame `center` of `window`, with `offsets[i]` the shift that puts
/// window frame `i` onto the centre frame (empty = a locked-off shot).
pub fn clean(window: &[Frame], center: usize, offsets: &[(f32, f32)], s: &Settings) -> Cleaned {
    let cur = &window[center];
    let (w, h) = (cur.w, cur.h);
    let aligned: Vec<Frame> = if offsets.is_empty() {
        Vec::new()
    } else {
        window.iter().zip(offsets).map(|(f, o)| f.warp(o.0, o.1, 1.0)).collect()
    };
    let frames: &[Frame] = if aligned.is_empty() { window } else { &aligned };
    let n = frames.len();
    let mut out = cur.clone();
    let mut conf = Gray::new(w as usize, h as usize);
    // pass 1: median, difference, confidence.
    //
    // Written for the hot loop, not for looks: this ran at 10.5 fps on
    // 960x540 real footage in its first form (a bounds-checked `at()` per
    // frame per channel, then a SECOND pass over the window to count
    // agreement). Direct indexing, one pass over the window, and a fast path
    // for the common case — a pixel no frame in the window disagrees about
    // is background, and needs no median at all.
    let mut covered = vec![false; (w * h) as usize];
    let mut med = vec![[0u8; 3]; (w * h) as usize];
    // Which entries of `med` are a real median rather than the fast path's
    // "this pixel is background" shortcut. A feathered ring pixel takes the
    // fast path but still needs the true median to blend toward, so pass 2
    // computes those lazily — without this the ring blended a pixel toward
    // itself and the feather did nothing.
    let mut exact = vec![false; (w * h) as usize];
    let mut scratch = vec![0u8; n];
    let (mut filled, mut unresolved) = (0u32, 0u32);
    let rows: Vec<&[u8]> = frames.iter().map(|f| f.px.as_slice()).collect();
    let thr3 = s.threshold * 3.0;
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize;
            let o = i * 3;
            let here = [cur.px[o], cur.px[o + 1], cur.px[o + 2]];
            // fast path: every frame agrees with this pixel -> background
            let mut all_agree = true;
            for f in &rows {
                let d = (f[o] as i32 - here[0] as i32).abs() + (f[o + 1] as i32 - here[1] as i32).abs()
                    + (f[o + 2] as i32 - here[2] as i32).abs();
                if d as f32 > thr3 { all_agree = false; break; }
            }
            if all_agree {
                med[i] = here;
                conf.px[i] = 255;
                continue;
            }
            exact[i] = true;
            let mut m = [0u8; 3];
            for c in 0..3 {
                for (k, f) in rows.iter().enumerate() { scratch[k] = f[o + c]; }
                m[c] = median_of(&mut scratch);
            }
            med[i] = m;
            let mut agree = 0usize;
            for f in &rows {
                let d = (f[o] as i32 - m[0] as i32).abs() + (f[o + 1] as i32 - m[1] as i32).abs()
                    + (f[o + 2] as i32 - m[2] as i32).abs();
                if d as f32 <= thr3 { agree += 1; }
            }
            let share = agree as f32 / n as f32;
            conf.px[i] = (share * 255.0) as u8;
            let diff = ((here[0] as i32 - m[0] as i32).abs() + (here[1] as i32 - m[1] as i32).abs()
                + (here[2] as i32 - m[2] as i32).abs()) as f32 / 3.0;
            let protected = s.protect.as_ref().map(|p| p.at(x as usize, y as usize) > 127).unwrap_or(false);
            if diff > s.threshold && !protected {
                if share >= s.min_confidence { covered[i] = true; } else { unresolved += 1; }
            }
        }
    }
    // pass 2: fill. The covered region is filled FULLY — feathering inside
    // it leaves a ring of the thing being removed (a 10 px block with a 2 px
    // feather kept 30% of its own pixels). The feather is a ring OUTSIDE it
    // instead: alpha ramps from 1 at the region's edge to 0 `feather` pixels
    // away, so the patch has no hard border against the plate.
    let r = s.feather as i32;
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize;
            let a = if covered[i] {
                1.0f32
            } else if r > 0 {
                // distance to the nearest covered pixel, within the ring
                let mut nearest = r + 1;
                for dy in -r..=r {
                    for dx in -r..=r {
                        let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                        if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 { continue; }
                        if covered[(ny as u32 * w + nx as u32) as usize] {
                            nearest = nearest.min(dx.abs().max(dy.abs()));
                        }
                    }
                }
                if nearest > r { continue; }
                1.0 - nearest as f32 / (r + 1) as f32
            } else {
                continue;
            };
            let here = cur.at(x, y);
            let i3 = i * 3;
            if !exact[i] {
                for c in 0..3 {
                    for (k, f) in frames.iter().enumerate() { scratch[k] = f.px[i3 + c]; }
                    med[i][c] = median_of(&mut scratch);
                }
                exact[i] = true;
            }
            let m = med[i];
            let mut v = [0u8; 3];
            for c in 0..3 { v[c] = (m[c] as f32 * a + here[c] as f32 * (1.0 - a)).round() as u8; }
            out.set(x, y, v);
            if covered[i] { filled += 1; }
        }
    }
    Cleaned { frame: out, confidence: conf, unresolved, filled }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Background: smooth low-frequency texture, the same every frame.
    fn background(w: u32, h: u32) -> Frame {
        let mut f = Frame::black(w, h);
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = (x as f32, y as f32);
                let a = 128.0 + 90.0 * (fx / 9.0).sin() * (fy / 7.0).cos();
                let b = 128.0 + 90.0 * (fx / 13.0 + fy / 11.0).sin();
                f.set(x, y, [a as u8, b as u8, ((a + b) / 2.0) as u8]);
            }
        }
        f
    }

    fn with_block(bg: &Frame, x0: i32, w: u32, colour: [u8; 3]) -> Frame {
        let mut f = bg.clone();
        for y in 6..bg.h - 6 {
            for x in x0.max(0)..(x0 + w as i32).min(bg.w as i32) {
                f.set(x as u32, y, colour);
            }
        }
        f
    }

    fn worst_diff(a: &Frame, b: &Frame, region: (u32, u32, u32, u32)) -> f32 {
        let mut worst = 0.0f32;
        for y in region.1..region.3 {
            for x in region.0..region.2 {
                let (p, q) = (a.at(x, y), b.at(x, y));
                for c in 0..3 { worst = worst.max((p[c] as f32 - q[c] as f32).abs()); }
            }
        }
        worst
    }

    #[test]
    fn a_fast_object_is_replaced_by_the_background_behind_it() {
        let bg = background(96, 54);
        // a 10 px block crossing at 14 px/frame: at the centre frame it
        // covers x 42..52, and in no other frame of the window
        let window: Vec<Frame> = (0..7).map(|i| with_block(&bg, 42 + (i as i32 - 3) * 14, 10, [255, 0, 0])).collect();
        let c = clean(&window, 3, &[], &Settings::default());
        assert!(c.filled > 300, "the block is about 10x42 px: filled {}", c.filled);
        assert_eq!(c.unresolved, 0);
        assert!(worst_diff(&c.frame, &bg, (44, 8, 50, 46)) <= 6.0,
                "the covered region must come back as background");
        // and nothing else moved
        assert_eq!(worst_diff(&c.frame, &bg, (0, 0, 30, 54)), 0.0);
    }

    #[test]
    fn a_protected_actor_is_never_cleaned_away() {
        let bg = background(96, 54);
        // the "actor": a still shape that IS a minority-free difference from
        // the median only because it is present in every frame — but a
        // stationary object that appears mid-window would be cleaned, which
        // is exactly why the mask exists
        let mut window: Vec<Frame> = (0..7).map(|_| bg.clone()).collect();
        // present in only the last two frames of seven, so the median is
        // still background and the maths WOULD erase it — the first version
        // of this fixture had it in four of seven, where the median is the
        // shape itself and nothing was ever flagged, so the mask was not
        // being tested at all.
        for (i, f) in window.iter_mut().enumerate() {
            if i >= 5 {
                for y in 10..40 { for x in 20..35 { f.set(x, y, [10, 200, 10]); } }
            }
        }
        let unprotected = clean(&window, 5, &[], &Settings::default());
        assert!(unprotected.filled > 100, "without a mask the late arrival is treated as an interruption");

        let mut mask = Gray::new(96, 54);
        for y in 10..40 { for x in 20..35 { mask.px[y * 96 + x] = 255; } }
        let protected = clean(&window, 5, &[], &Settings { protect: Some(mask), ..Default::default() });
        assert_eq!(protected.filled, 0, "inside the mask nothing is touched");
        assert_eq!(worst_diff(&protected.frame, &window[5], (0, 0, 96, 54)), 0.0);
    }

    #[test]
    fn a_slow_occluder_is_reported_not_smeared() {
        let bg = background(96, 54);
        // the block sits still for most of the window: the median IS the
        // block, so the background cannot be recovered here
        let window: Vec<Frame> = (0..7).map(|i| {
            if i < 5 { with_block(&bg, 40, 20, [255, 0, 0]) } else { bg.clone() }
        }).collect();
        let c = clean(&window, 2, &[], &Settings::default());
        // the centre frame agrees with the (block-coloured) median, so
        // nothing is even flagged as covered — and the confidence map is
        // what tells the editor the region is unusable
        let mid = c.confidence.at(45, 27);
        assert!(mid as f32 / 255.0 <= 0.75, "confidence inside a long occlusion must be low: {mid}");
        assert!(worst_diff(&c.frame, &window[2], (40, 8, 60, 46)) <= 1.0, "nothing was smeared in");
    }

    #[test]
    fn a_moving_camera_needs_its_offsets_and_uses_them() {
        let bg = background(96, 54);
        // camera pans 2 px/frame; the block crosses fast as before
        let window: Vec<Frame> = (0..7).map(|i| {
            let panned = bg.warp(-2.0 * (i as f32 - 3.0), 0.0, 1.0);
            with_block(&panned, 42 + (i as i32 - 3) * 14, 10, [255, 0, 0])
        }).collect();
        let naive = clean(&window, 3, &[], &Settings::default());
        let offsets: Vec<(f32, f32)> = (0..7).map(|i| (2.0 * (i as f32 - 3.0), 0.0)).collect();
        let aligned = clean(&window, 3, &offsets, &Settings::default());
        let region = (44, 12, 50, 42);
        let (a, nv) = (worst_diff(&aligned.frame, &window[3], region), worst_diff(&naive.frame, &window[3], region));
        assert!(a > 40.0, "the block is gone: it differed from the plate by {a}");
        assert!(aligned.filled > naive.filled,
                "unaligned, the pan makes every pixel look covered and confidence collapses: \
                 aligned filled {} vs naive {} (naive unresolved {}, aligned {})",
                aligned.filled, naive.filled, naive.unresolved, aligned.unresolved);
        assert!(naive.unresolved > aligned.unresolved);
    }

    #[test]
    fn feathering_cleans_up_the_soft_edge_the_threshold_missed() {
        let bg = background(96, 54);
        // A block with a motion-blurred edge: the edge columns are a 12%
        // blend, which is UNDER the threshold, so they are not flagged as
        // covered. Without a feather they survive as a faint outline of the
        // thing that was removed — which is what the feather is for. (A
        // hard-edged block needs no feather at all: outside it the frame and
        // the median are identical and blending changes nothing, which is
        // how the first version of this test managed to assert nothing.)
        let blur = |f: &mut Frame, x: u32, amount: f32| {
            for y in 6..f.h - 6 {
                let p = f.at(x, y);
                let mut v = [0u8; 3];
                let red = [255u8, 0, 0];
                for c in 0..3 { v[c] = (p[c] as f32 * (1.0 - amount) + red[c] as f32 * amount) as u8; }
                f.set(x, y, v);
            }
        };
        let window: Vec<Frame> = (0..5).map(|i| {
            let x0 = 42 + (i as i32 - 2) * 16;
            let mut f = with_block(&bg, x0, 8, [255, 0, 0]);
            if x0 > 0 { blur(&mut f, (x0 - 1) as u32, 0.12); }
            blur(&mut f, (x0 + 8) as u32, 0.12);
            f
        }).collect();
        let hard = clean(&window, 2, &[], &Settings { feather: 0, ..Default::default() });
        let soft = clean(&window, 2, &[], &Settings { feather: 3, ..Default::default() });
        let edge = (50u32, 27u32);  // the blurred column just outside the block
        assert!(worst_diff(&hard.frame, &bg, (edge.0, 20, edge.0 + 1, 34)) > 8.0,
                "unfeathered, the soft edge survives: {:?}", hard.frame.at(edge.0, edge.1));
        assert!(worst_diff(&soft.frame, &bg, (edge.0, 20, edge.0 + 1, 34))
                < worst_diff(&hard.frame, &bg, (edge.0, 20, edge.0 + 1, 34)),
                "feathered, it is pulled toward the plate");
        // the interior is filled the same either way
        assert_eq!(worst_diff(&hard.frame, &soft.frame, (43, 12, 49, 42)), 0.0);
    }
}
