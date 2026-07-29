//! multiband — splitting a signal into bands that add back up.
//!
//! ## The property that decides whether a multiband is any good
//!
//! Split a signal into bands, do nothing to them, add them back: you must get
//! the signal you started with. Sounds obvious; most naive implementations fail
//! it, and the failure is a broad dip or bump at every crossover that no amount
//! of knob-twiddling fixes because it is in the splitter, not the settings.
//!
//! **Linkwitz–Riley** crossovers are the standard answer: two cascaded
//! Butterworth sections per side, so each is −6 dB at the crossover and the two
//! halves sum to unity magnitude. A test asserts that reconstruction directly,
//! across the whole spectrum, rather than trusting the theory.
//!
//! ## The part that catches people out with three or more bands
//!
//! Splitting serially — low/high at f₁, then split the high at f₂ — leaves the
//! FIRST low band without the phase shift the second crossover imposed on
//! everything above it. The magnitudes are all still correct individually, and
//! the sum has a hole at f₁ anyway, because the bands no longer line up in
//! phase.
//!
//! The fix is to run each lower band through an **allpass** at every crossover
//! above it: unity magnitude, matching phase. That is why [`Splitter`] carries
//! allpass sections it never uses for filtering, and why the reconstruction test
//! covers three and four bands rather than only two — two-band is the case that
//! passes by accident.

use crate::{BandKind, Biquad, Compressor};

/// A Linkwitz–Riley 4th-order crossover: two cascaded Butterworth sections.
#[derive(Debug, Clone)]
pub struct Crossover {
    lp: [Biquad; 2],
    hp: [Biquad; 2],
    /// LR4 allpass, built as lp+hp of a matching pair — used to phase-align the
    /// bands below this crossover.
    ap_lp: [Biquad; 2],
    ap_hp: [Biquad; 2],
    pub freq: f32,
}

impl Crossover {
    pub fn new(rate: f32, freq: f32) -> Crossover {
        // Butterworth Q for a cascaded pair; two of these make LR4.
        let q = std::f32::consts::FRAC_1_SQRT_2;
        Crossover {
            lp: [Biquad::design(BandKind::LowPass, rate, freq, q, 0.0); 2],
            hp: [Biquad::design(BandKind::HighPass, rate, freq, q, 0.0); 2],
            ap_lp: [Biquad::design(BandKind::LowPass, rate, freq, q, 0.0); 2],
            ap_hp: [Biquad::design(BandKind::HighPass, rate, freq, q, 0.0); 2],
            freq,
        }
    }

    #[inline]
    pub fn split(&mut self, x: f32) -> (f32, f32) {
        let mut lo = x;
        for s in self.lp.iter_mut() { lo = s.process(lo); }
        let mut hi = x;
        for s in self.hp.iter_mut() { hi = s.process(hi); }
        // NO inversion. This is the order-dependent detail that catches people:
        // an LR2 crossover puts its two halves 180 degrees apart at fc and needs
        // one side flipped, but LR4 (two cascaded Butterworth sections, which is
        // what this is) brings them back IN PHASE — so flipping produces a deep
        // null exactly at the crossover instead of a flat sum. The first cut had
        // the LR2 rule here and the reconstruction test found the null at 1 kHz
        // immediately: -24.7 dB in, -42.9 dB out.
        (lo, hi)
    }

    /// Allpass: unity magnitude, the phase this crossover imposes. Applied to
    /// bands BELOW it so everything lines up when the bands are summed.
    #[inline]
    pub fn allpass(&mut self, x: f32) -> f32 {
        let mut lo = x;
        for s in self.ap_lp.iter_mut() { lo = s.process(lo); }
        let mut hi = x;
        for s in self.ap_hp.iter_mut() { hi = s.process(hi); }
        lo + hi
    }

    pub fn reset(&mut self) {
        for s in self.lp.iter_mut().chain(&mut self.hp)
            .chain(&mut self.ap_lp).chain(&mut self.ap_hp) { s.reset(); }
    }
}

