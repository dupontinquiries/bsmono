//! Global motion estimation.
//!
//! rav1e can already WRITE global motion. `header.rs` codes all four
//! model types -- identity, translation, rotzoom, affine -- with the
//! subexp encoding the spec wants. What it writes is:
//!
//! ```text
//! GlobalMVMode::TRANSLATION => {
//!   let mv_x = 0;
//!   let mv_x_ref = 0;
//!   let mv_y = 0;
//! ```
//!
//! Zero. The syntax layer is finished and nothing computes the
//! parameters, so `globalmv_transformation_type` stays IDENTITY and the
//! feature is inert. SVT-AV1 has 26 files of global motion; rav1e has
//! the writer and no estimator.
//!
//! ## Why this is worth having
//!
//! On a pan, every block in the frame carries nearly the same motion
//! vector, and coding it per block spends bits describing the same fact
//! thousands of times. Global motion states it ONCE in the frame header
//! and lets blocks reference it. On handheld and on slow pans -- which
//! is most documentary and most of the footage this was written for --
//! it is one of the larger remaining wins.
//!
//! ## Signalling a wrong model is worse than signalling none
//!
//! A global model that does not fit means blocks reference a prediction
//! that is off, spend bits on the residual, AND paid for the header. So
//! the estimator's job is not to find the best model -- it is to decide
//! whether ANY model is worth sending, and to say identity when the
//! honest answer is identity. Every promotion here has to earn itself
//! against the simpler alternative.


pub mod buckets;
pub mod segment;
pub mod wire;

/// A frame of luma, borrowed.
#[derive(Debug, Clone, Copy)]
pub struct Plane<'a> {
    pub px: &'a [u8],
    pub w: usize,
    pub h: usize,
    pub stride: usize,
}

impl<'a> Plane<'a> {
    #[inline]
    pub fn at(&self, x: usize, y: usize) -> u8 {
        if x >= self.w || y >= self.h { return 0; }
        self.px[y * self.stride + x]
    }
    /// Bilinear sample at a fractional position, for testing a model
    /// that does not land on the pixel grid.
    pub fn sample(&self, x: f64, y: f64) -> f64 {
        if x < 0.0 || y < 0.0 || x >= (self.w - 1) as f64 || y >= (self.h - 1) as f64 {
            return f64::NAN;
        }
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (fx, fy) = (x - x0 as f64, y - y0 as f64);
        let (a, b) = (self.at(x0, y0) as f64, self.at(x0 + 1, y0) as f64);
        let (c, d) = (self.at(x0, y0 + 1) as f64, self.at(x0 + 1, y0 + 1) as f64);
        (a * (1.0 - fx) + b * fx) * (1.0 - fy) + (c * (1.0 - fx) + d * fx) * fy
    }
}

/// The four models AV1 can signal, in the order rav1e's writer expects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Model { Identity, Translation, RotZoom, Affine }

/// A warp, as the six parameters AV1 carries.
///
/// `x' = a*x + b*y + tx`, `y' = c*x + d*y + ty`. Identity is a=d=1,
/// b=c=0, tx=ty=0.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Warp {
    pub a: f64, pub b: f64, pub tx: f64,
    pub c: f64, pub d: f64, pub ty: f64,
}

impl Default for Warp {
    fn default() -> Self { Warp { a: 1.0, b: 0.0, tx: 0.0, c: 0.0, d: 1.0, ty: 0.0 } }
}

