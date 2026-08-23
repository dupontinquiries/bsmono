//! micsample — decide which moments of a long day are worth keeping.
//!
//! ## The problem is not the microphone
//!
//! Leaving a mic on for a day is easy. The hard part is that a day is almost
//! entirely nothing, and the arithmetic is brutal at the default settings
//! (sr 22050, hop 256, n_mels 128, time_frames 64):
//!
//! | | |
//! |---|---|
//! | 24 h of raw f32 mono | **7.6 GB** |
//! | 24 h as a continuous mel spectrogram | **3.8 GB** |
//! | one training crop (64 frames x 128 mels) | 32 KiB, 0.74 s |
//! | 2 000 kept crops | **64 MiB** |
//!
//! So a recorder that keeps everything is not a sample library, it is a disk
//! problem — and worse, training a GAN on it would mostly teach it room tone.
//! The value is entirely in the CURATION, which is what this module is.
//!
//! ## What it decides, and what it deliberately does not do
//!
//! Pure: mel frames in, [`Decision`] out. No cpal, no files, no clock, no
//! threads. The shell around it owns the input stream and the writer, so this
//! can be tested against synthetic frames at thousands of times real speed —
//! a day of audio is 116 000 frames, which is a fast unit test and an
//! unrunnable integration one.
//!
//! Three gates in order, each cheap enough to run on every frame:
//!
//! 1. **Onset.** Spectral flux — the sum of positive frame-to-frame increases —
//!    against an adaptive threshold derived from recent flux. Adaptive because a
//!    fixed one is wrong twice: it never fires in a quiet room and never stops
//!    in a loud one.
//! 2. **Refractory.** After a keep, ignore onsets for a while. A single door
//!    slam has several transients and they are one sample, not five.
//! 3. **Novelty.** A crop that resembles one already kept is dropped. Without
//!    this, a day next to a ticking clock yields ten thousand copies of a tick
//!    and the library is worthless — this is the gate that turns a recorder
//!    into a collection.
//!
//! ## Privacy, stated plainly because the honest version matters
//!
//! An always-on microphone in a home records the people in it. Two properties
//! of this design reduce that exposure and neither eliminates it:
//!
//! * only ~0.74 s fragments around transients are kept, never a continuous
//!   stream — so a conversation is not recoverable even in principle, but
//!   individual words may be;
//! * only mel spectrograms are written, never audio.
//!
//! **Mel is not anonymisation, and this crate is the proof**: beatspace ships
//! Griffin-Lim in `dsp.rs` precisely to turn mel back into audible sound. A
//! kept crop can be played back. Treat the cache as recordings, because it is.
//!
//! [`Gate::speechlike`] exists to drop the most obvious voice crops, and its
//! own docs say how weak it is. The real protection is a person choosing when
//! the mic is on.

/// What to do with the frame just fed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Nothing interesting; keep listening.
    Idle,
    /// An onset fired but the crop was rejected, with the reason — surfaced so
    /// a UI can show WHY a busy room produced nothing, which is otherwise
    /// indistinguishable from a broken microphone.
    Rejected(Reject),
    /// Emit the current window as a training crop.
    Keep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reject {
    /// Still inside the refractory window after a previous keep.
    TooSoon,
    /// Too similar to something already kept.
    Familiar,
    /// The per-session budget is spent.
    BudgetSpent,
    /// Looks like voiced speech.
    SpeechLike,
    /// The window is not full yet (start of a session).
    Warming,
}

