//! "AI half-rate": synthesise a skipped non-reference frame from the two
//! decoded frames around it. The CPU reference -- pure, no dependencies.
//!
//! The mode this serves: with non-reference frames skipped (AV1 decode CPU
//! -41..46%, measured), every other frame is never decoded; this predicts
//! it from its neighbours, which is exactly where the reference graph puts
//! a non-reference frame -- between the frames it was predicted from.
//!
//! Pipeline (trained and verified by apps/vidkit-player/tools/aiframes;
//! weights in [`crate::aiframes_weights`]):
//! 1. symmetric Lucas-Kanade flow on a 1/16, 1/8, 1/4 pyramid (closed form);
//! 2. a small dilated CNN at 1/4 refines it and gives a blend mask;
//! 3. full size: warp both neighbours to the middle, blend.
//!
//! This is the SPECIFICATION the GPU shader is tested against, written to
//! PyTorch's exact conventions (bilinear with align_corners=False, border-
//! clamped grid_sample, zero-padded convolutions, floor pooling) and itself
//! tested against PyTorch's output on a committed fixture. Too slow for
//! real time on a CPU (~160 multiply-adds a pixel for the CNN alone); the
//! player runs the shader.

use crate::aiframes_weights as wts;

/// A single-channel float plane, row-major.
#[derive(Clone, Debug, PartialEq)]
pub struct Plane {
    pub w: usize,
    pub h: usize,
    pub px: Vec<f32>,
}

impl Plane {
    pub fn new(w: usize, h: usize) -> Self {
        Self { w, h, px: vec![0.0; w * h] }
    }
    pub fn from_u8(w: usize, h: usize, d: &[u8]) -> Self {
        Self { w, h, px: d[..w * h].iter().map(|&v| v as f32 / 255.0).collect() }
    }
    #[inline]
    pub fn at(&self, x: usize, y: usize) -> f32 {
        self.px[y * self.w + x]
    }
    /// Clamped integer fetch (replicate padding).
    #[inline]
    fn at_clamped(&self, x: isize, y: isize) -> f32 {
        let x = x.clamp(0, self.w as isize - 1) as usize;
        let y = y.clamp(0, self.h as isize - 1) as usize;
        self.at(x, y)
    }
    /// Bilinear sample at a pixel-centre coordinate, border clamped:
    /// grid_sample(padding_mode="border", align_corners=False).
    #[inline]
    pub fn sample(&self, x: f32, y: f32) -> f32 {
        let x = x.clamp(0.0, (self.w - 1) as f32);
        let y = y.clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let top = self.at(x0, y0) * (1.0 - fx) + self.at(x1, y0) * fx;
        let bot = self.at(x0, y1) * (1.0 - fx) + self.at(x1, y1) * fx;
        top * (1.0 - fy) + bot * fy
    }
}

/// avg_pool2d(k), floor size.
pub fn pool(p: &Plane, k: usize) -> Plane {
    let (w, h) = (p.w / k, p.h / k);
    let mut o = Plane::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for dy in 0..k {
                for dx in 0..k {
                    s += p.at(x * k + dx, y * k + dy);
                }
            }
            o.px[y * w + x] = s / (k * k) as f32;
        }
    }
    o
}

/// F.interpolate(bilinear, align_corners=False) of a multi-channel field.
pub fn upsample(c: &[Plane], w: usize, h: usize) -> Vec<Plane> {
    c.iter()
        .map(|p| {
            let (sx, sy) = (p.w as f32 / w as f32, p.h as f32 / h as f32);
            let mut o = Plane::new(w, h);
            for y in 0..h {
                let fy = ((y as f32 + 0.5) * sy - 0.5).max(0.0);
                for x in 0..w {
                    let fx = ((x as f32 + 0.5) * sx - 0.5).max(0.0);
                    o.px[y * w + x] = p.sample(fx, fy);
                }
            }
            o
        })
        .collect()
}