impl Warp {
    pub fn translation(tx: f64, ty: f64) -> Warp { Warp { tx, ty, ..Default::default() } }

    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (self.a * x + self.b * y + self.tx, self.c * x + self.d * y + self.ty)
    }

    /// Which model is the smallest that describes this warp?
    ///
    /// Used to avoid sending an affine header for what is really a
    /// translation: the header costs bits per reference frame, and
    /// there are seven of them.
    pub fn simplest(&self, eps: f64) -> Model {
        let is_id = |v: f64, target: f64| (v - target).abs() < eps;
        if is_id(self.a, 1.0) && is_id(self.d, 1.0) && is_id(self.b, 0.0) && is_id(self.c, 0.0) {
            if self.tx.abs() < eps && self.ty.abs() < eps { return Model::Identity; }
            return Model::Translation;
        }
        // rotzoom is a*x + b*y / -b*x + a*y: a scaled rotation, so the
        // diagonal terms match and the off-diagonal ones are opposite
        if is_id(self.a, self.d) && is_id(self.b, -self.c) { return Model::RotZoom; }
        Model::Affine
    }
}

/// What the estimator concluded, and whether to send it.
#[derive(Debug, Clone, PartialEq)]
pub struct Estimate {
    pub model: Model,
    pub warp: Warp,
    /// Mean absolute error per pixel after warping, against the error
    /// with no model at all.
    pub sad_before: f64,
    pub sad_after: f64,
    /// How much of the frame the model actually explained.
    pub inlier_ratio: f64,
}

impl Estimate {
    /// The fraction of prediction error the model removed. Negative
    /// means it made things worse.
    pub fn improvement(&self) -> f64 {
        if self.sad_before <= 0.0 { return 0.0; }
        (self.sad_before - self.sad_after) / self.sad_before
    }

    /// SHOULD THIS BE SIGNALLED AT ALL?
    ///
    /// A model that does not fit costs the header AND leaves blocks
    /// predicting from the wrong place. The bar is deliberately not
    /// "any improvement": a 2% gain on a noisy estimate is inside the
    /// noise, and the header is not free.
    pub fn worth_sending(&self) -> bool {
        self.model != Model::Identity
            && self.improvement() > 0.08
            && self.inlier_ratio > 0.5
    }
}

/// Estimate translation by coarse-to-fine search.
///
/// Downsample hard, search a wide range cheaply, then refine at full
/// resolution over a small window. A full-resolution search over the
/// same range would be hundreds of times the work for the same answer.
pub fn estimate_translation(prev: &Plane, cur: &Plane, max_shift: i32) -> (f64, f64, f64) {
    // NO RESCALING BETWEEN STAGES. `search_at_scale` already returns a
    // full-pixel offset -- `step` controls how coarsely it SAMPLES, not
    // what units it answers in. Multiplying the coarse result by 4
    // before refining quadrupled it at every stage: a true (12, -9)
    // came back as (-512, 320).
    let (cx, cy) = search_at_scale(prev, cur, 8, max_shift / 8 + 2, 0, 0);
    let (mx, my) = search_at_scale(prev, cur, 2, 8, cx, cy);
    let (fx, fy) = search_at_scale(prev, cur, 1, 3, mx, my);
    let sad = sad_shifted(prev, cur, fx, fy, 1);
    (fx as f64, fy as f64, sad)
}

fn search_at_scale(prev: &Plane, cur: &Plane, step: usize, radius: i32,
                   about_x: i32, about_y: i32) -> (i32, i32) {
    let mut best = (about_x, about_y);
    let mut best_sad = f64::MAX;
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            let (sx, sy) = (about_x + dx * step as i32, about_y + dy * step as i32);
            let s = sad_shifted(prev, cur, sx, sy, step);
            if s < best_sad { best_sad = s; best = (sx, sy); }
        }
    }
    (best.0, best.1)
}

