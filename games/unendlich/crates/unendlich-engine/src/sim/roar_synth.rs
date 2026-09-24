//! `sim::roar_synth` — render a dragon's roar from its genome, dry and mono.
//!
//! The parameters already existed and were only ever logged:
//! `roar_sample::roar` (pitch, length, growl, loudness, element from bulk /
//! neck / jaw / teeth) and `dragon_voice` (formant and ground resonance from
//! mass). This turns them into samples:
//!
//! * a harmonic stack on a rising-then-sagging pitch contour with a slow,
//!   seeded wander (no two roars of one dragon are sample-identical, but they
//!   are the same *voice*: the seed only moves the wander, never the timbre);
//! * brightness from the element sets how fast the partials fall off;
//! * growl = amplitude flutter at 22..48 Hz plus a subharmonic at f/2;
//! * breath noise, flavoured by element (Fire crackles, Air hisses, Water and
//!   the earthy elements are low and wet);
//! * two resonant formant filters at the voice's formant and 2.4× it;
//! * a sub-bass ground resonance under the sustain for the big ones;
//! * a soft clipper, then normalised to the roar's volume.
//!
//! No propagation here — distance, terrain echo and stereo are `land_echo` +
//! `sound_render`. Pure: same inputs, same samples. Allocation: one `Vec<f32>`.

use crate::sim::dragon_archetype::{self as arche, Genome};
use crate::sim::dragon_voice::{DragonVoice, VoiceElement};
use crate::sim::roar_sample;
use std::f32::consts::{PI, TAU};

/// Everything the synthesiser reads. Built from a genome, or by hand in tests
/// and the render tool.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoarVoice {
    pub pitch_hz: f32,
    pub duration_s: f32,
    /// 0..1 rasp.
    pub growl: f32,
    /// 0..1 output peak.
    pub volume: f32,
    /// Genome palette `0=Wood..5=Storm`.
    pub element: u8,
    pub formant_hz: f32,
    /// 0..1: partial roll-off (1 = bright, many strong harmonics).
    pub brightness: f32,
    pub sub_hz: f32,
    /// Mass the voice was computed for (kg), for logs.
    pub mass_kg: f32,
}

/// Genome palette → the voice module's attunements. The voice doc's six
/// (Fire/Storm/Air/Ice/Water/Earth) and the genome's six (Wood/Water/Air/
/// Dust/Fire/Storm) are different lists; Wood and Dust both read as Earth,
/// and nothing maps to Ice.
pub fn voice_element(genome_element: u8) -> VoiceElement {
    match genome_element % 6 {
        0 | 3 => VoiceElement::Earth,
        1 => VoiceElement::Water,
        2 => VoiceElement::Air,
        4 => VoiceElement::Fire,
        _ => VoiceElement::Storm,
    }
}

/// A dragon's mass (kg) from its size class: 50 kg for the smallest class,
/// cubic in the forge's linear size scale.
pub fn mass_kg(size_class: u8) -> f32 {
    let s = arche::size_scale(size_class) / arche::size_scale(0);
    50.0 * s * s * s
}

impl RoarVoice {
    pub fn from_genome(g: &Genome) -> RoarVoice {
        let r = roar_sample::roar(g);
        let a = arche::classify(g);
        let mass = mass_kg(a.size_class);
        let v = DragonVoice::from_genome(mass, voice_element(r.element), 20.0, 0.4);
        // roar_sample's pitch comes from bulk/neck alone; the size class pulls
        // it down further so a size-4 bruiser is not pitched like a lean size-0.
        let size_pull = (arche::size_scale(0) / arche::size_scale(a.size_class)).powf(0.35);
        RoarVoice {
            pitch_hz: (r.pitch_hz * size_pull).max(28.0),
            duration_s: r.duration_s * (1.0 + 0.12 * a.size_class as f32),
            growl: r.growl,
            volume: r.volume,
            element: r.element,
            formant_hz: v.formant_peak_hz,
            brightness: v.brightness,
            sub_hz: v.ground_resonance_hz,
            mass_kg: mass,
        }
    }
}

