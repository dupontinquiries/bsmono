//! voice — the primitives a robot voice needs and this crate did not have.
//!
//! `studio-fx` already covers the settled ground: `Biquad`/`Eq`, `Compressor`,
//! `Limiter`, saturation, multiband. Every voice treatment worth building is
//! mostly those in the right order — the `chain` module's whole thesis is that
//! order changes the sound. What was missing is the handful of parts that are
//! not EQ or dynamics:
//!
//! * [`Delay`] — a feedback line with a low-pass INSIDE the loop, which is what
//!   makes each repeat duller than the last.
//! * [`SubOctave`] — weight under a voice without touching the words.
//! * [`Shifter`] — granular pitch shift. `retune` detects pitch and decides a
//!   target and explicitly does not resynthesise; this is the hands.
//! * [`Crush`] — bit depth and sample-rate reduction.
//! * [`Gate`] — the one that makes a voice COLD, by removing breath.
//!
//! Same contract as the rest of the crate: a plain struct with a `process`, no
//! allocation in the hot path, so the same code runs in a bake and in a
//! realtime callback. Buffers are sized at construction.
//!
//! ## What these are for, in one line each
//!
//! **BIG is not pitch-down.** Pitch shifting alone sounds like a slowed tape
//! and eats intelligibility. Size is [`SubOctave`] low-passed and mixed UNDER
//! the dry at about −12 dB, plus a doubler, plus a low shelf — the words stay
//! where they are and the body gets bigger.
//!
//! **COLD is subtraction.** [`Gate`] to remove breath, a hard compressor, a
//! roll-off above 8 kHz, and no reverb at all. The absence is the effect.
//!
//! **The tuned-comb echo is not a room.** A short [`Delay`] (40–120 ms) whose
//! feedback path is pitch-shifted down and low-passed, so the voice trails
//! into something deeper and duller. Reaching for reverb is the usual mistake.
//!
//! **Glitch must be event-driven.** [`Crush`] and a stutter are information
//! when they fire on something and wallpaper when they run continuously.

/// A delay line with feedback and a one-pole low-pass in the loop.
///
/// The filter is INSIDE the feedback path on purpose: filtering the output
/// dulls everything once, while filtering the loop makes each repeat duller
/// than the one before, which is what makes a comb read as a material rather
/// than as an echo.
pub struct Delay {
    buf: Vec<f32>,
    write: usize,
    /// Read distance in samples, fractional for detune and pitch drift.
    delay: f32,
    feedback: f32,
    /// One-pole state for the in-loop low-pass.
    lp: f32,
    lp_coeff: f32,
}

impl Delay {
    /// `max_ms` sizes the buffer once; `process` never allocates.
    pub fn new(rate: f32, max_ms: f32, delay_ms: f32, feedback: f32, damp_hz: f32) -> Delay {
        let max = ((max_ms.max(1.0) / 1000.0) * rate).ceil() as usize + 4;
        let mut d = Delay {
            buf: vec![0.0; max],
            write: 0,
            delay: 1.0,
            feedback: feedback.clamp(0.0, 0.99),
            lp: 0.0,
            lp_coeff: 0.0,
        };
        d.set_delay_ms(rate, delay_ms);
        d.set_damping(rate, damp_hz);
        d
    }

    pub fn set_delay_ms(&mut self, rate: f32, ms: f32) {
        let want = (ms.max(0.0) / 1000.0) * rate;
        // Two samples of headroom so the fractional read never crosses the
        // write head, which would fold the loop back on itself.
        self.delay = want.clamp(1.0, self.buf.len() as f32 - 3.0);
    }

    pub fn set_damping(&mut self, rate: f32, cutoff_hz: f32) {
        let f = cutoff_hz.clamp(20.0, rate * 0.49);
        let x = (-std::f32::consts::TAU * f / rate).exp();
        self.lp_coeff = x.clamp(0.0, 0.999);
    }

