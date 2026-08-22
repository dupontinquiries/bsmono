//! adaptive_sat — saturation that listens before it decides.
//!
//! A fixed saturator applies one character to everything that reaches it,
//! which is exactly wrong for the two signals a trap record is made of. An
//! 808 wants tape: soft knee, second harmonic, compressive. The same setting
//! on a vocal turns every "s" into a spitting mess, because sibilance is
//! already the harshest thing in the take and a saturator's job is to add
//! harmonics on top of it. Producers solve this by hand — de-ess before the
//! drive, ride the drive against the register, print two passes. This stage
//! does it continuously from what the audio is doing.
//!
//! ## Two detectors, deliberately independent
//!
//! * **Register** — the fundamental, from the same coarse+refine detector
//!   [`crate::realtune`] uses, mapped to a 0..1 position between
//!   [`AdaptiveSat::low_hz`] and [`AdaptiveSat::high_hz`]. Low notes get
//!   more drive and a softer curve (an 808 can take it and wants it), high
//!   notes get less (a lead is already bright).
//! * **Sibilance** — the ratio of high-band energy to total, from two
//!   one-pole filters. Cheap, no pitch required, and it responds in
//!   milliseconds, which is what an "s" needs.
//!
//! They are independent because they answer different questions and a take
//! can be any combination of the two: a low sustained note with no
//! sibilance, a high note full of it. Either can be set to zero to disable
//! it, which is how you get plain register-following drive or plain
//! de-essing without touching the other.
//!
//! ## …and optionally tied
//!
//! [`Link::Tied`] makes the sibilance response scale with register, so the
//! stage backs off hardest where sibilance is actually painful — up top —
//! and leaves the low end alone even when a kick's transient briefly reads
//! as broadband. That is one gesture ("be careful up there") instead of two
//! knobs a person has to keep consistent, and it is a mode rather than the
//! default because tying them is wrong for material where the sibilance is
//! in the low register (a distorted bass with fret noise, for instance).
//!
//! ## What it does NOT do
//!
//! It does not switch [`crate::saturation::Profile`] mid-note. Profiles have
//! different harmonic signatures and different DC behaviour, so crossfading
//! between them per-sample would smear the very character the stage exists
//! to choose; instead the chosen profile is fixed and the *amount* and
//! *asymmetry* move, which is the part of "character" that can move
//! continuously without artefacts.

use crate::saturation::{Profile, Saturator};

/// How the two detectors relate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    /// Each acts on its own. The default: they measure different things.
    Independent,
    /// Sibilance response scales with register — careful up top, relaxed
    /// down low.
    Tied,
}

/// A one-pole, used for both band splitting and envelope following.
#[derive(Clone, Copy)]
struct Pole { a: f32, y: f32 }
impl Pole {
    fn new(fc: f32, rate: f32) -> Pole {
        let x = (-std::f32::consts::TAU * fc / rate).exp();
        Pole { a: 1.0 - x, y: 0.0 }
    }
    fn lp(&mut self, x: f32) -> f32 { self.y += self.a * (x - self.y); self.y }
}

pub struct AdaptiveSat {
    pub sat: Saturator,
    /// Drive with no modulation applied — what the detectors move around.
    pub base_drive: f32,
    /// 0..1. How much the register moves drive. 0 disables register
    /// following entirely.
    pub note_amount: f32,
    /// 0..1. How much sibilance pulls drive DOWN. 0 disables de-essing.
    pub sibilance_amount: f32,
    pub link: Link,
    /// The register window. A fundamental at or below `low_hz` reads as
    /// fully "low"; at or above `high_hz`, fully "high".
    pub low_hz: f32,
    pub high_hz: f32,
    /// Sibilance band split.
    pub sib_hz: f32,

    sample_rate: f32,
    channels: usize,
    phase: usize,
    // sibilance detection (channel 0)
    band: Pole,
    hi_env: Pole,
    all_env: Pole,
    sib: f32,
    // register detection (channel 0), shared with the tuner's approach
    ring: Vec<f32>,
    ring_write: usize,
    frame: Vec<f32>,
    coarse: Vec<f32>,
    hop: usize,
    hop_countdown: usize,
    /// 0 = low register, 1 = high. Smoothed; unvoiced frames hold it.
    register: f32,
    /// Per-channel saturators: one instance's curve state must not see the
    /// other channel's samples (the interleave convention `playrate` set).
    sats: Vec<Saturator>,
}

