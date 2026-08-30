//! optics.rs — the imaging model.
//!
//! Lithographic imaging is diffraction, and the whole model is three steps: the mask diffracts,
//! the pupil collects only the orders that fit inside the numerical aperture, and the image is the
//! interference of what got through. Everything specific to EUV is in the numbers — 13.5 nm rather
//! than 193 — not in the mathematics.
//!
//! The cutoff falls straight out of that and is the model's sharpest test. A grating of pitch `p`
//! sends its first diffracted order to spatial frequency `1/p`; the pupil passes frequencies up to
//! `NA/λ`. So under coherent illumination a pitch below `λ/NA` delivers only the zeroth order to
//! the wafer, and **an image formed from one order has no contrast at all** — not poor contrast,
//! none, because a single plane wave has uniform intensity. That is a hard physical edge with no
//! fitted constant in it, which makes it worth far more as a test than any resolution figure.
//!
//! Off-axis illumination is the standard answer: tilting the source shifts the pupil relative to
//! the diffraction orders, so an order that missed on-axis can be caught, extending the cutoff to
//! `λ/(NA(1+σ))`. Dipole illumination pushed to that limit is why the industry prints features
//! finer than the on-axis cutoff.
//!
//! Precision: this layer is f32-safe by the workspace policy — the computation is a bounded
//! feed-forward map with no ill-conditioned inverse and no long accumulation, and the output is a
//! threshold decision on an intensity. It is written in f64 here because the CPU is where it runs
//! today and f64 costs nothing on a CPU; the GPU port would drop to f32 without consequence.

use sim_num::complex::{c64, C64};
use sim_num::fft::{fft2, freq, Dir};

/// Optical column parameters.
#[derive(Clone, Copy, Debug)]
pub struct Optics {
    /// Wavelength, metres. EUV is 13.5 nm.
    pub lambda: f64,
    /// Numerical aperture at the wafer. 0.33 for standard EUV, 0.55 for High-NA.
    pub na: f64,
    /// Defocus, metres. Zero is best focus.
    pub defocus: f64,
}

impl Optics {
    pub fn euv() -> Optics {
        Optics { lambda: 13.5e-9, na: 0.33, defocus: 0.0 }
    }
    pub fn high_na_euv() -> Optics {
        Optics { lambda: 13.5e-9, na: 0.55, defocus: 0.0 }
    }

    /// Highest spatial frequency the pupil accepts, cycles per metre.
    pub fn cutoff_frequency(&self) -> f64 {
        self.na / self.lambda
    }

    /// Smallest grating pitch that still delivers a first order under on-axis illumination.
    pub fn coherent_pitch_limit(&self) -> f64 {
        self.lambda / self.na
    }

    /// Smallest resolvable pitch with illumination of partial coherence `sigma`.
    pub fn pitch_limit(&self, sigma: f64) -> f64 {
        self.lambda / (self.na * (1.0 + sigma))
    }

    /// Rayleigh half-pitch resolution for a given process factor k1.
    pub fn resolution(&self, k1: f64) -> f64 {
        k1 * self.lambda / self.na
    }

    /// Rayleigh depth of focus.
    pub fn depth_of_focus(&self, k2: f64) -> f64 {
        k2 * self.lambda / (self.na * self.na)
    }
}

/// A source point, given as direction cosines normalised to the NA.
#[derive(Clone, Copy, Debug)]
pub struct SourcePoint {
    pub sx: f64,
    pub sy: f64,
    pub weight: f64,
}

/// Illumination shapes. These are the knobs a lithographer actually turns.
#[derive(Clone, Copy, Debug)]
pub enum Illumination {
    /// A filled disc of radius `sigma`. Sigma zero is coherent.
    Conventional { sigma: f64 },
    /// A ring between two radii — the workhorse for dense periodic patterns.
    Annular { inner: f64, outer: f64 },
    /// Two poles along x, which maximises contrast for vertical lines at a chosen pitch.
    Dipole { sigma: f64, radius: f64 },
}

