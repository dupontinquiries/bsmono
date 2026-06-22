// src/upmix.rs
//
// Virtual channel expansion: turn a 2.0 stereo source into more channels so
// it can fill a claimed surround layout. Pure DSP — no I/O — so it's
// testable and runs as a stage on the coordinator before the splitter.
//
// Two classic, cheap, good-sounding techniques, combined:
//
//   1. MATRIX DECODE (Dolby-Pro-Logic-style passive matrix):
//        Center   C  = (L + R) * 0.5            (in-phase content)
//        Surround S  = (L - R) * 0.5            (out-of-phase / ambience)
//      The "side" signal S is what makes reverb tails, crowd noise, and
//      stereo width fold naturally to the rears.
//
//   2. REAR DECORRELATION + HAAS DELAY:
//      Feeding raw S to both rears sounds narrow and combs against the
//      fronts. We delay the rears by ~15ms (Haas — perceived as spaciousness
//      not echo) and decorrelate L-rear vs R-rear with a short allpass so
//      the rear field is wide and diffuse.
//
// The result for a 5.1 claim from a stereo source:
//   FL = L                      FR = R
//   C  = (L+R)/2                LFE = lowpass(L+R)   (optional)
//   SL = decorr(delay(S))       SR = decorr'(delay(S))
//
// Honest notes:
//   - This is psychoacoustic upmixing, not "real" surround. It convincingly
//     widens ambience and pulls dialogue to center, but it cannot invent
//     discrete rear *events* that weren't encoded. For genuinely immersive
//     content, use real multichannel sources.
//   - LFE from a full-range sum can boom; the lowpass + level trim matters.
//   - All filters here are minimal and allocation-free per-sample so they
//     can run in a streaming splitter.

/// A one-pole lowpass for the LFE derivation.
pub struct OnePoleLP { a: f32, z: f32 }
impl OnePoleLP {
    pub fn new(cutoff_hz: f32, sr: f32) -> Self {
        // a = e^{-2*pi*fc/sr}.
        // Defended: if cutoff_hz or sr is non-finite or sr <= 0, the
        // exponent is NaN/Inf and `a` becomes NaN, poisoning every
        // sample. Coerce to safe defaults so the LFE path remains
        // audible if a config is malformed.
        let cutoff_hz = if cutoff_hz.is_finite() && cutoff_hz > 0.0 {
            cutoff_hz
        } else { 120.0 };
        let sr = if sr.is_finite() && sr > 0.0 { sr } else { 48_000.0 };
        let a = (-2.0 * std::f32::consts::PI * cutoff_hz / sr).exp();
        let a = if a.is_finite() { a.clamp(0.0, 0.9999) } else { 0.5 };
        Self { a, z: 0.0 }
    }
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let x = if x.is_finite() { x } else { 0.0 };
        self.z = x * (1.0 - self.a) + self.z * self.a;
        if !self.z.is_finite() { self.z = 0.0; }
        self.z
    }
}

/// A short fractional-free delay line (integer samples) for Haas.
pub struct Delay { buf: Vec<f32>, idx: usize }
impl Delay {
    pub fn new(samples: usize) -> Self {
        Self { buf: vec![0.0; samples.max(1)], idx: 0 }
    }
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let x = if x.is_finite() { x } else { 0.0 };
        let out = self.buf[self.idx];
        self.buf[self.idx] = x;
        self.idx = (self.idx + 1) % self.buf.len();
        if out.is_finite() { out } else { 0.0 }
    }
}

/// A first-order allpass for cheap decorrelation (phase scramble, flat mag).
pub struct Allpass { a: f32, zx: f32, zy: f32 }
impl Allpass {
    pub fn new(coeff: f32) -> Self {
        // Coefficient must satisfy |a| < 1 for stability; coerce
        // non-finite or out-of-range values to a benign default.
        let a = if coeff.is_finite() && coeff.abs() < 1.0 {
            coeff
        } else { 0.5 };
        Self { a, zx: 0.0, zy: 0.0 }
    }
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let x = if x.is_finite() { x } else { 0.0 };
        // y[n] = a*x[n] + x[n-1] - a*y[n-1]
        let y = self.a * x + self.zx - self.a * self.zy;
        self.zx = x;
        if y.is_finite() {
            self.zy = y;
            y
        } else {
            // State corruption — reset and emit silence.
            self.zx = 0.0;
            self.zy = 0.0;
            0.0
        }
    }
}

pub struct Upmixer {
    lfe_lp: OnePoleLP,
    sl_delay: Delay,
    sr_delay: Delay,
    sl_ap: Allpass,
    sr_ap: Allpass,
    pub lfe_enabled: bool,
}

