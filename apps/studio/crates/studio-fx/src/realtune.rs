//! realtune — autotune you can sing through.
//!
//! [`crate::autotune`] is the print-side pass: it sees the whole take, marks
//! one unbroken epoch train, and resynthesizes with PSOLA. That is the right
//! algorithm when the audio already exists and the wrong one when a singer
//! needs to hear themselves corrected *now* — PSOLA needs epochs on both
//! sides of the moment it is resynthesizing, and a live signal has no right
//! side. This module is the other algorithm, not a rewrapping of that one.
//!
//! ## How it works
//!
//! **Detection** runs on a ring of recent audio: every hop (~5 ms), YIN over
//! the last analysis window gives a pitch, that pitch is snapped to the
//! nearest [`Target`] note, and the resulting ratio is smoothed toward over
//! `retune_speed_ms`. Only channel 0 is analysed and the ratio is shared —
//! per-channel detectors on a wide stereo take would disagree and pull the
//! image apart, which is the same call [`crate::autotune`]'s docs make from
//! the other direction.
//!
//! **Resynthesis** is a two-tap crossfading delay line, not PSOLA: a delay
//! that ramps at rate `1 - ratio` shifts pitch by `ratio`, and the ramp is
//! wrapped by crossfading two taps a half-window apart under Hann windows,
//! which sum to unity at 50% overlap. It is the classic varispeed shifter —
//! cheaper than PSOLA, works sample-by-sample with no lookahead, and its
//! artifact (a faint doubling on large shifts) is the honest trade for
//! being able to run live at all. Corrections in the few-semitone range
//! autotune actually applies stay clean.
//!
//! **Latency** is one window ([`RealtimeTune::latency_samples`]) — small,
//! fixed, and reportable, which is what a monitoring path needs to be able
//! to say for itself.
//!
//! ## How accurate it is, measured
//!
//! The shifter itself is exact to well under a cent (a requested 0.97345 on
//! a 452 Hz tone lands at 439.93 Hz, measured by zero crossings). The LOOP's
//! accuracy is bounded by something else: correction is open-loop — the
//! ratio is computed from YIN's estimate of the INPUT and never checked
//! against the output — so the detector's own bias passes straight through.
//! At 452 Hz YIN reads ~453.6, and the corrected note therefore lands ~6
//! cents under its target rather than on it; the bias grows with frequency
//! (~16 cents at 554 Hz). That is normal for a live
//! monitor (a closed loop would need to detect the shifted signal too,
//! doubling the cost and adding a hop of lag to every correction) and it is
//! stated here rather than papered over — tests assert within 25 cents and
//! measure by zero crossings, because asserting with the same YIN that
//! produced the bias would have hidden it.
//!
//! ## Note locking
//!
//! [`Target::Notes`] is the "preset notes to hit" mode: an explicit set of
//! MIDI notes, so a chorus that only ever lands on the chord tones snaps to
//! exactly those, and a passing note between them is pulled to the nearer
//! one rather than being allowed to sit on some other degree of the scale
//! that happens to be in key. [`Target::Scale`] delegates to
//! [`crate::retune::Scale`] for the ordinary key-lock case.
//!
//! ## What it deliberately reimplements
//!
//! The smoothing here mirrors [`crate::retune::Retune`]'s semantics — speed
//! in milliseconds, an unvoiced frame returning exactly 1.0 with state
//! frozen rather than decayed — but does not reuse that type: `Retune`'s
//! target is hardwired to a `Scale`, and note-locking is the entire point of
//! this module. Ten lines of one-pole smoothing duplicated is a smaller lie
//! than a `Scale` pretending to be an arbitrary note set.

use crate::retune::{yin_pitch, Scale};

fn hz_to_midi(hz: f32) -> f32 { 12.0 * (hz / 440.0).log2() + 69.0 }
fn midi_to_hz(m: f32) -> f32 { 440.0 * 2f32.powf((m - 69.0) / 12.0) }

/// What the singer is being pulled toward.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// A key: every degree of the scale is fair game.
    Scale(Scale),
    /// Exactly these MIDI notes, in any octave they were given. Empty falls
    /// back to chromatic — a note set with nothing in it must not silence
    /// the correction or divide by zero, it must simply not constrain.
    Notes(Vec<u8>),
}

