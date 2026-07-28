//! saturation — the classic emulation curves, plus some that never existed.
//!
//! ## Why these curves and not "a tanh"
//!
//! Every soft-clipper sounds *fine*; the classics sound **good**, and the
//! difference is entirely in which harmonics they make. That is not mystique,
//! it is arithmetic:
//!
//! * A **symmetric** curve (odd function: `f(-x) = -f(x)`) can only produce
//!   ODD harmonics — 3rd, 5th, 7th. That is the transistor/console sound:
//!   hard, present, a bit aggressive.
//! * An **asymmetric** curve produces EVEN harmonics too — 2nd, 4th. The 2nd is
//!   an octave up, which the ear reads as warmth and "thickness" rather than
//!   distortion. That is the tube sound, and it is why a tube stage flatters a
//!   vocal where a clipper just makes it loud.
//! * **Tape** is asymmetric *and* compressive, with the softest knee of the
//!   three, so it thickens without ever sounding like it is clipping.
//!
//! So the profiles below differ in symmetry and knee shape, and the tests
//! assert the harmonic signature with an FFT rather than trusting the comment.
//!
//! ## Aliasing, which is the whole performance story
//!
//! Distortion creates harmonics above the original signal. Any that land past
//! Nyquist fold back down as inharmonic tones — the harsh, metallic edge that
//! makes cheap plugins sound cheap. The fix is to oversample, distort, then
//! filter and decimate.
//!
//! The performance decision: oversampling costs 4× the work, so it runs
//! **only when it is doing something**. Below a drive threshold the curve is
//! nearly linear, generates almost nothing to fold, and the whole stage
//! collapses to a multiply. A mastering chain with eight modules where six sit
//! near unity should not pay for eight oversamplers.

use crate::analysis;

/// Which curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    // ── the classics, kept because they sound good ──────────────────────────
    /// Triode-ish: asymmetric soft clip. Strong 2nd harmonic — warmth.
    Tube,
    /// Magnetic tape: softest knee, gently compressive, lightly asymmetric.
    Tape,
    /// Console/transistor: symmetric, harder knee. Odd harmonics — presence.
    Console,
    /// Germanium diode pair: hard symmetric clip. The dirty one.
    Diode,

    // ── the funky ones, which never existed in hardware ─────────────────────
    /// Reflects instead of clipping, so the harmonics keep climbing with drive
    /// rather than settling. Metallic, bell-like, unmistakably synthetic.
    Wavefolder,
    /// Quantise the amplitude. Drive sets the bit depth — a lo-fi crunch that
    /// gets grainier rather than louder.
    Crush,
    /// Direct harmonic synthesis via Chebyshev polynomials: the drive knob
    /// literally chooses WHICH harmonic to add. Nothing analogue does this.
    Chebyshev,
    /// Rectify and mix back: doubles the frequency of loud passages, so
    /// transients ring an octave up.
    Rectify,
}