impl AdaptiveSat {
    pub fn new(profile: Profile, sample_rate: f32, channels: usize) -> AdaptiveSat {
        let channels = channels.max(1);
        let ring_len = (sample_rate / 70.0 * 3.0) as usize;
        let mut sat = Saturator::new(profile);
        sat.auto_gain = true;
        AdaptiveSat {
            sats: (0..channels).map(|_| {
                let mut s = Saturator::new(profile);
                s.auto_gain = true;
                s
            }).collect(),
            sat,
            base_drive: 0.35,
            note_amount: 0.5,
            sibilance_amount: 0.5,
            link: Link::Independent,
            low_hz: 60.0,
            high_hz: 600.0,
            sib_hz: 4500.0,
            sample_rate,
            channels,
            phase: 0,
            band: Pole::new(4500.0, sample_rate),
            hi_env: Pole::new(25.0, sample_rate),
            all_env: Pole::new(25.0, sample_rate),
            sib: 0.0,
            ring: vec![0.0; ring_len],
            ring_write: 0,
            frame: vec![0.0; ring_len],
            coarse: vec![0.0; ring_len / 4 + 1],
            hop: (sample_rate * 0.01) as usize,
            hop_countdown: 1,
            register: 0.5,
        }
    }

    /// 0..1 sibilance and 0..1 register, for a meter or a test — the two
    /// numbers that decide everything this stage does, exposed rather than
    /// hidden, because an adaptive effect whose decisions you cannot see is
    /// indistinguishable from a broken one.
    pub fn detectors(&self) -> (f32, f32) { (self.sib, self.register) }

    /// The drive the detectors are currently asking for.
    pub fn effective_drive(&self) -> f32 {
        // Register: low notes get up to +note_amount of headroom-relative
        // drive, high notes get that much less.
        let tilt = 1.0 - 2.0 * self.register; // +1 low, -1 high
        let note_gain = 1.0 + self.note_amount * tilt;
        // Sibilance: always a reduction. Tied scales it by how high the
        // material sits, so an "s" up top is caught hard and a low-register
        // transient is left alone.
        let weight = match self.link {
            Link::Independent => 1.0,
            Link::Tied => self.register,
        };
        let sib_cut = 1.0 - self.sibilance_amount * weight * self.sib;
        (self.base_drive * note_gain * sib_cut).clamp(0.0, 1.0)
    }

    pub fn process(&mut self, x: f32) -> f32 {
        let ch = self.phase;
        self.phase = (self.phase + 1) % self.channels;
        if ch == 0 {
            self.detect(x);
        }
        let drive = self.effective_drive();
        let s = &mut self.sats[ch];
        s.drive = drive;
        s.bias = self.sat.bias;
        s.mix = self.sat.mix;
        s.process(x)
    }

    fn detect(&mut self, x: f32) {
        // Sibilance: high-band energy over total, both envelope-followed.
        // A ratio rather than an absolute level, so it does not fire on a
        // loud passage that merely happens to be bright overall.
        let low = self.band.lp(x);
        let high = x - low;
        let h = self.hi_env.lp(high.abs());
        let a = self.all_env.lp(x.abs());
        self.sib = if a > 1e-6 { (h / a).clamp(0.0, 1.0) } else { 0.0 };

        self.ring[self.ring_write] = x;
        self.ring_write = (self.ring_write + 1) % self.ring.len();
        self.hop_countdown -= 1;
        if self.hop_countdown > 0 { return; }
        self.hop_countdown = self.hop.max(1);

        let split = self.ring.len() - self.ring_write;
        self.frame[..split].copy_from_slice(&self.ring[self.ring_write..]);
        self.frame[split..].copy_from_slice(&self.ring[..self.ring_write]);
        let dec_len = self.frame.len() / 4;
        for i in 0..dec_len {
            let b = i * 4;
            self.coarse[i] = (self.frame[b] + self.frame[b + 1]
                            + self.frame[b + 2] + self.frame[b + 3]) * 0.25;
        }
        let dec_rate = self.sample_rate / 4.0;
        let Some(p) = crate::retune::yin_pitch(
            &self.coarse[..dec_len], dec_rate, 40.0, 1000.0f32.min(dec_rate * 0.4), 0.2)
            .filter(|p| p.clarity >= 0.2)
        else {
            // Unvoiced: HOLD the register. A drum hit between two sung notes
            // must not swing the character to whatever noise reads as.
            return;
        };
        let target = ((p.hz.log2() - self.low_hz.log2())
            / (self.high_hz.log2() - self.low_hz.log2())).clamp(0.0, 1.0);
        // Smooth across hops: register is a property of the passage, not of
        // one 10 ms window, and a drive that jumps per hop is a tremolo.
        self.register += (target - self.register) * 0.25;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f32, secs: f32, rate: f32) -> Vec<f32> {
        let n = (rate * secs) as usize;
        (0..n).map(|i| (i as f32 / rate * hz * std::f32::consts::TAU).sin() * 0.4).collect()
    }