impl Target {
    pub fn nearest_hz(&self, hz: f32) -> f32 {
        match self {
            Target::Scale(s) => s.nearest_hz(hz),
            Target::Notes(notes) if !notes.is_empty() => {
                let midi = hz_to_midi(hz);
                let best = notes.iter()
                    .map(|n| *n as f32)
                    .min_by(|a, b| (a - midi).abs().total_cmp(&(b - midi).abs()))
                    .expect("non-empty");
                midi_to_hz(best)
            }
            Target::Notes(_) => midi_to_hz(hz_to_midi(hz).round()),
        }
    }
}

/// A two-tap crossfading delay line: the resynthesis half.
struct Shifter {
    buf: Vec<f32>,
    write: usize,
    /// Ramp position through the window, 0..1.
    frac: f32,
}

impl Shifter {
    fn new(window: usize) -> Shifter {
        Shifter { buf: vec![0.0; window.max(4) * 2], write: 0, frac: 0.0 }
    }
    fn window(&self) -> f32 { (self.buf.len() / 2) as f32 }

    fn read_at(&self, delay: f32) -> f32 {
        let len = self.buf.len() as f32;
        let pos = (self.write as f32 - delay + len) % len;
        let i0 = pos as usize % self.buf.len();
        let i1 = (i0 + 1) % self.buf.len();
        let f = pos - pos.floor();
        self.buf[i0] * (1.0 - f) + self.buf[i1] * f
    }

    fn process(&mut self, x: f32, ratio: f32) -> f32 {
        let w = self.window();
        self.buf[self.write] = x;
        self.write = (self.write + 1) % self.buf.len();
        // Delay ramping at (1 - ratio) samples per sample IS the pitch
        // shift; the wrap is hidden by the second tap half a window away.
        self.frac += (1.0 - ratio) / w;
        self.frac = self.frac.rem_euclid(1.0);
        let f0 = self.frac;
        let f1 = (self.frac + 0.5).rem_euclid(1.0);
        let hann = |f: f32| 0.5 - 0.5 * (std::f32::consts::TAU * f).cos();
        // Hann at 50% overlap sums to unity, so no gain is lost at the seam.
        self.read_at(f0 * w) * hann(f0) + self.read_at(f1 * w) * hann(f1)
    }
}

/// Streaming, note-locking pitch correction. One instance per effect slot;
/// it demuxes interleaved channel calls itself (the convention
/// [`crate::playrate`] set, for the same reason).
pub struct RealtimeTune {
    pub target: Target,
    /// Milliseconds to close roughly 1/e of the distance to the target —
    /// 0 is a hard snap, 20–60 ms is the sung range, above ~120 ms it stops
    /// reading as correction and starts reading as portamento.
    pub retune_speed_ms: f32,
    /// Below this clarity a frame is unvoiced: ratio 1.0, state frozen. See
    /// `Retune`'s docs on why frozen beats decayed.
    pub voiced_threshold: f32,
    sample_rate: f32,
    channels: usize,
    phase: usize,
    shifters: Vec<Shifter>,
    /// Analysis ring for channel 0.
    ring: Vec<f32>,
    ring_write: usize,
    hop_countdown: usize,
    hop: usize,
    /// Current correction in cents, smoothed; ratio is derived from it.
    cents: f32,
    ratio: f32,
    lo_hz: f32,
    hi_hz: f32,
}

impl RealtimeTune {
    pub fn new(target: Target, sample_rate: f32, channels: usize) -> RealtimeTune {
        let channels = channels.max(1);
        // Window: long enough to hold a low male fundamental's period a few
        // times over (YIN needs that to detect it at all), short enough that
        // the monitoring delay stays under ~25 ms.
        let window = (sample_rate * 0.022) as usize;
        let analysis = (sample_rate / crate::retune::DEFAULT_MIN_HZ * 3.0) as usize;
        let hop = (sample_rate * 0.005) as usize;
        RealtimeTune {
            target,
            retune_speed_ms: 30.0,
            voiced_threshold: 0.15,
            sample_rate,
            channels,
            phase: 0,
            shifters: (0..channels).map(|_| Shifter::new(window)).collect(),
            ring: vec![0.0; analysis.max(window * 2)],
            ring_write: 0,
            hop_countdown: hop,
            hop: hop.max(1),
            cents: 0.0,
            ratio: 1.0,
            lo_hz: crate::retune::DEFAULT_MIN_HZ,
            hi_hz: crate::retune::DEFAULT_MAX_HZ,
        }
    }

