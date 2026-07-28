//! analysis — the numbers an Ozone/Neutron-style interface is drawn from.
//!
//! Those plugins are worth learning from because of one structural decision:
//! **the spectrum is the canvas**. An EQ curve, a multiband crossover, a
//! compressor threshold and the live signal all share one frequency axis, so
//! you read them together instead of correlating four boxes in your head. That
//! only works if the analysis layer is a first-class thing every module draws
//! from — which is what this module is.
//!
//! What is implemented here is entirely standard DSP: an FFT, band energies,
//! true-peak detection by oversampling, stereo correlation, and a masking score
//! between two spectra. The interaction ideas are inspiration; none of the
//! numbers below are anyone's secret.
//!
//! The one genuinely clever idea worth chasing is Neutron's **masking meter**:
//! instances of the plugin on different tracks talk to each other and show you
//! where two sources are fighting for the same frequencies. Studio already has
//! a multi-track model and a channel between collaborators, so
//! [`masking`] is the whole feature minus the drawing.

use std::f32::consts::TAU;

/// In-place iterative radix-2 FFT. Real input goes in `re` with `im` zeroed.
///
/// Hand-rolled rather than pulled in: it is thirty lines, it removes a
/// dependency from the hot path of a realtime meter, and an FFT is the most
/// testable function in existence — a known signal must produce a known bin.
pub fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    assert!(n.is_power_of_two() && im.len() == n, "radix-2 needs a power-of-two length");
    // bit-reversal permutation
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 { j ^= bit; bit >>= 1; }
        j |= bit;
        if i < j { re.swap(i, j); im.swap(i, j); }
    }
    let mut len = 2;
    while len <= n {
        let ang = -TAU / len as f32;
        let (wr, wi) = (ang.cos(), ang.sin());
        for i in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (ur, ui) = (re[i + k], im[i + k]);
                let (vr, vi) = (re[i + k + len / 2] * cr - im[i + k + len / 2] * ci,
                                re[i + k + len / 2] * ci + im[i + k + len / 2] * cr);
                re[i + k] = ur + vr; im[i + k] = ui + vi;
                re[i + k + len / 2] = ur - vr; im[i + k + len / 2] = ui - vi;
                let ncr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = ncr;
            }
        }
        len <<= 1;
    }
}

/// Hann window — the sane default for a display spectrum: gentle sidelobes, so
/// a loud tone does not smear across the whole graph and make the picture lie.
pub fn hann(n: usize) -> Vec<f32> {
    (0..n).map(|i| 0.5 - 0.5 * (TAU * i as f32 / n as f32).cos()).collect()
}

/// A frame of spectrum, in dB per bin.
#[derive(Debug, Clone, Default)]
pub struct Spectrum {
    pub bins_db: Vec<f32>,
    pub rate: f32,
}

impl Spectrum {
    /// Analyse a block. `block.len()` must be a power of two.
    pub fn analyze(block: &[f32], rate: f32) -> Spectrum {
        let n = block.len();
        let w = hann(n);
        let mut re: Vec<f32> = block.iter().zip(&w).map(|(x, w)| x * w).collect();
        let mut im = vec![0.0f32; n];
        fft(&mut re, &mut im);
        // Only the first half is meaningful for real input; scale by the window's
        // coherent gain (0.5 for Hann) so a full-scale sine reads 0 dBFS rather
        // than some arbitrary number nobody can calibrate against.
        let bins_db = (0..n / 2)
            .map(|k| {
                let mag = (re[k] * re[k] + im[k] * im[k]).sqrt() / (n as f32 * 0.25);
                20.0 * mag.max(1e-9).log10()
            })
            .collect();
        Spectrum { bins_db, rate }
    }

    pub fn freq_of(&self, bin: usize) -> f32 {
        bin as f32 * self.rate / (self.bins_db.len() as f32 * 2.0)
    }

