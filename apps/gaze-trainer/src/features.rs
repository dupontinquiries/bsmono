//! What the model sees of a face: a small grey patch of each eye, normalised, plus nine numbers about where the face is and
//! how it is turned. Nothing about the pupil: in visible light at selfie distance an ellipse fit is only a coarse cue, and the
//! network can learn what matters (where the iris sits in the eye opening, how open the lid is) from the pixels.
//!
//! Everything is relative to the face's own ruler, the distance between the eyes, and to the line through the eyes, so a
//! head tilted or held closer gives the same numbers. Eye patches are sampled ALONG that line.

use gaze_kit::facefind::Face;
use orlosky_kit::Gray;

pub const PATCH_W: usize = 24;
pub const PATCH_H: usize = 14;
pub const GEOM: usize = 9;
pub const DIM: usize = 2 * PATCH_W * PATCH_H + GEOM;
/// Patch width in eye distances: an eye is about half the distance between the eyes wide, and the patch takes a margin.
pub const PATCH_EYE_DISTANCES: f32 = 0.55;
/// A face whose eyes are closer than this (in pixels) is too small to read an eye from.
pub const MIN_EYE_DISTANCE: f32 = 30.0;

/// Bilinear sample of a `w` x `h` window centred on (`cx`, `cy`), turned by `angle` (radians, along its own x axis),
/// resampled to `out_w` x `out_h`. Samples outside the image take the nearest edge pixel.
pub fn sample_turned(g: &Gray, cx: f32, cy: f32, w: f32, h: f32, angle: f32, out_w: usize, out_h: usize) -> Vec<f32> {
    let (s, c) = angle.sin_cos();
    let mut out = Vec::with_capacity(out_w * out_h);
    for y in 0..out_h {
        for x in 0..out_w {
            let (u, v) = ((x as f32 + 0.5) / out_w as f32 - 0.5, (y as f32 + 0.5) / out_h as f32 - 0.5);
            let (dx, dy) = (u * w, v * h);
            let (fx, fy) = ((cx + dx * c - dy * s - 0.5).clamp(0.0, g.w as f32 - 1.0), (cy + dx * s + dy * c - 0.5).clamp(0.0, g.h as f32 - 1.0));
            let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
            let (x1, y1) = ((x0 + 1).min(g.w - 1), (y0 + 1).min(g.h - 1));
            let (ax, ay) = (fx - x0 as f32, fy - y0 as f32);
            let p = |xx: usize, yy: usize| g.at(xx, yy) as f32;
            out.push(p(x0, y0) * (1.0 - ax) * (1.0 - ay) + p(x1, y0) * ax * (1.0 - ay) + p(x0, y1) * (1.0 - ax) * ay + p(x1, y1) * ax * ay);
        }
    }
    out
}

/// Subtract the mean, divide by the spread: lighting and exposure drop out, what is left is the pattern.
fn normalise(patch: &mut [f32]) {
    let n = patch.len() as f32;
    let mean = patch.iter().sum::<f32>() / n;
    let var = patch.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / n;
    let sd = var.sqrt().max(2.0); // a flat patch stays flat instead of blowing noise up
    for v in patch.iter_mut() { *v = (*v - mean) / sd; }
}