    pub fn set_feedback(&mut self, fb: f32) {
        // Never 1.0: a unity loop with any numerical drift is an oscillator,
        // and a slider CAN be dragged to the end.
        self.feedback = fb.clamp(0.0, 0.99);
    }

    fn read(&self) -> f32 {
        let pos = self.write as f32 - self.delay;
        let pos = if pos < 0.0 { pos + self.buf.len() as f32 } else { pos };
        let i = pos.floor() as usize % self.buf.len();
        let j = (i + 1) % self.buf.len();
        let frac = pos - pos.floor();
        self.buf[i] * (1.0 - frac) + self.buf[j] * frac
    }

    /// Returns the delayed signal only. The caller mixes it with the dry,
    /// because how much dry there is decides whether this reads as timbre or
    /// as an echo.
    pub fn process(&mut self, x: f32) -> f32 {
        let out = self.read();
        self.lp = out * (1.0 - self.lp_coeff) + self.lp * self.lp_coeff;
        self.buf[self.write] = x + self.lp * self.feedback;
        self.write = (self.write + 1) % self.buf.len();
        out
    }
}

/// An octave below, following the input's dynamics.
///
/// A flip-flop on rising zero crossings gives a square at half the input
/// frequency; multiplying by the rectified envelope makes it breathe with the
/// voice instead of sitting under it as a drone. That envelope multiply is the
/// difference between weight and a buzz.
///
/// Low-pass the result around 200 Hz and mix it at roughly −12 dB. Above that
/// it stops being weight and starts being a second voice.
pub struct SubOctave {
    flip: f32,
    was_positive: bool,
    env: f32,
    env_coeff: f32,
}

impl SubOctave {
    pub fn new(rate: f32) -> SubOctave {
        // ~20 ms envelope: fast enough to follow syllables, slow enough not to
        // follow the waveform.
        let x = (-1.0 / (0.020 * rate)).exp();
        SubOctave { flip: 1.0, was_positive: false, env: 0.0, env_coeff: x }
    }
    pub fn process(&mut self, x: f32) -> f32 {
        let positive = x > 0.0;
        if positive && !self.was_positive {
            self.flip = -self.flip;
        }
        self.was_positive = positive;
        let rect = x.abs();
        self.env = if rect > self.env {
            rect
        } else {
            rect * (1.0 - self.env_coeff) + self.env * self.env_coeff
        };
        self.flip * self.env
    }
}

/// Granular pitch shift: two grains, crossfaded, read at a different rate than
/// they are written.
///
/// `retune` in this crate detects pitch and decides a target and says plainly
/// that it does not resynthesise. This is a resynthesiser, and a simple one —
/// it is not TD-PSOLA and it does not preserve formants, so a large downward
/// shift will sound like a bigger body AND a slower tape. That is exactly why
/// [`SubOctave`] exists for weight: use a small shift here and get the size
/// from the sub layer.
pub struct Shifter {
    buf: Vec<f32>,
    write: usize,
    read: f32,
    ratio: f32,
    grain: usize,
}

impl Shifter {
    /// `semitones` negative shifts down. `grain_ms` around 50–80 is right for
    /// speech: shorter chatters, longer smears consonants.
    pub fn new(rate: f32, semitones: f32, grain_ms: f32) -> Shifter {
        let grain = ((grain_ms.max(5.0) / 1000.0) * rate) as usize;
        Shifter {
            buf: vec![0.0; grain * 4 + 4],
            write: 0,
            read: 0.0,
            ratio: 2f32.powf(semitones / 12.0),
            grain: grain.max(1),
        }
    }
    pub fn process(&mut self, x: f32) -> f32 {
        let n = self.buf.len();
        self.buf[self.write] = x;

        let a = self.read;
        let b = (self.read + self.grain as f32) % n as f32;
        let sample = |p: f32| -> f32 {
            let i = p.floor() as usize % n;
            let j = (i + 1) % n;
            let f = p - p.floor();
            self.buf[i] * (1.0 - f) + self.buf[j] * f
        };
        // Crossfade the two grains so the wrap is not a click. Equal-power
        // rather than linear: a linear crossfade dips 3 dB in the middle and
        // on a sustained vowel that dip is audible as a tremolo.
        let phase = (self.read % self.grain as f32) / self.grain as f32;
        let (ga, gb) = (
            ((1.0 - phase) * std::f32::consts::FRAC_PI_2).sin(),
            (phase * std::f32::consts::FRAC_PI_2).sin(),
        );
        let out = sample(a) * ga + sample(b) * gb;

        self.write = (self.write + 1) % n;
        self.read += self.ratio;
        if self.read >= n as f32 {
            self.read -= n as f32;
        }
        out
    }
}

