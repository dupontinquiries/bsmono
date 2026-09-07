//! Measuring what a speed setting costs the picture.
//!
//! Every throughput number this app reports — the reuse gate's 48x, the
//! cascade's 11x — is a trade against quality, and until now nothing here
//! measured the other side of it. These are the two standard metrics, so a
//! setting can be quoted as "1.9x faster, 0.4 dB down" rather than just
//! "faster".
//!
//! Both compare against a *reference render*, not against the source: the
//! question is never "how close is the upscale to the original small frame"
//! but "how much did turning the gate down move the output away from what the
//! slow settings produced".

use crate::frame::Frame;
use rayon::prelude::*;

/// BT.601 luma in 0–255, integer, matching the analysis path's fixed point.
fn luma_plane(f: &Frame) -> Vec<f32> {
    f.data.chunks_exact(3).map(|p| ((77 * p[0] as u32 + 150 * p[1] as u32 + 29 * p[2] as u32) >> 8) as f32).collect()
}

/// Mean SSIM over 8x8 windows of the luma plane, stride 4.
///
/// The standard constants (C1 = (0.01·255)², C2 = (0.03·255)²) and a box
/// window rather than the Gaussian one — the difference is well under the
/// precision anyone reads these numbers to, and a box lets this stay a few
/// lines with no filter machinery.
pub fn ssim(a: &Frame, b: &Frame) -> f64 {
    assert_eq!((a.width, a.height), (b.width, b.height));
    const WIN: usize = 8;
    const STRIDE: usize = 4;
    const C1: f64 = 6.5025; // (0.01*255)^2
    const C2: f64 = 58.5225; // (0.03*255)^2
    let (w, h) = (a.width as usize, a.height as usize);
    if w < WIN || h < WIN {
        return if a == b { 1.0 } else { 0.0 };
    }
    let (la, lb) = (luma_plane(a), luma_plane(b));
    let ys: Vec<usize> = (0..=(h - WIN)).step_by(STRIDE).collect();
    let (sum, n) = ys
        .par_iter()
        .map(|&y| {
            let mut s = 0.0;
            let mut c = 0u64;
            for x in (0..=(w - WIN)).step_by(STRIDE) {
                let (mut sa, mut sb, mut saa, mut sbb, mut sab) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
                for j in 0..WIN {
                    let ra = &la[(y + j) * w + x..(y + j) * w + x + WIN];
                    let rb = &lb[(y + j) * w + x..(y + j) * w + x + WIN];
                    for k in 0..WIN {
                        let (pa, pb) = (ra[k] as f64, rb[k] as f64);
                        sa += pa;
                        sb += pb;
                        saa += pa * pa;
                        sbb += pb * pb;
                        sab += pa * pb;
                    }
                }
                let nn = (WIN * WIN) as f64;
                let (ma, mb) = (sa / nn, sb / nn);
                let va = (saa / nn - ma * ma).max(0.0);
                let vb = (sbb / nn - mb * mb).max(0.0);
                let cov = sab / nn - ma * mb;
                s += ((2.0 * ma * mb + C1) * (2.0 * cov + C2)) / ((ma * ma + mb * mb + C1) * (va + vb + C2));
                c += 1;
            }
            (s, c)
        })
        .reduce(|| (0.0, 0), |x, y| (x.0 + y.0, x.1 + y.1));
    if n == 0 {
        1.0
    } else {
        sum / n as f64
    }
}

/// Running comparison of two renders of the same clip.
#[derive(Debug, Clone, Default)]
pub struct QualityReport {
    pub frames: u64,
    pub psnr_sum: f64,
    /// Frames where the two renders were bit-identical (infinite PSNR); these
    /// are excluded from the mean rather than poisoning it with infinities.
    pub identical: u64,
    pub psnr_min: f64,
    pub psnr_min_frame: u64,
    pub ssim_sum: f64,
    pub ssim_min: f64,
    pub ssim_min_frame: u64,
}

impl QualityReport {
    pub fn new() -> Self {
        QualityReport { psnr_min: f64::INFINITY, ssim_min: f64::INFINITY, ..Default::default() }
    }

    pub fn push(&mut self, index: u64, a: &Frame, b: &Frame) {
        let p = a.psnr(b);
        let s = ssim(a, b);
        if p.is_finite() {
            self.psnr_sum += p;
            if p < self.psnr_min {
                self.psnr_min = p;
                self.psnr_min_frame = index;
            }
        } else {
            self.identical += 1;
        }
        self.ssim_sum += s;
        if s < self.ssim_min {
            self.ssim_min = s;
            self.ssim_min_frame = index;
        }
        self.frames += 1;
    }