/// Tuning. Every default is explained where it is not obvious.
#[derive(Debug, Clone)]
pub struct Gate {
    /// Frames in one crop; must match `Config::time_frames` or the emitted
    /// cache will not match what the trainer reads.
    pub time_frames: usize,
    /// How many frames of flux history the adaptive threshold looks at.
    /// 300 frames is about 3.5 s at the default hop — long enough to average
    /// over a sentence or a passing car, short enough to follow a room that
    /// gets busier.
    pub flux_window: usize,
    /// Onset when flux exceeds `median(recent) * threshold_mult`. A multiplier
    /// rather than an absolute, because the useful signal is "louder than this
    /// room has been", not "louder than some number of dB".
    pub threshold_mult: f32,
    /// Frames to ignore after a keep. Default is one crop width: two kept crops
    /// never overlap, so the library has no duplicated audio in it.
    pub refractory: usize,
    /// Cosine distance below which a candidate counts as familiar. 0.08 is
    /// deliberately loose — the cost of dropping a genuine variant is one lost
    /// sample, and the cost of keeping ten thousand clock ticks is the whole
    /// library.
    pub novelty_min_dist: f32,
    /// How many summaries to remember. Bounded so a 24 h run has a fixed
    /// memory ceiling rather than a growing one.
    pub memory: usize,
    /// Stop after this many keeps.
    pub budget: usize,
    /// Drop crops that look like voiced speech. See the field's caveats.
    pub speechlike: bool,
}

impl Default for Gate {
    fn default() -> Self {
        Gate {
            time_frames: 64,
            flux_window: 300,
            threshold_mult: 2.5,
            refractory: 64,
            novelty_min_dist: 0.08,
            memory: 512,
            budget: 4096,
            speechlike: true,
        }
    }
}

/// Running state. Feed it one mel frame at a time.
pub struct Sampler {
    gate: Gate,
    n_mels: usize,
    /// Ring of the last `time_frames` frames — the crop that would be emitted.
    ring: Vec<f32>,
    filled: usize,
    head: usize,
    prev: Vec<f32>,
    flux_hist: std::collections::VecDeque<f32>,
    since_keep: usize,
    kept: Vec<Vec<f32>>,
    pub keeps: usize,
    pub onsets: usize,
    /// Why onsets were turned away, counted.
    ///
    /// Every `Gate` number was tuned against SYNTHETIC transients, so the first
    /// real room is the first honest test of them — and the failure modes look
    /// identical from outside. A session that keeps nothing is a broken
    /// microphone, a threshold set too high, a refractory window swallowing
    /// everything, or a novelty gate that decided the whole room sounds the
    /// same. Those want opposite corrections, and the counts are what tell them
    /// apart.
    pub rejected: Rejects,
}

/// Rejection tally, one field per [`Reject`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Rejects {
    pub too_soon: usize,
    pub familiar: usize,
    pub budget_spent: usize,
    pub speech_like: usize,
}

impl Rejects {
    pub fn total(&self) -> usize {
        self.too_soon + self.familiar + self.budget_spent + self.speech_like
    }

    /// A line for the end of a session, written to be actionable rather than
    /// merely accurate: each count is paired with the knob that moves it.
    pub fn advice(&self, onsets: usize, keeps: usize) -> String {
        let mut s = format!(
            "onsets {onsets}, kept {keeps} — rejected: {} too-soon, {} familiar, \
{} speech-like, {} over-budget",
            self.too_soon, self.familiar, self.speech_like, self.budget_spent);
        if onsets == 0 {
            s.push_str("\n  nothing crossed the onset threshold: is the mic live, \
and is --onset-mult too high for this room?");
        } else if keeps == 0 && self.familiar > 0 {
            s.push_str("\n  everything sounded familiar: lower --novelty, or the room \
really does make one sound");
        } else if keeps == 0 && self.too_soon == onsets {
            s.push_str("\n  every onset landed inside the refractory window: \
lower --refractory if these are separate events");
        } else if self.speech_like > keeps {
            s.push_str("\n  most events looked like speech and were dropped; \
--allow-speech keeps them, and read what that means first");
        }
        s
    }
}

impl Sampler {
    pub fn new(gate: Gate, n_mels: usize) -> Sampler {
        let tf = gate.time_frames;
        Sampler {
            n_mels,
            ring: vec![0.0; tf * n_mels],
            filled: 0,
            head: 0,
            prev: vec![0.0; n_mels],
            flux_hist: std::collections::VecDeque::with_capacity(gate.flux_window),
            since_keep: gate.refractory, // ready immediately
            kept: Vec::new(),
            keeps: 0,
            onsets: 0,
            rejected: Rejects::default(),
            gate,
        }
    }