/// Bit-depth and sample-rate reduction.
///
/// Two different artefacts and they are worth having separately: fewer bits is
/// quantisation grit that scales with level, while holding samples is aliasing
/// that scales with frequency. A voice usually wants the second more than the
/// first.
pub struct Crush {
    levels: f32,
    hold: f32,
    held: f32,
    phase: f32,
}

impl Crush {
    /// `bits` 1–16, `hold_hz` the rate to decimate to (at or above `rate` is
    /// no decimation).
    pub fn new(rate: f32, bits: u8, hold_hz: f32) -> Crush {
        let bits = bits.clamp(1, 16);
        Crush {
            levels: 2f32.powi(bits as i32) - 1.0,
            hold: (hold_hz.max(1.0) / rate).min(1.0),
            held: 0.0,
            phase: 1.0,
        }
    }
    pub fn process(&mut self, x: f32) -> f32 {
        self.phase += self.hold;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
            self.held = ((x.clamp(-1.0, 1.0) * 0.5 + 0.5) * self.levels).round() / self.levels
                * 2.0
                - 1.0;
        }
        self.held
    }
}

/// A noise gate with hold, for removing breath.
///
/// The hold is the part that matters: without it a gate chatters on every dip
/// inside a word, which is far more noticeable than the breath it was removing.
pub struct Gate {
    open: bool,
    countdown: u32,
    hold: u32,
    thresh: f32,
    env: f32,
    coeff: f32,
    gain: f32,
    release: f32,
}

