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
//! which sum to unity at 50% overlap. Unity for the WINDOWS is not unity
//! for a periodic voice: taps a fixed half-window apart are usually out of
//! phase and partly cancel (up to ~3 dB on tones, 8 dB on Goat's vocal), so
//! each tap is re-seated at its zero-weight wrap a whole number of detected
//! periods from the other (see [`Shifter`]). It is the classic varispeed shifter —
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

/// How far the coarse pitch search decimates. 4 keeps a 1 kHz fundamental
/// well inside the decimated Nyquist while cutting YIN's O(N × maxlag) cost
/// by roughly its square.
const DECIMATION: usize = 4;

/// Refine a coarse period estimate to sub-sample accuracy by parabolic
/// interpolation of the local difference function.
///
/// This is what closes most of the open-loop error the first version
/// documented: YIN's own period estimate is integer-ish, and correcting a
/// note from a starting point that is 6 cents wrong lands it 6 cents wrong.
/// Refining costs three difference evaluations over a couple of periods —
/// nothing next to the search that found the coarse lag.
fn refine_period(x: &[f32], coarse: f32) -> Option<f32> {
    let centre = coarse.round() as isize;
    // SEARCH, then interpolate. Interpolating alone was not enough and the
    // tests caught it: a coarse estimate from decimated audio is quantized
    // to DECIMATION samples of the full-rate period, so the true minimum can
    // sit several lags away — a parabola fitted at the wrong lag confidently
    // returns the wrong period. The window is the decimation factor plus a
    // sample of slack, which is exactly the uncertainty the coarse stage has.
    let reach = DECIMATION as isize + 1;
    let lo = (centre - reach).max(2);
    let hi = centre + reach;
    if hi as usize + 1 >= x.len() { return None; }
    let span = (x.len() - hi as usize - 1).min(centre as usize * 4);
    if span < centre as usize { return None; }
    let diff = |l: isize| -> f32 {
        let l = l as usize;
        crate::retune::sq_diff_sum(&x[..span], &x[l..l + span])
    };
    let mut best = lo;
    let mut best_d = f32::INFINITY;
    for l in lo..=hi {
        let d = diff(l);
        if d < best_d { best_d = d; best = l; }
    }
    if best <= 2 { return Some(best as f32); }
    let (dm, d0, dp) = (diff(best - 1), best_d, diff(best + 1));
    let denom = dm - 2.0 * d0 + dp;
    if denom.abs() < 1e-12 { return Some(best as f32); }
    let shift = (0.5 * (dm - dp) / denom).clamp(-0.5, 0.5);
    Some(best as f32 + shift)
}

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
///
/// Tap i reads at `phase_i * w + off[i]`, phase_0 = `frac`, phase_1 =
/// `frac + 0.5`, weighted hann(phase_i) (the weights sum to 1). With `off`
/// zero the taps sit a FIXED half-window apart -- and for a periodic voice
/// that spacing is rarely a whole number of periods, so the crossfade
/// partly CANCELS: measured 0 to -2.9 dB on steady tones while correcting,
/// 8 dB on Goat's tuned vocal. So each time a tap wraps (its weight is zero
/// there: moving it is inaudible), its offset is re-chosen in [0, period)
/// to put it a whole number of periods from the other tap. In phase, the
/// weights' unity sum is the output's. Delays stay in [0, w + period), which
/// the 2w buffer holds for any period below w. No period known (unvoiced,
/// not locked): offset 0, the fixed-spacing shifter exactly.
struct Shifter {
    buf: Vec<f32>,
    write: usize,
    /// Ramp position through the window, 0..1.
    frac: f32,
    /// Each tap's offset past `phase * w`, samples: 0 or in [0, period).
    off: [f32; 2],
}

