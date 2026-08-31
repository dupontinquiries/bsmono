//! # Learned compression-artifact restoration (closed-form)
//!
//! The analysis chain (corner detection, tracking, plate building) sees
//! transcoded, upscaled, artifacted pixels — "the noisy input is just as
//! much of a problem as the reconstruction." This module learns a
//! restoration mapping artifacts → clean patterns from example pairs,
//! RAISR-style: pixels are bucketed by local gradient structure
//! (orientation × strength), and each bucket gets its own linear filter
//! solved EXACTLY by least squares over the training pairs. No SGD, no
//! nondeterminism, CPU-cheap — this is the strong classical baseline a
//! neural restorer (candle/wgpu, the beatspace training stack, on the
//! Asahi Vulkan target) has to beat; the residual error it leaves is the
//! measured headroom for that model.
//!
//! Training pairs: for transcoded film, re-compress the available source
//! harder and learn to undo THAT hop (the available source is the best
//! clean reference that exists). For game content the story is better:
//! the engine itself is a ground-truth generator — render clean frames,
//! degrade them with the real codec, and the pair distribution matches
//! deployment exactly. That is the practical meaning of "the input is
//! defined by a known space".

/// Filter geometry: 5×5 taps + bias.
const TAPS: usize = 25;
const UNKNOWNS: usize = TAPS + 1;
/// 4 gradient orientations × 3 strength bands.
const BUCKETS: usize = 12;

pub struct RestoreModel {
    /// [bucket][tap] weights, last entry is the bias.
    pub filters: Vec<[f32; UNKNOWNS]>,
}

fn bucket_of(y: &[u8], w: usize, h: usize, x: usize, yy: usize) -> usize {
    // Sobel-ish central differences on the degraded luma.
    let g = |dx: i32, dy: i32| -> f32 {
        let sx = (x as i32 + dx).clamp(0, w as i32 - 1) as usize;
        let sy = (yy as i32 + dy).clamp(0, h as i32 - 1) as usize;
        y[sy * w + sx] as f32
    };
    let gx = g(1, 0) - g(-1, 0);
    let gy = g(0, 1) - g(0, -1);
    let mag = (gx * gx + gy * gy).sqrt();
    let strength = if mag < 8.0 {
        0
    } else if mag < 32.0 {
        1
    } else {
        2
    };
    // Orientation in 4 bins (0°, 45°, 90°, 135°); flat pixels all share
    // orientation 0 buckets via strength 0 (angle is noise there, but
    // keeping the same bucket arithmetic is simpler than special-casing).
    let ang = gy.atan2(gx); // -π..π
    let ori = ((ang + std::f32::consts::PI) / (std::f32::consts::PI / 4.0)) as usize % 4;
    strength * 4 + ori
}