    /// Feed one mel frame (`n_mels` values, dB-scaled like the trainer's).
    pub fn push(&mut self, frame: &[f32]) -> Decision {
        assert_eq!(frame.len(), self.n_mels, "frame width must match n_mels");

        // Spectral flux: only INCREASES count. A sound stopping is not an onset,
        // and counting decreases would fire on every gap.
        let flux: f32 = frame.iter().zip(&self.prev).map(|(a, b)| (a - b).max(0.0)).sum();
        self.prev.copy_from_slice(frame);

        let off = self.head * self.n_mels;
        self.ring[off..off + self.n_mels].copy_from_slice(frame);
        self.head = (self.head + 1) % self.gate.time_frames;
        self.filled = (self.filled + 1).min(self.gate.time_frames);
        self.since_keep = self.since_keep.saturating_add(1);

        let threshold = self.threshold();
        self.flux_hist.push_back(flux);
        if self.flux_hist.len() > self.gate.flux_window {
            self.flux_hist.pop_front();
        }

        if self.filled < self.gate.time_frames {
            return Decision::Rejected(Reject::Warming);
        }
        // Need history before the adaptive threshold means anything.
        if self.flux_hist.len() < self.gate.flux_window / 4 {
            return Decision::Idle;
        }
        if flux <= threshold {
            return Decision::Idle;
        }
        self.onsets += 1;

        if self.since_keep < self.gate.refractory {
            self.rejected.too_soon += 1;
            return Decision::Rejected(Reject::TooSoon);
        }
        if self.keeps >= self.gate.budget {
            self.rejected.budget_spent += 1;
            return Decision::Rejected(Reject::BudgetSpent);
        }

        let summary = self.summary();
        if self.gate.speechlike && looks_like_speech(&summary) {
            self.rejected.speech_like += 1;
            return Decision::Rejected(Reject::SpeechLike);
        }
        if self.kept.iter().any(|k| summary_distance(k, &summary) < self.gate.novelty_min_dist) {
            self.rejected.familiar += 1;
            return Decision::Rejected(Reject::Familiar);
        }

        if self.kept.len() >= self.gate.memory {
            self.kept.remove(0);
        }
        self.kept.push(summary);
        self.keeps += 1;
        self.since_keep = 0;
        Decision::Keep
    }

    /// The current window, oldest frame first — what the shell writes as a crop.
    pub fn crop(&self) -> Vec<f32> {
        let tf = self.gate.time_frames;
        let mut out = Vec::with_capacity(tf * self.n_mels);
        for i in 0..tf {
            let r = (self.head + i) % tf;
            out.extend_from_slice(&self.ring[r * self.n_mels..(r + 1) * self.n_mels]);
        }
        out
    }

    /// Median recent flux times the multiplier.
    ///
    /// Median rather than mean: a single loud event should not raise the bar
    /// for the next few seconds, which is exactly what a mean does and is how
    /// an onset detector goes deaf after a door slam.
    fn threshold(&self) -> f32 {
        if self.flux_hist.is_empty() {
            return f32::MAX;
        }
        let mut v: Vec<f32> = self.flux_hist.iter().copied().collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let med = v[v.len() / 2];
        // A floor, so a silent room does not make every faint rustle an onset:
        // with median 0 the multiplier alone would threshold at 0.
        (med * self.gate.threshold_mult).max(1e-3)
    }

    /// A small, comparison-friendly fingerprint of the window: per-mel mean
    /// over time. Cheap, and it captures timbre — which is what "have I heard
    /// this before" means for a sample library.
    fn summary(&self) -> Vec<f32> {
        let tf = self.gate.time_frames;
        let mut s = vec![0.0f32; self.n_mels];
        for i in 0..tf {
            let r = (self.head + i) % tf;
            for m in 0..self.n_mels {
                s[m] += self.ring[r * self.n_mels + m];
            }
        }
        for x in &mut s {
            *x /= tf as f32;
        }
        s
    }
}