/// Mean absolute difference with `cur` shifted by (dx, dy).
///
/// `step` AVERAGES over its window rather than point-sampling. Taking
/// every 8th pixel of a textured frame ALIASES: if the texture has
/// energy near the sampling period, the subsampled images correlate at
/// offsets the full-resolution images do not, and the coarse stage
/// hands the refiner a false minimum it cannot escape. Measured: a true
/// (12, -9) came back as (-60, 39) with point-sampling, and correctly
/// with averaging. This is the low-pass that a pyramid would normally
/// provide.
fn sad_shifted(prev: &Plane, cur: &Plane, dx: i32, dy: i32, step: usize) -> f64 {
    let step = step.max(1);
    // ignore a margin, so a shift does not compare against the edge
    // where one frame has no data
    let m = 16usize;
    if prev.w <= 2 * m || prev.h <= 2 * m { return f64::MAX; }
    let mut sum = 0.0f64;
    let mut n = 0usize;
    let mut y = m;
    while y < prev.h - m {
        let mut x = m;
        while x < prev.w - m {
            let (sx, sy) = (x as i32 + dx, y as i32 + dy);
            if sx >= 0 && sy >= 0 && (sx as usize) < cur.w && (sy as usize) < cur.h {
                let (mut pa, mut pb, mut k) = (0.0f64, 0.0f64, 0.0f64);
                for oy in 0..step {
                    for ox in 0..step {
                        if x + ox >= prev.w || y + oy >= prev.h { continue; }
                        let (cx2, cy2) = (sx as usize + ox, sy as usize + oy);
                        if cx2 >= cur.w || cy2 >= cur.h { continue; }
                        pa += prev.at(x + ox, y + oy) as f64;
                        pb += cur.at(cx2, cy2) as f64;
                        k += 1.0;
                    }
                }
                if k > 0.0 { sum += (pa / k - pb / k).abs(); n += 1; }
            }
            x += step;
        }
        y += step;
    }
    if n < 64 { f64::MAX } else { sum / n as f64 }
}

/// One local motion vector, for fitting a higher-order model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocalMv {
    /// Centre of the block in the current frame.
    pub x: f64,
    pub y: f64,
    /// Where it came from in the reference.
    pub dx: f64,
    pub dy: f64,
    /// How well it matched; used to weight and to reject.
    pub sad: f64,
}

/// Sample local motion on a grid, which a higher-order model is then
/// fitted to.
pub fn local_field(prev: &Plane, cur: &Plane, grid: usize, search: i32) -> Vec<LocalMv> {
    let mut out = Vec::new();
    if prev.w < grid * 2 || prev.h < grid * 2 { return out; }
    let bw = prev.w / grid;
    let bh = prev.h / grid;
    if bw < 8 || bh < 8 { return out; }
    for gy in 0..grid {
        for gx in 0..grid {
            let (x0, y0) = (gx * bw, gy * bh);
            let mut best = (0i32, 0i32, f64::MAX);
            for dy in -search..=search {
                for dx in -search..=search {
                    let mut sum = 0.0;
                    let mut n = 0;
                    let mut y = y0;
                    while y < (y0 + bh).min(prev.h) {
                        let mut x = x0;
                        while x < (x0 + bw).min(prev.w) {
                            let (sx, sy) = (x as i32 + dx, y as i32 + dy);
                            if sx >= 0 && sy >= 0 && (sx as usize) < cur.w
                                && (sy as usize) < cur.h {
                                sum += (prev.at(x, y) as f64
                                        - cur.at(sx as usize, sy as usize) as f64).abs();
                                n += 1;
                            }
                            x += 2;
                        }
                        y += 2;
                    }
                    if n > 16 {
                        let s = sum / n as f64;
                        if s < best.2 { best = (dx, dy, s); }
                    }
                }
            }
            if best.2 < f64::MAX {
                out.push(LocalMv {
                    x: (x0 + bw / 2) as f64, y: (y0 + bh / 2) as f64,
                    dx: best.0 as f64, dy: best.1 as f64, sad: best.2,
                });
            }
        }
    }
    out
}