/// Solve the dense symmetric system A·x = b by Gaussian elimination with
/// partial pivoting. Returns false if singular (empty/degenerate bucket).
fn solve(a: &mut [[f64; UNKNOWNS]; UNKNOWNS], b: &mut [f64; UNKNOWNS]) -> bool {
    for col in 0..UNKNOWNS {
        let mut piv = col;
        for r in col + 1..UNKNOWNS {
            if a[r][col].abs() > a[piv][col].abs() {
                piv = r;
            }
        }
        if a[piv][col].abs() < 1e-9 {
            return false;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        let d = a[col][col];
        for r in col + 1..UNKNOWNS {
            let f = a[r][col] / d;
            if f == 0.0 {
                continue;
            }
            for c in col..UNKNOWNS {
                a[r][c] -= f * a[col][c];
            }
            b[r] -= f * b[col];
        }
    }
    for col in (0..UNKNOWNS).rev() {
        let mut s = b[col];
        for c in col + 1..UNKNOWNS {
            s -= a[col][c] * b[c];
        }
        b[col] = s / a[col][col];
    }
    true
}

/// Train from (degraded, clean) luma frame pairs. Frames must share
/// dimensions. Samples every pixel (stride for speed on big frames).
pub fn train(pairs: &[(&[u8], &[u8])], w: usize, h: usize, stride: usize) -> RestoreModel {
    // Per-bucket normal equations: AtA (26×26) and Atb (26).
    let mut ata = vec![[[0.0f64; UNKNOWNS]; UNKNOWNS]; BUCKETS];
    let mut atb = vec![[0.0f64; UNKNOWNS]; BUCKETS];
    let mut row = [0.0f64; UNKNOWNS];
    for &(deg, clean) in pairs {
        let mut yy = 2;
        while yy < h - 2 {
            let mut x = 2;
            while x < w - 2 {
                let bkt = bucket_of(deg, w, h, x, yy);
                let mut k = 0;
                for dy in -2i32..=2 {
                    for dx in -2i32..=2 {
                        row[k] = deg[(yy as i32 + dy) as usize * w + (x as i32 + dx) as usize] as f64;
                        k += 1;
                    }
                }
                row[TAPS] = 1.0; // bias
                let target = clean[yy * w + x] as f64;
                let (aa, ab) = (&mut ata[bkt], &mut atb[bkt]);
                for i in 0..UNKNOWNS {
                    let ri = row[i];
                    ab[i] += ri * target;
                    for j in i..UNKNOWNS {
                        aa[i][j] += ri * row[j];
                    }
                }
                x += stride;
            }
            yy += stride;
        }
    }
    // Symmetrize, ridge-regularize lightly (keeps rare buckets sane), solve.
    let mut filters = Vec::with_capacity(BUCKETS);
    for bkt in 0..BUCKETS {
        let mut a = ata[bkt];
        let mut b = atb[bkt];
        for i in 0..UNKNOWNS {
            for j in 0..i {
                a[i][j] = a[j][i];
            }
            a[i][i] += 1.0; // ridge
        }
        let mut f = [0.0f32; UNKNOWNS];
        if solve(&mut a, &mut b) {
            for i in 0..UNKNOWNS {
                f[i] = b[i] as f32;
            }
        } else {
            // Degenerate bucket: identity filter (center tap 1).
            f[12] = 1.0;
        }
        filters.push(f);
    }
    RestoreModel { filters }
}

/// Apply to a luma plane.
pub fn apply(model: &RestoreModel, deg: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = deg.to_vec();
    for yy in 2..h - 2 {
        for x in 2..w - 2 {
            let f = &model.filters[bucket_of(deg, w, h, x, yy)];
            let mut acc = f[TAPS];
            let mut k = 0;
            for dy in -2i32..=2 {
                for dx in -2i32..=2 {
                    acc += f[k] * deg[(yy as i32 + dy) as usize * w + (x as i32 + dx) as usize] as f32;
                    k += 1;
                }
            }
            out[yy * w + x] = acc.round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Learn to undo a known blur: degraded = 3×3 box blur of clean.
    /// A 5×5 learned filter can substantially invert this; the test
    /// asserts restoration recovers ≥40% of the lost PSNR-domain error.
    #[test]
    fn learns_to_undo_box_blur() {
        let (w, h) = (96, 96);
        let mk = |seed: u32| -> Vec<u8> {
            (0..w * h)
                .map(|i| {
                    let (x, y) = ((i % w) as f32, (i / w) as f32);
                    let s = seed as f32;
                    (128.0
                        + 55.0 * ((x * 0.31 + s) .sin() * (y * 0.27 + s * 0.7).sin())
                        + 30.0 * ((x + y) * 0.12 + s).sin())
                    .round()
                    .clamp(0.0, 255.0) as u8
                })
                .collect()
        };
        let blur = |img: &[u8]| -> Vec<u8> {
            let mut out = img.to_vec();
            for y in 1..h - 1 {
                for x in 1..w - 1 {
                    let mut s = 0u32;
                    for dy in 0..3 {
                        for dx in 0..3 {
                            s += img[(y + dy - 1) * w + (x + dx - 1)] as u32;
                        }
                    }
                    out[y * w + x] = (s / 9) as u8;
                }
            }
            out
        };
        let clean_train: Vec<Vec<u8>> = (0..4).map(mk).collect();
        let deg_train: Vec<Vec<u8>> = clean_train.iter().map(|c| blur(c)).collect();
        let pairs: Vec<(&[u8], &[u8])> = deg_train
            .iter()
            .zip(&clean_train)
            .map(|(d, c)| (d.as_slice(), c.as_slice()))
            .collect();
        let model = train(&pairs, w, h, 1);

        let clean_test = mk(99);
        let deg_test = blur(&clean_test);
        let restored = apply(&model, &deg_test, w, h);
        let mae = |a: &[u8], b: &[u8]| -> f64 {
            let mut e = 0.0;
            let mut n = 0.0;
            for y in 4..h - 4 {
                for x in 4..w - 4 {
                    e += (a[y * w + x] as f64 - b[y * w + x] as f64).abs();
                    n += 1.0;
                }
            }
            e / n
        };
        let e_deg = mae(&deg_test, &clean_test);
        let e_res = mae(&restored, &clean_test);
        assert!(
            e_res < e_deg * 0.6,
            "restoration should recover ≥40% of blur error on held-out data: deg {e_deg:.2} restored {e_res:.2}"
        );
    }
}