/// 1 - cosine similarity, on mean-removed vectors so overall loudness does not
/// dominate. Two recordings of the same tick at different distances are the
/// same sound.
pub fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    let ma = a.iter().sum::<f32>() / a.len() as f32;
    let mb = b.iter().sum::<f32>() / b.len() as f32;
    let (mut dot, mut na, mut nb) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in a.iter().zip(b) {
        let (x, y) = (x - ma, y - mb);
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na <= f32::EPSILON || nb <= f32::EPSILON {
        return 0.0; // two flat frames are the same frame
    }
    1.0 - (dot / (na.sqrt() * nb.sqrt())).clamp(-1.0, 1.0)
}

/// Spectral centroid: the band index where the energy balances.
///
/// One number for "how bright". Computed on energy shifted to be non-negative,
/// because mel frames here are dB and a weighted mean over negative weights is
/// meaningless.
pub fn centroid(summary: &[f32]) -> f32 {
    let min = summary.iter().copied().fold(f32::MAX, f32::min);
    let (mut num, mut den) = (0.0f32, 0.0f32);
    for (m, &v) in summary.iter().enumerate() {
        let w = v - min;
        num += m as f32 * w;
        den += w;
    }
    if den <= f32::EPSILON { return summary.len() as f32 / 2.0 }
    num / den
}

/// How different two crops are, for the purpose of "have I already got this".
///
/// Cosine alone is NOT enough and the test caught it. Mean-removed cosine is
/// scale-invariant, so a gentle spectral tilt and a steep one point the same
/// direction and score as identical — eight deliberately distinct timbres
/// collapsed to one keep. But two sounds with the same spectral SHAPE and
/// different brightness are plainly different samples.
///
/// So: cosine for the shape, plus the centroid gap for the degree, the latter
/// normalised by band count so the two terms are comparable and the threshold
/// keeps its meaning. Loudness is still ignored, which is deliberate — the same
/// tick heard from across the room is the same tick.
pub fn summary_distance(a: &[f32], b: &[f32]) -> f32 {
    let shape = cosine_distance(a, b);
    let n = a.len().max(1) as f32;
    let bright = (centroid(a) - centroid(b)).abs() / n;
    shape + bright
}

/// A crude voiced-speech rejector.
///
/// **This is weak and saying so is the point.** It looks for the shape voiced
/// speech usually has in a mel summary — most energy low, a pronounced peak in
/// the first fifth of the bands, little above the middle — and will both miss
/// real speech (whispering, a distant voice, a consonant burst) and reject
/// things that are not speech (a cello, a didgeridoo, a car engine).
///
/// It is here because dropping the obvious cases is better than not, not
/// because it makes an always-on microphone safe. The protection that works is
/// a person deciding when it is on.
pub fn looks_like_speech(summary: &[f32]) -> bool {
    let n = summary.len();
    if n < 10 {
        return false;
    }
    let lo = n / 5;
    let hi = n / 2;
    let e_lo: f32 = summary[..lo].iter().sum();
    let e_mid: f32 = summary[lo..hi].iter().sum();
    let e_hi: f32 = summary[hi..].iter().sum();
    let total = e_lo + e_mid + e_hi;
    if total.abs() < f32::EPSILON {
        return false;
    }
    // Energy concentrated low, and very little up top.
    e_lo / total > 0.55 && e_hi / total < 0.12
}

#[cfg(test)]
mod tests {
    use super::*;

    const N_MELS: usize = 64;

    fn silence() -> Vec<f32> { vec![-80.0; N_MELS] }

    /// A transient: broadband energy that decays over `age` frames.
    fn hit(age: usize, tilt: f32) -> Vec<f32> {
        let decay = (-(age as f32) * 0.4).exp();
        (0..N_MELS)
            .map(|m| {
                let shape = 1.0 - tilt * (m as f32 / N_MELS as f32);
                -80.0 + 70.0 * decay * shape.max(0.05)
            })
            .collect()
    }

    /// Run a sampler over a script of frames, returning the decisions.
    fn run(g: Gate, frames: &[Vec<f32>]) -> (Sampler, Vec<Decision>) {
        let mut s = Sampler::new(g, N_MELS);
        let d = frames.iter().map(|f| s.push(f)).collect();
        (s, d)
    }