/// Splits into N bands that reconstruct.
#[derive(Debug, Clone)]
pub struct Splitter {
    xs: Vec<Crossover>,
    /// Per-band allpass sections: `aps[b][j]` aligns band `b` with the
    /// crossover `b + 1 + j`.
    ///
    /// A separate instance per band, which is the subtlety: the first cut had
    /// every lower band calling the SAME crossover's allpass, so they shared one
    /// filter's state and interfered with each other. Two- and three-band still
    /// passed; four-band came out 1.7 dB off at 400 Hz, which is exactly the
    /// kind of small, plausible error that ships.
    aps: Vec<Vec<Crossover>>,
}

impl Splitter {
    /// `freqs` must be ascending. N frequencies give N+1 bands.
    pub fn new(rate: f32, freqs: &[f32]) -> Splitter {
        let xs: Vec<Crossover> = freqs.iter().map(|f| Crossover::new(rate, *f)).collect();
        let aps = (0..xs.len())
            .map(|b| freqs[b + 1..].iter().map(|f| Crossover::new(rate, *f)).collect())
            .collect();
        Splitter { xs, aps }
    }

    pub fn bands(&self) -> usize { self.xs.len() + 1 }

    /// Split one sample into `bands()` values, lowest first.
    pub fn split(&mut self, x: f32, out: &mut [f32]) {
        let n = self.bands();
        debug_assert!(out.len() >= n);
        let mut rest = x;
        for i in 0..self.xs.len() {
            let (lo, hi) = self.xs[i].split(rest);
            out[i] = lo;
            rest = hi;
        }
        out[n - 1] = rest;
        // Phase-align: every band gets the allpass of each crossover ABOVE it.
        // Without this the sum has a hole at the lower crossovers, which is the
        // bug that makes a three-band sound scooped with everything at unity.
        for band in 0..n.saturating_sub(1) {
            for ap in self.aps[band].iter_mut() {
                out[band] = ap.allpass(out[band]);
            }
        }
    }

    pub fn reset(&mut self) {
        for x in self.xs.iter_mut() { x.reset(); }
        for row in self.aps.iter_mut() { for a in row.iter_mut() { a.reset(); } }
    }
}

/// Per-band dynamics: the multiband compressor.
#[derive(Debug, Clone)]
pub struct Multiband {
    pub splitter: Splitter,
    pub bands: Vec<Band>,
    scratch: Vec<f32>,
}

#[derive(Debug, Clone)]
pub struct Band {
    pub comp: Compressor,
    pub gain_db: f32,
    pub muted: bool,
    pub soloed: bool,
    /// Bypassed bands still pass through the splitter, so soloing one band and
    /// bypassing another cannot change the crossover behaviour of the rest.
    pub bypass: bool,
}

impl Multiband {
    pub fn new(rate: f32, freqs: &[f32]) -> Multiband {
        let n = freqs.len() + 1;
        Multiband {
            splitter: Splitter::new(rate, freqs),
            bands: (0..n).map(|_| Band {
                comp: Compressor::new(rate, -18.0, 3.0, 10.0, 120.0),
                gain_db: 0.0, muted: false, soloed: false, bypass: true,
            }).collect(),
            scratch: vec![0.0; n],
        }
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let n = self.splitter.bands();
        self.splitter.split(x, &mut self.scratch);
        let any_solo = self.bands.iter().any(|b| b.soloed);
        let mut sum = 0.0;
        for i in 0..n {
            let b = &mut self.bands[i];
            let mut v = self.scratch[i];
            if !b.bypass { v = b.comp.process(v); }
            v *= crate::from_db_pub(b.gain_db);
            // Solo wins over mute, and silences everything not soloed — the
            // rule every mixer shares and every reimplementation inverts once.
            let audible = if any_solo { b.soloed } else { !b.muted };
            if audible { sum += v; }
        }
        sum
    }

    /// Gain reduction per band, for the meters an Ozone-style display draws
    /// under each band of the spectrum.
    pub fn reductions_db(&self) -> Vec<f32> {
        self.bands.iter()
            .map(|b| if b.bypass { 0.0 } else { b.comp.reduction_db() })
            .collect()
    }