/// Backward warp: out(x) = img(x + flow(x)).
fn warp(img: &Plane, fx: &Plane, fy: &Plane) -> Plane {
    let mut o = Plane::new(img.w, img.h);
    for y in 0..img.h {
        for x in 0..img.w {
            let i = y * img.w + x;
            o.px[i] = img.sample(x as f32 + fx.px[i], y as f32 + fy.px[i]);
        }
    }
    o
}

/// One symmetric Lucas-Kanade step; returns (dx, dy).
fn lk_step(wa: &Plane, wc: &Plane) -> (Plane, Plane) {
    let (w, h) = (wa.w, wa.h);
    let mut prod = vec![Plane::new(w, h), Plane::new(w, h), Plane::new(w, h), Plane::new(w, h), Plane::new(w, h)];
    let s = |x: isize, y: isize| wa.at_clamped(x, y) + wc.at_clamped(x, y);
    for y in 0..h as isize {
        for x in 0..w as isize {
            let gx = 0.5 * (s(x + 1, y) - s(x - 1, y));
            let gy = 0.5 * (s(x, y + 1) - s(x, y - 1));
            let i = y as usize * w + x as usize;
            let it = wc.px[i] - wa.px[i];
            for (k, v) in [gx * gx, gx * gy, gy * gy, gx * it, gy * it].into_iter().enumerate() {
                prod[k].px[i] = v;
            }
        }
    }
    let r = (wts::LK_WIN / 2) as isize;
    let n = (wts::LK_WIN * wts::LK_WIN) as f32;
    let (mut dx, mut dy) = (Plane::new(w, h), Plane::new(w, h));
    for y in 0..h as isize {
        for x in 0..w as isize {
            let mut a = [0.0f32; 5];
            for oy in -r..=r {
                for ox in -r..=r {
                    for (k, ak) in a.iter_mut().enumerate() {
                        *ak += prod[k].at_clamped(x + ox, y + oy);
                    }
                }
            }
            let [a11, a12, a22, b1, b2] = a.map(|v| v / n);
            let tr = a11 + a22;
            let det = a11 * a22 - a12 * a12 + 1e-3 * tr * tr + 1e-12;
            let i = y as usize * w + x as usize;
            dx.px[i] = ((a22 * b1 - a12 * b2) / det).clamp(-2.0, 2.0);
            dy.px[i] = ((a11 * b2 - a12 * b1) / det).clamp(-2.0, 2.0);
        }
    }
    (dx, dy)
}

/// Flow to prev (0,1) and to next (2,3) at 1/4 scale, in pixels there.
pub fn lk_flow(y0: &Plane, y2: &Plane) -> Vec<Plane> {
    let mut flow: Option<Vec<Plane>> = None;
    for &lv in wts::LEVELS.iter() {
        let (a, c) = (pool(y0, lv), pool(y2, lv));
        let mut f = match flow {
            None => (0..4).map(|_| Plane::new(a.w, a.h)).collect(),
            Some(f) => upsample(&f, a.w, a.h).into_iter().map(|mut p| {
                p.px.iter_mut().for_each(|v| *v *= 2.0);
                p
            }).collect::<Vec<_>>(),
        };
        for _ in 0..wts::LK_ITERS {
            let (dx, dy) = lk_step(&warp(&a, &f[0], &f[1]), &warp(&c, &f[2], &f[3]));
            for i in 0..a.w * a.h {
                f[0].px[i] += dx.px[i];
                f[1].px[i] += dy.px[i];
                f[2].px[i] -= dx.px[i];
                f[3].px[i] -= dy.px[i];
            }
        }
        flow = Some(f);
    }
    flow.expect("levels")
}