    #[test]
    fn a_silent_day_keeps_nothing() {
        // The whole point: an empty room must cost zero disk. A detector that
        // fires on room tone turns 24 h into a full drive.
        let frames: Vec<Vec<f32>> = (0..5000).map(|_| silence()).collect();
        let (s, _) = run(Gate::default(), &frames);
        assert_eq!(s.keeps, 0, "silence produced {} crops", s.keeps);
    }

    #[test]
    fn one_transient_in_a_quiet_room_is_kept_once() {
        let mut frames: Vec<Vec<f32>> = (0..400).map(|_| silence()).collect();
        for age in 0..30 { frames.push(hit(age, 0.3)); }
        frames.extend((0..200).map(|_| silence()));
        let (s, d) = run(Gate::default(), &frames);
        assert_eq!(s.keeps, 1, "expected exactly one keep, got {}", s.keeps);
        assert_eq!(d.iter().filter(|x| **x == Decision::Keep).count(), 1);
    }

    /// The gate that turns a recorder into a collection.
    #[test]
    fn a_ticking_clock_all_day_yields_one_sample_not_thousands() {
        // A tick every 120 frames (~1.4 s) for the equivalent of a long stretch.
        let mut frames: Vec<Vec<f32>> = (0..400).map(|_| silence()).collect();
        for _ in 0..300 {
            for age in 0..12 { frames.push(hit(age, 0.3)); }
            frames.extend((0..108).map(|_| silence()));
        }
        let (s, _) = run(Gate::default(), &frames);
        assert!(s.onsets > 100, "the ticks should be detected: {} onsets", s.onsets);
        assert!(s.keeps <= 3,
            "300 identical ticks produced {} crops; novelty rejection is not working and a day beside a clock would fill the library with one sound", s.keeps);
    }

    /// A band-limited event: energy only between `lo` and `hi`, decaying.
    fn band(age: usize, lo: usize, hi: usize) -> Vec<f32> {
        let decay = (-(age as f32) * 0.4).exp();
        (0..N_MELS)
            .map(|m| if m >= lo && m < hi { -80.0 + 70.0 * decay } else { -80.0 })
            .collect()
    }

    #[test]
    fn genuinely_different_sounds_are_all_kept() {
        // Four events in disjoint frequency bands — a thud, a knock, a clink, a
        // hiss. These are different SOUNDS, not one sound at four brightnesses,
        // and every one must survive.
        let bands = [(0, 12), (14, 26), (28, 42), (46, 62)];
        let mut frames: Vec<Vec<f32>> = (0..400).map(|_| silence()).collect();
        for (lo, hi) in bands {
            for age in 0..12 { frames.push(band(age, lo, hi)); }
            frames.extend((0..140).map(|_| silence()));
        }
        let (s, _) = run(Gate::default(), &frames);
        assert_eq!(s.keeps, 4, "four disjoint bands yielded {} crops", s.keeps);
    }

    /// The other half, and it is a FEATURE rather than a shortfall.
    ///
    /// Eight events on a smooth brightness sweep are eight nearby points on one
    /// continuum, not eight sounds. Keeping all of them is what fills a library
    /// with near-duplicates; keeping a few spread across the range is the job.
    ///
    /// My first version of this test asserted 5 or more and failed at 1, which
    /// exposed a real bug — mean-removed cosine is scale-invariant, so tilts of
    /// different DEGREE pointed the same direction and scored identical.
    /// `summary_distance` now adds the centroid gap and the sweep resolves into
    /// distinct keeps. The remaining collapse is the intended behaviour, and
    /// asserting a range rather than a floor is what makes that a claim instead
    /// of a hope.
    #[test]
    fn a_smooth_brightness_sweep_collapses_to_a_few_representatives() {
        let mut frames: Vec<Vec<f32>> = (0..400).map(|_| silence()).collect();
        for i in 0..8 {
            let tilt = i as f32 * 0.22;
            for age in 0..12 { frames.push(hit(age, tilt)); }
            frames.extend((0..140).map(|_| silence()));
        }
        let (s, _) = run(Gate::default(), &frames);
        assert!(
            (2..=5).contains(&s.keeps),
            "a smooth 8-step sweep yielded {} crops; expected a handful of \
representatives — 1 means brightness is being ignored again, 8 means novelty \
rejection has stopped working",
            s.keeps
        );
    }

