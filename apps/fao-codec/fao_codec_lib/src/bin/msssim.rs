//! msssim — multi-scale SSIM, to adjudicate a question single-scale
//! metrics cannot.
//!
//! The deblocking work stalled on a real methodological problem: PSNR
//! and SSIM systematically reward NOT filtering (any filter moves pixels
//! away from the source, even when it removes an artifact a viewer
//! objects to), while `blockiness` measures one artifact and is gamed by
//! over-smoothing. Three strength rules produced three different winners
//! depending on which of those you weighted, and libvmaf is not compiled
//! into this ffmpeg.
//!
//! MS-SSIM (Wang, Simoncelli & Bovik, 2003) is the standard, PUBLISHED
//! answer to part of that. It evaluates structural similarity across a
//! pyramid of scales, so an error that survives downsampling — which is
//! what a blocking step on a 16-pixel grid does — is penalised at
//! several scales, while high-frequency noise that a single-scale SSIM
//! fixates on is discounted at coarser ones.
//!
//! Chosen deliberately because it is not mine. Inventing a metric to
//! settle an argument about my own filter would be circular; the point
//! of reaching for a published one with published constants and
//! published weights is that it cannot have been shaped to the answer.
//!
//! Honest limits, stated because MS-SSIM is often over-claimed:
//!   * it is still a signal-fidelity metric, not a viewer. It is better
//!     correlated with subjective scores than PSNR or SSIM on
//!     compression artifacts, not equivalent to them;
//!   * it says nothing about temporal artifacts — flicker, pumping,
//!     mosquito noise in motion — and this codec's per-frame mode
//!     switching is exactly the kind of thing that produces them;
//!   * luma only here, which is standard for codec comparison but blind
//!     to chroma bleeding.
//!
//! Usage: msssim <reference.y4m> <distorted.y4m> [more_distorted.y4m ...]

const C1: f64 = (0.01 * 255.0) * (0.01 * 255.0);
const C2: f64 = (0.03 * 255.0) * (0.03 * 255.0);
/// The five published scale weights. Not tuned here.
const WEIGHTS: [f64; 5] = [0.0448, 0.2856, 0.3001, 0.2363, 0.1333];

fn read_y4m(path: &str) -> (usize, usize, Vec<Vec<f64>>) {
    let data = std::fs::read(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let hdr_end = data.iter().position(|&b| b == b'\n').expect("y4m header");
    let hdr = String::from_utf8_lossy(&data[..hdr_end]).to_string();
    let (mut w, mut h) = (0usize, 0usize);
    for tok in hdr.split_whitespace() {
        match tok.as_bytes().first() {
            Some(b'W') => w = tok[1..].parse().unwrap_or(0),
            Some(b'H') => h = tok[1..].parse().unwrap_or(0),
            _ => {}
        }
    }
    let fsz = w * h * 3 / 2;
    let mut off = hdr_end + 1;
    let mut frames = Vec::new();
    while off < data.len() {
        let Some(rel) = data[off..].iter().position(|&b| b == b'\n') else { break };
        off += rel + 1;
        if off + fsz > data.len() {
            break;
        }
        frames.push(data[off..off + w * h].iter().map(|&v| v as f64).collect::<Vec<f64>>());
        off += fsz;
    }
    (w, h, frames)
}

/// Gaussian-weighted 11x11 SSIM statistics over the plane, returning
/// (mean luminance*contrast*structure, mean contrast*structure). The
/// second is what intermediate scales contribute; only the finest-to-
/// coarsest product uses luminance, per the paper.
fn ssim_pass(a: &[f64], b: &[f64], w: usize, h: usize) -> (f64, f64) {
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

fn ms_ssim(a: &[f64], b: &[f64], w: usize, h: usize) -> f64 {
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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: msssim <reference.y4m> <distorted.y4m> [more...]");
        std::process::exit(2);
    }
    let (w, h, refr) = read_y4m(&args[0]);
    println!("{:<24} {:>10}", "file", "MS-SSIM");
    for path in &args[1..] {
        let (dw, dh, dist) = read_y4m(path);
        if dw != w || dh != h {
            eprintln!("{path}: geometry {dw}x{dh} != reference {w}x{h}");
            continue;
        }
        let n = refr.len().min(dist.len());
        let mut acc = 0.0f64;
        for i in 0..n {
            acc += ms_ssim(&refr[i], &dist[i], w, h);
        }
        let name = path.rsplit('/').next().unwrap_or(path);
        println!("{:<24} {:>10.5}", name, acc / n as f64);
    }
}
