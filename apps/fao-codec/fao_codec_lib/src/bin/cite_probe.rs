//! cite_probe — dedup across a HARD CUT: can the first frame of shot B
//! be supplied from shot A, the same location from another angle?
//!
//! The residual coder pays for what the model does not supply, so the
//! question is bytes, not depth. Three numbers per pair:
//!   1. correspondences between A's last frame and B's first, and how
//!      many a homography explains vs a fundamental matrix — the
//!      excess of F over H is the parallax a 2D warp cannot supply;
//!   2. B's first frame predicted by warping A through the homography:
//!      fraction of regions within tolerance, MS-SSIM of the warp;
//!   3. B encoded with that warp as frame 0's model vs from scratch.
//!
//!   cite_probe <shotA.y4m> <shotB.y4m> [--dz 32]

use fao_codec::residual::ResidualOpts;
use fao_codec::two_view::{fit_homography, Mat3};
use fao_codec::v11_plate::{self as plate, Yuv420};

fn read_y4m(path: &str) -> (usize, usize, Vec<Yuv420>) {
    let data = std::fs::read(path).expect("read y4m");
    let hdr_end = data.iter().position(|&b| b == b'\n').expect("y4m header");
    let hdr = std::str::from_utf8(&data[..hdr_end]).unwrap();
    let (mut w, mut h) = (0usize, 0usize);
    for tok in hdr.split(' ') {
        if let Some(v) = tok.strip_prefix('W') { w = v.parse().unwrap(); }
        else if let Some(v) = tok.strip_prefix('H') { h = v.parse().unwrap(); }
    }
    let (ysz, csz) = (w * h, (w / 2) * (h / 2));
    let mut frames = Vec::new();
    let mut pos = hdr_end + 1;
    while pos + 6 <= data.len() {
        let fe = data[pos..].iter().position(|&b| b == b'\n').unwrap() + pos + 1;
        let mut f = Yuv420::new(w, h);
        f.y.copy_from_slice(&data[fe..fe + ysz]);
        f.u.copy_from_slice(&data[fe + ysz..fe + ysz + csz]);
        f.v.copy_from_slice(&data[fe + ysz + csz..fe + ysz + 2 * csz]);
        frames.push(f);
        pos = fe + ysz + 2 * csz;
    }
    (w, h, frames)
}

/// Corner-ish keypoints: best Harris response per cell of a grid.
fn keypoints(y: &[u8], w: usize, h: usize, cell: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut cy = 8;
    while cy + cell + 8 <= h {
        let mut cx = 8;
        while cx + cell + 8 <= w {
            let mut best = (0.0f32, 0usize, 0usize);
            for py in cy..cy + cell {
                for px in cx..cx + cell {
                    let (mut sxx, mut syy, mut sxy) = (0.0f32, 0.0f32, 0.0f32);
                    for dy in -2i32..=2 {
                        for dx in -2i32..=2 {
                            let i = ((py as i32 + dy) as usize) * w + (px as i32 + dx) as usize;
                            let gx = y[i + 1] as f32 - y[i - 1] as f32;
                            let gy = y[i + w] as f32 - y[i - w] as f32;
                            sxx += gx * gx;
                            syy += gy * gy;
                            sxy += gx * gy;
                        }
                    }
                    let det = sxx * syy - sxy * sxy;
                    let tr = sxx + syy;
                    let r = det - 0.04 * tr * tr;
                    if r > best.0 {
                        best = (r, px, py);
                    }
                }
            }
            if best.0 > 1.0e5 {
                out.push((best.1, best.2));
            }
            cx += cell;
        }
        cy += cell;
    }
    out
}

const P: usize = 7; // patch half-size -> 15x15