impl Illumination {
    /// Sample the source on a grid, keeping points inside the shape. Weights sum to one, so an
    /// aerial image is directly comparable across illumination shapes.
    pub fn sample(&self, n: usize) -> Vec<SourcePoint> {
        let mut pts = Vec::new();
        let inside = |x: f64, y: f64| -> bool {
            let r = x.hypot(y);
            match *self {
                Illumination::Conventional { sigma } => r <= sigma,
                Illumination::Annular { inner, outer } => r >= inner && r <= outer,
                Illumination::Dipole { sigma, radius } => {
                    (x - sigma).hypot(y) <= radius || (x + sigma).hypot(y) <= radius
                }
            }
        };
        // A single coherent point is a degenerate case the grid would miss.
        if let Illumination::Conventional { sigma } = *self {
            if sigma <= 1e-12 {
                return vec![SourcePoint { sx: 0.0, sy: 0.0, weight: 1.0 }];
            }
        }
        for i in 0..n {
            for j in 0..n {
                let x = -1.0 + 2.0 * (i as f64 + 0.5) / n as f64;
                let y = -1.0 + 2.0 * (j as f64 + 0.5) / n as f64;
                if inside(x, y) {
                    pts.push(SourcePoint { sx: x, sy: y, weight: 1.0 });
                }
            }
        }
        let total = pts.len() as f64;
        for p in pts.iter_mut() {
            p.weight = 1.0 / total;
        }
        pts
    }
}

/// A mask: complex transmission on a square grid of `n` points spaced `dx` apart.
#[derive(Clone, Debug)]
pub struct Mask {
    pub n: usize,
    pub dx: f64,
    pub t: Vec<C64>,
}

impl Mask {
    /// A line-space grating running along y, with `pitch` and duty cycle `duty` (fraction open).
    ///
    /// The pitch must be an exact whole number of pixels, and this is checked rather than assumed.
    /// The reason is the same as `Mesh::structured` demanding that interfaces land on grid lines,
    /// and the failure mode here is worse because it is invisible: computing the duty boundary in
    /// floating point flipped exactly ONE pixel per row at some pitches, and a single-pixel defect
    /// has a flat spectrum. That leaked broadband energy straight through the pupil and produced
    /// 0.26 contrast at a pitch where the physics says exactly zero — a spurious result that looked
    /// like a soft cutoff, which is precisely the sort of thing a lithography model is used to
    /// argue about.
    ///
    /// Pixel counts are computed as integers so periodicity is exact by construction.
    pub fn grating(n: usize, dx: f64, pitch: f64, duty: f64) -> Mask {
        let ppp_f = pitch / dx;
        let ppp = ppp_f.round() as usize;
        assert!(
            ppp >= 2 && (ppp_f - ppp as f64).abs() < 1e-9 * ppp_f,
            "grating pitch must be a whole number of pixels: pitch/dx = {ppp_f}"
        );
        assert!(
            n % ppp == 0,
            "grid of {n} pixels must hold a whole number of {ppp}-pixel periods"
        );
        let open = ((ppp as f64) * duty).round() as usize;
        let mut t = vec![c64(0.0, 0.0); n * n];
        for r in 0..n {
            for c in 0..n {
                if c % ppp < open {
                    t[r * n + c] = c64(1.0, 0.0);
                }
            }
        }
        Mask { n, dx, t }
    }

    /// A single isolated square opening of side `w`, centred.
    pub fn contact(n: usize, dx: f64, w: f64) -> Mask {
        let mut t = vec![c64(0.0, 0.0); n * n];
        let centre = n as f64 * dx / 2.0;
        for r in 0..n {
            for c in 0..n {
                let (x, y) = (c as f64 * dx, r as f64 * dx);
                if (x - centre).abs() <= w / 2.0 && (y - centre).abs() <= w / 2.0 {
                    t[r * n + c] = c64(1.0, 0.0);
                }
            }
        }
        Mask { n, dx, t }
    }

