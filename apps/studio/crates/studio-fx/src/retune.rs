//! retune — pitch detection and scale-snap correction targets for autotune.
//!
//! ## Scope, stated plainly
//!
//! This module answers two questions, frame by frame: *what pitch is this*,
//! and *what pitch should it become*. It does **not** shift audio. Turning a
//! correction ratio into resynthesized samples is TD-PSOLA — epoch marking on
//! the detected periods, then an overlap-add that reads source periods at a
//! different spacing than it writes them — and that is real, separate DSP
//! work this pass does not include. Building the brain (detect, decide,
//! smooth, humanize) first and being honest that the hands (resynthesis)
//! aren't attached yet is the same call `studio-io` made about its cpal
//! backend: a partial piece clearly labelled partial, not a full pretence.
//!
//! What's here is still the harder half to get right. A wrong pitch estimate
//! or a correction that fires on an unvoiced consonant is what makes cheap
//! autotune sound cheap; the actual pitch-shift math downstream of a correct
//! decision is comparatively mechanical.
//!
//! ## Detection: YIN, not autocorrelation
//!
//! Plain autocorrelation tends to lock onto the wrong period on sounds with a
//! strong first harmonic (octave errors). YIN fixes this with a **cumulative
//! mean normalized difference function**: the difference function at each lag
//! is divided by its own running average, which suppresses the bias toward
//! short-but-plausible lags that causes octave errors, and gives a
//! `0`-at-a-perfect-period, `1`-at-total-mismatch scale that doubles as a
//! confidence measure (`clarity` below) — a genuinely unvoiced or noisy frame
//! simply never dips below the threshold, and that absence of a fit *is* the
//! voicing decision, not a separate classifier bolted on afterward.
//!
//! ## Correction: cents, not Hz
//!
//! A fixed Hz offset is the wrong unit for musical pitch — the same 5Hz error
//! is enormous in the bass and inaudible an octave up. Everything here works
//! in cents (1/100 of a semitone, log-frequency), which is also what makes
//! `retune_speed_ms` meaningful as a single number across the whole vocal
//! range: a one-pole filter in the cents domain, where `0` is deliberately
//! the genre-defining hard snap — the reason this effect exists at all — and
//! larger values are a gentler, more natural-sounding correction.

use studio_core::Ticks;

/// A1..=G#9-ish is plenty for a vocal; callers pass their own range so a
/// bass instrument or a squeaky ad-lib isn't forced through vocal defaults.
pub const DEFAULT_MIN_HZ: f32 = 70.0;
pub const DEFAULT_MAX_HZ: f32 = 1000.0;

/// One frame's pitch estimate: the detected fundamental, and how well the
/// signal actually fit a single period (`0` = no fit at all, `1` = exact).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pitch {
    pub hz: f32,
    pub clarity: f32,
}

/// YIN pitch detection over one frame.
///
/// `threshold` is the CMND absolute-threshold from the YIN paper — the first
/// dip below it (then walked to its local minimum) is accepted as the period;
/// 0.10–0.15 is the paper's own suggested range. Returns `None` when the
/// frame is too short for `lo_hz`'s period to fit, or when the CMND never
/// dips below `threshold` anywhere in range — a frame with no periodic
/// content at all, not merely a bad estimate.
/// sum of (a[i] - b[i])^2 in eight independent lanes: one long chain of
/// float adds cannot be reordered, so it cannot use SIMD; eight can (SSE /
/// AVX / NEON). The difference function is YIN's whole cost -- run every 5 ms
/// by every live autotune. Summation order differs from a single chain:
/// float rounding only.
#[inline]
pub fn sq_diff_sum(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    let (a, b) = (&a[..n], &b[..n]);
    let mut acc = [0.0f32; 8];
    let (ca, cb) = (a.chunks_exact(8), b.chunks_exact(8));
    let (ra, rb) = (ca.remainder(), cb.remainder());
    for (x, y) in ca.zip(cb) {
        for k in 0..8 { let d = x[k] - y[k]; acc[k] += d * d; }
    }
    let mut tail = 0.0f32;
    for (x, y) in ra.iter().zip(rb) { let d = x - y; tail += d * d; }
    acc.iter().sum::<f32>() + tail
}