fn patch(y: &[u8], w: usize, x: usize, py: usize) -> Vec<f32> {
    let mut v = Vec::with_capacity((2 * P + 1) * (2 * P + 1));
    for dy in 0..=2 * P {
        for dx in 0..=2 * P {
            v.push(y[(py + dy - P) * w + x + dx - P] as f32);
        }
    }
    let m = v.iter().sum::<f32>() / v.len() as f32;
    let mut n = 0.0f32;
    for e in v.iter_mut() {
        *e -= m;
        n += *e * *e;
    }
    let n = n.sqrt().max(1e-3);
    for e in v.iter_mut() {
        *e /= n;
    }
    v
}

fn half(y: &[u8], w: usize, h: usize) -> (Vec<u8>, usize, usize) {
    let (hw, hh) = (w / 2, h / 2);
    let mut o = vec![0u8; hw * hh];
    for yy in 0..hh {
        for xx in 0..hw {
            let i = (yy * 2) * w + xx * 2;
            o[yy * hw + xx] = ((y[i] as u32 + y[i + 1] as u32 + y[i + w] as u32 + y[i + w + 1] as u32 + 2) / 4) as u8;
        }
    }
    (o, hw, hh)
}

/// NCC matching at HALF resolution (a 15x15 patch there is 31x31 of
/// context here), ratio test within a spatial radius, mutual best.
/// Wide-baseline tolerant enough for two angles on one location.
fn matches(a: &Yuv420, b: &Yuv420, cell: usize) -> Vec<(f32, f32, f32, f32)> {
    let (ay, w, h) = half(&a.y, a.w, a.h);
    let (by, _, _) = half(&b.y, b.w, b.h);
    let ka = keypoints(&ay, w, h, cell);
    let kb = keypoints(&by, w, h, cell);
    let pa: Vec<Vec<f32>> = ka.iter().map(|&(x, y)| patch(&ay, w, x, y)).collect();
    let pb: Vec<Vec<f32>> = kb.iter().map(|&(x, y)| patch(&by, w, x, y)).collect();
    let radius2 = (w as f32 * 0.25).powi(2);
    let best_of = |q: &Vec<f32>, (qx, qy): (usize, usize), set: &[Vec<f32>], pts: &[(usize, usize)]| -> (usize, f32, f32) {
        let (mut b1, mut s1, mut s2) = (usize::MAX, -2.0f32, -2.0f32);
        for (j, c) in set.iter().enumerate() {
            let d2 = (pts[j].0 as f32 - qx as f32).powi(2) + (pts[j].1 as f32 - qy as f32).powi(2);
            if d2 > radius2 {
                continue;
            }
            let s: f32 = q.iter().zip(c).map(|(u, v)| u * v).sum();
            if s > s1 {
                s2 = s1;
                s1 = s;
                b1 = j;
            } else if s > s2 {
                s2 = s;
            }
        }
        (b1, s1, s2)
    };
    let mut out = Vec::new();
    for (i, q) in pa.iter().enumerate() {
        let (j, s1, s2) = best_of(q, ka[i], &pb, &kb);
        if j == usize::MAX || s1 < 0.55 || s2 > 0.93 * s1 {
            continue;
        }
        let (i2, _, _) = best_of(&pb[j], kb[j], &pa, &ka);
        if i2 != i {
            continue;
        }
        out.push((ka[i].0 as f32 * 2.0, ka[i].1 as f32 * 2.0, kb[j].0 as f32 * 2.0, kb[j].1 as f32 * 2.0));
    }
    eprintln!("keypoints A {} B {} (half-res) -> mutual NCC matches {}", ka.len(), kb.len(), out.len());
    out
}

fn h_apply(h: &Mat3, x: f32, y: f32) -> (f32, f32) {
    let z = h[6] * x + h[7] * y + h[8];
    ((h[0] * x + h[1] * y + h[2]) / z, (h[3] * x + h[4] * y + h[5]) / z)
}