    /// Uniform open field — images to uniform intensity through any pupil, since only the zeroth
    /// order exists.
    pub fn clear(n: usize, dx: f64) -> Mask {
        Mask { n, dx, t: vec![c64(1.0, 0.0); n * n] }
    }
}

/// Aerial image intensity on the mask grid.
///
/// Abbe's method: each source point illuminates the mask from a different angle, which shifts the
/// diffraction pattern relative to the pupil. Each point is imaged coherently and the *intensities*
/// are summed — incoherently, because different source points do not interfere. That last word is
/// the whole content of "partially coherent" and getting it wrong (summing amplitudes) produces an
/// image with far too much contrast.
pub fn aerial_image(mask: &Mask, optics: &Optics, illum: &Illumination, source_grid: usize) -> Vec<f64> {
    let n = mask.n;
    let cutoff = optics.cutoff_frequency();

    // Diffract once: the mask spectrum does not depend on the source.
    let mut spectrum = mask.t.clone();
    fft2(&mut spectrum, n, Dir::Forward);

    let mut image = vec![0.0f64; n * n];
    let points = illum.sample(source_grid);

    for p in &points {
        // Illuminating at an angle shifts the pupil centre in frequency space by sigma*NA/lambda.
        let (fx0, fy0) = (p.sx * cutoff, p.sy * cutoff);
        let mut field = vec![C64::default(); n * n];
        for r in 0..n {
            let fy = freq(r, n, mask.dx);
            for c in 0..n {
                let fx = freq(c, n, mask.dx);
                let rho = (fx - fx0).hypot(fy - fy0);
                if rho <= cutoff {
                    let mut v = spectrum[r * n + c];
                    if optics.defocus != 0.0 {
                        // Defocus is a quadratic phase across the pupil — the standard
                        // parabolic approximation, valid while NA is not extreme.
                        let phi = std::f64::consts::PI * optics.lambda * optics.defocus * rho * rho;
                        v = v * C64::expi(-phi);
                    }
                    field[r * n + c] = v;
                }
            }
        }
        fft2(&mut field, n, Dir::Inverse);
        for i in 0..n * n {
            image[i] += p.weight * field[i].norm_sq();
        }
    }
    image
}

/// Image contrast (modulation) along a horizontal cut through the middle.
pub fn contrast(image: &[f64], n: usize) -> f64 {
    let row = n / 2;
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for c in 0..n {
        let v = image[row * n + c];
        lo = lo.min(v);
        hi = hi.max(v);
    }
    if hi + lo <= 0.0 {
        return 0.0;
    }
    (hi - lo) / (hi + lo)
}