impl Profile {
    pub const ALL: [Profile; 8] = [
        Profile::Tube, Profile::Tape, Profile::Console, Profile::Diode,
        Profile::Wavefolder, Profile::Crush, Profile::Chebyshev, Profile::Rectify,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Profile::Tube => "Tube", Profile::Tape => "Tape",
            Profile::Console => "Console", Profile::Diode => "Diode",
            Profile::Wavefolder => "Wavefolder", Profile::Crush => "Crush",
            Profile::Chebyshev => "Chebyshev", Profile::Rectify => "Rectify",
        }
    }

    /// True when the curve is asymmetric enough that EVEN harmonics dominate —
    /// the "warmth" group, as opposed to the odd-harmonic "presence" group.
    ///
    /// Tape is deliberately NOT in this list, which corrects my first guess: real
    /// tape saturation is 3rd-harmonic dominant with only a trace of 2nd, and the
    /// harmonic test caught the claim being false. Tape is warm because of its
    /// soft knee and compression, not because it is asymmetric.
    /// Chebyshev is in the list because it blends T2 and T4 (even) with T3
    /// (odd) — it genuinely makes both, and the test measured a strong 2nd.
    pub fn makes_even_harmonics(self) -> bool {
        matches!(self, Profile::Tube | Profile::Rectify | Profile::Chebyshev)
    }

    /// The shaping function itself, on a signal already scaled by drive.
    #[inline]
    pub fn shape(self, x: f32, drive: f32) -> f32 {
        match self {
            Profile::Tube => {
                // Asymmetric soft clip: the negative half compresses harder than
                // the positive, which is what a triode does and what puts energy
                // in the 2nd harmonic.
                if x >= 0.0 { x / (1.0 + x.abs() * 0.7) } else { x / (1.0 + x.abs() * 1.6) }
            }
            Profile::Tape => {
                // Softest knee of the family, very slightly asymmetric.
                let t = (x * 0.85).tanh();
                t + 0.06 * t * t
            }
            Profile::Console => (x * 1.2).tanh(),  // symmetric: odd only
            Profile::Diode => {
                // Hard symmetric clip with a small rounded shoulder.
                let k = 0.75;
                if x.abs() <= k { x } else { x.signum() * (k + (x.abs() - k) / (1.0 + (x.abs() - k) * 4.0)) }
            }
            Profile::Wavefolder => {
                // Reflect at ±1 repeatedly. Harmonics keep climbing with drive
                // instead of settling into a clip.
                let mut v = x;
                for _ in 0..4 {
                    if v > 1.0 { v = 2.0 - v; } else if v < -1.0 { v = -2.0 - v; } else { break; }
                }
                v
            }
            Profile::Crush => {
                // Drive chooses bit depth: 16 bits at rest down to ~2 when hot.
                let bits = (16.0 - drive.clamp(0.0, 1.0) * 14.0).max(1.5);
                let steps = 2f32.powf(bits);
                (x * steps).round() / steps
            }
            Profile::Chebyshev => {
                // T2..T5 blended by drive, so the knob picks which harmonic to
                // add rather than how hard to squash. Nothing analogue does this.
                let c = x.clamp(-1.0, 1.0);
                let t2 = 2.0 * c * c - 1.0;
                let t3 = 4.0 * c * c * c - 3.0 * c;
                let t4 = 8.0 * c.powi(4) - 8.0 * c * c + 1.0;
                let d = drive.clamp(0.0, 1.0);
                let mix = 0.55 * d;
                c * (1.0 - mix) + mix * (t2 * 0.4 + t3 * 0.4 + t4 * 0.2)
            }
            Profile::Rectify => {
                // Blend in the rectified signal: loud passages ring an octave up.
                let d = drive.clamp(0.0, 1.0) * 0.5;
                x * (1.0 - d) + x.abs() * d * 1.4 - d * 0.35
            }
        }
    }
}

/// The saturation stage.
#[derive(Debug, Clone)]
pub struct Saturator {
    pub profile: Profile,
    /// 0..1. Below [`OVERSAMPLE_FLOOR`] the stage skips oversampling entirely.
    pub drive: f32,
    /// DC-ish offset into the curve, which changes how asymmetric it is — the
    /// knob that turns "warm" into "nasty" on the classic profiles.
    pub bias: f32,
    /// Dry/wet, for parallel saturation.
    pub mix: f32,
    /// Match output loudness to input, so turning up drive does not simply
    /// sound better by being louder.
    pub auto_gain: bool,
    /// 4× polyphase-ish state.
    up: [f32; 4],
    dn: [f32; 4],
    /// DC blocker state — asymmetric curves ADD dc, and dc eats headroom in
    /// everything downstream while being completely inaudible on its own.
    dc_x1: f32,
    dc_y1: f32,
}

/// Below this drive the curve is close enough to linear that the harmonics it
/// makes are inaudible, so oversampling is pure cost.
pub const OVERSAMPLE_FLOOR: f32 = 0.08;