fn ransac_h(pairs: &[(f32, f32, f32, f32)], tol: f32, iters: usize) -> Option<(Mat3, Vec<bool>)> {
    if pairs.len() < 8 {
        return None;
    }
    let mut seed = 0x9E3779B97F4A7C15u64;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut best: Option<(usize, Vec<bool>)> = None;
    let k = 12.min(pairs.len());
    let mut tried = 0usize;
    for it in 0..=iters {
        // hypothesis 0: the whole set through the fitter's own
        // median rejection; then random 12-subsets
        let sub: Vec<(f32, f32, f32, f32)> = if it == 0 {
            pairs.to_vec()
        } else {
            let mut sub = Vec::with_capacity(k);
            while sub.len() < k {
                let p = pairs[(rnd() % pairs.len() as u64) as usize];
                if !sub.contains(&p) {
                    sub.push(p);
                }
            }
            sub
        };
        let Some(h) = fit_homography(&sub) else { continue };
        tried += 1;
        let inl: Vec<bool> = pairs
            .iter()
            .map(|p| {
                let (x, y) = h_apply(&h, p.0, p.1);
                ((x - p.2).powi(2) + (y - p.3).powi(2)).sqrt() < tol
            })
            .collect();
        let n = inl.iter().filter(|&&b| b).count();
        if best.as_ref().map(|b| n > b.0).unwrap_or(true) {
            best = Some((n, inl));
        }
    }
    eprintln!("ransac_h: {tried} hypotheses fitted");
    let (_, inl) = best?;
    let sub: Vec<_> = pairs.iter().zip(&inl).filter(|(_, &b)| b).map(|(p, _)| *p).collect();
    let h = fit_homography(&sub).or_else(|| fit_homography(pairs))?;
    let inl: Vec<bool> = pairs
        .iter()
        .map(|p| {
            let (x, y) = h_apply(&h, p.0, p.1);
            ((x - p.2).powi(2) + (y - p.3).powi(2)).sqrt() < tol
        })
        .collect();
    Some((h, inl))
}

/// Jacobi eigen-decomposition of a small symmetric matrix; returns the
/// eigenvector of the smallest eigenvalue.
fn smallest_eigvec(mut a: Vec<Vec<f64>>) -> Vec<f64> {
    let n = a.len();
    let mut v: Vec<Vec<f64>> = (0..n).map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect()).collect();
    for _ in 0..100 {
        let (mut p, mut q, mut m) = (0, 1, 0.0);
        for i in 0..n {
            for j in i + 1..n {
                if a[i][j].abs() > m {
                    m = a[i][j].abs();
                    p = i;
                    q = j;
                }
            }
        }
        if m < 1e-12 {
            break;
        }
        let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
        let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
        let c = 1.0 / (t * t + 1.0).sqrt();
        let s = t * c;
        for k in 0..n {
            let (akp, akq) = (a[k][p], a[k][q]);
            a[k][p] = c * akp - s * akq;
            a[k][q] = s * akp + c * akq;
        }
        for k in 0..n {
            let (apk, aqk) = (a[p][k], a[q][k]);
            a[p][k] = c * apk - s * aqk;
            a[q][k] = s * apk + c * aqk;
        }
        for k in 0..n {
            let (vkp, vkq) = (v[k][p], v[k][q]);
            v[k][p] = c * vkp - s * vkq;
            v[k][q] = s * vkp + c * vkq;
        }
    }
    let mut mi = 0;
    for i in 1..n {
        if a[i][i] < a[mi][mi] {
            mi = i;
        }
    }
    (0..n).map(|k| v[k][mi]).collect()
}