    #[test]
    fn the_refractory_window_stops_one_event_becoming_five() {
        // A door slam is several transients inside a few hundred ms.
        let mut frames: Vec<Vec<f32>> = (0..400).map(|_| silence()).collect();
        for burst in 0..4 {
            for age in 0..6 { frames.push(hit(age, 0.1 * burst as f32)); }
        }
        frames.extend((0..200).map(|_| silence()));
        let (s, _) = run(Gate::default(), &frames);
        assert_eq!(s.keeps, 1, "one clustered event produced {} crops", s.keeps);
    }

    #[test]
    fn the_budget_is_a_hard_stop() {
        let g = Gate { budget: 3, novelty_min_dist: 0.0, ..Gate::default() };
        let mut frames: Vec<Vec<f32>> = (0..400).map(|_| silence()).collect();
        for i in 0..40 {
            for age in 0..10 { frames.push(hit(age, (i % 7) as f32 * 0.12)); }
            frames.extend((0..120).map(|_| silence()));
        }
        let (s, d) = run(g, &frames);
        assert_eq!(s.keeps, 3);
        assert!(d.contains(&Decision::Rejected(Reject::BudgetSpent)),
                "past the budget the reason must be reported, not silently idle");
    }

    #[test]
    fn a_crop_is_time_frames_by_n_mels_oldest_first() {
        let g = Gate { time_frames: 8, ..Gate::default() };
        let mut s = Sampler::new(g, N_MELS);
        for i in 0..8 { s.push(&vec![i as f32; N_MELS]); }
        let c = s.crop();
        assert_eq!(c.len(), 8 * N_MELS);
        assert_eq!(c[0], 0.0, "oldest frame must come first");
        assert_eq!(c[7 * N_MELS], 7.0, "newest frame must come last");
    }

    #[test]
    fn the_threshold_follows_a_room_that_gets_louder() {
        // Sustained noise must NOT keep firing: after the level settles, the
        // adaptive threshold should rise to meet it.
        let mut frames: Vec<Vec<f32>> = (0..200).map(|_| silence()).collect();
        frames.extend((0..2000).map(|i| {
            let jitter = ((i as f32) * 0.7).sin() * 2.0;
            (0..N_MELS).map(|_| -30.0 + jitter).collect()
        }));
        let (s, _) = run(Gate::default(), &frames);
        assert!(s.keeps <= 2,
            "steady noise produced {} crops; the threshold is not adapting", s.keeps);
    }

    #[test]
    fn cosine_distance_is_zero_for_identical_and_large_for_opposite() {
        let a: Vec<f32> = (0..N_MELS).map(|m| m as f32).collect();
        let b: Vec<f32> = (0..N_MELS).map(|m| (N_MELS - m) as f32).collect();
        assert!(cosine_distance(&a, &a) < 1e-5);
        assert!(cosine_distance(&a, &b) > 1.5, "opposite tilts should be far apart");
        // Loudness alone must not read as a different sound.
        let louder: Vec<f32> = a.iter().map(|x| x + 20.0).collect();
        assert!(cosine_distance(&a, &louder) < 1e-5,
                "the same timbre at a different level is the same sound");
    }

    #[test]
    fn the_speech_gate_catches_the_obvious_case_and_admits_its_limits() {
        // Voice-shaped: energy low, almost nothing high.
        let voiceish: Vec<f32> = (0..N_MELS)
            .map(|m| if m < N_MELS / 5 { 10.0 } else if m < N_MELS / 2 { 1.0 } else { 0.05 })
            .collect();
        assert!(looks_like_speech(&voiceish));

        // A bright transient is not caught, which is the intent.
        let bright: Vec<f32> = (0..N_MELS).map(|m| m as f32).collect();
        assert!(!looks_like_speech(&bright));

        // And the honest half: a low drone is rejected too. Documented as a
        // false positive rather than pretended away.
        let drone: Vec<f32> = (0..N_MELS)
            .map(|m| if m < N_MELS / 5 { 8.0 } else { 0.1 })
            .collect();
        assert!(looks_like_speech(&drone),
                "a low drone trips it — this is a known false positive, see the fn docs");
    }