    /// Mean PSNR over the frames that actually differed.
    pub fn psnr_mean(&self) -> f64 {
        let n = self.frames - self.identical;
        if n == 0 {
            f64::INFINITY
        } else {
            self.psnr_sum / n as f64
        }
    }

    pub fn ssim_mean(&self) -> f64 {
        if self.frames == 0 {
            1.0
        } else {
            self.ssim_sum / self.frames as f64
        }
    }

    /// A plain reading of the numbers. The thresholds are the conventional
    /// ones for transcode work; they are a guide, not a verdict, because a
    /// mean hides a single bad frame and the minimum is what an eye catches.
    pub fn verdict(&self) -> &'static str {
        let (p, s) = (self.psnr_mean(), self.ssim_mean());
        if self.frames == self.identical {
            "identical"
        } else if p >= 45.0 && s >= 0.995 {
            "visually indistinguishable"
        } else if p >= 40.0 && s >= 0.99 {
            "very close"
        } else if p >= 35.0 && s >= 0.98 {
            "close — differences visible on close inspection"
        } else if p >= 30.0 {
            "noticeably different"
        } else {
            "clearly different"
        }
    }
}

impl std::fmt::Display for QualityReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} frames ({} identical): PSNR mean {:.2} dB, min {:.2} dB @ frame {}; SSIM mean {:.4}, min {:.4} @ frame {} — {}",
            self.frames,
            self.identical,
            self.psnr_mean(),
            self.psnr_min,
            self.psnr_min_frame,
            self.ssim_mean(),
            self.ssim_min,
            self.ssim_min_frame,
            self.verdict()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testimg::smooth;

    #[test]
    fn identical_frames_are_perfect() {
        let a = smooth(64, 48, 1);
        assert!((ssim(&a, &a) - 1.0).abs() < 1e-9);
        let mut r = QualityReport::new();
        r.push(0, &a, &a);
        assert_eq!(r.identical, 1);
        assert!(r.psnr_mean().is_infinite());
        assert_eq!(r.verdict(), "identical");
    }

    #[test]
    fn ssim_falls_with_distortion_and_tracks_psnr() {
        let a = smooth(96, 96, 2);
        let mut prev_ssim = 1.0;
        let mut prev_psnr = f64::INFINITY;
        for strength in [2u8, 6, 16, 40] {
            let mut b = a.clone();
            b.add_uniform_noise(strength, 7);
            let (s, p) = (ssim(&a, &b), a.psnr(&b));
            assert!(s < prev_ssim, "ssim did not fall at strength {strength}: {s} vs {prev_ssim}");
            assert!(p < prev_psnr, "psnr did not fall at strength {strength}");
            assert!((0.0..=1.0).contains(&s), "ssim out of range: {s}");
            prev_ssim = s;
            prev_psnr = p;
        }
        // Mild noise should still read as close, heavy noise should not.
        let mut mild = a.clone();
        mild.add_uniform_noise(2, 7);
        let mut heavy = a.clone();
        heavy.add_uniform_noise(40, 7);
        let mut r1 = QualityReport::new();
        r1.push(0, &a, &mild);
        let mut r2 = QualityReport::new();
        r2.push(0, &a, &heavy);
        assert!(r1.psnr_mean() > r2.psnr_mean() + 10.0);
        assert_ne!(r1.verdict(), r2.verdict());
    }

    /// A single bad frame must be visible in the minimum even when the mean
    /// stays high — the reason both are reported.
    #[test]
    fn the_minimum_catches_what_the_mean_hides() {
        let a = smooth(64, 64, 3);
        let mut r = QualityReport::new();
        for i in 0..20u64 {
            let mut b = a.clone();
            b.add_uniform_noise(if i == 13 { 60 } else { 1 }, i as u64);
            r.push(i, &a, &b);
        }
        assert_eq!(r.psnr_min_frame, 13);
        assert_eq!(r.ssim_min_frame, 13);
        assert!(r.psnr_mean() > r.psnr_min + 10.0, "mean {} min {}", r.psnr_mean(), r.psnr_min);
    }

    #[test]
    fn structure_matters_more_than_offset() {
        // A flat brightness shift moves PSNR a lot but SSIM's luminance term
        // is normalised, so it barely moves: that difference is exactly why
        // both are reported.
        let a = smooth(64, 64, 4);
        let mut shifted = a.clone();
        shifted.add_scalar_block(0, 0, 64, 6);
        let mut scrambled = a.clone();
        scrambled.add_uniform_noise(6, 1);
        assert!((a.psnr(&shifted) - a.psnr(&scrambled)).abs() < 6.0, "comparable PSNR expected");
        assert!(ssim(&a, &shifted) > ssim(&a, &scrambled), "SSIM should prefer the structured error");
    }
}