/// One frame of 5.1 output, channel order FL FR C LFE SL SR.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Surround51 {
    pub fl: f32, pub fr: f32, pub c: f32,
    pub lfe: f32, pub sl: f32, pub sr: f32,
}

impl Upmixer {
    pub fn new(sample_rate: f32) -> Self {
        let haas_samples = (0.015 * sample_rate) as usize;          // 15ms
        let haas_samples_r = (0.019 * sample_rate) as usize;        // 19ms (asymmetric → wider)
        Self {
            lfe_lp: OnePoleLP::new(120.0, sample_rate),
            sl_delay: Delay::new(haas_samples),
            sr_delay: Delay::new(haas_samples_r),
            sl_ap: Allpass::new(0.6),
            sr_ap: Allpass::new(-0.6),   // opposite sign → decorrelated from SL
            lfe_enabled: true,
        }
    }

    /// Process one stereo sample pair into a 5.1 frame.
    ///
    /// Sanitizes inputs: a non-finite L or R would propagate to the
    /// front-left/right channels verbatim (the underlying filters
    /// would sanitize their own paths, but `fl = l` and `fr = r`
    /// bypass them and would leak NaN into the FL/FR speakers).
    /// Coerce to zero to keep the output finite end-to-end.
    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> Surround51 {
        let l = if l.is_finite() { l } else { 0.0 };
        let r = if r.is_finite() { r } else { 0.0 };
        let c = (l + r) * 0.5;
        let s = (l - r) * 0.5;                  // ambience / side
        let lfe = if self.lfe_enabled {
            self.lfe_lp.process((l + r) * 0.5) * 0.7
        } else { 0.0 };
        // rears: delayed + decorrelated side signal, level-trimmed
        let sl = self.sl_ap.process(self.sl_delay.process(s)) * 0.9;
        let sr = self.sr_ap.process(self.sr_delay.process(s)) * 0.9;
        Surround51 { fl: l, fr: r, c, lfe, sl, sr }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn center_is_mono_sum() {
        let mut up = Upmixer::new(48000.0);
        // identical L/R (pure mono) → all energy in center, side ~0
        let f = up.process(0.5, 0.5);
        assert!((f.c - 0.5).abs() < 1e-6);
        // first sample: rears come from delay line (still zero) → 0
        assert!(f.sl.abs() < 1e-6 && f.sr.abs() < 1e-6);
    }

    #[test]
    fn antiphase_goes_to_surround_not_center() {
        let mut up = Upmixer::new(48000.0);
        // L = +x, R = -x (fully out of phase) → center cancels, side is max
        let f = up.process(0.5, -0.5);
        assert!(f.c.abs() < 1e-6);            // center cancels
        // side energy exists (will emerge from the rears after the delay)
    }

    #[test]
    fn fronts_pass_through_unchanged() {
        let mut up = Upmixer::new(48000.0);
        let f = up.process(0.3, -0.7);
        assert_eq!(f.fl, 0.3);
        assert_eq!(f.fr, -0.7);
    }

    #[test]
    fn lfe_can_disable() {
        let mut up = Upmixer::new(48000.0);
        up.lfe_enabled = false;
        let f = up.process(0.9, 0.9);
        assert_eq!(f.lfe, 0.0);
    }

    #[test]
    fn rears_emerge_after_haas_delay() {
        let mut up = Upmixer::new(48000.0);
        // feed a step of antiphase content; after >19ms the rears should be nonzero
        let mut last = up.process(1.0, -1.0);
        for _ in 0..(0.025 * 48000.0) as usize {
            last = up.process(1.0, -1.0);
        }
        assert!(last.sl.abs() > 1e-3 || last.sr.abs() > 1e-3);
    }

    #[test]
    fn upmixer_defends_against_nan_inputs() {
        // A non-finite stereo sample previously leaked into FL/FR
        // directly (the front pass-through bypassed the filter
        // defenses). With input sanitize at the entry, the upmix
        // output is finite end-to-end regardless of input quality.
        let mut up = Upmixer::new(48000.0);
        for (l, r) in [
            (f32::NAN, 0.5),
            (0.5, f32::NAN),
            (f32::INFINITY, f32::NEG_INFINITY),
            (f32::NAN, f32::NAN),
        ] {
            let f = up.process(l, r);
            assert!(f.fl.is_finite(), "fl={} for input ({}, {})", f.fl, l, r);
            assert!(f.fr.is_finite(), "fr={} for input ({}, {})", f.fr, l, r);
            assert!(f.c.is_finite());
            assert!(f.lfe.is_finite());
            assert!(f.sl.is_finite());
            assert!(f.sr.is_finite());
        }
    }
}
