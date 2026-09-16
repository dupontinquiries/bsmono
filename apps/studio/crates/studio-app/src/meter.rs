//! meter — the part of an Ozone-style display that is not drawing.
//!
//! The analysis exists: `studio_fx::analysis` gives a `Spectrum` and a true
//! peak, `loudness` gives LUFS, `multiband` gives per-band gain reduction. What
//! is missing between those numbers and a window is the display model, and it
//! is the reason a good meter looks good and a naive one looks broken.
//!
//! Two things do almost all of that work, and neither is drawing:
//!
//! 1. **Ballistics.** A meter that follows the signal exactly is unreadable —
//!    it is a strobe. Fast attack and slow release is what turns a sample
//!    stream into something an eye can track, and it is also what makes two
//!    meters comparable: they agree because they lag the same way.
//! 2. **Log frequency.** A spectrum drawn on a linear axis spends more than
//!    half its width on 10–20 kHz, where almost nothing a mix engineer adjusts
//!    lives. Every analyser worth using draws octaves evenly.
//!
//! Both are pure, and both are the kind of thing that is wrong for months
//! because "the meter looks a bit odd" never becomes a bug report.

/// A level meter with attack/release ballistics and a falling peak hold.
///
/// Values are dB throughout. dB rather than linear because the ballistics
/// should be perceptually even: a 6 dB fall takes the same time whether it is
/// from -6 to -12 or -40 to -46, which is what an engineer expects and what
/// linear smoothing does not give.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ballistics {
    /// Current displayed value, dB.
    level_db: f32,
    /// The held peak, dB.
    peak_db: f32,
    /// Seconds the peak has been held.
    held_s: f32,
}

/// Time to rise most of the way to a new louder value, seconds.
///
/// Near-instant. A meter that eases into a transient under-reads exactly the
/// moment you are watching for, which for a limiter display is the only moment
/// that matters.
pub const ATTACK_S: f32 = 0.005;

/// Time to fall 20 dB, seconds. The IEC PPM figure, and it is a compromise
/// with a reason: fast enough to follow a phrase, slow enough that the eye
/// integrates rather than flickers.
pub const RELEASE_20DB_S: f32 = 1.7;

/// How long a peak marker sits before it starts to fall.
pub const PEAK_HOLD_S: f32 = 1.5;

/// How fast the peak marker falls once it lets go, dB per second.
pub const PEAK_FALL_DB_S: f32 = 12.0;

/// The floor. Anything quieter is "silence" and is not worth a pixel.
pub const FLOOR_DB: f32 = -90.0;

impl Default for Ballistics {
    fn default() -> Self {
        Ballistics { level_db: FLOOR_DB, peak_db: FLOOR_DB, held_s: 0.0 }
    }
}

impl Ballistics {
    pub fn new() -> Ballistics {
        Ballistics::default()
    }

    pub fn level_db(&self) -> f32 {
        self.level_db
    }
    pub fn peak_db(&self) -> f32 {
        self.peak_db
    }

    /// Feed a measured value and advance by `dt` seconds.
    ///
    /// `dt` is clamped: a stalled frame must not teleport the meter to the
    /// floor, because the recovery from that reads as a dropout that did not
    /// happen.
    pub fn update(&mut self, measured_db: f32, dt: f32) {
        let dt = dt.clamp(0.0, 0.25);
        let target = measured_db.max(FLOOR_DB);

        if target > self.level_db {
            // Attack: exponential toward the target.
            let k = 1.0 - (-dt / ATTACK_S).exp();
            self.level_db += (target - self.level_db) * k;
        } else {
            // Release: a CONSTANT dB PER SECOND fall, not exponential.
            //
            // An exponential release never arrives, so a meter fed silence
            // hangs a few dB above the floor forever and the display never
            // looks idle. A linear-in-dB fall reaches the floor, which is what
            // "nothing is playing" should look like.
            let fall = (20.0 / RELEASE_20DB_S) * dt;
            self.level_db = (self.level_db - fall).max(target);
        }

        if target >= self.peak_db {
            self.peak_db = target;
            self.held_s = 0.0;
        } else {
            self.held_s += dt;
            if self.held_s > PEAK_HOLD_S {
                self.peak_db = (self.peak_db - PEAK_FALL_DB_S * dt).max(self.level_db);
            }
        }
    }

    /// Clear to silence — for a transport stop, where a decaying meter would
    /// imply audio that is no longer playing.
    pub fn reset(&mut self) {
        *self = Ballistics::default();
    }
}

/// A frequency axis with octaves evenly spaced.
#[derive(Debug, Clone, PartialEq)]
pub struct LogAxis {
    /// Lower edge of each display bin, Hz. `len() + 1` edges for `len()` bins.
    edges: Vec<f32>,
}

impl LogAxis {
    /// `bins` display columns spanning `lo`..`hi` Hz, geometrically spaced.
    pub fn new(lo: f32, hi: f32, bins: usize) -> LogAxis {
        let bins = bins.max(1);
        let (lo, hi) = (lo.max(1.0), hi.max(lo.max(1.0) * 2.0));
        let ratio = (hi / lo).powf(1.0 / bins as f32);
        let edges = (0..=bins).map(|i| lo * ratio.powi(i as i32)).collect();
        LogAxis { edges }
    }