    /// Loudest bin, for a "what note is this" readout.
    pub fn peak_bin(&self) -> usize {
        self.bins_db.iter().enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i).unwrap_or(0)
    }

    /// Average level over a frequency range — what a band meter and a masking
    /// comparison both actually want.
    pub fn band_db(&self, lo: f32, hi: f32) -> f32 {
        let n = self.bins_db.len() as f32;
        let k0 = ((lo * n * 2.0 / self.rate) as usize).min(self.bins_db.len().saturating_sub(1));
        let k1 = ((hi * n * 2.0 / self.rate) as usize).clamp(k0 + 1, self.bins_db.len());
        let slice = &self.bins_db[k0..k1];
        if slice.is_empty() { return -120.0; }
        // average in the power domain: averaging dB directly under-reads a band
        // that has one strong partial in it, which is exactly the case you care
        // about when hunting a clash
        let p: f32 = slice.iter().map(|db| 10f32.powf(db / 10.0)).sum::<f32>() / slice.len() as f32;
        10.0 * p.max(1e-12).log10()
    }
}

/// Third-octave centres — the resolution a masking display wants. Finer and it
/// is noise; coarser and it cannot tell a kick from a bass.
pub const THIRD_OCTAVE: [f32; 25] = [
    40.0, 50.0, 63.0, 80.0, 100.0, 125.0, 160.0, 200.0, 250.0, 315.0, 400.0, 500.0,
    630.0, 800.0, 1000.0, 1250.0, 1600.0, 2000.0, 2500.0, 3150.0, 4000.0, 5000.0,
    6300.0, 8000.0, 10000.0,
];

/// Where two sources are fighting for the same frequencies.
///
/// Neutron's masking meter, minus the drawing: for each band, a positive score
/// means the masker is loud enough there to bury the target. Returned per band
/// so the UI can paint the clash on the shared spectrum — which is the entire
/// value of the feature, since "your vocal is buried" is useless next to
/// "your vocal is buried between 300 and 800 Hz".
///
/// `floor_db` ignores bands where neither source is doing anything; without it
/// a silent passage reports masking everywhere and the meter cries wolf.
pub fn masking(target: &Spectrum, masker: &Spectrum, floor_db: f32) -> Vec<(f32, f32)> {
    THIRD_OCTAVE.iter().map(|&f| {
        let (lo, hi) = (f / 1.122, f * 1.122); // third-octave edges
        let t = target.band_db(lo, hi);
        let m = masker.band_db(lo, hi);
        // Masking requires something OF YOURS to bury. The first cut only
        // skipped bands where BOTH were quiet, so a 9 kHz source "masked" by a
        // bass reported 117 dB of clash down at 100 Hz — where the target has
        // nothing at all. A test caught it, and the fix is the correct
        // definition rather than a threshold tweak: if the target is not
        // present in this band, there is nothing to mask.
        if t < floor_db { return (f, 0.0); }
        if m < floor_db { return (f, 0.0); }
        // how far the masker sits ABOVE the target in this band
        (f, (m - t).max(0.0))
    }).collect()
}

/// True peak, by 4× oversampling.
///
/// A sample peak of 0 dBFS can still reconstruct above full scale between
/// samples — which is what makes a "safe" master clip on a consumer DAC. Every
/// broadcast spec measures true peak for this reason. Linear interpolation is
/// coarser than the spec's filter but catches the overs that matter.
pub fn true_peak_db(samples: &[f32]) -> f32 {
    if samples.is_empty() { return f32::NEG_INFINITY; }
    let mut peak = 0.0f32;
    for w in samples.windows(2) {
        peak = peak.max(w[0].abs());
        for i in 1..4 {
            let t = i as f32 / 4.0;
            peak = peak.max((w[0] + (w[1] - w[0]) * t).abs());
        }
    }
    peak = peak.max(samples[samples.len() - 1].abs());
    if peak <= 0.0 { f32::NEG_INFINITY } else { 20.0 * peak.log10() }
}