pub fn yin_pitch(frame: &[f32], sample_rate: f32, lo_hz: f32, hi_hz: f32, threshold: f32) -> Option<Pitch> {
    let max_lag = (sample_rate / lo_hz.max(1.0)) as usize;
    let min_lag = ((sample_rate / hi_hz.max(1.0)) as usize).max(1);
    if frame.len() <= max_lag + 1 || min_lag >= max_lag {
        return None;
    }

    // Difference function: d(tau) = sum over i of (x[i] - x[i+tau])^2.
    let mut d = vec![0.0f32; max_lag + 1];
    let usable = frame.len() - max_lag;
    for tau in 1..=max_lag {
        d[tau] = sq_diff_sum(&frame[..usable], &frame[tau..tau + usable]);
    }

    // Cumulative mean normalized difference: d(tau) scaled by tau over the
    // running average of d(1..=tau) — this is what suppresses octave errors.
    let mut cmnd = vec![1.0f32; max_lag + 1];
    let mut running_sum = 0.0f32;
    for tau in 1..=max_lag {
        running_sum += d[tau];
        cmnd[tau] = if running_sum > 0.0 { d[tau] * tau as f32 / running_sum } else { 1.0 };
    }

    // Absolute threshold: first dip below `threshold`, walked forward to its
    // local minimum — the YIN paper's step 4, which avoids locking onto the
    // very first (noisy) sample under the threshold.
    let mut chosen = None;
    let mut tau = min_lag;
    while tau <= max_lag {
        if cmnd[tau] < threshold {
            let mut t = tau;
            while t + 1 <= max_lag && cmnd[t + 1] < cmnd[t] {
                t += 1;
            }
            chosen = Some(t);
            break;
        }
        tau += 1;
    }
    let tau = chosen?;

    // Parabolic interpolation around the chosen lag for sub-sample (and
    // therefore sub-Hz) accuracy — without it, pitch estimates quantize to
    // whole-sample lags, which is audible as a stepped, slightly-off correction.
    let t0 = tau.saturating_sub(1).max(1);
    let t2 = (tau + 1).min(max_lag);
    let (d0, d1, d2) = (cmnd[t0], cmnd[tau], cmnd[t2]);
    let denom = 2.0 * (2.0 * d1 - d0 - d2);
    let refined_tau = if denom.abs() > 1e-9 {
        tau as f32 + (d0 - d2) / denom
    } else {
        tau as f32
    };
    if refined_tau <= 0.0 {
        return None;
    }

    Some(Pitch { hz: sample_rate / refined_tau, clarity: (1.0 - cmnd[tau].min(1.0)).max(0.0) })
}

/// A scale to snap to. `root` is a pitch class, `0..=11`, `C=0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scale {
    pub root: u8,
    pub mode: Mode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Natural minor: the trap/drill default. Semitone offsets from root:
    /// 0, 2, 3, 5, 7, 8, 10.
    Minor,
    /// Raised 7th. Semitone offsets: 0, 2, 3, 5, 7, 8, 11.
    HarmonicMinor,
    /// Every semitone — always available as a fallback, and the correct
    /// choice for material that isn't meaningfully "in a key" at all.
    Chromatic,
}

impl Mode {
    fn degrees(self) -> &'static [i32] {
        match self {
            Mode::Minor => &[0, 2, 3, 5, 7, 8, 10],
            Mode::HarmonicMinor => &[0, 2, 3, 5, 7, 8, 11],
            Mode::Chromatic => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
        }
    }
}

fn hz_to_midi(hz: f32) -> f32 {
    12.0 * (hz / 440.0).log2() + 69.0
}
fn midi_to_hz(midi: f32) -> f32 {
    440.0 * 2f32.powf((midi - 69.0) / 12.0)
}

impl Scale {
    /// The nearest scale degree to `hz`, as a frequency. Works in continuous
    /// MIDI-note space (not rounded to the nearest semitone first) so a
    /// severely detuned input still snaps to the closest *degree*, not the
    /// closest semitone that happens to also be a degree.
    pub fn nearest_hz(&self, hz: f32) -> f32 {
        let midi = hz_to_midi(hz);
        let degrees = self.mode.degrees();
        // Search the octave containing `midi` plus one either side — enough
        // to guarantee the true nearest degree is considered even when
        // `midi` sits just above/below an octave boundary near a degree in
        // the neighbouring octave.
        let center_octave = ((midi - self.root as f32) / 12.0).floor() as i32;
        let mut best = f32::INFINITY;
        let mut best_midi = midi;
        for octave in (center_octave - 1)..=(center_octave + 1) {
            for &deg in degrees {
                let candidate = self.root as f32 + deg as f32 + 12.0 * octave as f32;
                let dist = (candidate - midi).abs();
                if dist < best {
                    best = dist;
                    best_midi = candidate;
                }
            }
        }
        midi_to_hz(best_midi)
    }
}

