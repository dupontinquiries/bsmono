//! Kernel: **heat distortion** — a screen-space refraction post that makes air
//! above hot things (your `temperature` / `heat_source` / `thermal_mass`
//! modules) shimmer. Heat-above-ambient drives an animated noise offset that
//! *gathers* the already-lit colour, so it's a cheap full-screen resample with
//! no extra geometry.
//!
//! Per pixel: read the heat value, build a two-channel value-noise offset scrolled
//! upward by time (rising haze), scale it by heat × strength and clamp to a max
//! pixel offset, then bilinearly gather the colour buffer at the offset position
//! (clamp-to-edge). Where heat is 0 the offset is 0 and the image is untouched;
//! because it only gathers existing pixels it can never amplify or introduce new
//! colour.
//!
//! ## Analytics / frame pacing
//! Time the full-screen pass as a `gpu_timer` zone (suggest `"heat_post"`), or
//! report the CPU reference cost via [`BenchResult::metric_line`] with `name =
//! "heat_post"`; it then shows in the dashboard's GPU-timers panel under the
//! stage budget alongside `shadow`/`froxel`/`scatter`/`main`.
//!
//! ## GPU DISPATCH CONTRACT
//!   bindings: 0: lit colour texture (sampler, clamp-to-edge, bilinear),
//!     1: heat texture (heat-above-ambient projected to screen), 2: out colour.
//!   uniforms: time, freq, rise, strength, max_off (pixels), resolution.
//!   dispatch: one invocation per pixel; offset = clamp(noise2(uv*freq, uv*freq -
//!     time*rise) * heat * strength, ±max_off); out = texture(colour, uv +
//!     offset/resolution). Hardware bilinear + clamp matches [`distort`] within
//!     filtering precision.

use crate::sim::gpu_bench::{bench, BenchResult};

/// Integer hash → [0,1). Cheap, deterministic; only used to seed value noise.
#[inline]
fn hash2(ix: i32, iy: i32) -> f32 {
    let mut n = (ix as u32)
        .wrapping_mul(374_761_393)
        .wrapping_add((iy as u32).wrapping_mul(668_265_263));
    n = (n ^ (n >> 13)).wrapping_mul(1_274_126_177);
    ((n ^ (n >> 16)) as f32) / 4_294_967_296.0
}

/// Bilinear value noise with smoothstep interpolation; output ~[0,1], continuous.
pub fn value_noise(x: f32, y: f32) -> f32 {
    let x0 = x.floor();
    let y0 = y.floor();
    let fx = x - x0;
    let fy = y - y0;
    let (ix, iy) = (x0 as i32, y0 as i32);
    let sx = fx * fx * (3.0 - 2.0 * fx);
    let sy = fy * fy * (3.0 - 2.0 * fy);
    let a = hash2(ix, iy);
    let b = hash2(ix + 1, iy);
    let c = hash2(ix, iy + 1);
    let d = hash2(ix + 1, iy + 1);
    a + (b - a) * sx + (c - a) * sy + (a - b - c + d) * sx * sy
}

/// Heat-distortion tunables.
#[derive(Clone, Copy, Debug)]
pub struct HeatParams {
    /// Noise frequency in UV space.
    pub freq: f32,
    /// Upward scroll speed of the shimmer (rising haze).
    pub rise: f32,
    /// Offset gain per unit heat.
    pub strength: f32,
    /// Maximum per-axis offset in pixels.
    pub max_off: f32,
}

/// Screen-space shimmer offset (in pixels) at normalized coord `(u,v)` and
/// `time`, for a given `heat` value. Zero heat → zero offset; magnitude is
/// monotonic non-decreasing in heat up to `±max_off`.
pub fn shimmer_offset(u: f32, v: f32, time: f32, heat: f32, p: &HeatParams) -> [f32; 2] {
    let nx = value_noise(u * p.freq, v * p.freq - time * p.rise) * 2.0 - 1.0;
    let ny = value_noise(u * p.freq + 37.2, v * p.freq - time * p.rise + 91.7) * 2.0 - 1.0;
    let ox = (nx * heat * p.strength).clamp(-p.max_off, p.max_off);
    let oy = (ny * heat * p.strength).clamp(-p.max_off, p.max_off);
    [ox, oy]
}

/// An RGB image buffer (row-major).
#[derive(Clone, Debug)]
pub struct ColorImage {
    pub w: usize,
    pub h: usize,
    pub px: Vec<[f32; 3]>,
}

#[inline]
fn clampi(v: i32, n: usize) -> usize {
    if v < 0 {
        0
    } else if (v as usize) >= n {
        n - 1
    } else {
        v as usize
    }
}

impl ColorImage {
    pub fn new(w: usize, h: usize) -> Self {
        ColorImage { w, h, px: vec![[0.0; 3]; w * h] }
    }

    #[inline]
    fn texel(&self, x: i32, y: i32) -> [f32; 3] {
        self.px[clampi(y, self.h) * self.w + clampi(x, self.w)]
    }