    /// The audible range: 20 Hz to 20 kHz.
    pub fn audible(bins: usize) -> LogAxis {
        LogAxis::new(20.0, 20_000.0, bins)
    }

    pub fn bins(&self) -> usize {
        self.edges.len() - 1
    }

    pub fn edge(&self, i: usize) -> f32 {
        self.edges[i.min(self.edges.len() - 1)]
    }

    /// Centre frequency of a display bin, geometric.
    ///
    /// Geometric rather than arithmetic: the centre of a bin spanning 100–200
    /// Hz is 141 Hz, not 150. On a log axis the arithmetic midpoint is visibly
    /// off-centre, which makes a labelled grid line disagree with the bar it
    /// labels.
    pub fn centre(&self, i: usize) -> f32 {
        let a = self.edge(i);
        let b = self.edge(i + 1);
        (a * b).sqrt()
    }

    /// Resample an FFT spectrum onto this axis.
    ///
    /// Takes the MAXIMUM of the source bins in each display bin, not the mean.
    /// Averaging is the obvious choice and it is wrong here: a narrow
    /// resonance is exactly what an engineer is hunting, and at low
    /// frequencies one display bin can cover dozens of FFT bins, so the mean
    /// buries a 12 dB spike under its quiet neighbours. A peak-holding display
    /// over-reports a little; an averaging one hides the thing you opened the
    /// analyser to find.
    pub fn resample(&self, bins_db: &[f32], rate: f32) -> Vec<f32> {
        let n = bins_db.len();
        if n == 0 {
            return vec![FLOOR_DB; self.bins()];
        }
        let hz_per_bin = rate / (n as f32 * 2.0);
        (0..self.bins())
            .map(|i| {
                let (lo, hi) = (self.edge(i), self.edge(i + 1));
                let k0 = ((lo / hz_per_bin) as usize).min(n - 1);
                let k1 = ((hi / hz_per_bin).ceil() as usize).clamp(k0 + 1, n);
                bins_db[k0..k1].iter().cloned().fold(FLOOR_DB, f32::max)
            })
            .collect()
    }
}

/// A rolling history, for a gain-reduction trace or a loudness graph.
///
/// The limiter display everybody actually reads is not the current number, it
/// is the shape of the last few seconds: whether the reduction is occasional
/// and deep or constant and shallow is the whole question, and one number
/// cannot say.
#[derive(Debug, Clone, PartialEq)]
pub struct Trace {
    points: std::collections::VecDeque<f32>,
    cap: usize,
}

impl Trace {
    pub fn new(cap: usize) -> Trace {
        Trace { points: std::collections::VecDeque::new(), cap: cap.max(1) }
    }

    pub fn push(&mut self, v: f32) {
        if self.points.len() == self.cap {
            self.points.pop_front();
        }
        self.points.push_back(v);
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// Oldest first, which is left-to-right on screen.
    pub fn iter(&self) -> impl Iterator<Item = f32> + '_ {
        self.points.iter().copied()
    }

    /// Deepest value in the window — the number a limiter display puts in the
    /// corner, because "how hard did it hit" is what you check after a pass.
    pub fn worst(&self) -> Option<f32> {
        self.points.iter().cloned().fold(None, |a: Option<f32>, b| {
            Some(match a {
                None => b,
                Some(x) => x.min(b),
            })
        })
    }

