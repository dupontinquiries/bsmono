//! The lightweight pupil detector: `OrloskyPupilDetectorLite.py`, ported.
//!
//! Jason Orlosky's algorithm, https://github.com/JEOresearch/EyeTracker (MIT, Copyright (c) 2024 JEOresearch). The
//! steps are his: crop to 4:3 and resize to 640x480; find the darkest 20x20 area (sampled every 5 px, searched every
//! 10); threshold at that pixel's value + 15; keep only a 250 px square around it; dilate by a 5x5 kernel twice; take
//! the largest outer blob that is at least 1000 px and no more than 3:1; fit an ellipse to it.
//!
//! Three deliberate differences from the Python:
//!
//! * His `get_darkest_area` sums `uint8` pixels into a `uint8`-typed accumulator under numpy 2, so the sum wraps at
//!   256 and "the darkest area" is whichever block wrapped lowest: on the project's own test video that gave a
//!   715 px tall "pupil" on frame 1. Here the sum is exact. (His README notes a numpy 2.0 issue; this is it.)
//! * `cv2.fitEllipse` fits the contour; `ellipse::fit_cv` is the same least-squares fit, on every boundary pixel
//!   (OpenCV compresses straight runs to their end points first, which weights the points differently). A degenerate
//!   fit falls back to the ellipse with the filled blob's second moments.
//! * On a frame with nothing to find (flat, or all one tone) his threshold admits everything and the "pupil" is the
//!   whole 250 px search square. A blob that fills the square is refused here.
//!
//! The reported ellipse is of the DILATED blob, as his is: a pupil of true diameter D reports about D + 8.

use crate::gray::{Gray, Mask};

pub const FRAME_W: usize = 640;
pub const FRAME_H: usize = 480;

/// Search for the darkest block: skip this many pixels at the border, step this far between blocks, block size, and
/// the sampling step inside a block.
const IGNORE_BOUNDS: usize = 20;
const IMAGE_SKIP: usize = 10;
const SEARCH_AREA: usize = 20;
const INTERNAL_SKIP: usize = 5;
/// Pixels this much lighter than the darkest pixel still count as pupil.
const ADDED_THRESHOLD: i32 = 15;
const KEEP_SQUARE: usize = 250;
/// A blob smaller than this many pixels is not a pupil, and one longer than this ratio is not either.
const MIN_AREA: usize = 1000;
const MAX_RATIO: f64 = 3.0;
/// A blob this much of the search square is the threshold admitting the whole frame, not a pupil.
const MAX_FILL: f64 = 0.9;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pupil {
    pub cx: f64,
    pub cy: f64,
    /// Full axis lengths of the ellipse (major, minor) and the major axis' angle in degrees.
    pub major: f64,
    pub minor: f64,
    pub angle_deg: f64,
    /// Pixels inside the blob.
    pub area: usize,
    /// Where the darkest area was found and the grey value the threshold was cut from.
    pub darkest: (usize, usize),
    pub threshold: i32,
}

impl Pupil {
    /// Mean radius in pixels.
    pub fn radius(&self) -> f64 { (self.major + self.minor) / 4.0 }
}

/// The centre of the darkest `SEARCH_AREA` block, in his sampling pattern, with an exact sum.
pub fn darkest_area(g: &Gray) -> Option<(usize, usize)> {
    let mut best: Option<(u64, (usize, usize))> = None;
    let mut y = IGNORE_BOUNDS;
    while y + IGNORE_BOUNDS < g.h {
        let mut x = IGNORE_BOUNDS;
        while x + IGNORE_BOUNDS < g.w {
            let (mut sum, mut n) = (0u64, 0u64);
            let mut dy = 0;
            'rows: while dy < SEARCH_AREA {
                if y + dy >= g.h { break 'rows; }
                let mut dx = 0;
                while dx < SEARCH_AREA {
                    if x + dx >= g.w { break; }
                    sum += g.at(x + dx, y + dy) as u64;
                    n += 1;
                    dx += INTERNAL_SKIP;
                }
                dy += INTERNAL_SKIP;
            }
            if n > 0 && best.is_none_or(|(s, _)| sum < s) { best = Some((sum, (x + SEARCH_AREA / 2, y + SEARCH_AREA / 2))); }
            x += IMAGE_SKIP;
        }
        y += IMAGE_SKIP;
    }
    best.map(|(_, p)| p)
}

/// Detect the pupil in a frame already cropped and resized to 640x480.
pub fn detect(g: &Gray) -> Option<Pupil> {
    let darkest = darkest_area(g)?;
    let threshold = g.at(darkest.0, darkest.1) as i32 + ADDED_THRESHOLD;
    let mut m = Mask::dark(g, threshold);
    m.keep_square(darkest, KEEP_SQUARE);
    // the search square, clipped to the frame: what a blob that fills it would cover (its dilation is allowed to spill)
    let half = KEEP_SQUARE / 2;
    let square_area = (((darkest.0 + half).min(g.w) - darkest.0.saturating_sub(half)) * ((darkest.1 + half).min(g.h) - darkest.1.saturating_sub(half))) as f64;
    let m = m.dilate_square(4); // a 5x5 kernel, two iterations
    let best = m
        .blobs()
        .into_iter()
        .filter(|b| b.area >= MIN_AREA)
        .filter(|b| (b.area as f64) < MAX_FILL * square_area)
        .filter(|b| {
            let (w, h) = (b.width() as f64, b.height() as f64);
            (w / h).max(h / w) <= MAX_RATIO
        })
        .max_by_key(|b| b.area)?;
    // his cv2.fitEllipse on the contour; the filled blob's moment ellipse only if that fit is degenerate
    let (cx, cy, major, minor, angle_deg) = match crate::ellipse::fit_cv(&best.contour) {
        Some(e) => (e.cx, e.cy, e.major, e.minor, e.angle_deg),
        None => { let (a, b, ang) = best.ellipse(); (best.cx, best.cy, a, b, ang) }
    };
    Some(Pupil { cx, cy, major, minor, angle_deg, area: best.area, darkest, threshold })
}