/// Normalised image log slope at the threshold crossing — the quantity that actually predicts
/// whether a feature prints, because it measures how sharply intensity changes across the edge
/// rather than how deep the modulation is.
pub fn nils(image: &[f64], n: usize, dx: f64, feature: f64, threshold: f64) -> f64 {
    let row = n / 2;
    let mut best = 0.0f64;
    for c in 1..n - 1 {
        let (a, b) = (image[row * n + c], image[row * n + c + 1]);
        if (a - threshold) * (b - threshold) < 0.0 {
            let slope = (b - a) / dx;
            let i = 0.5 * (a + b);
            if i > 0.0 {
                best = best.max((slope / i).abs() * feature);
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grid sized so the pitch divides it exactly — otherwise the grating is not periodic across
    /// the FFT's implicit wrap and the spectrum smears, which looks like a physics result.
    fn grating_grid(pitch: f64, periods: usize, n: usize) -> f64 {
        pitch * periods as f64 / n as f64
    }

    #[test]
    fn the_coherent_cutoff_is_a_hard_edge() {
        // The sharpest test in this crate, and it needs no reference data: below the cutoff pitch
        // only the zeroth order reaches the wafer, and one plane wave has uniform intensity. So
        // contrast is not merely poor, it is zero.
        let o = Optics::euv();
        let limit = o.coherent_pitch_limit(); // 13.5nm / 0.33 = 40.9 nm
        let coherent = Illumination::Conventional { sigma: 0.0 };
        let n = 128;

        let below = limit * 0.85;
        let m = Mask::grating(n, grating_grid(below, 8, n), below, 0.5);
        let img = aerial_image(&m, &o, &coherent, 8);
        assert!(
            contrast(&img, n) < 1e-9,
            "below the cutoff the image must be flat, contrast was {}",
            contrast(&img, n)
        );

        let above = limit * 1.25;
        let m = Mask::grating(n, grating_grid(above, 8, n), above, 0.5);
        let img = aerial_image(&m, &o, &coherent, 8);
        assert!(
            contrast(&img, n) > 0.3,
            "above the cutoff the first order gets through, contrast was {}",
            contrast(&img, n)
        );
    }

    #[test]
    fn off_axis_illumination_prints_below_the_on_axis_cutoff() {
        // The reason dipole illumination exists: tilting the source shifts the pupil relative to
        // the diffraction orders, so an order that missed on-axis is caught. This is a pitch that
        // is genuinely unprintable coherently and printable with a dipole.
        let o = Optics::euv();
        let pitch = o.coherent_pitch_limit() * 0.8;
        let n = 128;
        let dx = grating_grid(pitch, 8, n);
        let m = Mask::grating(n, dx, pitch, 0.5);

        let flat = aerial_image(&m, &o, &Illumination::Conventional { sigma: 0.0 }, 8);
        assert!(contrast(&flat, n) < 1e-9, "coherent must fail here");

        let dipole = Illumination::Dipole { sigma: 0.7, radius: 0.2 };
        let img = aerial_image(&m, &o, &dipole, 24);
        assert!(
            contrast(&img, n) > 0.2,
            "dipole should recover contrast, got {}",
            contrast(&img, n)
        );
    }

    #[test]
    fn high_na_resolves_what_standard_na_cannot() {
        // The entire industrial argument for High-NA EUV, reproduced: same wavelength, same mask,
        // larger pupil.
        let pitch = Optics::euv().coherent_pitch_limit() * 0.75;
        let n = 128;
        let dx = grating_grid(pitch, 8, n);
        let m = Mask::grating(n, dx, pitch, 0.5);
        let coherent = Illumination::Conventional { sigma: 0.0 };

        let standard = aerial_image(&m, &Optics::euv(), &coherent, 8);
        let high = aerial_image(&m, &Optics::high_na_euv(), &coherent, 8);
        assert!(contrast(&standard, n) < 1e-9);
        assert!(contrast(&high, n) > 0.3, "High-NA contrast {}", contrast(&high, n));
    }

    #[test]
    fn a_clear_field_images_uniformly_and_conserves_its_intensity() {
        // With only a zeroth order there is nothing to interfere, so the image must be flat at
        // unit intensity. This catches normalisation errors in the transform pair, which would
        // otherwise scale every result silently.
        let o = Optics::euv();
        let n = 64;
        let m = Mask::clear(n, 2.0e-9);
        for illum in [
            Illumination::Conventional { sigma: 0.0 },
            Illumination::Conventional { sigma: 0.6 },
            Illumination::Annular { inner: 0.5, outer: 0.8 },
        ] {
            let img = aerial_image(&m, &o, &illum, 16);
            let (lo, hi) = img.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
                (a.min(*v), b.max(*v))
            });
            assert!((hi - lo).abs() < 1e-12, "clear field is not flat: {lo} to {hi}");
            assert!((hi - 1.0).abs() < 1e-9, "clear field intensity should be 1, got {hi}");
        }
    }

    #[test]
    fn contrast_falls_monotonically_as_the_pitch_approaches_the_limit() {
        // Monotonicity holds only NEAR the cutoff, and the first version of this test assumed it
        // held everywhere. It does not, and the reason is physical rather than numerical: the
        // image is a Fourier series of the mask truncated at whatever orders fit through the
        // pupil, and the number of orders that fit changes DISCRETELY with pitch. Adding an order
        // changes the Gibbs ringing, so peak-to-valley contrast can rise slightly when a new order
        // arrives. Measured at 3x and 2x the limit the values were 0.9945 and 0.9983 — out of
        // order by 0.4%, and correct.
        //
        // Approaching the cutoff the order count only falls, toward one, so the trend there is
        // genuine and that is the regime worth asserting.
        let o = Optics::euv();
        let n = 128;
        let limit = o.coherent_pitch_limit();
        let illum = Illumination::Conventional { sigma: 0.5 };
        let mut seen = Vec::new();
        for k in [1.6, 1.4, 1.2, 1.05] {
            let pitch = limit * k;
            let m = Mask::grating(n, grating_grid(pitch, 8, n), pitch, 0.5);
            seen.push(contrast(&aerial_image(&m, &o, &illum, 16), n));
        }
        for w in seen.windows(2) {
            assert!(w[1] <= w[0] + 1e-9, "contrast should fall toward the cutoff: {seen:?}");
        }
        assert!(seen[0] > seen[seen.len() - 1], "and fall overall: {seen:?}");
    }

    #[test]
    fn defocus_degrades_the_image() {
        // Best focus must be the best. A sign error in the defocus phase would still degrade the
        // image, so this is checked symmetrically: equal defocus either way must degrade equally,
        // which a sign error would break.
        let mut o = Optics::euv();
        let n = 128;
        let pitch = o.coherent_pitch_limit() * 2.0;
        let dx = grating_grid(pitch, 8, n);
        let m = Mask::grating(n, dx, pitch, 0.5);
        let illum = Illumination::Conventional { sigma: 0.5 };

        let sharp = contrast(&aerial_image(&m, &o, &illum, 12), n);
        o.defocus = 60e-9;
        let plus = contrast(&aerial_image(&m, &o, &illum, 12), n);
        o.defocus = -60e-9;
        let minus = contrast(&aerial_image(&m, &o, &illum, 12), n);

        assert!(plus < sharp, "defocus should reduce contrast: {plus} vs {sharp}");
        assert!(
            (plus - minus).abs() < 1e-9,
            "defocus must be symmetric about best focus: {plus} vs {minus}"
        );
    }

    #[test]
    fn the_stated_resolution_formulas_agree_with_the_pupil() {
        let o = Optics::euv();
        // k1 = 0.5 is the classical coherent limit, and the half-pitch there is exactly half the
        // coherent pitch limit — the two expressions must be consistent.
        assert!((o.resolution(0.5) - o.coherent_pitch_limit() / 2.0).abs() < 1e-18);
        assert!((o.cutoff_frequency() * o.coherent_pitch_limit() - 1.0).abs() < 1e-12);
        // Partial coherence extends the limit; sigma = 1 halves the printable pitch.
        assert!((o.pitch_limit(1.0) - o.coherent_pitch_limit() / 2.0).abs() < 1e-18);
        // High-NA buys resolution and costs depth of focus quadratically.
        let h = Optics::high_na_euv();
        assert!(h.resolution(0.5) < o.resolution(0.5));
        assert!(h.depth_of_focus(1.0) < o.depth_of_focus(1.0) * 0.4);
    }

    #[test]
    fn illumination_weights_sum_to_one() {
        for illum in [
            Illumination::Conventional { sigma: 0.0 },
            Illumination::Conventional { sigma: 0.9 },
            Illumination::Annular { inner: 0.6, outer: 0.9 },
            Illumination::Dipole { sigma: 0.7, radius: 0.2 },
        ] {
            let pts = illum.sample(32);
            assert!(!pts.is_empty(), "{illum:?} sampled to nothing");
            let w: f64 = pts.iter().map(|p| p.weight).sum();
            assert!((w - 1.0).abs() < 1e-12, "{illum:?} weights sum to {w}");
        }
    }
}