    pub fn clear(&mut self) {
        self.points.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A meter must catch a transient. Easing into it under-reads the only
    /// moment a limiter display exists for.
    #[test]
    fn a_transient_is_caught_almost_immediately() {
        let mut m = Ballistics::new();
        let (from, to) = (m.level_db(), -6.0f32);
        m.update(to, 1.0 / 60.0);
        // The property is the FRACTION of the gap closed in one frame, not an
        // absolute level. My first version asserted `> -8 dB` and it failed at
        // -9: rising from the -90 floor is an 84 dB jump, and covering 96% of
        // it still leaves 3 dB. The fraction is what "near-instant" means.
        let covered = (m.level_db() - from) / (to - from);
        assert!(covered > 0.95, "one frame covered only {:.0}%", covered * 100.0);
    }

    /// ...and then falls slowly enough to read.
    #[test]
    fn the_fall_is_slow_enough_to_follow() {
        let mut m = Ballistics::new();
        m.update(0.0, 0.1);
        let after_a_frame = { let mut n = m; n.update(FLOOR_DB, 1.0 / 60.0); n.level_db() };
        assert!(after_a_frame > -2.0, "a frame should barely move it: {after_a_frame}");
    }

    /// A LINEAR-IN-dB RELEASE ARRIVES. An exponential one never does, so a
    /// meter fed silence hangs above the floor forever and never looks idle.
    #[test]
    fn a_silent_input_eventually_reads_silence() {
        let mut m = Ballistics::new();
        m.update(0.0, 0.1);
        for _ in 0..600 {
            m.update(FLOOR_DB, 1.0 / 60.0);
        }
        assert_eq!(m.level_db(), FLOOR_DB, "it must actually reach the floor");
    }

    /// The 20 dB figure is the spec; hold it to roughly the stated time.
    #[test]
    fn twenty_db_takes_about_the_stated_time() {
        let mut m = Ballistics::new();
        m.update(0.0, 0.1);
        let mut t = 0.0f32;
        while m.level_db() > -20.0 && t < 10.0 {
            m.update(FLOOR_DB, 1.0 / 240.0);
            t += 1.0 / 240.0;
        }
        assert!((t - RELEASE_20DB_S).abs() < 0.15, "20 dB took {t}s");
    }

    /// The peak marker holds, then falls — and never falls below the bar,
    /// which would draw a peak marker inside a filled meter.
    #[test]
    fn the_peak_marker_holds_then_falls_but_never_below_the_level() {
        let mut m = Ballistics::new();
        m.update(-3.0, 0.01);
        let held = m.peak_db();
        for _ in 0..30 {
            m.update(-40.0, 1.0 / 60.0);
        }
        assert_eq!(m.peak_db(), held, "still holding after half a second");
        for _ in 0..300 {
            m.update(-40.0, 1.0 / 60.0);
        }
        assert!(m.peak_db() < held, "and then it lets go");
        assert!(m.peak_db() >= m.level_db(), "never below the bar it marks");
    }

    /// A stalled frame must not teleport the meter — the recovery reads as a
    /// dropout that never happened.
    #[test]
    fn a_long_stall_does_not_slam_the_meter_to_the_floor() {
        let mut m = Ballistics::new();
        m.update(0.0, 0.1);
        m.update(FLOOR_DB, 10.0);
        assert!(m.level_db() > -10.0, "one stalled frame dropped it to {}", m.level_db());
    }

    /// OCTAVES EVENLY SPACED, which is the whole point of the axis.
    #[test]
    fn each_octave_gets_the_same_width() {
        let a = LogAxis::audible(120);
        let bins_below_200 = (0..a.bins()).filter(|&i| a.centre(i) < 200.0).count();
        let bins_above_5k = (0..a.bins()).filter(|&i| a.centre(i) > 5_000.0).count();
        // 20-200 Hz and 5-20 kHz are both about 3.3 and 2 octaves; on a linear
        // axis the first would get 1% of the width.
        assert!(bins_below_200 > a.bins() / 5, "the low end got {bins_below_200} of {}", a.bins());
        assert!(bins_above_5k < bins_below_200, "and the top should not dominate");
    }

    /// Geometric centre: 100–200 Hz centres on 141, not 150. An arithmetic
    /// midpoint puts a grid label visibly off the bar it labels.
    #[test]
    fn bin_centres_are_geometric() {
        let a = LogAxis::new(100.0, 200.0, 1);
        assert!((a.centre(0) - 141.42).abs() < 0.5, "got {}", a.centre(0));
    }

    /// MAX, NOT MEAN. A narrow resonance is what an analyser is opened to
    /// find, and at low frequencies one display bin covers dozens of FFT bins.
    #[test]
    fn a_narrow_spike_survives_resampling() {
        let rate = 48_000.0;
        let n = 1024;
        let mut bins = vec![-80.0f32; n];
        // One loud FFT bin near 100 Hz.
        let hz_per_bin = rate / (n as f32 * 2.0);
        let k = (100.0 / hz_per_bin) as usize;
        bins[k] = -6.0;
        let out = LogAxis::audible(60).resample(&bins, rate);
        let loudest = out.iter().cloned().fold(f32::MIN, f32::max);
        assert!((loudest - (-6.0)).abs() < 0.01, "the spike was averaged away: {loudest}");
    }

    /// An empty spectrum is a floor, not a panic.
    #[test]
    fn resampling_nothing_gives_silence() {
        let out = LogAxis::audible(32).resample(&[], 48_000.0);
        assert_eq!(out.len(), 32);
        assert!(out.iter().all(|&v| v == FLOOR_DB));
    }

    /// THE LIMITER QUESTION. Occasional and deep versus constant and shallow
    /// is what you actually check, and one number cannot say it.
    #[test]
    fn the_trace_keeps_the_shape_and_the_worst_point() {
        let mut t = Trace::new(4);
        for v in [-1.0, -9.0, -2.0, -3.0, -4.0] {
            t.push(v);
        }
        assert_eq!(t.len(), 4, "it rolls");
        assert_eq!(t.iter().collect::<Vec<_>>(), vec![-9.0, -2.0, -3.0, -4.0],
                   "oldest first, which is left to right");
        assert_eq!(t.worst(), Some(-9.0), "deepest reduction in the window");
    }

    #[test]
    fn an_empty_trace_has_no_worst_point() {
        let t = Trace::new(8);
        assert!(t.is_empty());
        assert_eq!(t.worst(), None);
    }
}
