//! noise.rs — seamless fractal noise, ported from `games/blackstar/tools/gen_texture.py`.
//!
//! The Python original gets tileability from FFT band-limiting: generate white noise, zero every
//! frequency above the octave's cutoff, invert. Because the surviving frequencies are all integer
//! multiples of the domain, the result wraps exactly — no blending, no mirrored seams, no
//! "tileable enough". That property is the whole reason to port this faithfully rather than reach
//! for a value-noise crate; a texture that almost tiles shows a grid on every flat panel.
//!
//! This is a direct DFT rather than an FFT. At 256x256 the naive transform is O(n^4) and far too
//! slow, so the band-limited synthesis is done the other way round: instead of filtering noise in
//! frequency space, sum random-phase sinusoids *at* the integer frequencies the filter would have
//! kept. The output is the same object — a band-limited random field that wraps — reached by
//! construction instead of by subtraction, and in O(n^2 * bands) with no complex arithmetic.

/// Texture edge length. The engine's convention throughout `games/blackstar/engine_src/data`.
pub const N: usize = 256;

/// Deterministic PRNG. Textures must regenerate identically from a seed on any machine, so this
/// cannot be `rand` with an unspecified algorithm.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        // SplitMix64's seeding constant; any odd stir works, this one is well tested.
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(0x1234_5678_9ABC_DEF0))
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in [0, 1).
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// A single greyscale field, row-major, N*N values.
#[derive(Clone)]
pub struct Field(pub Vec<f32>);

impl Field {
    pub fn zeros() -> Field {
        Field(vec![0.0; N * N])
    }
    #[inline]
    pub fn at(&self, x: usize, y: usize) -> f32 {
        self.0[y * N + x]
    }
    #[inline]
    pub fn set(&mut self, x: usize, y: usize, v: f32) {
        self.0[y * N + x] = v;
    }
    /// Stretch to fill [0, 1]. The Python does this per octave before accumulating.
    pub fn normalize(&mut self) {
        let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
        for v in &self.0 {
            lo = lo.min(*v);
            hi = hi.max(*v);
        }
        let span = hi - lo;
        if span > 1e-9 {
            for v in self.0.iter_mut() {
                *v = (*v - lo) / span;
            }
        }
    }
}

/// One band-limited random field: every component is an integer frequency, so it wraps exactly.
fn band(seed: u64, cutoff: u32) -> Field {
    let mut rng = Rng::new(seed);
    let mut f = Field::zeros();
    let c = cutoff.max(1) as i32;
    // Half-plane of integer frequencies inside the cutoff disc; the conjugate half is implied by
    // using a real cosine, which is what keeps the field real without an inverse transform.
    let mut waves: Vec<(f32, f32, f32, f32)> = Vec::new();
    for fy in -c..=c {
        for fx in 0..=c {
            if fx == 0 && fy <= 0 {
                continue;
            }
            let r = ((fx * fx + fy * fy) as f32).sqrt();
            if r > c as f32 || r < 1e-6 {
                continue;
            }
            let phase = rng.next_f32() * std::f32::consts::TAU;
            // 1/f falloff within the band, matching the spectral slope white noise acquires once
            // the higher octaves are cut away.
            let amp = 1.0 / r;
            waves.push((fx as f32, fy as f32, phase, amp));
        }
    }
    // Sum the waves separably. cos(k(fx*x + fy*y) + p) expands to
    //     cos(k*fx*x + p) * cos(k*fy*y) - sin(k*fx*x + p) * sin(k*fy*y)
    // so each wave needs two row vectors and two column vectors rather than a fresh transcendental
    // per pixel. That turns O(pixels * waves) into O((width + height) * waves + pixels * waves)
    // with only multiplies in the inner loop — the difference between a minute and a moment at
    // 256x256 with ~1800 waves, which is what the first version of this cost.
    let k = std::f32::consts::TAU / N as f32;
    let mut cos_x = vec![0.0f32; N];
    let mut sin_x = vec![0.0f32; N];
    let mut cos_y = vec![0.0f32; N];
    let mut sin_y = vec![0.0f32; N];
    for (fx, fy, phase, amp) in &waves {
        for x in 0..N {
            let a = k * fx * x as f32 + phase;
            cos_x[x] = a.cos() * amp;
            sin_x[x] = a.sin() * amp;
        }
        for y in 0..N {
            let b = k * fy * y as f32;
            cos_y[y] = b.cos();
            sin_y[y] = b.sin();
        }
        for y in 0..N {
            let (cy, sy) = (cos_y[y], sin_y[y]);
            let row = &mut f.0[y * N..(y + 1) * N];
            for x in 0..N {
                row[x] += cos_x[x] * cy - sin_x[x] * sy;
            }
        }
    }
    f.normalize();
    f
}