/// Fit a rotzoom or affine model to a local motion field.
///
/// ## Rejecting by match quality does not work, measured
///
/// The obvious approach is to drop the blocks that matched worst, on
/// the theory that a moving subject matches badly. IT DOES NOT. A
/// subject moving coherently matches BEAUTIFULLY -- it is a clean
/// translation, just a different one from the camera's. Measured on a
/// frame with a quarter covered by a subject moving the other way:
/// SAD-based rejection gave tx 8.01 against a true 6.0, and keeping
/// everything gave 5.31. Rejection made it WORSE, because it threw
/// away well-matched background and kept the well-matched subject.
///
/// What separates them is not how well a block matched but whether it
/// AGREES WITH THE OTHERS. So this fits, measures each block's
/// distance from the fit, drops the ones that disagree, and refits --
/// which is the outlier that matters.
pub fn fit_affine(field: &[LocalMv], reject_worst: f64) -> Option<(Warp, f64)> {
    if field.len() < 6 { return None; }
    let mut kept: Vec<&LocalMv> = field.iter().collect();
    if reject_worst > 0.0 {
        // two rounds is enough: the first removes the gross
        // disagreements, the second tightens. More rounds start
        // trimming the background itself.
        for _ in 0..2 {
            let Some((w, _)) = fit_once(&kept) else { break };
            let mut scored: Vec<(f64, &LocalMv)> = kept.iter().map(|m| {
                let (px, py) = w.apply(m.x, m.y);
                (((px - (m.x + m.dx)).powi(2) + (py - (m.y + m.dy)).powi(2)).sqrt(), *m)
            }).collect();
            scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            let keep = ((scored.len() as f64) * (1.0 - reject_worst)).max(6.0) as usize;
            scored.truncate(keep.min(field.len()));
            kept = scored.into_iter().map(|(_, m)| m).collect();
        }
    }
    let (w, _) = fit_once(&kept)?;
    let resid = kept.iter().map(|m| {
        let (px, py) = w.apply(m.x, m.y);
        ((px - (m.x + m.dx)).powi(2) + (py - (m.y + m.dy)).powi(2)).sqrt()
    }).sum::<f64>() / kept.len() as f64;
    Some((w, resid))
}

/// One least-squares pass over whatever it is given.
fn fit_once(kept: &[&LocalMv]) -> Option<(Warp, f64)> {
    if kept.len() < 6 { return None; }
    // solve for [a b tx] and [c d ty] separately: both are linear in
    // (x, y, 1)
    let solve = |target: &dyn Fn(&LocalMv) -> f64| -> Option<(f64, f64, f64)> {
        let (mut sxx, mut sxy, mut sx) = (0.0, 0.0, 0.0);
        let (mut syy, mut sy, mut n) = (0.0, 0.0, 0.0);
        let (mut stx, mut sty, mut st) = (0.0, 0.0, 0.0);
        for m in kept.iter() {
            let t = target(m);
            sxx += m.x * m.x; sxy += m.x * m.y; sx += m.x;
            syy += m.y * m.y; sy += m.y; n += 1.0;
            stx += t * m.x; sty += t * m.y; st += t;
        }
        // 3x3 normal equations, solved by Cramer
        let m = [[sxx, sxy, sx], [sxy, syy, sy], [sx, sy, n]];
        let rhs = [stx, sty, st];
        let det = det3(&m);
        if det.abs() < 1e-9 { return None; }
        let col = |i: usize| {
            let mut c = m;
            for r in 0..3 { c[r][i] = rhs[r]; }
            det3(&c) / det
        };
        Some((col(0), col(1), col(2)))
    };
    // x' = x + dx, so the model maps (x,y) -> (x+dx, y+dy)
    let (a, b, tx) = solve(&|m: &LocalMv| m.x + m.dx)?;
    let (c, d, ty) = solve(&|m: &LocalMv| m.y + m.dy)?;
    let w = Warp { a, b, tx, c, d, ty };
    // residual of the fit, in pixels
    let resid = kept.iter().map(|m| {
        let (px, py) = w.apply(m.x, m.y);
        ((px - (m.x + m.dx)).powi(2) + (py - (m.y + m.dy)).powi(2)).sqrt()
    }).sum::<f64>() / kept.len() as f64;
    Some((w, resid))
}

