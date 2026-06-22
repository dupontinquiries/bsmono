//! dead_air: cleave dead air (and, later, filler) out of a voice
//! capture — the signature reVos/kss-lineage feature ported to the
//! dev loop. (Roadmap item 4.)
//!
//! Ported from the ACTUAL uploaded source (reVos IDEA.md "SS/Lecture
//! Protocol" + kss6.py's silence detection), keeping what made kss
//! robust rather than a naive fixed-threshold gate:
//!   - ADAPTIVE threshold derived from THIS recording's loudness
//!     distribution (kss6 line 738: frac*(max+mean)/2) — no magic dB
//!     constant, so it auto-adapts to a quiet vs loud speaker / silent
//!     vs noisy room;
//!   - DUAL-signal keep (kss6 line 739): a frame is voice if loudness
//!     OR spread exceeds its (adaptive) threshold — the OR catches
//!     soft consonants/transients a loudness-only gate clips;
//!   - HYSTERESIS (kss period/keep-silence): a silent run must exceed
//!     a minimum duration before it's cleaved, so natural inter-word
//!     micro-gaps aren't chopped into robotic staccato;
//!   - SPEED-CURVE generalization (SS Protocol): each cleaved gap
//!     carries a suggested compression by duration so the caller can
//!     hard-cut OR variable-speed it.
//!
//! Pure: takes per-frame audio LEVELS (the capture path computes
//! them), returns a cleave plan. No audio decoding here — that's the
//! pipeline's job. See docs/design/dead_air_cleaving.md.

/// One fixed-duration analysis frame's measurements. The capture path
/// supplies these; this module never touches raw samples.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    /// Loudness, dBFS-like (higher = louder; silence very negative).
    pub loudness: f32,
    /// Spread / variation within the frame (kss's second signal) —
    /// e.g. peak-to-trough or short-time variance. Catches transients
    /// that mean-loudness misses. Pass 0.0 if unavailable (the keep
    /// rule then degrades gracefully to loudness-only).
    pub spread: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct CleaveConfig {
    /// Fraction applied to the (max+mean)/2 midpoint for the loudness
    /// threshold (kss DEFAULT_THRESHOLD). 1.0 = the midpoint itself;
    /// <1 keeps more (cleaves less). kss presets vary this per mode.
    pub loudness_frac: f32,
    /// Same, for the spread signal (kss DEFAULT_REACH_THRESH = frac *
    /// threshold). Applied to the spread distribution's midpoint.
    pub spread_frac: f32,
    /// Minimum number of consecutive silent frames before a run is
    /// cleaved (hysteresis). Shorter silent runs are kept as voice so
    /// inter-word micro-gaps survive.
    pub min_silence_frames: usize,
    /// Frames to keep as a lead/tail pad around kept voice when
    /// cleaving (so cuts don't clip word onsets). kss keep_silence.
    pub pad_frames: usize,
    /// Duration of one frame in ms (for time spans + the speed curve).
    pub frame_ms: u32,
}

impl Default for CleaveConfig {
    fn default() -> Self {
        Self {
            loudness_frac: 1.0,
            spread_frac: 1.0,
            min_silence_frames: 6, // e.g. 6×~50ms = ~300ms min gap
            pad_frames: 1,
            frame_ms: 50,
        }
    }
}