/// The feature vector of one face in an UPRIGHT grey frame, or None when the face is too small or its eyes fall outside.
pub fn extract(g: &Gray, face: &Face) -> Option<Vec<f32>> {
    let (a, b) = face.eyes();
    let ed = face.eye_distance();
    if ed < MIN_EYE_DISTANCE || !ed.is_finite() { return None; }
    let margin = ed * 0.25;
    for p in [a, b] {
        if p.0 < margin || p.1 < margin || p.0 > g.w as f32 - margin || p.1 > g.h as f32 - margin { return None; }
    }
    let angle = (b.1 - a.1).atan2(b.0 - a.0);
    let (w, h) = (PATCH_EYE_DISTANCES * ed, PATCH_EYE_DISTANCES * ed * PATCH_H as f32 / PATCH_W as f32);
    let mut v = Vec::with_capacity(DIM);
    for e in [a, b] {
        let mut patch = sample_turned(g, e.0, e.1, w, h, angle, PATCH_W, PATCH_H);
        normalise(&mut patch);
        v.extend(patch);
    }
    // geometry in the eye-line frame, in eye distances
    let mid = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
    let (s, c) = angle.sin_cos();
    let rel = |p: (f32, f32)| (((p.0 - mid.0) * c + (p.1 - mid.1) * s) / ed, (-(p.0 - mid.0) * s + (p.1 - mid.1) * c) / ed);
    let nose = rel(face.nose);
    let mouth_mid = ((face.mouth_a.0 + face.mouth_b.0) / 2.0, (face.mouth_a.1 + face.mouth_b.1) / 2.0);
    let mouth = rel(mouth_mid);
    let mouth_w = (face.mouth_b.0 - face.mouth_a.0).hypot(face.mouth_b.1 - face.mouth_a.1) / ed;
    v.extend([
        nose.0, nose.1, mouth.0, mouth.1, mouth_w, angle,
        (face.x + face.w / 2.0) / g.w as f32, (face.y + face.h / 2.0) / g.h as f32,
        ed / g.w as f32,
    ]);
    debug_assert_eq!(v.len(), DIM);
    v.iter().all(|x| x.is_finite()).then_some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(ea: (f32, f32), eb: (f32, f32)) -> Face {
        let mid = ((ea.0 + eb.0) / 2.0, (ea.1 + eb.1) / 2.0);
        let ed = (eb.0 - ea.0).hypot(eb.1 - ea.1);
        Face { x: mid.0 - ed, y: mid.1 - ed * 0.6, w: ed * 2.0, h: ed * 2.6, score: 0.9, eye_a: ea, eye_b: eb, nose: (mid.0, mid.1 + ed * 0.6), mouth_a: (mid.0 - ed * 0.4, mid.1 + ed * 1.1), mouth_b: (mid.0 + ed * 0.4, mid.1 + ed * 1.1) }
    }

    /// A frame with a dark dot (an iris) on each eye position of a face, the dot offset by `look` eye distances.
    fn frame(f: &Face, look: f32) -> Gray {
        let ed = f.eye_distance();
        Gray::from_fn(720, 1280, |x, y| {
            let mut v = 170.0f32;
            for e in [f.eye_a, f.eye_b] {
                let (cx, cy) = (e.0 + look * ed, e.1);
                if ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt() < ed * 0.09 { v = 40.0; }
            }
            v as u8
        })
    }

    #[test]
    fn a_feature_vector_has_the_documented_size_and_every_value_is_finite() {
        let f = face((250.0, 500.0), (470.0, 500.0));
        let v = extract(&frame(&f, 0.0), &f).unwrap();
        assert_eq!(v.len(), DIM);
        assert!(v.iter().all(|x| x.is_finite()));
        // each eye patch is normalised: zero mean, unit-ish spread
        for patch in [&v[..PATCH_W * PATCH_H], &v[PATCH_W * PATCH_H..2 * PATCH_W * PATCH_H]] {
            let mean = patch.iter().sum::<f32>() / patch.len() as f32;
            assert!(mean.abs() < 1e-3, "{mean}");
        }
    }

    #[test]
    fn where_the_iris_sits_in_the_eye_changes_the_patch_and_the_face_size_does_not() {
        let f = face((250.0, 500.0), (470.0, 500.0));
        let straight = extract(&frame(&f, 0.0), &f).unwrap();
        let left = extract(&frame(&f, -0.08), &f).unwrap();
        let right = extract(&frame(&f, 0.08), &f).unwrap();
        let d = |a: &[f32], b: &[f32]| a[..PATCH_W * PATCH_H].iter().zip(b).map(|(x, y)| (x - y).powi(2)).sum::<f32>();
        assert!(d(&straight, &left) > 20.0 && d(&straight, &right) > 20.0 && d(&left, &right) > 20.0, "the three looks must differ");
        // the same look on a face half the size (everything scaled by ed): the patches match, so the model sees the eye not the distance
        let small = face((300.0, 500.0), (410.0, 500.0));
        let small_v = extract(&frame(&small, 0.08), &small).unwrap();
        assert!(d(&right, &small_v) < d(&straight, &right) * 0.35, "scale-invariant: {} vs {}", d(&right, &small_v), d(&straight, &right));
    }

    #[test]
    fn a_tilted_head_gives_the_same_patches_as_an_upright_one_and_the_tilt_is_a_feature() {
        let up = face((250.0, 500.0), (470.0, 500.0));
        let ang = 0.3f32;
        let (s, c) = ang.sin_cos();
        let mid = (360.0f32, 500.0f32);
        let rot = |p: (f32, f32)| (mid.0 + (p.0 - mid.0) * c - (p.1 - mid.1) * s, mid.1 + (p.0 - mid.0) * s + (p.1 - mid.1) * c);
        let tilted = Face { eye_a: rot(up.eye_a), eye_b: rot(up.eye_b), nose: rot(up.nose), mouth_a: rot(up.mouth_a), mouth_b: rot(up.mouth_b), ..up };
        let flat = Gray::new(720, 1280, 170);
        let (a, b) = (extract(&flat, &up).unwrap(), extract(&flat, &tilted).unwrap());
        assert!((b[2 * PATCH_W * PATCH_H + 5] - 0.3).abs() < 1e-4, "the eye-line angle is the sixth geometry value");
        assert!((a[2 * PATCH_W * PATCH_H + 5]).abs() < 1e-4);
        // nose and mouth offsets are in the eye-line frame, so tilting the whole face leaves them where they were
        for i in 0..4 { assert!((a[2 * PATCH_W * PATCH_H + i] - b[2 * PATCH_W * PATCH_H + i]).abs() < 1e-3, "geometry {i}"); }
    }

    #[test]
    fn a_face_too_small_or_at_the_edge_has_no_features() {
        let flat = Gray::new(720, 1280, 170);
        assert!(extract(&flat, &face((300.0, 500.0), (320.0, 500.0))).is_none(), "eyes 20 px apart");
        assert!(extract(&flat, &face((5.0, 500.0), (225.0, 500.0))).is_none(), "an eye at the frame edge");
        assert!(extract(&flat, &face((250.0, 1270.0), (470.0, 1270.0))).is_none(), "eyes at the bottom edge");
        let nan = Face { nose: (f32::NAN, 0.0), ..face((250.0, 500.0), (470.0, 500.0)) };
        assert!(extract(&flat, &nan).is_none(), "non-finite landmarks are refused, not propagated");
    }
}