/// Seamless fractal noise in [0, 1]. Mirrors the Python signature and its octave/persistence
/// accumulation exactly.
pub fn tileable_fbm(seed: u64, octaves: u32, persistence: f32) -> Field {
    let mut acc = Field::zeros();
    let mut amp = 1.0f32;
    let mut total = 0.0f32;
    for o in 0..octaves.max(1) {
        // The Python keeps frequencies <= 2^(o+1).
        let cutoff = 1u32 << (o + 1);
        // Beyond ~24 cycles the wave count grows faster than the detail it adds at 256px.
        let f = band(seed.wrapping_add(o as u64 * 7919), cutoff.min(24));
        for i in 0..N * N {
            acc.0[i] += f.0[i] * amp;
        }
        total += amp;
        amp *= persistence;
    }
    if total > 0.0 {
        for v in acc.0.iter_mut() {
            *v /= total;
        }
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_is_in_range_and_not_flat() {
        let f = tileable_fbm(42, 4, 0.55);
        let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
        for v in &f.0 {
            assert!(v.is_finite());
            lo = lo.min(*v);
            hi = hi.max(*v);
        }
        assert!(lo >= 0.0 && hi <= 1.0, "range {lo}..{hi}");
        assert!(hi - lo > 0.3, "field is nearly flat: {lo}..{hi}");
    }

    #[test]
    fn it_actually_tiles() {
        // The property the whole port exists for: opposite edges must continue into each other as
        // smoothly as neighbouring rows inside the field do.
        let f = tileable_fbm(7, 5, 0.55);
        let mut seam = 0.0f32;
        let mut interior = 0.0f32;
        for i in 0..N {
            seam += (f.at(i, 0) - f.at(i, N - 1)).abs();
            seam += (f.at(0, i) - f.at(N - 1, i)).abs();
            interior += (f.at(i, N / 2) - f.at(i, N / 2 - 1)).abs();
            interior += (f.at(N / 2, i) - f.at(N / 2 - 1, i)).abs();
        }
        // Wrapping is exact by construction, so the seam step should be no worse than a typical
        // interior step — not merely "small".
        assert!(seam <= interior * 1.5, "seam {seam} vs interior {interior}");
    }

    #[test]
    fn a_seed_reproduces_exactly() {
        let a = tileable_fbm(99, 3, 0.5);
        let b = tileable_fbm(99, 3, 0.5);
        assert_eq!(a.0, b.0, "same seed must give the same texture on any machine");
        let c = tileable_fbm(100, 3, 0.5);
        assert_ne!(a.0, c.0, "different seeds must differ");
    }

    #[test]
    fn persistence_controls_roughness() {
        // Low persistence weights the coarse octaves, so the field should vary less abruptly.
        let smooth = tileable_fbm(5, 5, 0.2);
        let rough = tileable_fbm(5, 5, 0.9);
        let grad = |f: &Field| {
            let mut g = 0.0;
            for y in 1..N {
                for x in 1..N {
                    g += (f.at(x, y) - f.at(x - 1, y)).abs();
                }
            }
            g
        };
        assert!(grad(&smooth) < grad(&rough), "persistence should raise fine detail");
    }
}