/// Detect in any size of frame: crop to 4:3, resize to 640x480, then `detect`. The returned pupil is in the 640x480 frame.
pub fn detect_any(g: &Gray) -> Option<Pupil> { detect(&g.crop_resize(FRAME_W, FRAME_H)) }

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic eye: skin 200, an iris of radius `ri` at (cx, cy) with value 95, a pupil of radius `rp` with value
    /// 18, and a glint (255) inside the iris.
    pub fn eye(cx: f64, cy: f64, ri: f64, rp: f64, glint: bool) -> Gray {
        Gray::from_fn(FRAME_W, FRAME_H, |x, y| {
            let d = ((x as f64 - cx).powi(2) + (y as f64 - cy).powi(2)).sqrt();
            let gl = ((x as f64 - (cx - rp * 0.4)).powi(2) + (y as f64 - (cy - rp * 0.5)).powi(2)).sqrt();
            if glint && gl < rp * 0.18 { 255 } else if d <= rp { 18 } else if d <= ri { 95 } else { 200 }
        })
    }

    #[test]
    fn it_finds_a_synthetic_pupil_to_within_a_pixel_and_its_dilated_size_to_within_a_pixel() {
        for (cx, cy) in [(320.0, 240.0), (250.0, 200.0), (400.0, 300.0), (300.0, 260.0)] {
            let p = detect(&eye(cx, cy, 110.0, 45.0, false)).expect("a pupil");
            assert!((p.cx - cx).abs() < 1.0 && (p.cy - cy).abs() < 1.0, "({cx},{cy}) found ({:.1},{:.1})", p.cx, p.cy);
            // 2 x 45, plus the 4 px the dilation adds each side (as in the original), plus the pixel-centre outline: 99.6 measured
            assert!((p.major - 99.6).abs() < 1.0 && (p.minor - 99.6).abs() < 1.0, "diameter {:.1} x {:.1}", p.major, p.minor);
        }
    }

    #[test]
    fn the_threshold_is_the_darkest_value_plus_fifteen_so_a_dark_iris_is_left_out() {
        // pupil 18, iris 40: +15 cuts at 33 and keeps the pupil alone; a looser offset would swallow the iris too
        let g = Gray::from_fn(FRAME_W, FRAME_H, |x, y| {
            let d = ((x as f64 - 320.0).powi(2) + (y as f64 - 240.0).powi(2)).sqrt();
            if d <= 45.0 { 18 } else if d <= 110.0 { 40 } else { 200 }
        });
        let p = detect(&g).expect("a pupil");
        assert_eq!(p.threshold, 33);
        assert!((p.major - 99.6).abs() < 1.0, "the pupil only, not the iris: {:.1}", p.major);
    }

    #[test]
    fn a_glint_inside_the_pupil_does_not_move_it() {
        let a = detect(&eye(300.0, 230.0, 110.0, 45.0, false)).unwrap();
        let b = detect(&eye(300.0, 230.0, 110.0, 45.0, true)).unwrap();
        assert!((a.cx - b.cx).abs() < 1.5 && (a.cy - b.cy).abs() < 1.5, "{a:?} vs {b:?}");
    }

    #[test]
    fn the_darkest_area_sum_does_not_wrap_the_way_the_python_does() {
        // two blocks: one uniformly 40 (16 samples = 640, wraps to 128 as a u8), one uniformly 10 (160). The python
        // would call the first darker; the true darkest is the second.
        let g = Gray::from_fn(FRAME_W, FRAME_H, |x, y| if x >= 100 && x < 130 && y >= 100 && y < 130 { 10 } else if x >= 300 && x < 330 && y >= 300 && y < 330 { 40 } else { 220 });
        let (x, y) = darkest_area(&g).unwrap();
        assert!(x >= 100 && x < 140 && y >= 100 && y < 140, "found ({x},{y})");
    }

    #[test]
    fn nothing_pupil_shaped_is_none_not_a_guess() {
        assert!(detect(&Gray::new(FRAME_W, FRAME_H, 128)).is_none(), "a flat frame");
        // a thin dark line is longer than 3:1
        let line = Gray::from_fn(FRAME_W, FRAME_H, |_, y| if (238..252).contains(&y) { 10 } else { 200 });
        assert!(detect(&line).is_none(), "a line is not a pupil");
    }

    #[test]
    fn a_frame_of_another_size_is_cropped_to_four_by_three_first() {
        let big = Gray::from_fn(1280, 720, |x, y| {
            let d = ((x as f64 - 640.0).powi(2) + (y as f64 - 360.0).powi(2)).sqrt();
            if d <= 80.0 { 18 } else if d <= 200.0 { 95 } else { 200 }
        });
        let p = detect_any(&big).expect("a pupil");
        // 1280x720 crops to 960x720 (offset 160) and scales by 2/3: centre 640 -> (640-160)*2/3 = 320
        assert!((p.cx - 320.0).abs() < 2.0 && (p.cy - 240.0).abs() < 2.0, "{p:?}");
    }
}