impl Shifter {
    fn new(window: usize) -> Shifter {
        Shifter { buf: vec![0.0; window.max(4) * 2], write: 0, frac: 0.0, off: [0.0; 2] }
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

    /// `period`: the input's period in samples, when voiced -- what the
    /// taps are kept in phase with.
    fn process(&mut self, x: f32, ratio: f32, period: Option<f32>) -> f32 {
        let w = self.window();
        self.buf[self.write] = x;
        self.write = (self.write + 1) % self.buf.len();
        // Delay ramping at (1 - ratio) samples per sample IS the pitch
        // shift; the wrap is hidden by the second tap half a window away.
        let (p0, p1) = (self.frac, (self.frac + 0.5).rem_euclid(1.0));
        self.frac = (self.frac + (1.0 - ratio) / w).rem_euclid(1.0);
        let f0 = self.frac;
        let f1 = (self.frac + 0.5).rem_euclid(1.0);
        // a tap WRAPPED when its phase jumped by more than half a cycle
        let wrapped = |a: f32, b: f32| (a - b).abs() > 0.5;
        let period = period.filter(|t| *t >= 2.0 && *t < w);
        for (i, (before, now, other_ph)) in [(p0, f0, f1), (p1, f1, f0)].into_iter().enumerate() {
            if wrapped(before, now) {
                self.off[i] = match period {
                    Some(t) => (other_ph * w + self.off[1 - i] - now * w).rem_euclid(t),
                    None => 0.0,
                };
            }
        }
        let hann = |f: f32| 0.5 - 0.5 * (std::f32::consts::TAU * f).cos();
        // Hann at 50% overlap sums to unity, so no gain is lost at the seam.
        self.read_at(f0 * w + self.off[0]) * hann(f0) + self.read_at(f1 * w + self.off[1]) * hann(f1)
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
    /// SLIDE: each new note (a change of target note, or the first voiced
    /// moment after a breath) starts UNCORRECTED and glides into key at
    /// `retune_speed_ms` -- the "slide into key" feel. Off, the correction
    /// runs continuously and a new note inherits the last one's (the hard,
    /// held feel at speed 0). Until c20 there was only the latter.
    pub slide: bool,
    /// How much of the correction to apply, 0..=1: less than 1 leaves some of
    /// the singer's own pitch in the note.
    pub amount: f32,
    /// A note already within this many cents of its target is left alone.
    pub tolerance_cents: f32,
    /// Current correction in cents, smoothed; ratio is derived from it.
    cents: f32,
    /// The target note being corrected toward (for SLIDE's new-note test);
    /// `None` after a breath.
    last_target: Option<f32>,
    /// The ratio actually applied this sample, ramped toward `ratio_target`
    /// across the hop that follows each analysis.
    ratio: f32,
    ratio_target: f32,
    ratio_step: f32,
    /// The input's period (samples) at the last voiced analysis; `None`
    /// unvoiced. Keeps the shifter's taps in phase (see [`Shifter`]).
    period: Option<f32>,
    lo_hz: f32,
    hi_hz: f32,
    /// Preallocated analysis scratch. Allocating per hop — which the first
    /// version did — is a malloc inside the audio callback, the one thing a
    /// realtime path must never do; it is invisible in tests and audible as
    /// a dropout the first time the allocator takes a lock.
    frame: Vec<f32>,
    /// Decimated copy of `frame`, for the coarse pitch search.
    coarse: Vec<f32>,
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
        let ring_len = analysis.max(window * 2);
        RealtimeTune {
            target,
            retune_speed_ms: 30.0,
            voiced_threshold: 0.15,
            sample_rate,
            channels,
            phase: 0,
            shifters: (0..channels).map(|_| Shifter::new(window)).collect(),
            ring: vec![0.0; ring_len],
            ring_write: 0,
            hop_countdown: hop,
            hop: hop.max(1),
            cents: 0.0,
            slide: false,
            amount: 1.0,
            tolerance_cents: 0.0,
            last_target: None,
            ratio: 1.0,
            ratio_target: 1.0,
            ratio_step: 0.0,
            period: None,
            frame: vec![0.0; ring_len],
            coarse: vec![0.0; ring_len / DECIMATION + 1],
            lo_hz: crate::retune::DEFAULT_MIN_HZ,
            hi_hz: crate::retune::DEFAULT_MAX_HZ,
        }
    }

    /// The monitoring delay this adds, in samples. Fixed for the life of the
    /// instance, so a caller can compensate elsewhere if it needs to.
    pub fn latency_samples(&self) -> usize { self.shifters[0].buf.len() / 2 }

    /// Put the analysis grid on the SONG's sample positions: the next input
    /// frame is song frame `at`. A render started anywhere -- a seek, a
    /// parallel export's segment -- then analyses at the same instants as a
    /// render from the start. (Counted from the instance's creation, the grid
    /// moved with every start, and so did every decision after it.)
    pub fn align(&mut self, at: i64) {
        let h = self.hop as i64;
        let r = at.rem_euclid(h);
        self.hop_countdown = if r == 0 { self.hop } else { (h - r) as usize };
    }

    /// Back to the state of a fresh instance -- at digital SILENCE, where there
    /// is nothing to correct and nothing to remember: after any silent gap the
    /// tuner is in the same state however the render began.
    fn rest(&mut self) {
        self.cents = 0.0;
        self.last_target = None;
        self.ratio = 1.0;
        self.ratio_target = 1.0;
        self.ratio_step = 0.0;
        self.period = None;
        for sh in self.shifters.iter_mut() { sh.frac = 0.0; sh.off = [0.0; 2]; sh.buf.fill(0.0); sh.write = 0; }
    }

    /// The correction being applied right now, in cents.
    pub fn correction_cents(&self) -> f32 { self.cents }

    /// The input period (samples) the shifter's taps are kept in phase with:
    /// the last voiced analysis, `None` while unvoiced.
    pub fn period_samples(&self) -> Option<f32> { self.period }

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
        if ch == self.channels - 1 {
            // Ramp the applied ratio toward the target across one hop, once
            // per frame rather than once per channel. A ratio that jumped at
            // each analysis boundary steps the delay line's read rate, and
            // that step is audible as a tick every 5 ms on sustained notes —
            // the difference between "corrected" and "processed".
            if self.ratio_step != 0.0 {
                self.ratio += self.ratio_step;
                let done = if self.ratio_step > 0.0 { self.ratio >= self.ratio_target }
                           else { self.ratio <= self.ratio_target };
                if done { self.ratio = self.ratio_target; self.ratio_step = 0.0; }
            }
        }
        self.shifters[ch].process(x, self.ratio, self.period)
    }