/// Normalized 8-point fundamental matrix from >= 8 pairs.
fn fit_f(pairs: &[(f32, f32, f32, f32)]) -> Option<[f64; 9]> {
    if pairs.len() < 8 {
        return None;
    }
    let n = pairs.len() as f64;
    let norm = |sel: &dyn Fn(&(f32, f32, f32, f32)) -> (f64, f64)| {
        let (mut cx, mut cy) = (0.0, 0.0);
        for p in pairs {
            let (x, y) = sel(p);
            cx += x;
            cy += y;
        }
        cx /= n;
        cy /= n;
        let mut md = 0.0;
        for p in pairs {
            let (x, y) = sel(p);
            md += ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
        }
        let s = std::f64::consts::SQRT_2 / (md / n).max(1e-9);
        (cx, cy, s)
    };
    let (cx0, cy0, s0) = norm(&|p| (p.0 as f64, p.1 as f64));
    let (cx1, cy1, s1) = norm(&|p| (p.2 as f64, p.3 as f64));
    let mut ata = vec![vec![0.0f64; 9]; 9];
    for p in pairs {
        let (x, y) = ((p.0 as f64 - cx0) * s0, (p.1 as f64 - cy0) * s0);
        let (u, v) = ((p.2 as f64 - cx1) * s1, (p.3 as f64 - cy1) * s1);
        let row = [u * x, u * y, u, v * x, v * y, v, x, y, 1.0];
        for i in 0..9 {
            for j in 0..9 {
                ata[i][j] += row[i] * row[j];
            }
        }
    }
    let f = smallest_eigvec(ata);
    // denormalize: F = T1^T Fn T0
    let t0 = [s0, 0.0, -s0 * cx0, 0.0, s0, -s0 * cy0, 0.0, 0.0, 1.0];
    let t1 = [s1, 0.0, -s1 * cx1, 0.0, s1, -s1 * cy1, 0.0, 0.0, 1.0];
    let mul = |a: &[f64; 9], b: &[f64; 9]| -> [f64; 9] {
        let mut o = [0.0; 9];
        for i in 0..3 {
            for j in 0..3 {
                for k in 0..3 {
                    o[i * 3 + j] += a[i * 3 + k] * b[k * 3 + j];
                }
            }
        }
        o
    };
    let t1t = [t1[0], t1[3], t1[6], t1[1], t1[4], t1[7], t1[2], t1[5], t1[8]];
    let fm: [f64; 9] = f.try_into().ok()?;
    Some(mul(&mul(&t1t, &fm), &t0))
}

fn sampson(f: &[f64; 9], p: &(f32, f32, f32, f32)) -> f64 {
    let (x, y, u, v) = (p.0 as f64, p.1 as f64, p.2 as f64, p.3 as f64);
    let fx = [f[0] * x + f[1] * y + f[2], f[3] * x + f[4] * y + f[5], f[6] * x + f[7] * y + f[8]];
    let ftu = [f[0] * u + f[3] * v + f[6], f[1] * u + f[4] * v + f[7], f[2] * u + f[5] * v + f[8]];
    let e = u * fx[0] + v * fx[1] + fx[2];
    let d = fx[0] * fx[0] + fx[1] * fx[1] + ftu[0] * ftu[0] + ftu[1] * ftu[1];
    (e * e / d.max(1e-12)).sqrt()
}

fn ransac_f(pairs: &[(f32, f32, f32, f32)], tol: f64, iters: usize) -> Option<([f64; 9], Vec<bool>)> {
    if pairs.len() < 8 {
        return None;
    }
    let mut seed = 0x2545F4914F6CDD1Du64;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut best: Option<(usize, [f64; 9])> = None;
    for _ in 0..iters {
        let mut sub: Vec<(f32, f32, f32, f32)> = Vec::with_capacity(8);
        while sub.len() < 8 {
            let p = pairs[(rnd() % pairs.len() as u64) as usize];
            if !sub.contains(&p) {
                sub.push(p);
            }
        }
        let Some(f) = fit_f(&sub) else { continue };
        let n = pairs.iter().filter(|p| sampson(&f, p) < tol).count();
        if best.as_ref().map(|b| n > b.0).unwrap_or(true) {
            best = Some((n, f));
        }
    }
    let (_, f0) = best?;
    let sub: Vec<_> = pairs.iter().filter(|p| sampson(&f0, p) < tol).copied().collect();
    let f = fit_f(&sub)?;
    let inl: Vec<bool> = pairs.iter().map(|p| sampson(&f, p) < tol).collect();
    Some((f, inl))
}