/// The live correction engine: takes detected pitch frames, decides how far
/// to move, and hands back a **linear pitch ratio** (`1.0` = no change) a
/// future shifter would apply to that frame. Owns the one-pole smoothing
/// state in the cents domain across calls, which is why this is a struct and
/// `yin_pitch`/`Scale::nearest_hz` are free functions — smoothing has memory,
/// detection and scale-snapping don't.
#[derive(Debug, Clone)]
pub struct Retune {
    pub scale: Scale,
    /// 0 = hard snap (no smoothing at all). Larger values glide toward the
    /// target more slowly, in milliseconds to traverse roughly 1/e of the
    /// remaining distance — the standard one-pole reading of a "speed" in ms.
    pub retune_speed_ms: f32,
    /// Frames with clarity below this are treated as unvoiced: passthrough,
    /// no state update. 0.1–0.2 is the practical range for sung vocals.
    pub voiced_threshold: f32,
    /// Bounded cents of deterministic pseudo-random wobble added on top of
    /// the corrected pitch, for a less robotic result. `0` disables it.
    pub humanize_cents: f32,
    smoothed_cents: f32,
    frame_index: u64,
    seed: u64,
}

impl Retune {
    pub fn new(scale: Scale, seed: u64) -> Retune {
        Retune {
            scale,
            retune_speed_ms: 0.0,
            voiced_threshold: 0.15,
            humanize_cents: 0.0,
            smoothed_cents: 0.0,
            frame_index: 0,
            seed,
        }
    }

    /// Process one detection result (`None` for a frame `yin_pitch` couldn't
    /// estimate at all) and return the pitch ratio to apply. `hop_ms` is how
    /// much audio this frame advances by — needed to convert
    /// `retune_speed_ms` (a time) into a per-frame smoothing coefficient.
    ///
    /// An unvoiced frame — no detection, or clarity below
    /// `voiced_threshold` — returns exactly `1.0` and leaves all internal
    /// state untouched. This is deliberate and tested: smearing sibilance
    /// with a hangover correction from the last voiced frame is the classic
    /// PSOLA-autotune tell, and freezing state (rather than decaying it
    /// toward 1.0) means the *next* voiced frame resumes from wherever the
    /// correction curve actually was, not from a reset.
    pub fn process(&mut self, detected: Option<Pitch>) -> f32 {
        self.process_with_hop(detected, 10.0)
    }

    pub fn process_with_hop(&mut self, detected: Option<Pitch>, hop_ms: f32) -> f32 {
        self.frame_index += 1;
        let Some(pitch) = detected else { return 1.0 };
        if pitch.clarity < self.voiced_threshold || pitch.hz <= 0.0 {
            return 1.0;
        }

        let target_hz = self.scale.nearest_hz(pitch.hz);
        let target_cents = 1200.0 * (target_hz / pitch.hz).log2();

        if self.retune_speed_ms <= 0.0 {
            // Hard snap: no smoothing state to carry, by definition.
            self.smoothed_cents = target_cents;
        } else {
            // One-pole toward target_cents; alpha derived from how many hops
            // fit in retune_speed_ms so the perceived speed is independent
            // of hop size.
            let hops = (self.retune_speed_ms / hop_ms.max(0.1)).max(1.0);
            let alpha = 1.0 - (-1.0f32 / hops).exp();
            self.smoothed_cents += (target_cents - self.smoothed_cents) * alpha;
        }

        let humanize = if self.humanize_cents > 0.0 {
            let mut rng = core_kit::rng::SplitMix64::new(self.seed ^ self.frame_index);
            (rng.next_f64() as f32 - 0.5) * 2.0 * self.humanize_cents
        } else {
            0.0
        };

        2f32.powf((self.smoothed_cents + humanize) / 1200.0)
    }