/// Conv2d, 3x3, zero padding = dilation.
fn conv(x: &[Plane], cin: usize, cout: usize, dil: usize, wt: &[f32], b: &[f32]) -> Vec<Plane> {
    let (w, h) = (x[0].w, x[0].h);
    let mut out: Vec<Plane> = (0..cout).map(|o| Plane { w, h, px: vec![b[o]; w * h] }).collect();
    for (o, op) in out.iter_mut().enumerate() {
        for i in 0..cin {
            for ky in 0..3 {
                for kx in 0..3 {
                    let k = wt[((o * cin + i) * 3 + ky) * 3 + kx];
                    let (oy, ox) = ((ky as isize - 1) * dil as isize, (kx as isize - 1) * dil as isize);
                    for y in 0..h as isize {
                        let sy = y + oy;
                        if sy < 0 || sy >= h as isize {
                            continue;
                        }
                        for xx in 0..w as isize {
                            let sx = xx + ox;
                            if sx < 0 || sx >= w as isize {
                                continue;
                            }
                            op.px[y as usize * w + xx as usize] += k * x[i].px[sy as usize * w + sx as usize];
                        }
                    }
                }
            }
        }
    }
    out
}

/// (flow to prev x, y, to next x, y, mask logit) at 1/4.
pub fn model_flow(y0: &Plane, y2: &Plane) -> Vec<Plane> {
    let fl = lk_flow(y0, y2);
    let (a, c) = (pool(y0, 4), pool(y2, 4));
    let (wa, wc) = (warp(&a, &fl[0], &fl[1]), warp(&c, &fl[2], &fl[3]));
    let diff = Plane { w: wa.w, h: wa.h, px: wa.px.iter().zip(&wc.px).map(|(p, q)| p - q).collect() };
    let scaled = |p: &Plane| Plane { w: p.w, h: p.h, px: p.px.iter().map(|v| v * 0.25).collect() };
    let mut x = vec![wa, wc, diff, scaled(&fl[0]), scaled(&fl[1]), scaled(&fl[2]), scaled(&fl[3])];
    let last = wts::LAYERS.len() - 1;
    for (li, &(cin, cout, dil, wt, b)) in wts::LAYERS.iter().enumerate() {
        x = conv(&x, cin, cout, dil, wt, b);
        if li != last {
            for p in x.iter_mut() {
                p.px.iter_mut().for_each(|v| *v = if *v > 0.0 { *v } else { 0.1 * *v });
            }
        }
    }
    let mut out = fl;
    for (o, r) in out.iter_mut().zip(&x[..4]) {
        o.px.iter_mut().zip(&r.px).for_each(|(v, d)| *v += d);
    }
    out.push(x[4].clone());
    out
}

/// The synthesised middle of `p0`/`p2` (any plane size: luma or chroma).
pub fn synth(out: &[Plane], p0: &Plane, p2: &Plane) -> Plane {
    let up = upsample(out, p0.w, p0.h);
    let scale = p0.w as f32 / out[0].w as f32;
    let mut o = Plane::new(p0.w, p0.h);
    for y in 0..p0.h {
        for x in 0..p0.w {
            let i = y * p0.w + x;
            let m = 1.0 / (1.0 + (-up[4].px[i]).exp());
            let a = p0.sample(x as f32 + up[0].px[i] * scale, y as f32 + up[1].px[i] * scale);
            let c = p2.sample(x as f32 + up[2].px[i] * scale, y as f32 + up[3].px[i] * scale);
            o.px[i] = m * a + (1.0 - m) * c;
        }
    }
    o
}

/// Mean |warped prev - warped next| at 1/4 after the final flow: the
/// shader's scene-cut test (above [`CUT_RESIDUAL`], fall back to a blend).
pub fn residual(y0: &Plane, y2: &Plane, out: &[Plane]) -> f32 {
    let (a, c) = (pool(y0, 4), pool(y2, 4));
    let (wa, wc) = (warp(&a, &out[0], &out[1]), warp(&c, &out[2], &out[3]));
    wa.px.iter().zip(&wc.px).map(|(p, q)| (p - q).abs()).sum::<f32>() / wa.px.len() as f32
}