/// splitmix64 — the seeded wander and noise.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in [-1, 1).
    fn bi(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 23) as f32 - 1.0
    }
}

/// RBJ band-pass (constant 0 dB peak), direct form I.
#[derive(Clone, Copy)]
struct Bandpass {
    b0: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}
impl Bandpass {
    fn new(f: f32, q: f32, rate: f32) -> Bandpass {
        let w = TAU * (f / rate).clamp(1e-4, 0.49);
        let alpha = w.sin() / (2.0 * q);
        let a0 = 1.0 + alpha;
        Bandpass {
            b0: alpha / a0,
            b2: -alpha / a0,
            a1: -2.0 * w.cos() / a0,
            a2: (1.0 - alpha) / a0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }
    fn run(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b2 * self.x2 - self.a1 * self.y1 - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// One-pole low-pass coefficient for cutoff `hz`.
pub fn one_pole_coeff(hz: f32, rate: f32) -> f32 {
    1.0 - (-TAU * hz.max(1.0) / rate).exp()
}

/// Render the roar: mono f32 in [-1, 1], `duration_s × rate` samples.
pub fn render(v: &RoarVoice, rate: u32, seed: u64) -> Vec<f32> {
    let rate_f = rate.max(8000) as f32;
    let dur = if v.duration_s.is_finite() { v.duration_s.clamp(0.2, 6.0) } else { 1.0 };
    let n = (dur * rate_f) as usize;
    let mut out = Vec::with_capacity(n);
    let mut rng = Rng(seed ^ 0x5EED_0F_D2A6);

    let pitch = if v.pitch_hz.is_finite() { v.pitch_hz.clamp(20.0, 2000.0) } else { 120.0 };
    let bright = v.brightness.clamp(0.0, 1.0);
    let growl = v.growl.clamp(0.0, 1.0);
    // Partials up to ~7 kHz, at most 48.
    let n_harm = ((7000.0 / pitch) as usize).clamp(4, 48);
    // Roll-off exponent: dark voices ~1/k^2, bright ones ~1/k^0.9.
    let roll = 2.0 - 1.1 * bright;
    let amps: Vec<f32> = (1..=n_harm).map(|k| 1.0 / (k as f32).powf(roll)).collect();
    let amp_norm = 1.0 / amps.iter().sum::<f32>().max(1e-6);
    // One phase for the fundamental; every partial k is sin(k·θ), generated by
    // the Chebyshev recurrence sin((k+1)θ) = 2cosθ·sin(kθ) − sin((k−1)θ). Two
    // trig calls per sample instead of one per partial (48× fewer at the low
    // end) -- this was 24-60 ms per roar on x86 with per-partial sin(), which
    // is too much for an A53. Partials are phase-locked to the fundamental,
    // as a glottal-style source is anyway.
    let mut theta = rng.bi().abs() * TAU;
    let mut sub_phase = 0.0f32;
    let mut ground_phase = 0.0f32;
    let growl_rate = 22.0 + 26.0 * ((seed >> 7) & 0xff) as f32 / 255.0;

    // Element noise flavour: (amount, lowpass Hz, crackle impulses per second).
    let (noise_amt, noise_lp, crackle) = match v.element % 6 {
        4 => (0.30, 5000.0, 40.0), // Fire: crackling breath
        5 => (0.28, 6500.0, 18.0), // Storm: gritty
        2 => (0.38, 9000.0, 0.0),  // Air: hiss
        1 => (0.22, 1400.0, 0.0),  // Water: low, wet
        _ => (0.20, 1100.0, 0.0),  // Wood / Dust: earthy
    };
    let noise_k = one_pole_coeff(noise_lp, rate_f);
    let mut noise_lp_state = 0.0f32;

    let mut f1 = Bandpass::new(v.formant_hz.clamp(80.0, 4000.0), 4.0, rate_f);
    let mut f2 = Bandpass::new((v.formant_hz * 2.4).clamp(150.0, 9000.0), 5.0, rate_f);
    let body_k = one_pole_coeff(v.formant_hz.clamp(150.0, 3000.0) * 1.5, rate_f);
    let mut body_lp = 0.0f32;

    // Pitch wander: a random walk, smoothed.
    let mut wander = 0.0f32;
    let mut wander_target = 0.0f32;
    let wander_step = (rate_f * 0.08) as usize;

    for i in 0..n {
        let t = i as f32 / rate_f;
        let u = t / dur;
        // Envelope: fast swell (12%), hold, long release from 60%.
        let env = if u < 0.12 {
            let a = u / 0.12;
            a * a * (3.0 - 2.0 * a)
        } else if u < 0.6 {
            1.0
        } else {
            let r = (u - 0.6) / 0.4;
            (1.0 - r).max(0.0).powf(1.6)
        };
        if i % wander_step.max(1) == 0 {
            wander_target = rng.bi() * 0.035;
        }
        wander += (wander_target - wander) * 0.0008;
        // Contour: rises into the roar, sags at the end.
        let f0 = pitch * (1.0 + 0.22 * (PI * u).sin()) * (1.0 - 0.18 * u) * (1.0 + wander);

        theta = (theta + TAU * f0 / rate_f) % TAU;
        let (s1, c1) = theta.sin_cos();
        let two_c = 2.0 * c1;
        let k_max = ((rate_f * 0.45 / f0) as usize).min(n_harm);
        let (mut s_prev, mut s_k) = (0.0f32, s1);
        let mut voiced = 0.0f32;
        for a in amps.iter().take(k_max) {
            voiced += a * s_k;
            let s_next = two_c * s_k - s_prev;
            s_prev = s_k;
            s_k = s_next;
        }
        voiced *= amp_norm;
        // Subharmonic, weighted by growl.
        sub_phase = (sub_phase + TAU * 0.5 * f0 / rate_f) % TAU;
        voiced += growl * 0.35 * sub_phase.sin();
        // Growl flutter.
        let flutter = 1.0 - growl * 0.55 * (0.5 + 0.5 * (TAU * growl_rate * t).sin());
        voiced *= flutter;

        // Breath noise (+ crackle).
        let mut nz = rng.bi();
        if crackle > 0.0 && rng.bi().abs() < crackle / rate_f * 2.0 {
            nz += rng.bi() * 6.0;
        }
        noise_lp_state += (nz - noise_lp_state) * noise_k;
        let breath = noise_lp_state * noise_amt * (0.6 + 0.4 * u);

        let src = voiced + breath;
        body_lp += (src - body_lp) * body_k;
        let shaped = 0.55 * body_lp + 1.4 * f1.run(src) + 0.7 * f2.run(src);

        // Ground resonance under the sustain.
        ground_phase = (ground_phase + TAU * v.sub_hz.clamp(10.0, 80.0) / rate_f) % TAU;
        let ground = 0.25 * ground_phase.sin() * (1.0 - (u - 0.4).abs() * 2.0).max(0.0);

        out.push(((shaped + ground) * env * 1.6).tanh());
    }
    // Normalise to the roar's volume.
    let peak = out.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    if peak > 1e-6 {
        let g = v.volume.clamp(0.05, 1.0) * 0.95 / peak;
        for s in out.iter_mut() {
            *s *= g;
        }
    }
    out
}

/// Zero-crossing rate (crossings per second): a cheap brightness/pitch proxy
/// the tests and the render tool report.
pub fn zero_cross_rate(x: &[f32], rate: u32) -> f32 {
    let c = x.windows(2).filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0)).count();
    c as f32 * rate as f32 / x.len().max(1) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: u32 = 24_000;

    fn voice(pitch: f32) -> RoarVoice {
        RoarVoice {
            pitch_hz: pitch,
            duration_s: 1.2,
            growl: 0.4,
            volume: 0.8,
            element: 4,
            formant_hz: 700.0,
            brightness: 0.6,
            sub_hz: 25.0,
            mass_kg: 800.0,
        }
    }

    #[test]
    fn deterministic_and_bounded() {
        let v = voice(120.0);
        let a = render(&v, R, 42);
        assert_eq!(a, render(&v, R, 42));
        assert_eq!(a.len(), (1.2 * R as f32) as usize);
        let peak = a.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!((peak - 0.8 * 0.95).abs() < 1e-3, "peak {peak}");
        assert!(a.iter().all(|x| x.is_finite()));
    }

    #[test]
    fn the_seed_moves_the_wander_not_the_voice() {
        let v = voice(120.0);
        let a = render(&v, R, 1);
        let b = render(&v, R, 2);
        assert_ne!(a, b);
        let (za, zb) = (zero_cross_rate(&a, R), zero_cross_rate(&b, R));
        assert!((za - zb).abs() / za < 0.15, "{za} vs {zb}");
    }

    #[test]
    fn envelope_swells_and_dies() {
        let a = render(&voice(150.0), R, 3);
        let rms = |s: &[f32]| (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt();
        let n = a.len();
        let (head, mid, tail) = (rms(&a[..n / 50]), rms(&a[n / 3..n / 2]), rms(&a[n - n / 50..]));
        assert!(mid > head * 3.0 && mid > tail * 3.0, "head {head} mid {mid} tail {tail}");
    }

    #[test]
    fn bigger_dragons_roar_lower_and_longer() {
        let small = RoarVoice::from_genome(&Genome::from_seed_element(1, 4));
        let mut big_g = Genome::from_seed_element(1, 4);
        big_g.bulk = 1.0;
        big_g.neck_len = 1.0;
        big_g.jaw_depth = 1.0;
        let big = RoarVoice::from_genome(&big_g);
        assert!(big.pitch_hz < small.pitch_hz);
        assert!(big.duration_s > small.duration_s);
        assert!(big.mass_kg >= small.mass_kg);
        let zs = zero_cross_rate(&render(&small, R, 5), R);
        let zb = zero_cross_rate(&render(&big, R, 5), R);
        assert!(zb < zs, "big {zb} vs small {zs} crossings/s");
    }

    #[test]
    fn element_changes_the_sound() {
        let mut v = voice(110.0);
        v.element = 2; // Air: hiss
        let air = zero_cross_rate(&render(&v, R, 9), R);
        v.element = 1; // Water: low and wet
        let water = zero_cross_rate(&render(&v, R, 9), R);
        assert!(air > water * 1.2, "air {air} water {water}");
    }

    #[test]
    fn brighter_voices_carry_more_upper_partials() {
        let mut v = voice(110.0);
        v.growl = 0.0;
        v.element = 1; // quiet, dark breath so the partials dominate
        v.brightness = 0.95;
        let bright = zero_cross_rate(&render(&v, R, 4), R);
        v.brightness = 0.1;
        let dark = zero_cross_rate(&render(&v, R, 4), R);
        assert!(bright > dark * 1.15, "bright {bright} dark {dark}");
    }

    #[test]
    fn mass_grows_with_size_class() {
        let m: Vec<f32> = (0..=arche::SIZE_CLASS_MAX).map(mass_kg).collect();
        assert!((m[0] - 50.0).abs() < 1e-3);
        assert!(m.windows(2).all(|w| w[1] > w[0]));
    }

    #[test]
    fn degenerate_voice_does_not_panic() {
        let v = RoarVoice { pitch_hz: f32::NAN, duration_s: f32::INFINITY, formant_hz: 0.0, ..voice(1.0) };
        let a = render(&v, 8000, 0);
        assert!(!a.is_empty() && a.iter().all(|x| x.is_finite()));
    }
}
