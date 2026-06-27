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

/// One frame of quadraphonic (4.0) output: front pair + rear pair, no
/// center, no LFE. This is the classic quad layout and the natural target
/// for a four-speaker mesh fed a stereo source. The rears carry the
/// decorrelated, Haas-delayed difference (side) signal so the field opens
/// up behind the listener without the rears just echoing the fronts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quad40 {
    pub fl: f32, pub fr: f32, pub rl: f32, pub rr: f32,
}

/// One frame of quad + a derived sub: 4.1. The LFE is the low-passed mono
/// sum, for a mesh that has four satellites plus a sub-capable speaker.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quad41 {
    pub fl: f32, pub fr: f32, pub rl: f32, pub rr: f32, pub lfe: f32,
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

    /// Process one stereo pair into a quadraphonic (4.0) frame.
    ///
    /// Quad has no center: the front pair passes through unchanged (the
    /// mixing engineer's phantom center is preserved between FL/FR), and
    /// the rears carry the decorrelated, Haas-delayed side signal — the
    /// same rear derivation as the 5.1 path, so a stereo source gains a
    /// wide, diffuse rear field on a four-speaker mesh. No LFE here (see
    /// `process_quad41` for the sub-capable variant).
    ///
    /// Inputs are sanitized to keep the output finite even if the decoder
    /// hands us NaN/Inf (the front pass-through would otherwise leak it).
    #[inline]
    pub fn process_quad(&mut self, l: f32, r: f32) -> Quad40 {
        let l = if l.is_finite() { l } else { 0.0 };
        let r = if r.is_finite() { r } else { 0.0 };
        let s = (l - r) * 0.5;
        let rl = self.sl_ap.process(self.sl_delay.process(s)) * 0.9;
        let rr = self.sr_ap.process(self.sr_delay.process(s)) * 0.9;
        Quad40 { fl: l, fr: r, rl, rr }
    }

    /// Quad + derived LFE (4.1) for a mesh with a sub-capable speaker.
    /// Identical to `process_quad` plus the low-passed mono sum on LFE
    /// (respecting `lfe_enabled`). Sharing the rear filters with the other
    /// process methods is fine because a given Upmixer instance is only
    /// ever driven through ONE of them per stream.
    #[inline]
    pub fn process_quad41(&mut self, l: f32, r: f32) -> Quad41 {
        let q = self.process_quad(l, r);
        let l = if l.is_finite() { l } else { 0.0 };
        let r = if r.is_finite() { r } else { 0.0 };
        let lfe = if self.lfe_enabled {
            self.lfe_lp.process((l + r) * 0.5) * 0.7
        } else { 0.0 };
        Quad41 { fl: q.fl, fr: q.fr, rl: q.rl, rr: q.rr, lfe }
    }

    /// Derive a single LFE (sub) sample from a stereo pair, independent of
    /// the full upmix. Basic LFE synthesis for a mesh where one speaker is
    /// the sub: low-pass the mono sum (120 Hz one-pole) and trim. Honors
    /// `lfe_enabled`. Pure per-sample; call it alongside passing the
    /// fronts/rears through untouched.
    #[inline]
    pub fn lfe_sample(&mut self, l: f32, r: f32) -> f32 {
        if !self.lfe_enabled { return 0.0; }
        let l = if l.is_finite() { l } else { 0.0 };
        let r = if r.is_finite() { r } else { 0.0 };
        self.lfe_lp.process((l + r) * 0.5) * 0.7
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
    fn quad_fronts_pass_through_no_center() {
        let mut up = Upmixer::new(48000.0);
        let f = up.process_quad(0.3, -0.7);
        // Fronts are verbatim; there's no center channel in quad at all.
        assert_eq!(f.fl, 0.3);
        assert_eq!(f.fr, -0.7);
        // First sample: rears come from a still-empty delay line → 0.
        assert!(f.rl.abs() < 1e-6 && f.rr.abs() < 1e-6);
    }

    #[test]
    fn quad_rears_emerge_from_side_after_delay() {
        let mut up = Upmixer::new(48000.0);
        // Pure antiphase = max side content; after the Haas delay the
        // rears should carry it.
        let mut last = up.process_quad(1.0, -1.0);
        for _ in 0..(0.025 * 48000.0) as usize {
            last = up.process_quad(1.0, -1.0);
        }
        assert!(last.rl.abs() > 1e-3 || last.rr.abs() > 1e-3,
            "rears should carry the decorrelated side signal");
    }

    #[test]
    fn quad_mono_source_has_quiet_rears() {
        let mut up = Upmixer::new(48000.0);
        // Identical L/R = no side content = nothing to steer to the rears.
        let mut last = up.process_quad(0.5, 0.5);
        for _ in 0..2400 { last = up.process_quad(0.5, 0.5); }
        assert!(last.rl.abs() < 1e-4 && last.rr.abs() < 1e-4,
            "a mono source has no side signal, so rears stay quiet");
    }

    #[test]
    fn lfe_sample_is_low_passed_and_disables() {
        let mut up = Upmixer::new(48000.0);
        // A DC-ish (low freq) sum passes; settle the one-pole then check.
        let mut v = 0.0;
        for _ in 0..2000 { v = up.lfe_sample(1.0, 1.0); }
        assert!(v > 0.0, "low-frequency content should reach the LFE");
        // Disable → silent.
        up.lfe_enabled = false;
        assert_eq!(up.lfe_sample(1.0, 1.0), 0.0);
    }

    #[test]
    fn quad41_lfe_matches_enabled_flag() {
        let mut up = Upmixer::new(48000.0);
        let mut f = up.process_quad41(0.8, 0.8);
        for _ in 0..2000 { f = up.process_quad41(0.8, 0.8); }
        assert!(f.lfe.abs() > 0.0, "4.1 should derive a sub from the sum");
        up.lfe_enabled = false;
        let f2 = up.process_quad41(0.8, 0.8);
        assert_eq!(f2.lfe, 0.0);
    }

    #[test]
    fn quad_defends_against_nan_inputs() {
        let mut up = Upmixer::new(48000.0);
        for (l, r) in [(f32::NAN, 0.5), (0.5, f32::NAN), (f32::NAN, f32::NAN)] {
            let f = up.process_quad(l, r);
            assert!(f.fl.is_finite() && f.fr.is_finite());
            assert!(f.rl.is_finite() && f.rr.is_finite());
            let f41 = up.process_quad41(l, r);
            assert!(f41.lfe.is_finite());
        }
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