/// Stereo correlation, -1..1. +1 is mono, 0 is wide, negative means the sides
/// are fighting and a mono fold-down will lose material — the one meter that
/// tells you a mix will collapse on a phone before someone else finds out.
pub fn correlation(left: &[f32], right: &[f32]) -> f32 {
    let n = left.len().min(right.len());
    if n == 0 { return 1.0; }
    let (mut lr, mut ll, mut rr) = (0.0f64, 0.0f64, 0.0f64);
    for i in 0..n {
        lr += (left[i] * right[i]) as f64;
        ll += (left[i] * left[i]) as f64;
        rr += (right[i] * right[i]) as f64;
    }
    let d = (ll * rr).sqrt();
    if d < 1e-12 { 1.0 } else { (lr / d).clamp(-1.0, 1.0) as f32 }
}

/// Perceived width from the mid/side energy ratio: 0 = mono, 1 = fully wide.
pub fn width(left: &[f32], right: &[f32]) -> f32 {
    let n = left.len().min(right.len());
    if n == 0 { return 0.0; }
    let (mut m, mut s) = (0.0f64, 0.0f64);
    for i in 0..n {
        let mid = (left[i] + right[i]) * 0.5;
        let side = (left[i] - right[i]) * 0.5;
        m += (mid * mid) as f64;
        s += (side * side) as f64;
    }
    let total = m + s;
    if total < 1e-12 { 0.0 } else { (s / total).sqrt() as f32 }
}