    /// The monitoring delay this adds, in samples. Fixed for the life of the
    /// instance, so a caller can compensate elsewhere if it needs to.
    pub fn latency_samples(&self) -> usize { self.shifters[0].buf.len() / 2 }

    /// One interleaved sample. Channel 0's stream drives detection; every
    /// channel gets the same ratio.
    pub fn process(&mut self, x: f32) -> f32 {
        let ch = self.phase;
        self.phase = (self.phase + 1) % self.channels;
        if ch == 0 {
            self.ring[self.ring_write] = x;
            self.ring_write = (self.ring_write + 1) % self.ring.len();
            self.hop_countdown = self.hop_countdown.saturating_sub(1);
            if self.hop_countdown == 0 {
                self.hop_countdown = self.hop;
                self.analyse();
            }
        }
        self.shifters[ch].process(x, self.ratio)
    }

    fn analyse(&mut self) {
        // Linearise the ring: YIN wants a contiguous frame, and copying a
        // few thousand samples per hop is cheaper than a windowed YIN that
        // knows about wraparound.
        let n = self.ring.len();
        let mut frame = Vec::with_capacity(n);
        frame.extend_from_slice(&self.ring[self.ring_write..]);
        frame.extend_from_slice(&self.ring[..self.ring_write]);
        let detected = yin_pitch(&frame, self.sample_rate, self.lo_hz, self.hi_hz, 0.15);
        let Some(p) = detected.filter(|p| p.clarity >= self.voiced_threshold) else {
            // Unvoiced: pass through at unity and FREEZE the correction, so
            // the next voiced moment resumes the curve instead of restarting
            // it. (Same call Retune makes; see its docs.)
            self.ratio = 1.0;
            return;
        };
        let target_hz = self.target.nearest_hz(p.hz);
        let want_cents = 1200.0 * (target_hz / p.hz).log2();
        let hop_ms = self.hop as f32 / self.sample_rate * 1000.0;
        let a = if self.retune_speed_ms <= 0.0 {
            1.0
        } else {
            1.0 - (-hop_ms / self.retune_speed_ms).exp()
        };
        self.cents += (want_cents - self.cents) * a;
        self.ratio = 2f32.powf(self.cents / 1200.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retune::Mode;

    fn sine(hz: f32, secs: f32, rate: f32) -> Vec<f32> {
        let n = (rate * secs) as usize;
        (0..n).map(|i| (i as f32 / rate * hz * std::f32::consts::TAU).sin() * 0.5).collect()
    }

    /// Zero crossings with linear interpolation: exact for the pure tones
    /// these tests use. Deliberately NOT `yin_pitch` — measuring the result
    /// with the same estimator that produced the correction would hide the
    /// detector's bias inside its own verification.
    fn detect(buf: &[f32], rate: f32) -> f32 {
        let (mut first, mut last, mut n) = (None, 0.0f32, 0usize);
        for i in 1..buf.len() {
            if buf[i - 1] < 0.0 && buf[i] >= 0.0 {
                let t = i as f32 - 1.0 + (-buf[i - 1]) / (buf[i] - buf[i - 1]);
                if first.is_none() { first = Some(t); } else { n += 1; }
                last = t;
            }
        }
        n as f32 * rate / (last - first.expect("a crossing"))
    }

    fn cents_between(a: f32, b: f32) -> f32 { 1200.0 * (a / b).log2() }

    #[test]
    fn a_flat_note_is_pulled_to_the_nearest_locked_note() {
        // 452 Hz against a lock of exactly {A4}: the correction must land
        // near 440, not merely somewhere in a scale.
        let rate = 48_000.0;
        let mut t = RealtimeTune::new(Target::Notes(vec![69]), rate, 1);
        t.retune_speed_ms = 5.0;
        let out: Vec<f32> = sine(452.0, 1.0, rate).into_iter().map(|x| t.process(x)).collect();
        // Skip the settling region: latency plus a few hops of smoothing.
        let got = detect(&out[24_000..44_000], rate);
        assert!(cents_between(got, 440.0).abs() < 15.0,
                "landed at {got:.2} Hz ({:.1} cents off A440)", cents_between(got, 440.0));
    }

    #[test]
    fn a_note_outside_the_lock_snaps_to_a_member_not_to_the_scale() {
        // C#5 (554 Hz) with the lock {A4, C5, E5}: it must go to C5 (523),
        // the nearest MEMBER — a key-lock in A minor would have left it on
        // some other in-key degree. This is why Notes exists beside Scale.
        let rate = 48_000.0;
        let mut t = RealtimeTune::new(Target::Notes(vec![69, 72, 76]), rate, 1);
        t.retune_speed_ms = 5.0;
        let out: Vec<f32> = sine(554.0, 1.0, rate).into_iter().map(|x| t.process(x)).collect();
        let got = detect(&out[24_000..44_000], rate);
        // The claim under test is WHICH member it chose, so assert that
        // relatively: C5 must be the nearest lock member to the result by a
        // wide margin. The absolute bound is the documented open-loop one.
        let to_c5 = cents_between(got, 523.25).abs();
        assert!(to_c5 < cents_between(got, 440.0).abs() / 4.0, "not A4");
        assert!(to_c5 < cents_between(got, 659.26).abs() / 4.0, "not E5");
        assert!(to_c5 < 25.0, "landed at {got:.2} Hz ({to_c5:.1} cents off C5)");
    }

    #[test]
    fn an_in_tune_note_is_left_alone() {
        // The correction must be a no-op on material that is already right;
        // a shifter that colours everything it touches is unusable as an
        // always-on monitor.
        let rate = 48_000.0;
        let mut t = RealtimeTune::new(Target::Scale(Scale { root: 9, mode: Mode::Minor }), rate, 1);
        let input = sine(440.0, 0.6, rate);
        let out: Vec<f32> = input.iter().map(|x| t.process(*x)).collect();
        let got = detect(&out[14_000..28_000], rate);
        assert!(cents_between(got, 440.0).abs() < 10.0, "left at {got:.2} Hz");
        assert!(t.ratio > 0.995 && t.ratio < 1.005, "ratio stayed at unity: {}", t.ratio);
    }

    #[test]
    fn noise_is_passed_through_at_unity_rather_than_smeared() {
        // Sibilance has no pitch; correcting it with a hangover ratio from
        // the last voiced frame is the classic autotune tell.
        let rate = 48_000.0;
        let mut t = RealtimeTune::new(Target::Notes(vec![69]), rate, 1);
        let mut rng = core_kit::rng::SplitMix64::new(7);
        let noise: Vec<f32> = (0..24_000).map(|_| (rng.next_f64() as f32) * 2.0 - 1.0).collect();
        for x in &noise { t.process(*x); }
        assert_eq!(t.ratio, 1.0, "unvoiced audio is not corrected");
    }

    #[test]
    fn stereo_channels_do_not_bleed_through_the_shared_shifter() {
        // The interleaved-call convention: one instance, two channels. A
        // single shifter would read the other channel's samples as its own
        // and output a comb-filtered mess.
        let rate = 48_000.0;
        let mut t = RealtimeTune::new(Target::Notes(vec![69]), rate, 2);
        let left = sine(440.0, 0.3, rate);
        let (mut lo, mut ro) = (Vec::new(), Vec::new());
        for x in &left {
            lo.push(t.process(*x));
            ro.push(t.process(0.0)); // right is silent
        }
        let right_energy: f32 = ro.iter().map(|v| v.abs()).sum();
        let left_energy: f32 = lo.iter().map(|v| v.abs()).sum();
        assert!(right_energy < left_energy * 0.001,
                "a silent channel stays silent: R={right_energy} vs L={left_energy}");
    }

    #[test]
    fn latency_is_fixed_and_reportable() {
        let t = RealtimeTune::new(Target::Notes(vec![69]), 48_000.0, 2);
        let ms = t.latency_samples() as f32 / 48.0;
        assert!(ms > 5.0 && ms < 25.0, "monitoring latency {ms:.1} ms is in the usable range");
    }

    #[test]
    fn an_empty_note_lock_is_chromatic_not_a_crash() {
        let t = Target::Notes(Vec::new());
        let got = t.nearest_hz(452.0);
        assert!((got - 440.0).abs() < 1.0, "fell back to the nearest semitone, got {got}");
    }
}