impl Gate {
    pub fn new(rate: f32, threshold_db: f32, hold_ms: f32, release_ms: f32) -> Gate {
        Gate {
            open: false,
            countdown: 0,
            hold: ((hold_ms.max(0.0) / 1000.0) * rate) as u32,
            thresh: 10f32.powf(threshold_db / 20.0),
            env: 0.0,
            coeff: (-1.0 / (0.005 * rate)).exp(),
            gain: 0.0,
            release: (-1.0 / ((release_ms.max(1.0) / 1000.0) * rate)).exp(),
        }
    }
    pub fn is_open(&self) -> bool {
        self.open
    }
    pub fn process(&mut self, x: f32) -> f32 {
        let rect = x.abs();
        self.env = if rect > self.env {
            rect
        } else {
            rect * (1.0 - self.coeff) + self.env * self.coeff
        };
        if self.env > self.thresh {
            self.open = true;
            self.countdown = self.hold;
        } else if self.countdown > 0 {
            self.countdown -= 1;
        } else {
            self.open = false;
        }
        let target = if self.open { 1.0 } else { 0.0 };
        self.gain = target * (1.0 - self.release) + self.gain * self.release;
        x * self.gain
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn sine(freq: f32, secs: f32) -> Vec<f32> {
        let n = (secs * SR) as usize;
        (0..n).map(|i| (std::f32::consts::TAU * freq * i as f32 / SR).sin()).collect()
    }

    /// Zero crossings per second — a cheap frequency estimate that needs no FFT.
    fn crossings_hz(v: &[f32]) -> f32 {
        let mut n = 0;
        for w in v.windows(2) {
            if w[0] <= 0.0 && w[1] > 0.0 {
                n += 1;
            }
        }
        n as f32 / (v.len() as f32 / SR)
    }

    #[test]
    fn a_delayed_impulse_arrives_exactly_when_it_should() {
        let mut d = Delay::new(SR, 500.0, 100.0, 0.0, 20_000.0);
        let want = (0.1 * SR) as usize;
        let mut at = None;
        for i in 0..(want + 100) {
            let y = d.process(if i == 0 { 1.0 } else { 0.0 });
            if y.abs() > 0.5 && at.is_none() {
                at = Some(i);
            }
        }
        let at = at.expect("the impulse never came back");
        assert!((at as i64 - want as i64).abs() <= 1, "arrived at {at}, wanted {want}");
    }

    #[test]
    fn the_low_pass_is_in_the_loop_so_repeats_get_duller() {
        // The design point. Filtering the OUTPUT dulls everything once;
        // filtering the LOOP makes each repeat duller than the last, which is
        // what makes a comb read as a material rather than an echo.
        let mut d = Delay::new(SR, 200.0, 20.0, 0.85, 1200.0);
        let mut bright = sine(3000.0, 0.02);
        bright.extend(std::iter::repeat(0.0).take((0.2 * SR) as usize));
        let out: Vec<f32> = bright.iter().map(|x| d.process(*x)).collect();

        let step = (0.02 * SR) as usize;
        let energy = |r: std::ops::Range<usize>| -> f32 {
            out[r].iter().map(|s| s * s).sum::<f32>()
        };
        let first = energy(step * 1..step * 2);
        let third = energy(step * 3..step * 4);
        assert!(third < first * 0.5, "repeats are not losing brightness: {first} -> {third}");
    }

    #[test]
    fn feedback_can_never_be_set_to_an_oscillator() {
        let mut d = Delay::new(SR, 100.0, 10.0, 5.0, 20_000.0);
        d.set_feedback(2.0);
        let mut out = 0.0f32;
        for i in 0..(SR as usize) {
            out = d.process(if i == 0 { 1.0 } else { 0.0 });
            assert!(out.is_finite(), "the loop diverged at sample {i}");
        }
        assert!(out.abs() < 2.0, "still ringing at {out}");
    }

    #[test]
    fn the_sub_octave_is_an_octave_below() {
        let mut s = SubOctave::new(SR);
        let input = sine(220.0, 0.5);
        let out: Vec<f32> = input.iter().map(|x| s.process(*x)).collect();
        let f = crossings_hz(&out[(0.05 * SR) as usize..]);
        assert!((f - 110.0).abs() < 12.0, "sub came out at {f:.0} Hz, wanted ~110");
    }

    #[test]
    fn the_sub_octave_follows_dynamics_instead_of_droning() {
        // Without the envelope multiply this is a square wave that keeps going
        // after the voice stops — which is a buzz, not weight.
        let mut s = SubOctave::new(SR);
        let mut input = sine(200.0, 0.2);
        input.extend(std::iter::repeat(0.0).take((0.3 * SR) as usize));
        let out: Vec<f32> = input.iter().map(|x| s.process(*x)).collect();
        let tail: f32 = out[(0.45 * SR) as usize..].iter().map(|s| s.abs()).sum();
        assert!(tail < 1.0, "the sub kept droning after the input stopped: {tail}");
    }

    #[test]
    fn a_downward_shift_lowers_the_pitch() {
        let mut sh = Shifter::new(SR, -12.0, 60.0);
        let input = sine(400.0, 0.6);
        let out: Vec<f32> = input.iter().map(|x| sh.process(*x)).collect();
        let f = crossings_hz(&out[(0.2 * SR) as usize..]);
        assert!(f < 320.0, "an octave down came out at {f:.0} Hz");
        assert!(f > 120.0, "shifted far past an octave: {f:.0} Hz");
    }

    #[test]
    fn a_ratio_of_one_is_close_to_transparent() {
        let mut sh = Shifter::new(SR, 0.0, 60.0);
        let input = sine(300.0, 0.4);
        let out: Vec<f32> = input.iter().map(|x| sh.process(*x)).collect();
        let f = crossings_hz(&out[(0.1 * SR) as usize..]);
        assert!((f - 300.0).abs() < 20.0, "unshifted came out at {f:.0} Hz");
    }

    #[test]
    fn crushing_bits_limits_how_many_values_come_out() {
        let mut c = Crush::new(SR, 3, SR);
        let mut seen: Vec<i32> = Vec::new();
        for x in sine(100.0, 0.1) {
            let y = c.process(x);
            let k = (y * 10_000.0).round() as i32;
            if !seen.contains(&k) {
                seen.push(k);
            }
        }
        assert!(seen.len() <= 8, "3 bits produced {} distinct values", seen.len());
    }

    #[test]
    fn holding_samples_and_crushing_bits_are_different_artefacts() {
        // Worth having separately: fewer bits is quantisation grit that scales
        // with LEVEL, holding samples is aliasing that scales with FREQUENCY.
        let mut hold_only = Crush::new(SR, 16, 4_000.0);
        let out: Vec<f32> = sine(1000.0, 0.05).iter().map(|x| hold_only.process(*x)).collect();
        let runs = out.windows(2).filter(|w| w[0] == w[1]).count();
        assert!(runs > out.len() / 3, "decimation did not hold samples: {runs}");
    }

    #[test]
    fn the_gate_closes_on_silence_and_opens_on_speech() {
        let mut g = Gate::new(SR, -40.0, 50.0, 20.0);
        for x in sine(200.0, 0.2) {
            g.process(x);
        }
        assert!(g.is_open(), "the gate never opened on a loud signal");
        for _ in 0..(0.4 * SR) as usize {
            g.process(0.0);
        }
        assert!(!g.is_open(), "the gate never closed on silence");
    }

    #[test]
    fn the_hold_stops_the_gate_chattering_inside_a_word() {
        // THE PART THAT MATTERS. Without hold, a gate slams shut on every dip
        // between syllables, which is far more noticeable than the breath it
        // was removing.
        let mut g = Gate::new(SR, -30.0, 120.0, 20.0);
        let mut closes = 0;
        let mut was = false;
        // speech-like: 60 ms of tone, 40 ms of near-silence, repeated
        for _ in 0..6 {
            for x in sine(200.0, 0.06) {
                g.process(x);
                if was && !g.is_open() {
                    closes += 1;
                }
                was = g.is_open();
            }
            for _ in 0..(0.04 * SR) as usize {
                g.process(0.0001);
                if was && !g.is_open() {
                    closes += 1;
                }
                was = g.is_open();
            }
        }
        assert_eq!(closes, 0, "the gate chattered {closes} times inside a word");
    }

    #[test]
    fn nothing_here_allocates_after_construction_or_returns_a_nan() {
        // The crate's contract: same code in a bake and in a realtime
        // callback. A NaN in a feedback path is permanent, so it is checked
        // rather than assumed.
        let mut d = Delay::new(SR, 200.0, 30.0, 0.9, 800.0);
        let mut s = SubOctave::new(SR);
        let mut sh = Shifter::new(SR, -7.0, 50.0);
        let mut c = Crush::new(SR, 6, 8000.0);
        let mut g = Gate::new(SR, -35.0, 40.0, 15.0);
        let (dc, dn, dsh, dcr, dg) =
            (d.buf.len(), s.env_coeff, sh.buf.len(), c.levels, g.hold);
        for x in sine(180.0, 0.3) {
            let y = g.process(x);
            let y = c.process(y);
            let y = sh.process(y);
            let y = y + s.process(y) * 0.25 + d.process(y) * 0.3;
            assert!(y.is_finite(), "chain produced {y}");
        }
        assert_eq!((d.buf.len(), sh.buf.len()), (dc, dsh), "a buffer was resized mid-stream");
        assert_eq!((s.env_coeff, c.levels, g.hold), (dn, dcr, dg));
    }
}