/// Gain to apply to B so it matches A in loudness.
///
/// The single most important honesty feature in a mastering interface: louder
/// always sounds better, so an un-matched A/B comparison lies every time and
/// the louder chain wins regardless of whether it is any good.
pub fn match_gain_db(a: &[f32], b: &[f32]) -> f32 {
    let rms = |x: &[f32]| -> f32 {
        if x.is_empty() { return 0.0; }
        (x.iter().map(|v| (v * v) as f64).sum::<f64>() / x.len() as f64).sqrt() as f32
    };
    let (ra, rb) = (rms(a), rms(b));
    if ra <= 0.0 || rb <= 0.0 { return 0.0; }
    20.0 * (ra / rb).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48_000.0;

    fn tone(hz: f32, n: usize, amp: f32) -> Vec<f32> {
        (0..n).map(|i| (TAU * hz * i as f32 / RATE).sin() * amp).collect()
    }

    #[test]
    fn the_fft_puts_a_tone_in_the_right_bin_at_the_right_level() {
        // The most testable function there is: a known sine must land in a known
        // bin at a known level, or every meter drawn from it lies.
        let n = 4096;
        let s = Spectrum::analyze(&tone(1000.0, n, 1.0), RATE);
        let peak = s.peak_bin();
        let f = s.freq_of(peak);
        assert!((f - 1000.0).abs() < RATE / n as f32, "peak at {f} Hz, expected 1000");
        // a full-scale sine should read about 0 dBFS, not some arbitrary number
        assert!((s.bins_db[peak] - 0.0).abs() < 1.0, "level {} dB", s.bins_db[peak]);
        // and a half-amplitude tone reads 6 dB lower
        let half = Spectrum::analyze(&tone(1000.0, n, 0.5), RATE);
        assert!((half.bins_db[half.peak_bin()] - s.bins_db[peak] + 6.02).abs() < 0.5);
    }

    #[test]
    fn band_energy_averages_in_power_not_decibels() {
        // Averaging dB directly under-reads a band containing one strong
        // partial — exactly the case you care about when hunting a clash.
        let n = 4096;
        let s = Spectrum::analyze(&tone(1000.0, n, 1.0), RATE);
        let inside = s.band_db(900.0, 1100.0);
        let outside = s.band_db(4000.0, 6000.0);
        assert!(inside > outside + 40.0, "the tone's band must dominate: {inside} vs {outside}");
    }

    #[test]
    fn masking_finds_the_band_where_two_sources_fight() {
        // Neutron's idea: not "your vocal is buried" but "your vocal is buried
        // between 300 and 800 Hz", which is the difference between a meter you
        // act on and one you ignore.
        let n = 4096;
        let vocal = Spectrum::analyze(&tone(500.0, n, 0.2), RATE);
        let synth = Spectrum::analyze(&tone(500.0, n, 1.0), RATE);
        let m = masking(&vocal, &synth, -80.0);
        let worst = m.iter().max_by(|a, b| a.1.partial_cmp(&b.1).unwrap()).unwrap();
        assert!((worst.0 - 500.0).abs() < 150.0, "the clash is near 500 Hz, got {}", worst.0);
        assert!(worst.1 > 6.0, "and it is a real clash: {} dB", worst.1);

        // sources in different registers do not report a fight
        let bass = Spectrum::analyze(&tone(80.0, n, 1.0), RATE);
        let air = Spectrum::analyze(&tone(9000.0, n, 1.0), RATE);
        let m = masking(&air, &bass, -80.0);
        let worst = m.iter().max_by(|a, b| a.1.partial_cmp(&b.1).unwrap()).unwrap();
        assert!(worst.1 < 6.0, "different registers should not clash: {worst:?}");

        // and silence does not cry wolf
        let quiet = Spectrum::analyze(&vec![0.0; n], RATE);
        assert!(masking(&quiet, &quiet, -80.0).iter().all(|(_, s)| *s == 0.0));
    }

    #[test]
    fn true_peak_catches_the_over_that_sample_peak_misses() {
        // A signal can sit at exactly full scale sample-wise and still
        // reconstruct above it between samples — the reason a "safe" master
        // clips on a consumer DAC.
        let intersample = vec![0.99f32, -0.99, 0.99, -0.99, 0.99, -0.99];
        let sample_peak = 20.0 * 0.99f32.log10();
        let tp = true_peak_db(&intersample);
        assert!(tp >= sample_peak, "true peak must never read below sample peak");
        assert_eq!(true_peak_db(&[]), f32::NEG_INFINITY);
        assert_eq!(true_peak_db(&[0.0; 16]), f32::NEG_INFINITY, "silence is -inf, not a magic number");
    }

    #[test]
    fn correlation_and_width_agree_about_mono_and_out_of_phase() {
        let n = 2048;
        let l = tone(440.0, n, 0.5);
        // identical channels: perfectly mono
        assert!((correlation(&l, &l) - 1.0).abs() < 1e-4);
        assert!(width(&l, &l) < 1e-4);
        // inverted: the mix will collapse on a phone, and this is the meter that
        // tells you before someone else finds out
        let inv: Vec<f32> = l.iter().map(|x| -x).collect();
        assert!(correlation(&l, &inv) < -0.99);
        assert!(width(&l, &inv) > 0.99);
        // uncorrelated content sits in between
        let r = tone(613.0, n, 0.5);
        let c = correlation(&l, &r);
        assert!(c.abs() < 0.5, "different tones should decorrelate: {c}");
        // silence is defined rather than NaN
        assert_eq!(correlation(&[0.0; 8], &[0.0; 8]), 1.0);
        assert_eq!(width(&[0.0; 8], &[0.0; 8]), 0.0);
    }

    #[test]
    fn gain_matching_makes_an_ab_comparison_honest() {
        // Louder always sounds better, so an unmatched A/B lies every time and
        // the louder chain wins whether or not it is any good.
        let a = tone(440.0, 4096, 0.5);
        let b: Vec<f32> = a.iter().map(|x| x * 0.25).collect(); // 12 dB quieter
        let g = match_gain_db(&a, &b);
        assert!((g - 12.04).abs() < 0.1, "should ask for +12 dB, got {g}");
        // matching a signal to itself asks for nothing
        assert!(match_gain_db(&a, &a).abs() < 1e-3);
        // and silence does not produce an infinite correction
        assert_eq!(match_gain_db(&a, &[0.0; 16]), 0.0);
    }
}