fn warp(a: &Yuv420, h: &Mat3) -> Yuv420 {
    // B pixel (x,y) shows A content at H^-1 (x,y): invert H
    let m = h;
    let det = m[0] * (m[4] * m[8] - m[5] * m[7]) - m[1] * (m[3] * m[8] - m[5] * m[6]) + m[2] * (m[3] * m[7] - m[4] * m[6]);
    let inv: Mat3 = [
        (m[4] * m[8] - m[5] * m[7]) / det,
        (m[2] * m[7] - m[1] * m[8]) / det,
        (m[1] * m[5] - m[2] * m[4]) / det,
        (m[5] * m[6] - m[3] * m[8]) / det,
        (m[0] * m[8] - m[2] * m[6]) / det,
        (m[2] * m[3] - m[0] * m[5]) / det,
        (m[3] * m[7] - m[4] * m[6]) / det,
        (m[1] * m[6] - m[0] * m[7]) / det,
        (m[0] * m[4] - m[1] * m[3]) / det,
    ];
    let (w, hh) = (a.w, a.h);
    let mut o = Yuv420::new(w, hh);
    let bil = |p: &[u8], pw: usize, ph: usize, x: f32, y: f32| -> u8 {
        if x < 0.0 || y < 0.0 || x > (pw - 1) as f32 || y > (ph - 1) as f32 {
            return 128;
        }
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(pw - 1), (y0 + 1).min(ph - 1));
        let (tx, ty) = (x - x0 as f32, y - y0 as f32);
        let top = p[y0 * pw + x0] as f32 * (1.0 - tx) + p[y0 * pw + x1] as f32 * tx;
        let bot = p[y1 * pw + x0] as f32 * (1.0 - tx) + p[y1 * pw + x1] as f32 * tx;
        (top * (1.0 - ty) + bot * ty).round().clamp(0.0, 255.0) as u8
    };
    for y in 0..hh {
        for x in 0..w {
            let (sx, sy) = h_apply(&inv, x as f32, y as f32);
            o.y[y * w + x] = bil(&a.y, w, hh, sx, sy);
        }
    }
    let (cw, ch) = (w / 2, hh / 2);
    for y in 0..ch {
        for x in 0..cw {
            let (sx, sy) = h_apply(&inv, (x * 2) as f32, (y * 2) as f32);
            o.u[y * cw + x] = bil(&a.u, cw, ch, sx / 2.0, sy / 2.0);
            o.v[y * cw + x] = bil(&a.v, cw, ch, sx / 2.0, sy / 2.0);
        }
    }
    o
}

