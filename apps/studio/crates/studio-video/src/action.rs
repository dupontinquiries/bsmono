//! action — where in a shot the performer's movement surges.
//!
//! What this is, verified: a detector of movement WITHIN the frame, with the
//! camera's own motion cancelled, and of the surges in it. Its tests pin that
//! a gesture is found where it happens, that a pan with a still subject is
//! not action, and that accents are judged relative to the shot's own level.
//!
//! What it is NOT, measured against "All My Life": a predictor of where that
//! editor cut. The edit does end shots during active stretches -- flow-based
//! motion runs about 15% above the shot's norm over the last half second
//! before a cut, still at 10 frames back (p = 0.018), so it is not just
//! transitions at the cut -- but that is a soft tendency, not a rule about
//! peaks or rising edges:
//!
//! * real out-points sat FARTHER from movement peaks than random points did
//!   (0.83 s vs 0.60 s);
//! * they fell inside a rising window (take-off to peak) about as often as
//!   random points did (26% vs 22%).
//!
//! So it marks movement for an editor to SEE. It does not suggest cut
//! frames, and nothing should call it a suggestion until a rule is found that
//! predicts real cuts better than chance. `examples/action_check` is that test.
//!
//! * [`body_motion`]: per-frame movement within the frame -- the stabilizer's
//!   block search measures the camera's shift and it is cancelled first.
//! * [`accents`]: local peaks at least `rel` x the shot's own median.
//! * [`cut_windows`]: rising stretches, take-off to peak.

use crate::slate_ocr::Gray;
use crate::stabilize::estimate;

/// Movement within the frame, per frame (the first is 0): mean absolute
/// difference against the previous frame after cancelling the camera's shift.
pub fn body_motion(frames: &[Gray], max_shift: u32) -> Vec<f32> {
    let mut out = vec![0.0f32; frames.len()];
    for i in 1..frames.len() {
        let (a, b) = (&frames[i - 1], &frames[i]);
        let m = estimate(a, b, max_shift);
        let (dx, dy) = (m.dx.round() as i64, m.dy.round() as i64);
        let (w, h) = (a.w as i64, a.h as i64);
        let margin = (max_shift as i64 + 2).min(w / 4).min(h / 4);
        let (mut sum, mut n) = (0u64, 0u64);
        for y in margin..h - margin {
            for x in margin..w - margin {
                let (sx, sy) = (x - dx, y - dy);
                if sx < 0 || sy < 0 || sx >= w || sy >= h { continue; }
                let d = a.px[(sy * w + sx) as usize] as i32 - b.px[(y * w + x) as usize] as i32;
                sum += d.unsigned_abs() as u64; n += 1;
            }
        }
        out[i] = if n > 0 { sum as f32 / n as f32 } else { 0.0 };
    }
    out
}

/// A movement accent: the frame of the peak and how far above the shot's own
/// typical motion it rises.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Accent { pub frame: usize, pub strength: f32 }

/// Surges in `motion` (one shot's worth): local peaks >= `rel` x the shot's
/// median, then the strongest kept when two are within `min_gap` seconds.
pub fn accents(motion: &[f32], fps: f64, rel: f32, min_gap: f64) -> Vec<Accent> {
    let n = motion.len();
    if n < 5 { return Vec::new(); }
    // smooth over ~0.1 s: an accent is a movement, not one noisy frame
    let k = ((fps * 0.1).round() as usize).max(1);
    let sm: Vec<f32> = (0..n).map(|i| {
        let (a, b) = (i.saturating_sub(k), (i + k + 1).min(n));
        motion[a..b].iter().sum::<f32>() / (b - a) as f32
    }).collect();
    let mut sorted = sm[1..].to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = sorted[sorted.len() / 2].max(1e-6);
    let mut cand: Vec<Accent> = (1..n - 1)
        .filter(|&i| sm[i] >= sm[i - 1] && sm[i] > sm[i + 1] && sm[i] / median >= rel)
        .map(|i| Accent { frame: i, strength: sm[i] / median })
        .collect();
    cand.sort_by(|a, b| b.strength.partial_cmp(&a.strength).unwrap_or(std::cmp::Ordering::Equal));
    let gap = (min_gap * fps).round() as usize;
    let mut kept: Vec<Accent> = Vec::new();
    for c in cand {
        if kept.iter().all(|k| k.frame.abs_diff(c.frame) >= gap) { kept.push(c); }
    }
    kept.sort_by_key(|a| a.frame);
    kept
}

/// A rising stretch of movement: from where it takes off to where it peaks.
///
/// Built on the hypothesis that an editor cutting on action cuts while a
/// movement is still building, so the peak never happens in the outgoing
/// shot. Tested against "All My Life" it did not hold: real out-points fell
/// inside these windows about as often as random points (26% vs 22%). Kept
/// because the windows are still what an editor looking for "the move" wants
/// to see, and because the test that refuted it is the one to re-run on other
/// edits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CutWindow { pub start: usize, pub peak: usize, pub strength: f32 }