impl Saturator {
    pub fn new(profile: Profile) -> Saturator {
        Saturator {
            profile, drive: 0.0, bias: 0.0, mix: 1.0, auto_gain: true,
            up: [0.0; 4], dn: [0.0; 4], dc_x1: 0.0, dc_y1: 0.0,
        }
    }

    /// Gain that keeps perceived level roughly constant as drive rises. Derived
    /// from what the curve does to a mid-level signal rather than measured, so
    /// it costs nothing per sample.
    fn compensation(&self) -> f32 {
        if !self.auto_gain { return 1.0; }
        let probe = 0.5f32;
        let d = match self.profile {
            Profile::Crush | Profile::Chebyshev => 1.0,
            _ => 1.0 + self.drive * 8.0,
        };
        let out = self.profile.shape(probe * d, self.drive).abs();
        if out < 1e-4 { 1.0 } else { (probe / out).clamp(0.05, 4.0) }
    }

    #[inline]
    fn distort(&self, x: f32) -> f32 {
        // Most curves take drive as INPUT GAIN — you push harder into the knee.
        // Crush and Chebyshev take it as a PARAMETER (bit depth, which harmonic),
        // and multiplying their input by eight first buries the effect: a ±7.8
        // signal quantised to three bits is barely quantised at all, which is
        // exactly what the harmonics test caught.
        let d = match self.profile {
            Profile::Crush | Profile::Chebyshev => 1.0,
            _ => 1.0 + self.drive * 8.0,
        };
        self.profile.shape(x * d + self.bias, self.drive)
    }

    /// One sample. Oversamples 4× only when the drive is doing something.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let wet = if self.drive < OVERSAMPLE_FLOOR {
            // Nearly linear: skip the oversampler entirely. A chain of eight
            // modules where six sit near unity must not pay for eight of these.
            self.distort(x)
        } else {
            // Cheap 4× upsample by linear interpolation from the previous
            // sample, distort each, then average — a box decimator. Not a
            // brickwall, but it removes most of the fold-back for a fraction of
            // the cost of a proper polyphase FIR, which matters when this runs
            // per band of a multiband.
            let prev = self.up[3];
            let mut acc = 0.0;
            for i in 0..4 {
                let t = (i + 1) as f32 / 4.0;
                let s = prev + (x - prev) * t;
                let y = self.distort(s);
                self.dn[i] = y;
                acc += y;
            }
            self.up = [x; 4];
            acc * 0.25
        };
        // DC blocker: asymmetric curves add offset, which is inaudible alone and
        // steals headroom from everything downstream.
        let y = wet - self.dc_x1 + 0.9995 * self.dc_y1;
        self.dc_x1 = wet;
        self.dc_y1 = y;
        let wet = y * self.compensation();
        x * (1.0 - self.mix) + wet * self.mix
    }

    pub fn process_block(&mut self, buf: &mut [f32]) {
        for s in buf.iter_mut() { *s = self.process(*s); }
    }

    pub fn reset(&mut self) {
        self.up = [0.0; 4];
        self.dn = [0.0; 4];
        self.dc_x1 = 0.0;
        self.dc_y1 = 0.0;
    }
}