    /// Harmonic content above the fundamental — how much the stage coloured
    /// the signal, which is what "character" means here.
    fn harmonic_energy(buf: &[f32], rate: f32, fundamental: f32) -> f32 {
        let n = buf.len();
        let mut total = 0.0f32;
        for k in 2..=6 {
            let f = fundamental * k as f32;
            if f > rate * 0.45 { break; }
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for (i, x) in buf.iter().enumerate() {
                let t = std::f32::consts::TAU * f * i as f32 / rate;
                re += x * t.cos();
                im += x * t.sin();
            }
            total += (re * re + im * im).sqrt() / n as f32;
        }
        total
    }

    fn run(s: &mut AdaptiveSat, input: &[f32]) -> Vec<f32> {
        input.iter().map(|x| s.process(*x)).collect()
    }

    #[test]
    fn a_low_note_is_driven_harder_than_a_high_one() {
        // The 808-versus-lead decision, made by the stage instead of by
        // hand: same settings, different register, different character.
        let rate = 48_000.0;
        let mut low = AdaptiveSat::new(Profile::Tape, rate, 1);
        let mut high = AdaptiveSat::new(Profile::Tape, rate, 1);
        run(&mut low, &sine(55.0, 0.5, rate));
        run(&mut high, &sine(660.0, 0.5, rate));
        let (_, low_reg) = low.detectors();
        let (_, high_reg) = high.detectors();
        assert!(low_reg < 0.2, "55 Hz reads as low register ({low_reg:.2})");
        assert!(high_reg > 0.8, "660 Hz reads as high register ({high_reg:.2})");
        assert!(low.effective_drive() > high.effective_drive() * 1.5,
                "low {:.3} vs high {:.3}", low.effective_drive(), high.effective_drive());
    }

    #[test]
    fn sibilance_pulls_the_drive_down() {
        let rate = 48_000.0;
        let mut s = AdaptiveSat::new(Profile::Tube, rate, 1);
        run(&mut s, &sine(200.0, 0.3, rate));
        let voiced_drive = s.effective_drive();
        // An "s": high-band noise, no fundamental.
        let mut rng = core_kit::rng::SplitMix64::new(3);
        let hiss: Vec<f32> = (0..14_400).map(|_| (rng.next_f64() as f32) * 2.0 - 1.0)
            .collect();
        // Pre-emphasise so the energy really is up top, as sibilance is.
        let mut prev = 0.0;
        let hiss: Vec<f32> = hiss.iter().map(|x| { let y = x - prev * 0.9; prev = *x; y * 0.4 }).collect();
        run(&mut s, &hiss);
        let (sib, _) = s.detectors();
        assert!(sib > 0.5, "the hiss reads as sibilant ({sib:.2})");
        assert!(s.effective_drive() < voiced_drive * 0.8,
                "drive backed off: {:.3} from {:.3}", s.effective_drive(), voiced_drive);
    }

    #[test]
    fn either_detector_can_be_switched_off_without_the_other() {
        // Independence is the whole claim of the default mode.
        let rate = 48_000.0;
        let mut only_notes = AdaptiveSat::new(Profile::Tape, rate, 1);
        only_notes.sibilance_amount = 0.0;
        let mut only_sib = AdaptiveSat::new(Profile::Tape, rate, 1);
        only_sib.note_amount = 0.0;
        let low = sine(55.0, 0.4, rate);
        run(&mut only_notes, &low);
        run(&mut only_sib, &low);
        assert!(only_notes.effective_drive() > only_sib.effective_drive(),
                "register moves one and not the other");
        // Near the base, not exactly it: a real high-band detector reads a
        // percent or two even on a pure low sine (the band split is a
        // one-pole, not a brick wall), and demanding exactness here would
        // be asserting an idealised filter rather than the one that ships.
        assert!((only_sib.effective_drive() - only_sib.base_drive).abs()
                    < only_sib.base_drive * 0.05,
                "with note_amount 0, drive stays at the base: {:.4} vs {:.4}",
                only_sib.effective_drive(), only_sib.base_drive);
    }