    fn analyse(&mut self) {
        // Linearise the ring into the preallocated frame: YIN wants a
        // contiguous buffer, and a copy is cheaper than teaching it to wrap.
        let split = self.ring.len() - self.ring_write;
        self.frame[..split].copy_from_slice(&self.ring[self.ring_write..]);
        self.frame[split..].copy_from_slice(&self.ring[..self.ring_write]);

        // Coarse search on a decimated copy. YIN is O(N × maxlag); at 48 kHz
        // with a 70 Hz floor that is ~2 million multiply-adds every 5 ms,
        // which is most of a core for one voice. Decimating by 4 cuts both
        // factors — ~16× less work — and costs nothing in accuracy here,
        // because the coarse estimate is only used to locate the period that
        // the full-rate refinement below then measures properly.
        let dec = &mut self.coarse[..self.frame.len() / DECIMATION];
        for (i, out) in dec.iter_mut().enumerate() {
            // Mean of the group, which is a crude but adequate anti-alias
            // for a detector that only needs the fundamental.
            let base = i * DECIMATION;
            let mut acc = 0.0;
            for k in 0..DECIMATION { acc += self.frame[base + k]; }
            *out = acc / DECIMATION as f32;
        }
        let dec_rate = self.sample_rate / DECIMATION as f32;
        // SILENCE skips the search: YIN cannot find a pitch in it (every
        // difference 0, the normalized curve flat at 1, no dip), and a vocal
        // bus is silent between phrases -- most of a song. Below -200 dB
        // (mean square 1e-20), which only a filter's decaying ring reaches.
        let energy: f32 = dec.iter().map(|v| v * v).sum();
        let silent = energy <= 1e-20 * dec.len() as f32;
        if silent { self.rest(); return; }
        let detected = if silent { None } else { yin_pitch(dec, dec_rate, self.lo_hz, self.hi_hz.min(dec_rate * 0.4), 0.15) }
            .map(|p| crate::retune::Pitch {
                hz: refine_period(&self.frame, self.sample_rate / p.hz)
                    .map(|period| self.sample_rate / period)
                    .unwrap_or(p.hz),
                clarity: p.clarity,
            });
        let Some(p) = detected.filter(|p| p.clarity >= self.voiced_threshold) else {
            // Unvoiced: pass through at unity and FREEZE the correction, so
            // the next voiced moment resumes the curve instead of restarting
            // it. (Same call Retune makes; see its docs.)
            self.ratio_target = 1.0;
            self.ratio_step = (1.0 - self.ratio) / self.hop as f32;
            self.last_target = None;
            self.period = None;
            return;
        };
        self.period = Some(self.sample_rate / p.hz);
        let target_hz = self.target.nearest_hz(p.hz);
        let raw = 1200.0 * (target_hz / p.hz).log2();
        if self.slide && self.last_target.is_none_or(|t| (t - target_hz).abs() > 0.5) {
            self.cents = 0.0; // a new note starts as sung, then slides in
        }
        self.last_target = Some(target_hz);
        let want_cents = if raw.abs() <= self.tolerance_cents { 0.0 } else { raw * self.amount.clamp(0.0, 1.0) };
        let hop_ms = self.hop as f32 / self.sample_rate * 1000.0;
        let a = if self.retune_speed_ms <= 0.0 {
            1.0
        } else {
            1.0 - (-hop_ms / self.retune_speed_ms).exp()
        };
        self.cents += (want_cents - self.cents) * a;
        self.ratio_target = 2f32.powf(self.cents / 1200.0);
        self.ratio_step = (self.ratio_target - self.ratio) / self.hop as f32;
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
        assert!(cents_between(got, 440.0).abs() < 5.0,
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
        assert!(to_c5 < 5.0, "landed at {got:.2} Hz ({to_c5:.1} cents off C5)");
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
        assert!(cents_between(got, 440.0).abs() < 3.0, "left at {got:.2} Hz");
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
    fn the_applied_ratio_ramps_instead_of_stepping_at_hop_boundaries() {
        // A ratio that jumped at each analysis boundary steps the delay
        // line's read rate, which is audible as a tick every 5 ms on a
        // sustained note. Watch the applied ratio across the settling
        // region: no single sample may move it more than one hop's worth.
        let rate = 48_000.0;
        let mut t = RealtimeTune::new(Target::Notes(vec![69]), rate, 1);
        t.retune_speed_ms = 0.0; // hard snap: the worst case for stepping
        let input = sine(452.0, 0.5, rate);
        let mut prev = t.ratio;
        let mut worst = 0.0f32;
        for x in &input {
            t.process(*x);
            worst = worst.max((t.ratio - prev).abs());
            prev = t.ratio;
        }
        // One hop's full correction spread over `hop` samples; a step would
        // show up as the entire ~2.7% ratio change in a single sample.
        assert!(worst < 0.001, "largest single-sample ratio jump was {worst}");
        assert!((t.ratio - 0.9735).abs() < 0.005, "it still got there: {}", t.ratio);
    }

    #[test]
    fn analysis_runs_far_faster_than_realtime() {
        // A monitoring insert has to fit in a callback with room to spare,
        // and the detector is the expensive part: full-rate YIN at a 70 Hz
        // floor is ~2M multiply-adds per 5 ms hop, which is most of a core
        // for ONE voice. The decimated coarse search plus a local fine
        // search is what makes this affordable. Generous bound so the test
        // reports a real regression rather than machine noise.
        let rate = 48_000.0;
        let mut t = RealtimeTune::new(Target::Scale(Scale { root: 9, mode: Mode::Minor }), rate, 2);
        let input = sine(220.0, 1.0, rate);
        let start = std::time::Instant::now();
        for x in &input {
            t.process(*x);
            t.process(*x); // stereo: two calls per frame
        }
        let took = start.elapsed().as_secs_f32();

        // CALIBRATED, not absolute. The bound used to be a flat 0.25 s,
        // which measures the MACHINE rather than the code: the same debug
        // build takes 0.13 s on an x86_64 sandbox and 0.961 s on an Asahi
        // aarch64 laptop, so the fixed bound passed on one and reported a
        // "regression" on the other with the algorithm untouched. A perf
        // test that fails on slow hardware trains people to ignore it,
        // which costs more than the regression it was meant to catch.
        //
        // So: time a fixed float workload on THIS machine, and require
        // the tuner to stay within a multiple of it. That still catches a
        // real algorithmic regression — going from a decimated coarse
        // search back to a full one is an order of magnitude, not a
        // factor of two — while surviving a machine that is uniformly 7x
        // slower.
        let cal_start = std::time::Instant::now();
        let mut acc = 0.0f32;
        for i in 0..2_000_000u32 {
            acc += (i as f32).sqrt().sin();
        }
        std::hint::black_box(acc);
        let cal = cal_start.elapsed().as_secs_f32().max(1e-6);

        let ratio = took / cal;
        assert!(
            ratio < 12.0,
            "1 s of stereo took {took:.3} s against a {cal:.3} s calibration \
             (ratio {ratio:.1}, limit 12; this machine measured ~1.9 when healthy) — that is an algorithmic regression, \
             not a slow machine"
        );
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

    fn run(t: &mut RealtimeTune, hz: f32, secs: f32) {
        for x in sine(hz, secs, 48_000.0) { t.process(x); }
    }
    fn chromatic(speed: f32) -> RealtimeTune {
        let mut t = RealtimeTune::new(Target::Scale(crate::retune::Scale { root: 0, mode: Mode::Chromatic }), 48_000.0, 1);
        t.retune_speed_ms = speed;
        t
    }

    #[test]
    fn slide_starts_each_new_note_as_sung_and_glides_in_hard_carries_on() {
        // 452 Hz wants about -47 cents (to A4); 485 Hz about +31 (to B4) -- a different
        // note. (479 Hz was the first choice: it is nearest A#4, ALSO -47 -- no change to see.)
        let (mut slide, mut held) = (chromatic(150.0), chromatic(150.0));
        slide.slide = true;
        for t in [&mut slide, &mut held] { run(t, 452.0, 1.0); }
        assert!((slide.correction_cents() + 47.0).abs() < 10.0, "settled on the first note: {}", slide.correction_cents());
        for t in [&mut slide, &mut held] { run(t, 485.0, 0.06); }
        assert!(slide.correction_cents().abs() < 25.0, "just after the change the new note is barely corrected: {}", slide.correction_cents());
        assert!(held.correction_cents() < -10.0, "without slide the OLD correction is still pulling: {}", held.correction_cents());
        run(&mut slide, 485.0, 1.0);
        assert!((slide.correction_cents() - 31.0).abs() < 10.0, "and then it glides into key: {}", slide.correction_cents());
    }

    #[test]
    fn tolerance_leaves_close_notes_alone_and_amount_corrects_part_way() {
        let mut t = chromatic(0.0);
        t.tolerance_cents = 30.0;
        run(&mut t, 445.0, 0.5); // ~ +20 cents off A4
        assert!(t.correction_cents().abs() < 1e-3, "{}", t.correction_cents());
        let mut u = chromatic(0.0);
        run(&mut u, 445.0, 0.5);
        assert!((u.correction_cents() + 20.0).abs() < 8.0, "no tolerance: corrected, {}", u.correction_cents());
        let mut h = chromatic(0.0);
        h.amount = 0.5;
        run(&mut h, 452.0, 0.5);
        assert!((h.correction_cents() + 23.5).abs() < 6.0, "half of -47: {}", h.correction_cents());
    }
}

#[cfg(test)]
mod level_tests {
    use super::*;

    /// CORRECTING must not cost level. A tone 40 cents flat of a C-major note,
    /// held 3 s: once the tuner is correcting, the output RMS should match the
    /// input's. Found through Goat: its "voxmel" vocal came out 8 dB quieter
    /// in a whole-song render (tuner locked and correcting) than in a render
    /// started mid-song (not locked yet, passing dry). The shifter's two taps
    /// sit a FIXED half-window (11 ms) apart; for a periodic voice they are
    /// often out of phase and cancel. Returns the level change, dB.
    pub fn correction_level_db(hz: f32) -> f32 {
        let rate = 48_000.0f32;
        let x: Vec<f32> = (0..(rate * 3.0) as usize).map(|i| 0.3 * (std::f32::consts::TAU * hz * i as f32 / rate).sin()).collect();
        level_while_correcting(&x, rate).0
    }

    /// (level change dB over the last third, mean |correction| in cents over
    /// that third) -- the second so a test can prove the tuner WAS
    /// correcting: an idle tuner keeps the level trivially.
    fn level_while_correcting(x: &[f32], rate: f32) -> (f32, f32) {
        let mut t = RealtimeTune::new(Target::Notes(vec![]), rate, 1);
        let n = x.len();
        let (mut c, mut m) = (0.0f64, 0usize);
        let y: Vec<f32> = x.iter().enumerate().map(|(i, v)| {
            let o = t.process(*v);
            if i >= n * 2 / 3 { c += t.correction_cents().abs() as f64; m += 1; }
            o
        }).collect();
        let tail = |v: &[f32]| (v[n * 2 / 3..].iter().map(|s| s * s).sum::<f32>() / (n - n * 2 / 3) as f32).sqrt();
        (20.0 * (tail(&y) / tail(x)).log10(), (c / m.max(1) as f64) as f32)
    }

    /// 3 s of a voice-like note 30 cents flat of `base`: eight harmonics (1/k),
    /// 5 Hz +-15 cent vibrato.
    fn vibrato_note(base: f32, rate: f32) -> Vec<f32> {
        let hz0 = base * 2f32.powf(-0.3 / 12.0);
        let mut ph = 0.0f32;
        (0..(rate * 3.0) as usize).map(|i| {
            let t = i as f32 / rate;
            ph += hz0 * 2f32.powf(15.0 / 1200.0 * (std::f32::consts::TAU * 5.0 * t).sin()) / rate;
            (1..=8).map(|k| 0.15 / k as f32 * (std::f32::consts::TAU * ph * k as f32).sin()).sum::<f32>()
        }).collect()
    }

    /// A VOICE-like note: eight harmonics (1/k), 30 cents flat, with a 5 Hz
    /// +-15 cent vibrato (inside one note's catchment, so the target holds)
    /// -- each harmonic cancels at its own phase under a fixed tap spacing,
    /// and the period keeps moving. Level held to 1.5 dB, and the tuner
    /// provably correcting (mean > 15 cents applied).
    #[test]
    fn a_harmonic_note_with_vibrato_keeps_its_level_while_corrected() {
        let rate = 48_000.0f32;
        for base in [196.0f32, 247.0, 311.0] {
            let x = vibrato_note(base, rate);
            let (g, cents) = level_while_correcting(&x, rate);
            eprintln!("VOICE {base} Hz: {g:+.2} dB, mean correction {cents:.0} cents");
            assert!(cents > 15.0, "{base} Hz: the tuner must be correcting (mean {cents:.0} cents)");
            assert!(g.abs() < 1.5, "{base} Hz voice-like note: {g:+.1} dB while correcting");
        }
    }

    #[test]
    fn correcting_keeps_the_level_across_pitches() {
        for semis in [0.0f32, 2.0, 4.0, 7.0, 9.0, 11.0, 14.0] {
            let hz = 220.0 * 2f32.powf((semis - 0.4) / 12.0);
            let x: Vec<f32> = (0..144_000).map(|i| 0.3 * (std::f32::consts::TAU * hz * i as f32 / 48_000.0).sin()).collect();
            let (g, cents) = level_while_correcting(&x, 48_000.0);
            assert!((30.0..50.0).contains(&cents), "{hz:.1} Hz: correcting the 40 cents ({cents:+.1})");
            assert!(g.abs() < 1.5, "{hz:.1} Hz: {g:+.1} dB while correcting");
        }
    }

    /// Re-seating a tap happens only where its weight is zero, so a note
    /// corrected across many wraps stays click-free: the output's largest
    /// second difference (where a click shows) stays near the input's. The
    /// signal is the VIBRATO note: on a steady tone a wrap jumps a whole
    /// number of periods and is seamless even unweighted, so a steady tone
    /// cannot show a click here (the break test found that).
    #[test]
    fn re_seating_the_taps_does_not_click() {
        let rate = 48_000.0f32;
        for base in [196.0f32, 247.0, 311.0] {
            let x = vibrato_note(base, rate);
            let hz = base;
            let mut t = RealtimeTune::new(Target::Notes(vec![]), rate, 1);
            let y: Vec<f32> = x.iter().map(|v| t.process(*v)).collect();
            let d2 = |v: &[f32]| v.windows(3).skip(rate as usize / 2).map(|w| (w[0] - 2.0 * w[1] + w[2]).abs()).fold(0.0f32, f32::max);
            let (dx, dy) = (d2(&x), d2(&y));
            eprintln!("CLICK {hz} Hz: output d2 {dy:.2e}, input {dx:.2e} ({:.2}x)", dy / dx);
            assert!(dy < 3.0 * dx, "{hz} Hz: output second difference {dy:.2e} vs input {dx:.2e} -- a click");
        }
    }

    /// The silence shortcut in `analyse` rests on this: YIN finds no pitch
    /// in silence, so skipping it there changes nothing.
    #[test]
    fn yin_finds_no_pitch_in_silence() {
        let dec_rate = 48_000.0 / DECIMATION as f32;
        assert!(yin_pitch(&[0.0f32; 700], dec_rate, 70.0, 1_000.0, 0.15).is_none());
        assert!(yin_pitch(&[1e-12f32; 700], dec_rate, 70.0, 1_000.0, 0.15).is_none(), "a DC floor either");
    }

    /// A render that starts mid-song (a seek, a parallel export's segment)
    /// must end up where one from the start is: aligned to the song position,
    /// and past a silent gap, the two tuners' outputs are bit-identical.
    #[test]
    fn after_a_silence_a_tuner_started_mid_song_matches_one_from_the_start() {
        let rate = 48_000usize;
        let x: Vec<f32> = (0..rate * 5).map(|i| {
            let t = i as f32 / rate as f32;
            if (2.0..2.5).contains(&t) { 0.0 } else { 0.3 * (std::f32::consts::TAU * 213.0 * t).sin() }
        }).collect();
        let mut whole = RealtimeTune::new(Target::Notes(vec![]), rate as f32, 1);
        whole.align(0);
        let a: Vec<f32> = x.iter().map(|v| whole.process(*v)).collect();
        let start = rate + 777; // mid-song, off the hop grid
        let mut late = RealtimeTune::new(Target::Notes(vec![]), rate as f32, 1);
        late.align(start as i64);
        let b: Vec<f32> = x[start..].iter().map(|v| late.process(*v)).collect();
        let from = rate * 5 / 2 + rate / 10; // 2.6 s: past the silence
        assert!(a[from..].iter().zip(&b[from - start..]).all(|(p, q)| p.to_bits() == q.to_bits()), "identical after the gap");
        // (and before the gap they are allowed to differ: different histories)
        assert!(a[start + 4_800..rate * 2].iter().zip(&b[4_800..rate * 2 - start]).any(|(p, q)| p != q), "the test sees a difference before the gap");
    }

    /// Records the measured loss per pitch (always passes; prints).
    #[test]
    fn measure_the_correction_level_loss() {
        for semis in [0.0f32, 2.0, 4.0, 7.0, 9.0, 11.0, 14.0] {
            let hz = 220.0 * 2f32.powf((semis - 0.4) / 12.0);
            eprintln!("LOSS {hz:7.1} Hz: {:+.1} dB", correction_level_db(hz));
        }
    }
}