    pub fn reset(&mut self) {
        self.smoothed_cents = 0.0;
        self.frame_index = 0;
    }
}

/// A hop size in frames, and the corresponding time in ms — the pairing a
/// caller needs to convert an audio callback's buffer size into the units
/// `Retune::process_with_hop` wants. Not load-bearing logic, just arithmetic
/// callers would otherwise duplicate at every call site.
pub fn hop_ms(hop_frames: usize, sample_rate: f32) -> f32 {
    Ticks::from_samples(hop_frames as i64, sample_rate as u32).to_seconds() as f32 * 1000.0
}

#[cfg(test)]
mod tests {
    /// The eight-lane sum is the plain sum of squared differences, to float
    /// rounding, at every remainder length (0..7 past a multiple of eight).
    #[test]
    fn the_lane_sum_is_the_sum_of_squared_differences() {
        let x: Vec<f32> = (0..300).map(|i| ((i * 7919) % 1000) as f32 / 500.0 - 1.0).collect();
        for n in [0usize, 1, 7, 8, 9, 63, 64, 65, 250] {
            let want: f64 = (0..n).map(|i| ((x[i] - x[i + 37]) as f64).powi(2)).sum();
            let got = sq_diff_sum(&x[..n], &x[37..37 + n]) as f64;
            assert!((got - want).abs() <= 1e-5 * want.max(1.0), "n {n}: {got} vs {want}");
        }
    }

    use super::*;