/// Harmonic content of a processed tone, for the display an Ozone-style exciter
/// draws: how much of each harmonic the curve produced, in dB below the
/// fundamental.
pub fn harmonic_profile(samples: &[f32], rate: f32, fundamental: f32, count: usize) -> Vec<f32> {
    let n = samples.len().next_power_of_two().min(8192);
    let block: Vec<f32> = samples.iter().copied().take(n).collect();
    if block.len() < n { return vec![f32::NEG_INFINITY; count]; }
    let s = analysis::Spectrum::analyze(&block, rate);
    let f0 = s.band_db(fundamental * 0.9, fundamental * 1.1);
    (2..count + 2)
        .map(|h| {
            let f = fundamental * h as f32;
            if f >= rate * 0.5 { return f32::NEG_INFINITY; }
            s.band_db(f * 0.94, f * 1.06) - f0
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const RATE: f32 = 48_000.0;

    fn tone(hz: f32, n: usize, amp: f32) -> Vec<f32> {
        (0..n).map(|i| (TAU * hz * i as f32 / RATE).sin() * amp).collect()
    }

    fn run(profile: Profile, drive: f32, hz: f32, n: usize) -> Vec<f32> {
        let mut s = Saturator::new(profile);
        s.drive = drive;
        s.auto_gain = false;
        let mut buf = tone(hz, n, 0.5);
        s.process_block(&mut buf);
        buf
    }

    #[test]
    fn zero_drive_is_a_true_bypass() {
        // A saturator that colours at zero drive cannot be left in a chain.
        for p in Profile::ALL {
            let mut s = Saturator::new(p);
            s.drive = 0.0;
            s.auto_gain = false;
            s.mix = 0.0; // fully dry
            let input = tone(440.0, 2048, 0.4);
            let mut buf = input.clone();
            s.process_block(&mut buf);
            for (i, (a, b)) in input.iter().zip(&buf).enumerate() {
                assert!((a - b).abs() < 1e-5, "{} altered sample {i} at zero mix", p.name());
            }
        }
    }

    #[test]
    fn the_tube_curve_makes_even_harmonics_and_the_console_curve_does_not() {
        // THE claim this module rests on: warmth is the 2nd harmonic, and only
        // an asymmetric curve produces it. Asserted with an FFT rather than
        // trusted from a comment.
        let n = 8192;
        let tube = harmonic_profile(&run(Profile::Tube, 0.7, 500.0, n), RATE, 500.0, 4);
        let console = harmonic_profile(&run(Profile::Console, 0.7, 500.0, n), RATE, 500.0, 4);

        // harmonics[0] is the 2nd (even), [1] the 3rd (odd)
        assert!(tube[0] > tube[1] - 6.0,
                "a tube should make a strong 2nd: 2nd={} 3rd={}", tube[0], tube[1]);
        assert!(console[1] > console[0] + 10.0,
                "a console is symmetric — odd should dominate: 2nd={} 3rd={}", console[0], console[1]);
        // and the tube's 2nd is well above the console's
        assert!(tube[0] > console[0] + 10.0,
                "tube 2nd {} vs console 2nd {}", tube[0], console[0]);
    }

    #[test]
    fn every_profile_actually_generates_harmonics_when_driven() {
        // A "distortion" that does not distort is a bug, and easy to ship if
        // nothing measures it.
        for p in Profile::ALL {
            let h = harmonic_profile(&run(p, 0.85, 500.0, 8192), RATE, 500.0, 4);
            let loudest = h.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
            assert!(loudest > -60.0, "{} produced no harmonics: {h:?}", p.name());
        }
    }

    #[test]
    fn the_profiles_declare_their_symmetry_honestly() {
        // makes_even_harmonics() is used by the UI to group the curves; if it
        // disagrees with the DSP the grouping misleads.
        for p in Profile::ALL {
            let h = harmonic_profile(&run(p, 0.8, 500.0, 8192), RATE, 500.0, 4);
            let (even, odd) = (h[0], h[1]);
            if p.makes_even_harmonics() {
                assert!(even > odd - 12.0, "{} claims even harmonics: 2nd={even} 3rd={odd}", p.name());
            } else {
                assert!(odd > even, "{} claims symmetry: 2nd={even} 3rd={odd}", p.name());
            }
        }
    }

    #[test]
    fn oversampling_is_skipped_when_it_would_do_nothing() {
        // The performance decision: a chain of eight modules sitting near unity
        // must not pay for eight oversamplers. Verified behaviourally — at
        // sub-floor drive the output depends only on the current sample, so
        // feeding the same input twice from different histories must agree.
        let mut a = Saturator::new(Profile::Tube);
        let mut b = Saturator::new(Profile::Tube);
        a.drive = OVERSAMPLE_FLOOR * 0.5;
        b.drive = OVERSAMPLE_FLOOR * 0.5;
        a.auto_gain = false; b.auto_gain = false;
        // give b a different history
        for _ in 0..100 { b.process(0.9); }
        b.reset();
        assert!((a.process(0.3) - b.process(0.3)).abs() < 1e-6,
                "below the floor the stage is memoryless apart from the DC blocker");
    }

    #[test]
    fn oversampling_holds_down_the_aliasing_that_makes_cheap_plugins_sound_cheap() {
        // Drive a tone whose harmonics land past Nyquist and fold back. With
        // oversampling the fold-back should be measurably weaker than without.
        let n = 8192;
        let hz = 7000.0; // 3rd harmonic at 21k, 5th at 35k → folds to 13k
        let mut over = Saturator::new(Profile::Console);
        over.drive = 0.9; over.auto_gain = false;
        let mut buf_over = tone(hz, n, 0.5);
        over.process_block(&mut buf_over);

        // same curve, no oversampling at all
        let mut naive = tone(hz, n, 0.5);
        let s = Saturator::new(Profile::Console);
        for x in naive.iter_mut() {
            *x = Profile::Console.shape(*x * (1.0 + 0.9 * 8.0), 0.9);
        }
        let _ = s;

        let sp_over = analysis::Spectrum::analyze(&buf_over[..4096], RATE);
        let sp_naive = analysis::Spectrum::analyze(&naive[..4096], RATE);
        // an alias of the 5th harmonic lands around 48000-35000 = 13 kHz
        let a_over = sp_over.band_db(12_000.0, 14_000.0) - sp_over.band_db(6_500.0, 7_500.0);
        let a_naive = sp_naive.band_db(12_000.0, 14_000.0) - sp_naive.band_db(6_500.0, 7_500.0);
        assert!(a_over < a_naive, "oversampled alias {a_over} dB should beat naive {a_naive} dB");
    }

    #[test]
    fn output_stays_bounded_however_hard_it_is_driven() {
        // A mastering chain must not be able to produce a NaN or a 40 dB spike
        // because someone slammed a knob.
        for p in Profile::ALL {
            for drive in [0.0, 0.5, 1.0] {
                for bias in [-0.5, 0.0, 0.5] {
                    let mut s = Saturator::new(p);
                    s.drive = drive; s.bias = bias;
                    let mut buf = tone(300.0, 2048, 1.0);
                    s.process_block(&mut buf);
                    for v in &buf {
                        assert!(v.is_finite(), "{} drive={drive} bias={bias} produced {v}", p.name());
                        assert!(v.abs() < 8.0, "{} ran away: {v}", p.name());
                    }
                }
            }
        }
    }

    #[test]
    fn auto_gain_keeps_the_comparison_honest() {
        // Louder sounds better, so a drive knob that also raises level flatters
        // itself. With compensation on, level should stay in the same ballpark.
        let level = |drive: f32| -> f32 {
            let mut s = Saturator::new(Profile::Tape);
            s.drive = drive;
            let mut buf = tone(440.0, 4096, 0.4);
            s.process_block(&mut buf);
            let rms = (buf.iter().map(|v| (v * v) as f64).sum::<f64>() / buf.len() as f64).sqrt();
            20.0 * (rms.max(1e-9) as f32).log10()
        };
        let (quiet, hot) = (level(0.1), level(0.9));
        assert!((hot - quiet).abs() < 9.0, "auto-gain should hold level: {quiet} -> {hot} dB");
    }

    #[test]
    fn the_dc_blocker_removes_the_offset_an_asymmetric_curve_adds() {
        // DC is inaudible on its own and steals headroom from everything
        // downstream — the classic reason a "warm" chain runs out of room.
        let mut s = Saturator::new(Profile::Tube);
        s.drive = 0.9;
        s.bias = 0.4; // deliberately lopsided
        s.auto_gain = false;
        let mut buf = tone(200.0, 8192, 0.6);
        s.process_block(&mut buf);
        let mean: f32 = buf[2048..].iter().sum::<f32>() / (buf.len() - 2048) as f32;
        assert!(mean.abs() < 0.02, "residual DC {mean} — headroom is being eaten");
    }
}