/// Build analysis frames from mono f32 samples (range ~[-1,1]) at a
/// given sample rate, using `frame_ms` windows. Each frame's loudness
/// is RMS in dBFS; spread is the peak-to-RMS ratio in dB (a crude
/// crest factor — high for transient/consonant content, low for
/// steady tone or silence). This is the bridge the capture path uses
/// to feed plan_cleave; kept here so the module is usable end-to-end
/// from raw samples and the bridge itself is tested. Pure.
pub fn frames_from_samples(samples: &[f32], sample_rate: u32, frame_ms: u32) -> Vec<Frame> {
    if samples.is_empty() || sample_rate == 0 || frame_ms == 0 {
        return Vec::new();
    }
    let win = ((sample_rate as u64 * frame_ms as u64) / 1000) as usize;
    if win == 0 { return Vec::new(); }
    let mut frames = Vec::with_capacity(samples.len() / win + 1);
    let mut i = 0;
    while i < samples.len() {
        let end = (i + win).min(samples.len());
        let chunk = &samples[i..end];
        let n = chunk.len() as f32;
        let mut sum_sq = 0.0f32;
        let mut peak = 0.0f32;
        for &s in chunk {
            // v0.1.528: a non-finite sample (inf/NaN from a hardware
            // glitch, buffer underrun, or upstream DSP) would poison
            // this frame's stats — an inf sample makes loudness inf,
            // which makes the ADAPTIVE THRESHOLD inf, which classifies
            // the entire recording as silence. Treat non-finite samples
            // as 0 (a silence contribution) so a glitch degrades to
            // "quiet" rather than breaking the whole cleave plan.
            // (Found by adversarial probe.)
            let s = if s.is_finite() { s } else { 0.0 };
            sum_sq += s * s;
            let a = s.abs();
            if a > peak { peak = a; }
        }
        let rms = (sum_sq / n).sqrt();
        // dBFS: 20*log10(rms), floored so digital silence isn't -inf.
        let loudness = if rms > 1e-9 { 20.0 * rms.log10() } else { -120.0 };
        // crest: peak/rms in dB. Steady/silent → ~0; transient → high.
        let spread = if rms > 1e-9 && peak > 1e-9 {
            20.0 * (peak / rms).log10()
        } else {
            0.0
        };
        frames.push(Frame { loudness, spread });
        i = end;
    }
    frames
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentKind { Voice, DeadAir }

/// A contiguous run of frames classified the same way, in ms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub start_ms: u32,
    pub end_ms: u32,
    pub kind: SegmentKind,
    /// For DeadAir: suggested compression in [0,1] where 0 = drop
    /// entirely, 1 = keep full length. From the speed curve (longer
    /// gaps compress harder). Voice segments are always 1.0.
    pub keep_factor: f32,
}

impl Segment {
    pub fn duration_ms(&self) -> u32 { self.end_ms.saturating_sub(self.start_ms) }
}

/// The result of planning a cleave over a frame sequence.
#[derive(Clone, Debug, Default)]
pub struct CleavePlan {
    pub segments: Vec<Segment>,
    /// Total ms classified as dead air (before compression).
    pub dead_air_ms: u32,
    /// Total ms of the input.
    pub total_ms: u32,
}

impl CleavePlan {
    /// Fraction of the recording that is dead air (0..1).
    pub fn dead_air_fraction(&self) -> f32 {
        if self.total_ms == 0 { 0.0 } else { self.dead_air_ms as f32 / self.total_ms as f32 }
    }
    /// Estimated ms after applying each dead-air segment's keep_factor
    /// (voice kept whole). The realistic "compressed length".
    pub fn compressed_ms(&self) -> u32 {
        let mut out = 0.0f32;
        for s in &self.segments {
            out += match s.kind {
                SegmentKind::Voice => s.duration_ms() as f32,
                SegmentKind::DeadAir => s.duration_ms() as f32 * s.keep_factor,
            };
        }
        out.round() as u32
    }
}

/// Midpoint-of-(max+mean) adaptive threshold (kss6 line 738), scaled
/// by `frac`. Returns -inf for an empty slice (keeps everything).
fn adaptive_threshold(values: &[f32], frac: f32) -> f32 {
    if values.is_empty() { return f32::NEG_INFINITY; }
    let max = values.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let mean = values.iter().sum::<f32>() / values.len() as f32;
    frac * (max + mean) / 2.0
}

/// Speed curve (SS Protocol): how much of a dead-air gap to keep, by
/// its duration. Short gaps keep more (a natural pause), long gaps
/// compress hard (drop toward a small floor). Smooth, in [floor,1].
fn keep_factor_for_gap(gap_ms: u32) -> f32 {
    // Below 400ms: keep most (natural pause). Above ~3s: floor near
    // 0.05. Exponential-ish decay between. Monotonic decreasing.
    let g = gap_ms as f32;
    if g <= 400.0 { return 1.0; }
    let floor = 0.05;
    // decay from 1.0 at 400ms toward floor with ~1.2s half-life
    let decayed = (-(g - 400.0) / 1200.0).exp();
    floor + (1.0 - floor) * decayed
}