/// Rising stretches of movement at least `rel` x the shot's median. A surge
/// still rising at the end of the motion slice ends its window at the last
/// frame -- exactly the case a cut on action produces.
pub fn cut_windows(motion: &[f32], fps: f64, rel: f32) -> Vec<CutWindow> {
    let n = motion.len();
    if n < 5 { return Vec::new(); }
    let k = ((fps * 0.1).round() as usize).max(1);
    let sm: Vec<f32> = (0..n).map(|i| {
        let (a, b) = (i.saturating_sub(k), (i + k + 1).min(n));
        motion[a..b].iter().sum::<f32>() / (b - a) as f32
    }).collect();
    let mut sorted = sm[1..].to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = sorted[sorted.len() / 2].max(1e-6);
    let above: Vec<bool> = sm.iter().map(|v| v / median >= rel).collect();
    let mut out = Vec::new();
    let mut i = 1;
    while i < n {
        if above[i] {
            // walk back to where it took off (last point at or below the median)
            let mut s = i;
            while s > 1 && sm[s - 1] < sm[s] && sm[s - 1] / median > 1.0 { s -= 1; }
            let mut j = i;
            while j + 1 < n && above[j + 1] { j += 1; }
            let peak = (i..=j).max_by(|&a, &b| sm[a].partial_cmp(&sm[b]).unwrap_or(std::cmp::Ordering::Equal)).unwrap();
            out.push(CutWindow { start: s, peak, strength: sm[peak] / median });
            i = j + 1;
        } else { i += 1; }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A textured scene viewed with the camera at (cx, 0), with a "performer"
    /// block that sits still or moves by `jump` px between frames.
    fn frame(w: usize, h: usize, cam: i64, performer_x: i64) -> Gray {
        let mut g = Gray::new(w, h);
        for y in 0..h { for x in 0..w {
            let sx = x as i64 + cam;
            let v = 128.0 + 60.0 * ((sx as f32) / 7.0).sin() * ((y as f32) / 5.0).cos();
            g.px[y * w + x] = v as u8;
        }}
        // the performer: a bright block
        for y in 30..60 { for x in 0..18 {
            let px = performer_x + x as i64 - cam;
            if px >= 0 && (px as usize) < w { g.px[y * w + px as usize] = 245; }
        }}
        g
    }

    #[test]
    fn a_gesture_is_found_where_it_happens() {
        // still for 1 s, a quick move at 1.0-1.3 s, still, another at 2.2-2.4 s
        let mut x = 60i64;
        let frames: Vec<Gray> = (0..84).map(|i| {
            if (24..31).contains(&i) || (53..58).contains(&i) { x += 5; }
            frame(160, 90, 0, x)
        }).collect();
        let m = body_motion(&frames, 12);
        let a = accents(&m, 24.0, 1.5, 0.5);
        assert_eq!(a.len(), 2, "{a:?}");
        assert!((24..32).contains(&a[0].frame) && (52..59).contains(&a[1].frame), "{a:?}");
    }

    #[test]
    fn a_pan_with_a_still_performer_is_not_action() {
        // the camera pans 3 px/frame; the performer stands still in the scene
        let frames: Vec<Gray> = (0..60).map(|i| frame(160, 90, 3 * i as i64, 80)).collect();
        let m = body_motion(&frames, 12);
        let a = accents(&m, 24.0, 1.5, 0.5);
        assert!(a.is_empty(), "camera motion is cancelled before judging action: {a:?}");
        // and the same pan with a gesture in it still finds the gesture
        let mut x = 80i64;
        let frames: Vec<Gray> = (0..60).map(|i| {
            if (30..36).contains(&i) { x += 6; }
            frame(160, 90, 3 * i as i64, x)
        }).collect();
        let a = accents(&body_motion(&frames, 12), 24.0, 1.5, 0.5);
        assert_eq!(a.len(), 1, "{a:?}");
        assert!((29..38).contains(&a[0].frame), "{a:?}");
    }

    #[test]
    fn accents_are_relative_to_the_shot_and_kept_apart() {
        // a busy shot: constant motion with two stronger bursts close together
        let mut m = vec![2.0f32; 100];
        for i in 40..44 { m[i] = 6.0; }
        for i in 46..49 { m[i] = 5.0; }   // 6 frames later: within the 0.5 s gap
        for i in 80..83 { m[i] = 5.5; }
        let a = accents(&m, 24.0, 1.5, 0.5);
        assert_eq!(a.len(), 2, "the close pair collapses to its stronger peak: {a:?}");
        assert!((40..44).contains(&a[0].frame) && (80..83).contains(&a[1].frame), "{a:?}");
        assert!(accents(&vec![2.0; 100], 24.0, 1.5, 0.5).is_empty(), "constant motion has no accent");
    }
}
