//! MS-SSIM as a library function, so the ENCODER can score with it.
//!
//! Lifted out of `bin/msssim` for one reason: `encode_gop_routed` was
//! choosing between the plate and no-plate branches on SSE, and SSE is
//! the PSNR-domain criterion this codec has repeatedly been caught
//! trusting. On bloopers shot 9 the SSE router picks PLATE while
//! MS-SSIM shows no-plate is STRICTLY better — fewer bytes AND higher
//! quality — so the router was inheriting exactly the measurement error
//! the rest of the session spent its time removing.
//!
//! Affordable here because the routing decision is one binary choice
//! per GOP over branches that have already been fully encoded, not an
//! inner-loop cost.

use crate::v11_plate::Yuv420;

const C1: f64 = (0.01 * 255.0) * (0.01 * 255.0);
const C2: f64 = (0.03 * 255.0) * (0.03 * 255.0);
/// The five published scale weights. Not tuned here.
const WEIGHTS: [f64; 5] = [0.0448, 0.2856, 0.3001, 0.2363, 0.1333];

/// Gaussian-weighted 11x11 SSIM statistics over the plane, returning
/// (mean luminance*contrast*structure, mean contrast*structure). The
/// second is what intermediate scales contribute; only the finest-to-
/// coarsest product uses luminance, per the paper.
pub fn ssim_pass(a: &[f64], b: &[f64], w: usize, h: usize) -> (f64, f64) {
    // 11-tap Gaussian, sigma 1.5, normalised — the paper's window.
    let mut g = [0.0f64; 11];
    let sigma = 1.5f64;
    let mut sum = 0.0;
    for (i, v) in g.iter_mut().enumerate() {
        let x = i as f64 - 5.0;
        *v = (-(x * x) / (2.0 * sigma * sigma)).exp();
        sum += *v;
    }
    for v in g.iter_mut() {
        *v /= sum;
    }
    // Separable blur helper.
    let blur = |src: &[f64]| -> Vec<f64> {
        let mut tmp = vec![0.0f64; w * h];
        for y in 0..h {
            for x in 0..w {
                let mut acc = 0.0;
                for (k, gk) in g.iter().enumerate() {
                    let sx = (x as i32 + k as i32 - 5).clamp(0, w as i32 - 1) as usize;
                    acc += gk * src[y * w + sx];
                }
                tmp[y * w + x] = acc;
            }
        }
        let mut out = vec![0.0f64; w * h];
        for y in 0..h {
            for x in 0..w {
                let mut acc = 0.0;
                for (k, gk) in g.iter().enumerate() {
                    let sy = (y as i32 + k as i32 - 5).clamp(0, h as i32 - 1) as usize;
                    acc += gk * tmp[sy * w + x];
                }
                out[y * w + x] = acc;
            }
        }
        out
    };
    let aa: Vec<f64> = a.iter().map(|v| v * v).collect();
    let bb: Vec<f64> = b.iter().map(|v| v * v).collect();
    let ab: Vec<f64> = a.iter().zip(b).map(|(p, q)| p * q).collect();
    let mu_a = blur(a);
    let mu_b = blur(b);
    let s_aa = blur(&aa);
    let s_bb = blur(&bb);
    let s_ab = blur(&ab);
    let (mut full, mut cs, mut n) = (0.0f64, 0.0f64, 0u64);
    for i in 0..w * h {
        let (ma, mb) = (mu_a[i], mu_b[i]);
        let va = (s_aa[i] - ma * ma).max(0.0);
        let vb = (s_bb[i] - mb * mb).max(0.0);
        let cab = s_ab[i] - ma * mb;
        let l = (2.0 * ma * mb + C1) / (ma * ma + mb * mb + C1);
        let c = (2.0 * cab + C2) / (va + vb + C2);
        full += l * c;
        cs += c;
        n += 1;
    }
    (full / n as f64, cs / n as f64)
}

fn downsample(src: &[f64], w: usize, h: usize) -> (Vec<f64>, usize, usize) {
    let (nw, nh) = (w / 2, h / 2);
    let mut out = vec![0.0f64; nw * nh];
    for y in 0..nh {
        for x in 0..nw {
            out[y * nw + x] = (src[(y * 2) * w + x * 2]
                + src[(y * 2) * w + x * 2 + 1]
                + src[(y * 2 + 1) * w + x * 2]
                + src[(y * 2 + 1) * w + x * 2 + 1])
                / 4.0;
        }
    }
    (out, nw, nh)
}

pub fn ms_ssim(a: &[f64], b: &[f64], w: usize, h: usize) -> f64 {
    let (mut ca, mut cb) = (a.to_vec(), b.to_vec());
    let (mut cw, mut ch) = (w, h);
    let mut prod = 1.0f64;
    for (level, weight) in WEIGHTS.iter().enumerate() {
        // The window is 11 wide; below that the statistics are
        // meaningless, so stop and renormalise rather than report a
        // number computed on a 5-pixel image.
        if cw < 11 || ch < 11 {
            break;
        }
        let (full, cs) = ssim_pass(&ca, &cb, cw, ch);
        let term = if level == WEIGHTS.len() - 1 { full } else { cs };
        prod *= term.abs().powf(*weight);
        if level + 1 < WEIGHTS.len() {
            let (na, nw, nh) = downsample(&ca, cw, ch);
            let (nb, _, _) = downsample(&cb, cw, ch);
            ca = na;
            cb = nb;
            cw = nw;
            ch = nh;
        }
    }
    prod
}


/// Mean MS-SSIM of a reconstruction against its source, luma only.
pub fn recon_score(recon: &[Yuv420], source: &[Yuv420]) -> f64 {
    if recon.is_empty() {
        return 0.0;
    }
    let (w, h) = (source[0].w, source[0].h);
    let mut acc = 0.0f64;
    let mut n = 0usize;
    for (r, s) in recon.iter().zip(source) {
        let rf: Vec<f64> = r.y.iter().map(|&v| v as f64).collect();
        let sf: Vec<f64> = s.y.iter().map(|&v| v as f64).collect();
        acc += ms_ssim(&sf, &rf, w, h);
        n += 1;
    }
    acc / n as f64
}