    pub fn reset(&mut self) { self.splitter.reset(); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::Spectrum;
    use std::f32::consts::TAU;

    const RATE: f32 = 48_000.0;

    /// White-ish sweep: enough content everywhere to measure reconstruction
    /// across the whole spectrum rather than at one convenient frequency.
    fn sweep(n: usize) -> Vec<f32> {
        (0..n).map(|i| {
            let t = i as f32 / RATE;
            let f = 20.0 * (1000.0f32).powf(i as f32 / n as f32); // 20 Hz → 20 kHz
            (TAU * f * t).sin() * 0.4
        }).collect()
    }

    fn tone(hz: f32, n: usize, amp: f32) -> Vec<f32> {
        (0..n).map(|i| (TAU * hz * i as f32 / RATE).sin() * amp).collect()
    }

    #[test]
    fn bands_sum_back_to_the_signal_they_came_from() {
        // THE property. A splitter that fails this has a dip or bump at every
        // crossover that no amount of knob-twiddling can fix, because it is in
        // the splitter rather than the settings.
        //
        // Measured with STEADY tones rather than a sweep. The first version
        // swept 20 Hz to 20 kHz and compared a window of the output against the
        // same window of the input — but the filters have group delay, so those
        // two windows hold different parts of the sweep, and a four-band chain
        // read 1.8 dB "wrong" from measurement artefact alone. A steady tone
        // that has settled has no such ambiguity.
        //
        // Two bands is the case that passes by ACCIDENT (nothing needs phase
        // alignment yet), so three and four are the ones that matter.
        for freqs in [vec![1000.0f32], vec![200.0, 2000.0], vec![120.0, 800.0, 5000.0]] {
            let mut s = Splitter::new(RATE, &freqs);
            let mut scratch = vec![0.0; s.bands()];
            for f in [60.0f32, 150.0, 400.0, 1000.0, 3000.0, 8000.0, 14_000.0] {
                s.reset();
                let input = tone(f, 12_288, 0.5);
                let summed: Vec<f32> = input.iter().map(|x| {
                    s.split(*x, &mut scratch);
                    scratch.iter().sum::<f32>()
                }).collect();
                // skip the settling transient, then compare RMS
                let rms = |v: &[f32]| (v[8192..].iter().map(|x| (x * x) as f64).sum::<f64>()
                                       / (v.len() - 8192) as f64).sqrt();
                let db = 20.0 * (rms(&summed) / rms(&input)).max(1e-9).log10();
                assert!(db.abs() < 0.5,
                        "{} bands at {f} Hz reconstructed {db:+.2} dB off unity",
                        freqs.len() + 1);
            }
        }
    }

    #[test]
    fn each_band_actually_contains_its_own_range() {
        // A splitter that reconstructs but does not separate is just a delay.
        let mut s = Splitter::new(RATE, &[200.0, 2000.0]);
        let mut low = Vec::new();
        let mut mid = Vec::new();
        let mut high = Vec::new();
        let mut scratch = vec![0.0; 3];
        for x in tone(1000.0, 8192, 0.5) {
            s.split(x, &mut scratch);
            low.push(scratch[0]); mid.push(scratch[1]); high.push(scratch[2]);
        }
        let e = |v: &[f32]| -> f32 {
            20.0 * (v[4096..].iter().map(|x| (x * x) as f64).sum::<f64>()
                    / 4096.0).sqrt().max(1e-9).log10() as f32
        };
        let (el, em, eh) = (e(&low), e(&mid), e(&high));
        assert!(em > el + 20.0, "a 1 kHz tone belongs to the mid band: mid={em} low={el}");
        assert!(em > eh + 20.0, "and not the high band: mid={em} high={eh}");
    }

    #[test]
    fn a_bypassed_multiband_is_transparent() {
        // Every band bypassed and at unity must be indistinguishable from wire,
        // or the module cannot be left in a chain.
        let mut m = Multiband::new(RATE, &[250.0, 3000.0]);
        for f in [80.0f32, 500.0, 4000.0, 12_000.0] {
            m.reset();
            let input = tone(f, 12_288, 0.4);
            let out: Vec<f32> = input.iter().map(|x| m.process(*x)).collect();
            let rms = |v: &[f32]| (v[8192..].iter().map(|x| (x * x) as f64).sum::<f64>()
                                   / (v.len() - 8192) as f64).sqrt();
            let db = 20.0 * (rms(&out) / rms(&input)).max(1e-9).log10();
            assert!(db.abs() < 0.5, "bypassed at {f} Hz: {db:+.2} dB");
        }
    }

    #[test]
    fn compressing_one_band_leaves_the_others_alone() {
        // The reason to have a multiband at all: tame the low end without
        // dulling the top.
        let mut m = Multiband::new(RATE, &[250.0, 3000.0]);
        m.bands[0].bypass = false;
        m.bands[0].comp = Compressor::new(RATE, -30.0, 20.0, 1.0, 50.0);

        // a loud low tone plus a quiet high one
        let n = 16_384;
        let mixed: Vec<f32> = tone(100.0, n, 0.8).iter()
            .zip(tone(8000.0, n, 0.05).iter())
            .map(|(a, b)| a + b).collect();
        let out: Vec<f32> = mixed.iter().map(|x| m.process(*x)).collect();

        let a = Spectrum::analyze(&mixed[8192..12_288], RATE);
        let b = Spectrum::analyze(&out[8192..12_288], RATE);
        let low_in = a.band_db(90.0, 110.0);
        let low_out = b.band_db(90.0, 110.0);
        let high_in = a.band_db(7500.0, 8500.0);
        let high_out = b.band_db(7500.0, 8500.0);
        assert!(low_out < low_in - 6.0, "the low band should be squashed: {low_in} → {low_out}");
        assert!((high_out - high_in).abs() < 2.0,
                "the high band must be untouched: {high_in} → {high_out}");
        // and the meter reports it
        let r = m.reductions_db();
        assert!(r[0] < -3.0, "band 0 reduction {}", r[0]);
        assert_eq!(r[2], 0.0, "a bypassed band reports no reduction");
    }

    #[test]
    fn solo_beats_mute_and_silences_everything_else() {
        // The rule every mixer shares and every reimplementation inverts once.
        let mut m = Multiband::new(RATE, &[250.0, 3000.0]);
        let input = tone(1000.0, 4096, 0.5);

        m.bands[1].soloed = true;
        m.bands[0].muted = false;
        let out: Vec<f32> = input.iter().map(|x| m.process(*x)).collect();
        let e: f32 = out[2048..].iter().map(|x| x.abs()).sum();
        assert!(e > 1.0, "the soloed band containing the tone is heard");

        m.reset();
        m.bands[1].soloed = false;
        m.bands[0].soloed = true; // solo a band with nothing in it
        let out: Vec<f32> = input.iter().map(|x| m.process(*x)).collect();
        let e2: f32 = out[2048..].iter().map(|x| x.abs()).sum();
        assert!(e2 < e * 0.1, "soloing an empty band silences the rest: {e2} vs {e}");

        // a muted band with no solo anywhere is simply gone
        m.reset();
        m.bands[0].soloed = false;
        m.bands[1].muted = true;
        let out: Vec<f32> = input.iter().map(|x| m.process(*x)).collect();
        let e3: f32 = out[2048..].iter().map(|x| x.abs()).sum();
        assert!(e3 < e * 0.1, "muting the band holding the tone removes it");
    }

    #[test]
    fn band_gain_is_applied_in_decibels() {
        let mut m = Multiband::new(RATE, &[250.0, 3000.0]);
        let input = tone(1000.0, 8192, 0.4);
        let flat: Vec<f32> = input.iter().map(|x| m.process(*x)).collect();
        m.reset();
        m.bands[1].gain_db = -6.0;
        let cut: Vec<f32> = input.iter().map(|x| m.process(*x)).collect();
        let e = |v: &[f32]| (v[4096..].iter().map(|x| (x * x) as f64).sum::<f64>() / 4096.0).sqrt();
        let ratio = 20.0 * (e(&cut) / e(&flat)).max(1e-9).log10();
        assert!((ratio + 6.0).abs() < 0.6, "-6 dB on the band should be -6 dB out: {ratio}");
    }
}