    /// Clamp-to-edge bilinear gather. Returns the exact texel at integer coords.
    pub fn sample_bilinear(&self, px: f32, py: f32) -> [f32; 3] {
        let x0 = px.floor() as i32;
        let y0 = py.floor() as i32;
        let fx = px - x0 as f32;
        let fy = py - y0 as f32;
        let a = self.texel(x0, y0);
        let b = self.texel(x0 + 1, y0);
        let c = self.texel(x0, y0 + 1);
        let d = self.texel(x0 + 1, y0 + 1);
        let mut out = [0.0f32; 3];
        for ch in 0..3 {
            out[ch] = a[ch] * (1.0 - fx) * (1.0 - fy)
                + b[ch] * fx * (1.0 - fy)
                + c[ch] * (1.0 - fx) * fy
                + d[ch] * fx * fy;
        }
        out
    }
}

/// Apply heat distortion to `img` given a per-pixel `heat` field (length
/// `img.w*img.h`) and animation `time`. Returns a new image.
pub fn distort(img: &ColorImage, heat: &[f32], time: f32, p: &HeatParams) -> ColorImage {
    let (w, h) = (img.w, img.h);
    let mut out = vec![[0.0f32; 3]; w * h];
    for y in 0..h {
        for x in 0..w {
            let hv = heat[y * w + x];
            let off = shimmer_offset(x as f32 / w as f32, y as f32 / h as f32, time, hv, p);
            out[y * w + x] = img.sample_bilinear(x as f32 + off[0], y as f32 + off[1]);
        }
    }
    ColorImage { w, h, px: out }
}

/// Benchmark a full-frame distortion over `iters` runs.
pub fn bench_distort(img: &ColorImage, heat: &[f32], time: f32, p: &HeatParams, iters: usize) -> BenchResult {
    bench(iters, || {
        let o = distort(img, heat, time, p);
        std::hint::black_box(&o);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> HeatParams {
        HeatParams { freq: 8.0, rise: 2.0, strength: 3.0, max_off: 4.0 }
    }

    fn random_img(w: usize, h: usize) -> ColorImage {
        // deterministic pseudo-random fill in [0,1]
        let mut img = ColorImage::new(w, h);
        let mut s: u32 = 0x1234_5678;
        for p in img.px.iter_mut() {
            for ch in 0..3 {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                p[ch] = (s >> 8) as f32 / 16_777_216.0;
            }
        }
        img
    }

    #[test]
    fn zero_heat_is_identity() {
        let img = random_img(24, 24);
        let heat = vec![0.0f32; img.w * img.h];
        let out = distort(&img, &heat, 1.3, &params());
        for i in 0..img.px.len() {
            for ch in 0..3 {
                assert!((out.px[i][ch] - img.px[i][ch]).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn gather_stays_within_source_range() {
        let img = random_img(24, 24);
        let mut heat = vec![0.0f32; img.w * img.h];
        let mut s: u32 = 99;
        for hv in heat.iter_mut() {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *hv = (s >> 8) as f32 / 16_777_216.0;
        }
        let out = distort(&img, &heat, 0.7, &params());
        let lo = img.px.iter().flatten().cloned().fold(f32::INFINITY, f32::min);
        let hi = img.px.iter().flatten().cloned().fold(f32::NEG_INFINITY, f32::max);
        for v in out.px.iter().flatten() {
            assert!(*v >= lo - 1e-6 && *v <= hi + 1e-6, "{v} outside [{lo},{hi}]");
        }
    }

    #[test]
    fn offset_clamped_and_monotonic_in_heat() {
        let p = params();
        let mut prev = 0.0f32;
        for k in 0..=10 {
            let heat = k as f32 / 10.0;
            let o = shimmer_offset(0.3, 0.3, 0.5, heat, &p);
            let mag = (o[0] * o[0] + o[1] * o[1]).sqrt();
            assert!(o[0].abs() <= p.max_off + 1e-6 && o[1].abs() <= p.max_off + 1e-6);
            assert!(mag >= prev - 1e-6, "magnitude dropped with more heat");
            prev = mag;
        }
        // zero heat → zero offset
        assert_eq!(shimmer_offset(0.3, 0.3, 0.5, 0.0, &p), [0.0, 0.0]);
    }

    #[test]
    fn noise_is_continuous() {
        let d = (value_noise(3.10, 5.20) - value_noise(3.11, 5.20)).abs();
        assert!(d < 0.1, "noise jumped {d} for a 0.01 step");
    }

    #[test]
    fn bilinear_identity_at_integer() {
        let img = random_img(8, 8);
        let got = img.sample_bilinear(5.0, 7.0);
        assert_eq!(got, img.px[7 * 8 + 5]);
    }

    #[test]
    fn bench_runs() {
        let img = random_img(32, 32);
        let heat = vec![0.5f32; img.w * img.h];
        let br = bench_distort(&img, &heat, 0.5, &params(), 3);
        assert_eq!(br.samples.len(), 3);
        assert!(br.metric_line("heat_post", Some(8.3)).contains("heat_post n=3"));
    }
}