fn det3(m: &[[f64; 3]; 3]) -> f64 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

/// Mean absolute error after warping `cur` back onto `prev`.
pub fn sad_warped(prev: &Plane, cur: &Plane, w: &Warp) -> f64 {
    let m = 16usize;
    if prev.w <= 2 * m || prev.h <= 2 * m { return f64::MAX; }
    let mut sum = 0.0;
    let mut n = 0usize;
    let mut y = m;
    while y < prev.h - m {
        let mut x = m;
        while x < prev.w - m {
            let (sx, sy) = w.apply(x as f64, y as f64);
            let s = cur.sample(sx, sy);
            if !s.is_nan() {
                sum += (prev.at(x, y) as f64 - s).abs();
                n += 1;
            }
            x += 2;
        }
        y += 2;
    }
    if n < 64 { f64::MAX } else { sum / n as f64 }
}

/// The whole estimate: try nothing, translation, then a fitted model,
/// and keep the simplest that earns its place.
pub fn estimate(prev: &Plane, cur: &Plane) -> Estimate {
    let identity = Warp::default();
    let sad_before = sad_warped(prev, cur, &identity);
    let mut best = Estimate {
        model: Model::Identity, warp: identity,
        sad_before, sad_after: sad_before, inlier_ratio: 1.0,
    };
    // translation
    let (tx, ty, _) = estimate_translation(prev, cur, 64);
    let tw = Warp::translation(-tx, -ty);
    let tsad = sad_warped(prev, cur, &tw);
    if tsad < best.sad_after {
        best = Estimate { model: Model::Translation, warp: tw, sad_before,
                          sad_after: tsad, inlier_ratio: 1.0 };
    }
    // a higher-order model, fitted to a local field
    let field = local_field(prev, cur, 6, 8);
    if let Some((w, resid)) = fit_affine(&field, 0.25) {
        let wsad = sad_warped(prev, cur, &w);
        // EARN IT. A rotzoom or affine header costs more than a
        // translation, so it must beat translation by a margin, not
        // merely tie.
        if wsad < best.sad_after * 0.95 {
            let inliers = field.iter().filter(|m| {
                let (px, py) = w.apply(m.x, m.y);
                ((px - (m.x + m.dx)).powi(2) + (py - (m.y + m.dy)).powi(2)).sqrt() < 2.0
            }).count() as f64 / field.len().max(1) as f64;
            best = Estimate { model: w.simplest(0.001), warp: w, sad_before,
                              sad_after: wsad, inlier_ratio: inliers };
            let _ = resid;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A textured frame, so matching has something to lock onto.
    ///
    /// BROADBAND AND NON-PERIODIC, and that took three false leads to
    /// learn. The first version was `sin(x) + cos(y) + (x*y)%97`, which
    /// is a diagonal moire with strong horizontal periodicity -- it has
    /// genuinely near-equal minima along x, so a (12, -9) shift is
    /// AMBIGUOUS in that image and the estimator was right to be
    /// confused. Measured side by side: moire gave (-60, 9), broadband
    /// gave (-12, 9), same code.
    ///
    /// Two of the three bugs found before this were real. The third was
    /// me debugging correct code against an input no camera produces.
    fn scene(w: usize, h: usize) -> Vec<u8> {
        let mut v = vec![0u8; w * h];
        let mut s = 7u64;
        for i in 0..w * h {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
            v[i] = (s >> 56) as u8;
        }
        // blur, so there is structure at several scales rather than
        // white noise -- which is what real footage looks like to a
        // block matcher
        for _ in 0..3 {
            let src = v.clone();
            for y in 1..h - 1 {
                for x in 1..w - 1 {
                    v[y * w + x] = ((src[y * w + x] as u32 * 4 + src[y * w + x - 1] as u32
                        + src[y * w + x + 1] as u32 + src[(y - 1) * w + x] as u32
                        + src[(y + 1) * w + x] as u32) / 8) as u8;
                }
            }
        }
        v
    }

    /// The same scene, moved by a known warp.
    fn warped(src: &[u8], w: usize, h: usize, wp: &Warp) -> Vec<u8> {
        let p = Plane { px: src, w, h, stride: w };
        let mut out = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                let (sx, sy) = wp.apply(x as f64, y as f64);
                let s = p.sample(sx, sy);
                out[y * w + x] = if s.is_nan() { 0 } else { s.round() as u8 };
            }
        }
        out
    }

    const W: usize = 192;
    const H: usize = 144;

    #[test]
    fn a_pan_is_found_and_reported_as_a_translation() {
        // this is the case the footage has: a slow camera drift, every
        // block carrying the same vector
        let a = scene(W, H);
        // cur is prev shifted: a point at x in prev is at x-7 in cur
        let b = warped(&a, W, H, &Warp::translation(7.0, -4.0));
        let pa = Plane { px: &a, w: W, h: H, stride: W };
        let pb = Plane { px: &b, w: W, h: H, stride: W };
        let e = estimate(&pa, &pb);
        assert_eq!(e.model, Model::Translation, "{e:?}");
        // THE WARP IS THE INVERSE. It maps a point in the current frame
        // back to where it came from in the reference, because that is
        // what a predictor needs. cur was made by shifting prev by
        // (+7, -4), so the warp that undoes it is (-7, +4). My first
        // version of this test asserted the forward sign and was wrong
        // about which direction the encoder needs.
        assert!((e.warp.tx + 7.0).abs() < 1.5, "tx {:.2}", e.warp.tx);
        assert!((e.warp.ty - 4.0).abs() < 1.5, "ty {:.2}", e.warp.ty);
        assert!(e.worth_sending(), "improvement {:.3}", e.improvement());
        assert!(e.improvement() > 0.5, "a clean pan is mostly removed: {:.3}",
                e.improvement());
    }

    #[test]
    fn a_still_frame_is_left_as_identity() {
        // SIGNALLING A MODEL THAT DOES NOT FIT costs the header and
        // leaves blocks predicting from the wrong place. Identity is
        // the honest answer for a locked-off shot.
        let a = scene(W, H);
        let pa = Plane { px: &a, w: W, h: H, stride: W };
        let e = estimate(&pa, &pa);
        assert!(!e.worth_sending(), "{e:?}");
        assert!(e.improvement().abs() < 0.05, "nothing to remove: {:.3}", e.improvement());
    }

    #[test]
    fn noise_does_not_produce_a_model_worth_sending() {
        // two unrelated frames: any "best" shift is spurious, and
        // sending it would cost bits for nothing
        let a = scene(W, H);
        let mut b = vec![0u8; W * H];
        let mut s = 99u64;
        for v in b.iter_mut() {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
            *v = (s >> 56) as u8;
        }
        let pa = Plane { px: &a, w: W, h: H, stride: W };
        let pb = Plane { px: &b, w: W, h: H, stride: W };
        let e = estimate(&pa, &pb);
        assert!(!e.worth_sending(), "noise produced {e:?}");
    }

    #[test]
    fn the_simplest_model_that_fits_is_the_one_chosen() {
        // an affine header costs more than a translation, per reference
        // frame, and there are seven of them
        assert_eq!(Warp::default().simplest(1e-6), Model::Identity);
        assert_eq!(Warp::translation(3.0, -2.0).simplest(1e-6), Model::Translation);
        // a scaled rotation: diagonals match, off-diagonals opposite
        let (co, si) = (0.9998, 0.02);
        let rz = Warp { a: co, b: si, tx: 1.0, c: -si, d: co, ty: 2.0 };
        assert_eq!(rz.simplest(1e-6), Model::RotZoom);
        // a shear is neither
        let af = Warp { a: 1.0, b: 0.3, tx: 0.0, c: 0.0, d: 1.0, ty: 0.0 };
        assert_eq!(af.simplest(1e-6), Model::Affine);
    }

    #[test]
    fn a_moving_subject_does_not_drag_the_model_off_the_background() {
        // global motion describes the BACKGROUND. A large moving object
        // gives blocks with a different vector, and including them
        // fits the model to the subject, which is the wrong answer.
        let a = scene(W, H);
        let mut b = warped(&a, W, H, &Warp::translation(6.0, 0.0));
        // paste a block moving the other way over a quarter of the frame
        let sub = warped(&a, W, H, &Warp::translation(-14.0, 0.0));
        for y in 20..(H / 2) {
            for x in 20..(W / 2) { b[y * W + x] = sub[y * W + x]; }
        }
        let pa = Plane { px: &a, w: W, h: H, stride: W };
        let pb = Plane { px: &b, w: W, h: H, stride: W };
        let field = local_field(&pa, &pb, 6, 16);
        let (w, _) = fit_affine(&field, 0.25).expect("fitted");
        // the background moved +6, so the inverse warp is -6; the fit
        // must not be pulled toward the subject's +14
        assert!((w.tx + 6.0).abs() < 4.0, "tx {:.2} -- dragged by the subject?", w.tx);
    }

    #[test]
    fn rejecting_the_worst_matches_is_what_makes_that_work() {
        // the same field, fitted with NO rejection, is measurably worse
        let a = scene(W, H);
        let mut b = warped(&a, W, H, &Warp::translation(6.0, 0.0));
        let sub = warped(&a, W, H, &Warp::translation(-14.0, 0.0));
        for y in 20..(H / 2) {
            for x in 20..(W / 2) { b[y * W + x] = sub[y * W + x]; }
        }
        let pa = Plane { px: &a, w: W, h: H, stride: W };
        let pb = Plane { px: &b, w: W, h: H, stride: W };
        let field = local_field(&pa, &pb, 6, 16);
        let (with, _) = fit_affine(&field, 0.35).expect("with rejection");
        let (without, _) = fit_affine(&field, 0.0).expect("without");
        // residual-based rejection beats keeping everything; SAD-based
        // rejection did NOT, which is why it is not what this does
        assert!((with.tx + 6.0).abs() < (without.tx + 6.0).abs(),
                "rejection helps: {:.2} vs {:.2}", with.tx, without.tx);
    }

    #[test]
    fn a_model_must_beat_translation_by_a_margin_not_merely_tie() {
        // a pure pan should NOT be promoted to affine just because a
        // six-parameter fit can express it slightly better
        let a = scene(W, H);
        let b = warped(&a, W, H, &Warp::translation(5.0, 3.0));
        let pa = Plane { px: &a, w: W, h: H, stride: W };
        let pb = Plane { px: &b, w: W, h: H, stride: W };
        let e = estimate(&pa, &pb);
        assert_eq!(e.model, Model::Translation,
                   "a pan stayed a pan rather than becoming {:?}", e.model);
    }

    #[test]
    fn the_search_is_coarse_to_fine_and_still_lands_on_the_answer() {
        // a full-resolution search over the same range is hundreds of
        // times the work
        let a = scene(W, H);
        for (tx, ty) in [(1.0, 0.0), (12.0, -9.0), (-20.0, 15.0)] {
            let b = warped(&a, W, H, &Warp::translation(tx, ty));
            let pa = Plane { px: &a, w: W, h: H, stride: W };
            let pb = Plane { px: &b, w: W, h: H, stride: W };
            let (fx, fy, _) = estimate_translation(&pa, &pb, 48);
            assert!((fx + tx).abs() < 2.0 && (fy + ty).abs() < 2.0,
                    "({tx},{ty}) found as ({fx},{fy})");
        }
    }
}