    fn sine(hz: f32, sr: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| (2.0 * std::f32::consts::PI * hz * i as f32 / sr).sin()).collect()
    }

    #[test]
    fn yin_detects_a_pure_tones_frequency_accurately() {
        let sr = 48_000.0;
        // 55Hz is deliberately excluded: it's below DEFAULT_MIN_HZ (70), so
        // its ~873-sample period falls outside the search range by design —
        // testing it here would be testing the wrong thing.
        for hz in [110.0, 220.0, 440.0] {
            let frame = sine(hz, sr, 4096);
            let p = yin_pitch(&frame, sr, DEFAULT_MIN_HZ, DEFAULT_MAX_HZ, 0.15).expect("should detect a clean tone");
            assert!((p.hz - hz).abs() < 1.0, "expected ~{hz}Hz, got {}", p.hz);
            assert!(p.clarity > 0.9, "a pure tone should read very high clarity, got {}", p.clarity);
        }
    }

    #[test]
    fn yin_detects_a_low_tone_when_the_range_is_widened_to_cover_it() {
        let sr = 48_000.0;
        let frame = sine(55.0, sr, 8192);
        let p = yin_pitch(&frame, sr, 40.0, DEFAULT_MAX_HZ, 0.15).expect("55Hz is in range once lo_hz covers it");
        assert!((p.hz - 55.0).abs() < 1.0, "expected ~55Hz, got {}", p.hz);
    }

    #[test]
    fn yin_does_not_octave_error_on_a_strong_first_harmonic() {
        // A fundamental plus a LOUD first harmonic is the classic case naive
        // autocorrelation gets wrong (locks onto the harmonic's period,
        // reporting double the true pitch).
        let sr = 48_000.0;
        let n = 4096;
        let fundamental = 130.0;
        let frame: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / sr;
                (2.0 * std::f32::consts::PI * fundamental * t).sin()
                    + 0.8 * (2.0 * std::f32::consts::PI * fundamental * 2.0 * t).sin()
            })
            .collect();
        let p = yin_pitch(&frame, sr, DEFAULT_MIN_HZ, DEFAULT_MAX_HZ, 0.15).expect("should still detect");
        assert!((p.hz - fundamental).abs() < 2.0, "expected the fundamental {fundamental}Hz, got {} (an octave error would read ~{})", p.hz, fundamental * 2.0);
    }

    #[test]
    fn yin_reports_low_clarity_or_no_detection_on_noise() {
        let sr = 48_000.0;
        let mut rng = core_kit::rng::SplitMix64::new(42);
        let frame: Vec<f32> = (0..4096).map(|_| (rng.next_f64() as f32 - 0.5) * 2.0).collect();
        match yin_pitch(&frame, sr, DEFAULT_MIN_HZ, DEFAULT_MAX_HZ, 0.15) {
            None => {} // no periodic content found at all — a valid outcome
            Some(p) => assert!(p.clarity < 0.5, "white noise should not read as a confident pitch, got clarity {}", p.clarity),
        }
    }

    #[test]
    fn yin_returns_none_for_a_frame_too_short_for_the_requested_range() {
        let sr = 48_000.0;
        let short = sine(440.0, sr, 32);
        assert_eq!(yin_pitch(&short, sr, DEFAULT_MIN_HZ, DEFAULT_MAX_HZ, 0.15), None);
    }

    #[test]
    fn scale_snaps_a_detuned_pitch_to_the_nearest_minor_degree() {
        // A minor, natural minor degrees from A(=69 midi): A B C D E F G.
        let scale = Scale { root: 9, mode: Mode::Minor }; // 9 = A
        // A sung slightly sharp of C (midi 72) should still land on C.
        let sharp_c = midi_to_hz(72.3);
        let snapped = scale.nearest_hz(sharp_c);
        assert!((snapped - midi_to_hz(72.0)).abs() < 0.5, "should snap to C, got {snapped}Hz vs {}Hz", midi_to_hz(72.0));
    }

    #[test]
    fn harmonic_minor_offers_the_raised_seventh_that_natural_minor_lacks() {
        let natural = Scale { root: 9, mode: Mode::Minor };
        let harmonic = Scale { root: 9, mode: Mode::HarmonicMinor };
        // G# (midi 80) IS a harmonic-minor degree (the raised 7th) but is
        // NOT a natural-minor degree at all — so at that exact pitch,
        // harmonic minor should match it exactly, while natural minor (whose
        // nearest options are G at 79 and A at 81) cannot.
        let g_sharp = midi_to_hz(80.0);
        let harm = harmonic.nearest_hz(g_sharp);
        let nat = natural.nearest_hz(g_sharp);
        assert!((harm - g_sharp).abs() < 0.5, "harmonic minor has G# as a real degree, should match exactly, got {harm}Hz");
        assert!((nat - g_sharp).abs() > 20.0, "natural minor has no G# degree, should NOT land on it, got {nat}Hz");
    }

    #[test]
    fn chromatic_snaps_to_the_nearest_semitone_full_stop() {
        let scale = Scale { root: 0, mode: Mode::Chromatic };
        let slightly_sharp = midi_to_hz(60.3);
        let snapped = scale.nearest_hz(slightly_sharp);
        assert!((snapped - midi_to_hz(60.0)).abs() < 0.5);
    }

    #[test]
    fn nearest_hz_handles_octave_boundary_correctly() {
        // A note just below the root's octave line must still find the
        // nearest degree in the octave ABOVE if that's genuinely closer —
        // the reason the search spans neighbouring octaves, not just one.
        let scale = Scale { root: 0, mode: Mode::Minor }; // C minor: C D Eb F G Ab Bb
        // midi 71 (B) is between Bb (70, this octave) and C (72, next
        // octave's root) — closer to Bb by 1 semitone vs C by 1 semitone,
        // effectively tied; nudge toward C (71.6) to make C unambiguously closer.
        let near_next_octave_root = midi_to_hz(71.6);
        let snapped = scale.nearest_hz(near_next_octave_root);
        assert!((snapped - midi_to_hz(72.0)).abs() < 0.5, "should reach into the next octave for C, got {snapped}Hz");
    }

    #[test]
    fn hard_snap_at_speed_zero_lands_within_a_few_cents_after_one_frame() {
        let scale = Scale { root: 9, mode: Mode::Minor }; // A minor
        let mut r = Retune::new(scale, 1);
        r.retune_speed_ms = 0.0;
        // Sung ~30 cents sharp of A (midi 69).
        let detuned_hz = midi_to_hz(69.0 + 0.30);
        let ratio = r.process(Some(Pitch { hz: detuned_hz, clarity: 0.9 }));
        let corrected_hz = detuned_hz * ratio;
        let cents_off = 1200.0 * (corrected_hz / midi_to_hz(69.0)).log2();
        assert!(cents_off.abs() <= 5.0, "speed=0 should land within 5 cents of the degree after one frame, got {cents_off} cents off");
    }

    #[test]
    fn unvoiced_frames_are_bit_exact_passthrough_and_never_touch_state() {
        let scale = Scale { root: 0, mode: Mode::Minor };
        let mut r = Retune::new(scale, 1);
        r.retune_speed_ms = 50.0;
        // Warm up with a voiced frame so there's real smoothed state...
        r.process(Some(Pitch { hz: 300.0, clarity: 0.9 }));
        let state_before = r.smoothed_cents;
        // ...then feed low-clarity and None frames.
        assert_eq!(r.process(Some(Pitch { hz: 300.0, clarity: 0.01 })), 1.0, "low clarity must passthrough exactly");
        assert_eq!(r.process(None), 1.0, "no detection must passthrough exactly");
        assert_eq!(r.smoothed_cents, state_before, "unvoiced frames must not touch the smoothing state at all");
    }

    #[test]
    fn nonzero_speed_converges_gradually_rather_than_snapping_immediately() {
        let scale = Scale { root: 9, mode: Mode::Minor };
        let mut r = Retune::new(scale, 1);
        r.retune_speed_ms = 200.0; // a slow, obviously-gliding correction
        let detuned_hz = midi_to_hz(69.0 + 0.5); // 50 cents sharp
        let first_ratio = r.process_with_hop(Some(Pitch { hz: detuned_hz, clarity: 0.9 }), 10.0);
        let first_corrected_cents = 1200.0 * (detuned_hz * first_ratio / midi_to_hz(69.0)).log2();
        assert!(first_corrected_cents.abs() > 5.0, "a slow correction should NOT already be within a few cents after one 10ms hop, got {first_corrected_cents}");

        // But it should keep closing the gap and eventually get close.
        let mut last_ratio = first_ratio;
        for _ in 0..500 {
            last_ratio = r.process_with_hop(Some(Pitch { hz: detuned_hz, clarity: 0.9 }), 10.0);
        }
        let final_cents = 1200.0 * (detuned_hz * last_ratio / midi_to_hz(69.0)).log2();
        assert!(final_cents.abs() < 2.0, "after many hops a slow correction should still converge close to the degree, got {final_cents} cents off");
    }

    #[test]
    fn humanize_adds_bounded_deterministic_wobble() {
        let scale = Scale { root: 9, mode: Mode::Minor };
        let mut a = Retune::new(scale, 42);
        a.retune_speed_ms = 0.0;
        a.humanize_cents = 8.0;
        let mut b = Retune::new(scale, 42);
        b.retune_speed_ms = 0.0;
        b.humanize_cents = 8.0;

        let detuned_hz = midi_to_hz(69.0);
        let mut a_ratios = Vec::new();
        let mut b_ratios = Vec::new();
        for _ in 0..20 {
            a_ratios.push(a.process(Some(Pitch { hz: detuned_hz, clarity: 0.9 })));
            b_ratios.push(b.process(Some(Pitch { hz: detuned_hz, clarity: 0.9 })));
        }
        assert_eq!(a_ratios, b_ratios, "the same seed must reproduce identical humanize wobble every time");
        // And it must actually vary rather than being a no-op constant.
        let distinct = a_ratios.windows(2).any(|w| (w[0] - w[1]).abs() > 1e-6);
        assert!(distinct, "humanize_cents > 0 should make consecutive frames differ");

        // Bound check: no single frame's cents deviation should exceed the
        // hard-snap target (0, since input is exactly on-degree) by more
        // than humanize_cents, with a little floating slack.
        for ratio in &a_ratios {
            let cents = 1200.0 * (ratio.ln() / 2f32.ln());
            assert!(cents.abs() <= 8.0 + 0.01, "humanize must stay within its bound, got {cents} cents");
        }
    }

    #[test]
    fn humanize_zero_is_a_true_no_op() {
        let scale = Scale { root: 9, mode: Mode::Minor };
        let mut r = Retune::new(scale, 7);
        r.retune_speed_ms = 0.0;
        r.humanize_cents = 0.0;
        let detuned_hz = midi_to_hz(69.0);
        let ratio = r.process(Some(Pitch { hz: detuned_hz, clarity: 0.9 }));
        assert_eq!(ratio, 1.0, "already on-degree with no humanize should be exactly unity, not approximately");
    }

    #[test]
    fn hop_ms_converts_frames_to_milliseconds() {
        assert!((hop_ms(480, 48_000.0) - 10.0).abs() < 1e-4);
        assert!((hop_ms(2400, 48_000.0) - 50.0).abs() < 1e-4);
    }
}