fn region_stats(pred: &Yuv420, f: &Yuv420) -> (f64, f64) {
    let (w, h) = (f.w, f.h);
    let (gw, gh) = (w / 16, h / 16);
    let (mut within2, mut within8) = (0usize, 0usize);
    for gy in 0..gh {
        for gx in 0..gw {
            let mut sad = 0u32;
            for dy in 0..16 {
                for dx in 0..16 {
                    let i = (gy * 16 + dy) * w + gx * 16 + dx;
                    sad += (pred.y[i] as i32 - f.y[i] as i32).unsigned_abs();
                }
            }
            if sad <= 2 * 256 { within2 += 1; }
            if sad <= 8 * 256 { within8 += 1; }
        }
    }
    let n = (gw * gh) as f64;
    (within2 as f64 / n, within8 as f64 / n)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (w, h, fa) = read_y4m(&args[0]);
    let (_, _, fb) = read_y4m(&args[1]);
    let dz: f32 = args.iter().position(|a| a == "--dz").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(32.0);
    // A's last non-flash frame, B's first non-flash frame
    let is_flash = |f: &Yuv420| { let m = f.y.iter().map(|&v| v as u64).sum::<u64>() / f.y.len() as u64; m > 235 || m < 8 };
    let a = fa.iter().rev().find(|f| !is_flash(f)).expect("A frame");
    let b0i = fb.iter().position(|f| !is_flash(f)).expect("B frame");
    let fb = &fb[b0i..];
    let b = &fb[0];
    println!("A {}x{} ({} frames), B from frame {} ({} frames)", w, h, fa.len(), b0i, fb.len());

    let pairs = matches(a, b, 20);
    let Some((hm, hin)) = ransac_h(&pairs, 3.0, 800) else { println!("no homography"); return };
    let nh = hin.iter().filter(|&&x| x).count();
    let (fm, fin) = ransac_f(&pairs, 2.0, 800).unwrap_or(([0.0; 9], vec![false; pairs.len()]));
    let nf = fin.iter().filter(|&&x| x).count();
    // parallax: how far H is off on the F-inliers that H rejects
    let mut off: Vec<f32> = pairs
        .iter()
        .zip(hin.iter().zip(&fin))
        .filter(|(_, (&hi, &fi))| fi && !hi)
        .map(|(p, _)| { let (x, y) = h_apply(&hm, p.0, p.1); ((x - p.2).powi(2) + (y - p.3).powi(2)).sqrt() })
        .collect();
    off.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!(
        "matches {}  homography inliers (3px) {}  fundamental inliers (2px Sampson) {}  F-only {}  median H error on F-only {:.1}px",
        pairs.len(), nh, nf, nf.saturating_sub(nh), off.get(off.len() / 2).copied().unwrap_or(0.0)
    );
    let _ = fm;

    let pred = warp(a, &hm);
    let (w2, w8) = region_stats(&pred, b);
    let ms = fao_codec::msssim::recon_score(&[pred.clone()], &[b.clone()]);
    println!("A warped by H -> B frame 0: regions within 2 lvl {:.1}%  within 8 lvl {:.1}%  MS-SSIM {:.4}", w2 * 100.0, w8 * 100.0, ms);
    let (u2, u8_) = region_stats(a, b);
    println!("A unwarped      -> B frame 0: regions within 2 lvl {:.1}%  within 8 lvl {:.1}%", u2 * 100.0, u8_ * 100.0);

    // Bytes. Same core, same GMC for frames > 0; only frame 0's model differs.
    std::env::set_var("FAO_GMC", "1");
    let opts = ResidualOpts { dead_zone: dz, ..ResidualOpts::default() };
    let gray = { let mut g = Yuv420::new(w, h); for v in g.y.iter_mut() { *v = 128; } for v in g.u.iter_mut().chain(g.v.iter_mut()) { *v = 128; } g };
    let bytes = |r: &plate::PlateEncodeReportV2| -> (usize, usize, usize) {
        let f0 = r.frames[0].mode_bytes + r.frames[0].residual_bytes;
        (r.plate_bytes + r.motion_bytes + r.frames.iter().map(|c| c.mode_bytes + c.residual_bytes).sum::<usize>(), f0, r.plate_bytes)
    };
    let (rs, _) = plate::encode_gop_v3_core(fb, &mut |_| gray.clone(), 0, 0, &opts, None);
    let (t, f0, _) = bytes(&rs);
    println!("B from scratch (intra frame 0, GMC after):   {t:>8} B  frame0 {f0:>7} B  MS-SSIM {:.5}", fao_codec::msssim::recon_score(&rs.recon, fb));
    let (rc, _) = plate::encode_gop_v3_core(fb, &mut |_| pred.clone(), 24, 36, &opts, None);
    let (t, f0, _) = bytes(&rc);
    println!("B citing A through H (24 B ref + 36 B H):    {t:>8} B  frame0 {f0:>7} B  MS-SSIM {:.5}", fao_codec::msssim::recon_score(&rc.recon, fb));
}