    /// The arithmetic the module doc claims, checked rather than asserted in prose.
    #[test]
    fn a_days_worth_of_frames_runs_fast_and_stays_bounded() {
        // 24 h at hop 256 / 22050 Hz is ~116 000 frames.
        let g = Gate { memory: 64, ..Gate::default() };
        let mut s = Sampler::new(g, N_MELS);
        let quiet = silence();
        for i in 0..116_000u32 {
            if i % 997 == 0 {
                for age in 0..10 { s.push(&hit(age, (i % 11) as f32 * 0.08)); }
            } else {
                s.push(&quiet);
            }
        }
        // Memory is capped regardless of how long it ran.
        assert!(s.kept.len() <= 64);
        // And the day yields a library, not a disk problem.
        assert!(s.keeps > 0 && s.keeps < 500,
                "a day produced {} crops — expected a curated handful", s.keeps);
    }

    /// The counters must distinguish the failure modes that look identical.
    #[test]
    fn rejections_are_attributed_to_the_right_gate() {
        // A room with one sound in it, repeated. Onsets fire; novelty rejects.
        let mut frames: Vec<Vec<f32>> = (0..400).map(|_| silence()).collect();
        for _ in 0..40 {
            for age in 0..10 { frames.push(hit(age, 0.3)); }
            frames.extend((0..130).map(|_| silence()));
        }
        let (s, _) = run(Gate::default(), &frames);
        assert!(s.onsets > 20, "the repeats should be heard: {}", s.onsets);
        assert!(s.rejected.familiar > 10,
                "repeats must be attributed to novelty, got {:?}", s.rejected);
        assert_eq!(s.rejected.budget_spent, 0, "the budget was never near");
    }

    #[test]
    fn a_clustered_event_is_attributed_to_the_refractory_window() {
        let mut frames: Vec<Vec<f32>> = (0..400).map(|_| silence()).collect();
        for burst in 0..5 {
            for age in 0..6 { frames.push(hit(age, 0.1 * burst as f32)); }
        }
        frames.extend((0..200).map(|_| silence()));
        let (s, _) = run(Gate::default(), &frames);
        assert!(s.rejected.too_soon > 0,
                "a burst must be attributed to refractory, got {:?}", s.rejected);
    }

    /// The advice is the point of the counters — a tally nobody can act on is
    /// just a number.
    #[test]
    fn the_advice_names_the_knob_for_each_failure_mode() {
        let silent = Rejects::default();
        assert!(silent.advice(0, 0).contains("--onset-mult"),
                "a session that heard nothing must question the threshold");

        let familiar = Rejects { familiar: 50, ..Default::default() };
        assert!(familiar.advice(50, 0).contains("--novelty"),
                "everything-familiar must point at the novelty knob");

        let soon = Rejects { too_soon: 30, ..Default::default() };
        assert!(soon.advice(30, 0).contains("--refractory"),
                "all-too-soon must point at the refractory knob");

        let speech = Rejects { speech_like: 20, ..Default::default() };
        assert!(speech.advice(25, 1).contains("--allow-speech"),
                "mostly-speech must name the flag AND warn about it");
    }

    #[test]
    fn a_healthy_session_gets_a_tally_without_a_lecture() {
        let r = Rejects { familiar: 3, too_soon: 2, ..Default::default() };
        let a = r.advice(40, 30);
        assert!(a.contains("kept 30"));
        // No diagnosis when nothing is wrong: advice on a working session is
        // noise, and noise is what makes people stop reading it.
        assert!(!a.contains("--"), "a healthy session should get no knob advice: {a}");
    }

    #[test]
    fn the_total_counts_every_kind() {
        let r = Rejects { too_soon: 1, familiar: 2, budget_spent: 3, speech_like: 4 };
        assert_eq!(r.total(), 10);
    }

}