/// Above this mean residual the neighbours did not match after motion
/// compensation: a scene cut; show the blend. Measured on the training
/// clip: both real cuts above it, none of 175 moving triplets.
pub const CUT_RESIDUAL: f32 = 0.02;

pub fn psnr(a: &Plane, b: &Plane) -> f64 {
    let mse = a.px.iter().zip(&b.px).map(|(p, q)| ((p - q) as f64).powi(2)).sum::<f64>() / a.px.len() as f64;
    10.0 * (1.0 / mse.max(1e-12)).log10()
}

/// The committed parity fixture: (w, h, prev, real middle, next, PyTorch's
/// synthesised luma).
pub fn fixture() -> (usize, usize, Vec<u8>, Vec<u8>, Vec<u8>, Vec<f32>) {
    let b: &[u8] = include_bytes!("../testdata/aiframes_fixture.bin");
    let w = u32::from_le_bytes(b[0..4].try_into().unwrap()) as usize;
    let h = u32::from_le_bytes(b[4..8].try_into().unwrap()) as usize;
    let n = w * h;
    let (p, m, q) = (b[8..8 + n].to_vec(), b[8 + n..8 + 2 * n].to_vec(), b[8 + 2 * n..8 + 3 * n].to_vec());
    let s = b[8 + 3 * n..].chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect();
    (w, h, p, m, q, s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Rust reference reproduces PyTorch's synthesised frame on the
    /// committed fixture (a held-out, high-motion crop).
    #[test]
    fn the_reference_matches_pytorch() {
        let (w, h, p, m, q, want) = fixture();
        let (y0, y1, y2) = (Plane::from_u8(w, h, &p), Plane::from_u8(w, h, &m), Plane::from_u8(w, h, &q));
        let out = model_flow(&y0, &y2);
        let got = synth(&out, &y0, &y2);
        let py = Plane { w, h, px: want };
        let agree = psnr(&got, &py);
        let max = got.px.iter().zip(&py.px).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(agree > 60.0, "Rust vs PyTorch: {agree:.1} dB, max |diff| {max}");
        // And it is the model we measured: better than the blend here.
        let blend = Plane { w, h, px: y0.px.iter().zip(&y2.px).map(|(a, b)| (a + b) / 2.0).collect() };
        let (ms, bs) = (psnr(&got, &y1), psnr(&blend, &y1));
        assert!(ms > bs + 1.0, "model {ms:.2} dB vs blend {bs:.2} dB on the fixture");
        eprintln!("DIAG fixture {w}x{h}: Rust vs PyTorch {agree:.1} dB (max diff {max:.2e}); vs the real frame: model {ms:.2} dB, blend {bs:.2} dB");
    }

    /// A known pan is recovered by the analytic stage alone.
    #[test]
    fn a_known_pan_is_recovered() {
        let (w, h, p, ..) = fixture();
        let src = Plane::from_u8(w, h, &p);
        let (vx, vy) = (6.5f32, -3.25f32);
        let shifted = |s: f32| {
            let mut o = Plane::new(w, h);
            for y in 0..h {
                for x in 0..w {
                    o.px[y * w + x] = src.sample(x as f32 + s * vx, y as f32 + s * vy);
                }
            }
            o
        };
        let fl = lk_flow(&shifted(1.0), &shifted(-1.0));
        // Interior median, in full-size pixels (flow is at 1/4).
        let med = |p: &Plane| {
            let mut v: Vec<f32> = (8..p.h - 8).flat_map(|y| (8..p.w - 8).map(move |x| (x, y))).map(|(x, y)| p.at(x, y) * 4.0).collect();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            v[v.len() / 2]
        };
        let (fx, fy) = (med(&fl[0]), med(&fl[1]));
        assert!((fx + vx).abs() < 0.5 && (fy + vy).abs() < 0.5, "flow to prev ({fx:.2},{fy:.2}), want ({},{})", -vx, -vy);
    }
}