    #[test]
    fn tying_them_makes_de_essing_register_dependent() {
        // Tied is 'be careful up top': the same sibilance reading must cut
        // more on a high passage than on a low one, while Independent cuts
        // the same either way.
        let rate = 48_000.0;
        let mut indep_low = AdaptiveSat::new(Profile::Tube, rate, 1);
        let mut tied_low = AdaptiveSat::new(Profile::Tube, rate, 1);
        tied_low.link = Link::Tied;
        for s in [&mut indep_low, &mut tied_low] {
            s.note_amount = 0.0; // isolate the sibilance path
            s.register = 0.1;    // a low passage
            s.sib = 1.0;         // fully sibilant, both the same
        }
        assert!(tied_low.effective_drive() > indep_low.effective_drive(),
                "down low, tied backs off less");

        let mut indep_high = AdaptiveSat::new(Profile::Tube, rate, 1);
        let mut tied_high = AdaptiveSat::new(Profile::Tube, rate, 1);
        tied_high.link = Link::Tied;
        for s in [&mut indep_high, &mut tied_high] {
            s.note_amount = 0.0;
            s.register = 1.0;
            s.sib = 1.0;
        }
        assert!((tied_high.effective_drive() - indep_high.effective_drive()).abs() < 1e-6,
                "up top, tied and independent agree — that is where the cut belongs");
    }

    #[test]
    fn with_both_amounts_at_zero_it_is_a_plain_saturator() {
        // The escape hatch: an adaptive stage must be able to stop adapting,
        // or it cannot be A/B'd against the fixed one it replaces.
        let rate = 48_000.0;
        let mut a = AdaptiveSat::new(Profile::Console, rate, 1);
        a.note_amount = 0.0;
        a.sibilance_amount = 0.0;
        a.base_drive = 0.6;
        let input = sine(220.0, 0.2, rate);
        let adaptive = run(&mut a, &input);

        let mut plain = Saturator::new(Profile::Console);
        plain.auto_gain = true;
        plain.drive = 0.6;
        let fixed: Vec<f32> = input.iter().map(|x| plain.process(*x)).collect();
        assert_eq!(adaptive, fixed, "no modulation means no difference at all");
    }

    #[test]
    fn an_unvoiced_stretch_holds_the_register_rather_than_swinging_it() {
        // A drum hit between two sung notes must not throw the character.
        let rate = 48_000.0;
        let mut s = AdaptiveSat::new(Profile::Tape, rate, 1);
        run(&mut s, &sine(80.0, 0.4, rate));
        let (_, before) = s.detectors();
        let mut rng = core_kit::rng::SplitMix64::new(11);
        let noise: Vec<f32> = (0..9_600).map(|_| (rng.next_f64() as f32) * 2.0 - 1.0).collect();
        run(&mut s, &noise);
        let (_, after) = s.detectors();
        assert!((after - before).abs() < 0.15,
                "register held through the noise: {before:.2} -> {after:.2}");
    }

    #[test]
    fn stereo_channels_keep_their_own_curve_state() {
        let rate = 48_000.0;
        let mut s = AdaptiveSat::new(Profile::Diode, rate, 2);
        s.base_drive = 0.9;
        let left = sine(110.0, 0.2, rate);
        let (mut lo, mut ro) = (Vec::new(), Vec::new());
        for x in &left {
            lo.push(s.process(*x));
            ro.push(s.process(0.0));
        }
        let r: f32 = ro.iter().map(|v| v.abs()).sum();
        let l: f32 = lo.iter().map(|v| v.abs()).sum();
        assert!(r < l * 0.001, "a silent channel stays silent: R={r} vs L={l}");
    }

    #[test]
    fn driving_harder_really_does_add_harmonics() {
        // Ties the abstract 'drive' number to something audible, so the
        // register test above is measuring character and not a label.
        let rate = 48_000.0;
        let input = sine(110.0, 0.2, rate);
        let mut soft = AdaptiveSat::new(Profile::Tube, rate, 1);
        soft.note_amount = 0.0; soft.sibilance_amount = 0.0; soft.base_drive = 0.05;
        let mut hard = AdaptiveSat::new(Profile::Tube, rate, 1);
        hard.note_amount = 0.0; hard.sibilance_amount = 0.0; hard.base_drive = 0.95;
        let a = harmonic_energy(&run(&mut soft, &input), rate, 110.0);
        let b = harmonic_energy(&run(&mut hard, &input), rate, 110.0);
        // 2x, not an arbitrary 3x: auto_gain normalises level, so what grows
        // is the harmonic RATIO, and the measured 2.6x is the real figure
        // for Tube across this drive range. Asserting a number the stage
        // does not actually hit would just be a test I tuned until green.
        assert!(b > a * 2.0, "hard drive is audibly dirtier: {b:.4} vs {a:.4}");
    }
}