/// Plan a cleave over the frame sequence. Pure: same input → same
/// plan. The caller acts on it (hard-cut, variable-speed, or display).
pub fn plan_cleave(frames: &[Frame], cfg: &CleaveConfig) -> CleavePlan {
    let mut plan = CleavePlan::default();
    if frames.is_empty() { return plan; }
    plan.total_ms = frames.len() as u32 * cfg.frame_ms;

    // Adaptive thresholds from this recording's distributions.
    let loud: Vec<f32> = frames.iter().map(|f| f.loudness).collect();
    let spread: Vec<f32> = frames.iter().map(|f| f.spread).collect();
    let vt = adaptive_threshold(&loud, cfg.loudness_frac);
    let vrt = adaptive_threshold(&spread, cfg.spread_frac);

    // Per-frame keep decision: dual-signal OR — BUT the spread signal
    // may only RESCUE a frame that has some loudness. A digitally
    // silent frame has spread 0 (peak≈rms≈0); without this guard the
    // adaptive spread threshold can sit at/below 0 and silence would
    // be "kept" by spread, defeating cleaving entirely. So spread only
    // counts when loudness is above a silence floor relative to vt.
    let silence_floor = vt - 20.0; // 20dB below the loudness threshold
    let mut keep: Vec<bool> = frames.iter()
        .map(|f| f.loudness > vt || (f.loudness > silence_floor && f.spread > vrt))
        .collect();

    // Hysteresis: silent runs shorter than min_silence_frames revert
    // to voice (micro-gaps survive).
    let mut i = 0;
    while i < keep.len() {
        if !keep[i] {
            let start = i;
            while i < keep.len() && !keep[i] { i += 1; }
            if i - start < cfg.min_silence_frames {
                for k in start..i { keep[k] = true; }
            }
        } else {
            i += 1;
        }
    }

    // Pad: re-mark up to pad_frames on each side of a cleaved run as
    // voice, so cuts don't clip word onsets/tails.
    if cfg.pad_frames > 0 {
        let base = keep.clone();
        for idx in 0..base.len() {
            if !base[idx] { continue; }
            // voice frame: pad does nothing here; padding extends the
            // VOICE side into adjacent silence, handled below.
        }
        // For each silent run boundary adjacent to voice, keep pad
        // frames of the silence as voice.
        let mut j = 0;
        while j < base.len() {
            if !base[j] {
                let start = j;
                while j < base.len() && !base[j] { j += 1; }
                let end = j; // exclusive
                // pad at the front (after preceding voice)
                if start > 0 {
                    for k in start..(start + cfg.pad_frames).min(end) { keep[k] = true; }
                }
                // pad at the back (before following voice)
                if end < base.len() {
                    let lo = end.saturating_sub(cfg.pad_frames).max(start);
                    for k in lo..end { keep[k] = true; }
                }
            } else {
                j += 1;
            }
        }
    }

    // Coalesce into segments.
    let mut idx = 0;
    while idx < keep.len() {
        let kind = if keep[idx] { SegmentKind::Voice } else { SegmentKind::DeadAir };
        let start = idx;
        while idx < keep.len() && (keep[idx] == (kind == SegmentKind::Voice)) {
            idx += 1;
        }
        let start_ms = start as u32 * cfg.frame_ms;
        let end_ms = idx as u32 * cfg.frame_ms;
        let keep_factor = match kind {
            SegmentKind::Voice => 1.0,
            SegmentKind::DeadAir => {
                plan.dead_air_ms += end_ms - start_ms;
                keep_factor_for_gap(end_ms - start_ms)
            }
        };
        plan.segments.push(Segment { start_ms, end_ms, kind, keep_factor });
    }

    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(loud: f32) -> Frame { Frame { loudness: loud, spread: 0.0 } }
    fn cfg() -> CleaveConfig { CleaveConfig { pad_frames: 0, ..Default::default() } }

    #[test]
    fn empty_input_empty_plan() {
        let p = plan_cleave(&[], &cfg());
        assert!(p.segments.is_empty());
        assert_eq!(p.total_ms, 0);
        assert_eq!(p.dead_air_fraction(), 0.0);
    }

    #[test]
    fn adaptive_threshold_separates_voice_silence() {
        // voice ~-15, silence ~-55
        let vt = adaptive_threshold(&[-15.0, -14.0, -55.0, -56.0], 1.0);
        assert!(-15.0 > vt, "voice above threshold");
        assert!(-55.0 < vt, "silence below threshold");
    }

    #[test]
    fn threshold_adapts_to_louder_recording() {
        let quiet = adaptive_threshold(&[-15.0, -55.0], 1.0);
        let loud = adaptive_threshold(&[-5.0, -45.0], 1.0);
        assert!(loud > quiet, "louder recording → higher threshold");
    }

    #[test]
    fn long_silence_cleaved_short_kept() {
        // voice, then a long silence (>= min 6 frames), then voice
        let mut frames = vec![f(-15.0), f(-15.0)];
        for _ in 0..8 { frames.push(f(-55.0)); }
        frames.push(f(-15.0));
        let p = plan_cleave(&frames, &cfg());
        // expect Voice, DeadAir, Voice
        let kinds: Vec<SegmentKind> = p.segments.iter().map(|s| s.kind).collect();
        assert_eq!(kinds, vec![SegmentKind::Voice, SegmentKind::DeadAir, SegmentKind::Voice]);
        assert!(p.dead_air_ms > 0);
    }

    #[test]
    fn micro_gap_not_cleaved() {
        // a single silent frame inside speech (< min 6) stays voice
        let frames = vec![f(-15.0), f(-55.0), f(-15.0), f(-15.0)];
        let p = plan_cleave(&frames, &cfg());
        assert_eq!(p.segments.len(), 1, "one Voice segment, micro-gap absorbed");
        assert_eq!(p.segments[0].kind, SegmentKind::Voice);
        assert_eq!(p.dead_air_ms, 0);
    }

    #[test]
    fn dual_signal_keeps_low_loud_high_spread() {
        // a frame whose loudness is below threshold but spread is high
        // (a soft consonant) should be KEPT by the OR rule.
        let frames = vec![
            Frame { loudness: -15.0, spread: 5.0 },
            Frame { loudness: -50.0, spread: 90.0 }, // quiet but spready
            Frame { loudness: -15.0, spread: 5.0 },
            Frame { loudness: -55.0, spread: 1.0 },
            Frame { loudness: -55.0, spread: 1.0 },
            Frame { loudness: -55.0, spread: 1.0 },
            Frame { loudness: -55.0, spread: 1.0 },
            Frame { loudness: -55.0, spread: 1.0 },
            Frame { loudness: -55.0, spread: 1.0 },
            Frame { loudness: -15.0, spread: 5.0 },
        ];
        let c = CleaveConfig { min_silence_frames: 3, pad_frames: 0, ..Default::default() };
        let p = plan_cleave(&frames, &c);
        // the high-spread quiet frame is NOT its own dead-air run
        // (kept by spread), the long low-everything run IS dead air.
        let voice_first = p.segments[0].kind == SegmentKind::Voice;
        assert!(voice_first);
        assert!(p.segments.iter().any(|s| s.kind == SegmentKind::DeadAir));
        // first dead-air run starts at/after the spready frame, not at it
        let first_dead = p.segments.iter().find(|s| s.kind == SegmentKind::DeadAir).unwrap();
        assert!(first_dead.start_ms >= 3 * c.frame_ms, "spready quiet frame kept as voice");
    }

    #[test]
    fn speed_curve_monotonic_decreasing() {
        // longer gaps keep less
        assert_eq!(keep_factor_for_gap(300), 1.0);
        let short = keep_factor_for_gap(600);
        let med = keep_factor_for_gap(1500);
        let long = keep_factor_for_gap(4000);
        assert!(short > med && med > long, "{} {} {}", short, med, long);
        assert!(long >= 0.05, "never below floor");
        assert!(short <= 1.0);
    }

    #[test]
    fn compressed_ms_shorter_than_total_with_dead_air() {
        let mut frames = vec![f(-15.0), f(-15.0)];
        for _ in 0..20 { frames.push(f(-55.0)); } // 1s of dead air at 50ms
        frames.push(f(-15.0));
        let p = plan_cleave(&frames, &cfg());
        assert!(p.compressed_ms() < p.total_ms, "dead air compressed out");
        assert!(p.dead_air_fraction() > 0.5, "mostly dead air");
    }

    #[test]
    fn non_finite_samples_dont_poison_analysis() {
        // v0.1.528 (adversarial probe): an inf/NaN sample (hardware
        // glitch / underrun) must not make a frame's loudness inf,
        // which would poison the adaptive threshold and classify
        // everything as silence. Sanitized to 0.
        let mut samples = vec![0.6f32; 100];
        samples[50] = f32::INFINITY; // glitch in a loud frame
        samples.extend(vec![0.0f32; 100]);
        samples[150] = f32::NAN; // glitch in a silent frame
        samples.extend(vec![0.6f32; 100]);
        let frames = frames_from_samples(&samples, 1000, 100);
        for f in &frames {
            assert!(f.loudness.is_finite(), "loudness finite despite glitch: {}", f.loudness);
            assert!(f.spread.is_finite(), "spread finite despite glitch: {}", f.spread);
        }
        // the plan still works: first/last loud frames are voice
        let p = plan_cleave(&frames, &CleaveConfig { min_silence_frames: 1, pad_frames: 0, frame_ms: 100, ..Default::default() });
        assert!(p.segments.iter().any(|s| s.kind == SegmentKind::Voice),
            "voice still detected despite glitched samples");
    }

    #[test]
    fn frames_from_samples_windows_correctly() {
        // 1000 samples at 1000Hz, 100ms frames → 100 samples/frame → 10 frames
        let samples = vec![0.5f32; 1000];
        let frames = frames_from_samples(&samples, 1000, 100);
        assert_eq!(frames.len(), 10);
    }

    #[test]
    fn frames_from_samples_empty_and_degenerate() {
        assert!(frames_from_samples(&[], 48000, 50).is_empty());
        assert!(frames_from_samples(&[0.1; 100], 0, 50).is_empty());
        assert!(frames_from_samples(&[0.1; 100], 48000, 0).is_empty());
    }

    #[test]
    fn silence_is_very_quiet_loud_is_loud() {
        // a loud frame (amplitude 0.8) then a silent frame (~0)
        let mut samples = vec![0.8f32; 100];
        samples.extend(vec![0.0f32; 100]);
        let frames = frames_from_samples(&samples, 1000, 100);
        assert_eq!(frames.len(), 2);
        assert!(frames[0].loudness > -5.0, "loud frame near 0 dBFS, got {}", frames[0].loudness);
        assert!(frames[1].loudness <= -100.0, "silent frame floored, got {}", frames[1].loudness);
    }

    #[test]
    fn transient_has_higher_spread_than_steady() {
        // steady tone: peak ≈ rms → low crest. transient: one spike
        // among quiet samples → high crest.
        let steady = vec![0.5f32; 100];
        let mut transient = vec![0.01f32; 100];
        transient[50] = 0.9; // a spike
        let fs = frames_from_samples(&steady, 1000, 100);
        let ft = frames_from_samples(&transient, 1000, 100);
        assert!(ft[0].spread > fs[0].spread,
            "transient crest {} should exceed steady crest {}", ft[0].spread, fs[0].spread);
    }

    #[test]
    fn end_to_end_samples_to_plan() {
        // build a recording: 0.3s voice, 1s silence, 0.3s voice at 1kHz
        let mut samples = vec![0.6f32; 300];
        samples.extend(vec![0.0f32; 1000]);
        samples.extend(vec![0.6f32; 300]);
        let frames = frames_from_samples(&samples, 1000, 100); // 16 frames
        let p = plan_cleave(&frames, &CleaveConfig { min_silence_frames: 3, pad_frames: 0, frame_ms: 100, ..Default::default() });
        assert!(p.dead_air_fraction() > 0.4, "the 1s silence dominates");
        assert!(p.compressed_ms() < p.total_ms);
        // structure: Voice, DeadAir, Voice
        let kinds: Vec<SegmentKind> = p.segments.iter().map(|s| s.kind).collect();
        assert_eq!(kinds, vec![SegmentKind::Voice, SegmentKind::DeadAir, SegmentKind::Voice]);
    }
}
