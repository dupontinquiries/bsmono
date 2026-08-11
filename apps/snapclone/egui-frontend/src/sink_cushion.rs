//! sink_cushion — the playout-underrun fix from `GroupVoiceReceiver`.
//!
//! [`crate::voice_playout`] already schedules per-sender frames: play, conceal
//! or go silent. This is the layer BELOW that — how many frames to hand the
//! audio sink each tick, which is a different question and had a different bug.
//!
//! ## The underrun, with its cause named
//!
//! *"The old design wrote exactly one 20 ms frame per 20 ms timer tick into the
//! sink with no buffer headroom — but a software timer can't stay phase-locked
//! to the device's sample clock, so any tick jitter starved the sink (underrun
//! -> crackle), independent of CPU governor."*
//!
//! That last clause is what makes it worth porting. The obvious diagnosis for
//! crackling audio is "the CPU is too slow", and this says it is not: **a timer
//! and an audio clock are two different oscillators**, and one frame per tick
//! has zero margin for the difference however fast the machine is. A governor
//! change would not have fixed it and neither would a faster phone.
//!
//! The fix is structural: *"give the sink a real cushion and top it up to that
//! cushion every tick, and run the tick at half a frame so it refills well
//! before the cushion drains."*
//!
//! ## The latency numbers were tuned downward and the history is kept
//!
//! `kCushionFrames` was 4 (~80 ms) and is 3 (~60 ms) — *"bounds latency"*. The
//! number looks arbitrary without that note, and the next person to "optimise"
//! latency needs to know somebody already walked it down to where underruns
//! start.
//!
//! ## Opus frame durations are snapped, not validated
//!
//! *"Snap a requested ms to the nearest legal one so a stray setting can't
//! produce un-decodable frames."* A bad setting should cost a slightly
//! different frame size, not a call that fails to start.

/// Wire format for group voice.
pub const SAMPLE_RATE: u32 = 16_000;
pub const CHANNELS: u16 = 1;
/// Default frame duration.
pub const FRAME_MS: u32 = 20;

/// Frames to prime before playing. Taken from voice_playout rather than
/// redefined, so the two layers cannot disagree about the buffer they share.
pub use crate::voice_playout::TARGET_DEPTH as JITTER_TARGET;
/// Ceiling on the configurable depth. ~1 s.
pub const JITTER_MAX: usize = 50;

/// Frames of cushion kept in the sink. ~60 ms.
pub const CUSHION_FRAMES: usize = 3;

/// Modest clamped makeup gain for low decoded levels, ~+3.5 dB.
pub const PLAYOUT_GAIN: f32 = 1.5;

/// Largest Opus frame at 16 kHz: 120 ms.
pub const MAX_DECODE_SAMPLES: usize = 1920;
/// Largest payload accepted from the wire.
pub const MAX_PAYLOAD_BYTES: usize = 4096;

/// Frame durations Opus can actually encode.
pub const LEGAL_FRAME_MS: &[u32] = &[5, 10, 20, 40, 60];

/// Snap a requested frame duration to the nearest legal one.
///
/// Snapping rather than rejecting: a stray setting should cost a slightly
/// different frame size, not a call that will not start.
pub fn snap_frame_ms(requested: u32) -> u32 {
    let mut best = LEGAL_FRAME_MS[0];
    let mut best_d = u32::MAX;
    for &v in LEGAL_FRAME_MS {
        let d = v.abs_diff(requested);
        if d < best_d { best_d = d; best = v; }
    }
    best
}

/// Samples in one frame of the given duration.
pub fn frame_samples(frame_ms: u32) -> usize {
    (SAMPLE_RATE as usize * frame_ms as usize) / 1000
}

/// Run the tick at half a frame, so the cushion refills well before it drains.
///
/// Half rather than one: at one tick per frame the refill and the drain race,
/// and the whole point of the fix is to stop them racing.
pub fn tick_interval_ms(frame_ms: u32) -> u32 { (frame_ms / 2).max(1) }

/// Configured latency knobs, clamped.
#[derive(Clone, Copy, Debug)]
pub struct Latency {
    pub jitter_target: usize,
    pub jitter_max: usize,
    pub frame_ms: u32,
}

impl Default for Latency {
    fn default() -> Self {
        Latency { jitter_target: JITTER_TARGET, jitter_max: JITTER_MAX, frame_ms: FRAME_MS }
    }
}

impl Latency {
    /// Build from settings, clamping as the original clamps.
    ///
    /// `jitter_max` is floored at `jitter_target` because a max below the
    /// target is a buffer that can never reach the depth it is told to prime
    /// to — it would drop frames forever while waiting to start.
    pub fn from_settings(target: usize, max: usize, frame_ms: u32) -> Latency {
        let jitter_target = target.clamp(1, JITTER_MAX);
        Latency {
            jitter_target,
            jitter_max: max.max(jitter_target),
            frame_ms: snap_frame_ms(frame_ms),
        }
    }

    /// Latency the jitter buffer alone adds, in ms.
    pub fn prime_ms(&self) -> u32 { self.jitter_target as u32 * self.frame_ms }
}

/// One peer's playout buffer.
#[derive(Clone, Debug, Default)]
pub struct Playout {
    queue: std::collections::VecDeque<Vec<f32>>,
    /// Frames dropped because the buffer was full.
    pub dropped: u64,
    /// Ticks where the buffer had nothing to give.
    pub underruns: u64,
    /// True once primed; playback does not start before then.
    started: bool,
}

impl Playout {
    pub fn depth(&self) -> usize { self.queue.len() }
    pub fn started(&self) -> bool { self.started }

    /// A decoded frame arrived.
    ///
    /// Drops the OLDEST when full, not the newest: in a live call the newest
    /// audio is what the listener needs, and dropping it to keep stale frames
    /// increases the delay it was supposed to bound.
    pub fn push(&mut self, frame: Vec<f32>, l: &Latency) {
        if self.queue.len() >= l.jitter_max {
            self.queue.pop_front();
            self.dropped += 1;
        }
        self.queue.push_back(frame);
        if !self.started && self.queue.len() >= l.jitter_target { self.started = true; }
    }

    /// Top the sink up to its cushion. Returns the frames to write.
    ///
    /// Tops up TO a cushion rather than writing one frame: one frame per tick
    /// has zero margin against timer-versus-audio-clock drift, which is the
    /// underrun this whole design exists to prevent.
    pub fn fill(&mut self, sink_depth: usize, l: &Latency) -> Vec<Vec<f32>> {
        if !self.started { return Vec::new(); }
        let want = CUSHION_FRAMES.saturating_sub(sink_depth);
        let mut out = Vec::new();
        for _ in 0..want {
            match self.queue.pop_front() {
                Some(f) => out.push(f),
                None => {
                    // Only an underrun if playback had started and the sink is
                    // actually running dry — a partially filled cushion is the
                    // normal state between frames.
                    if sink_depth == 0 && out.is_empty() { self.underruns += 1; }
                    break;
                }
            }
        }
        let _ = l;
        out
    }
}

/// Apply the makeup gain, clamped.
///
/// Clamped because a boost that can clip turns quiet audio into distorted
/// audio, which is worse than quiet.
pub fn apply_gain(sample: f32) -> f32 { (sample * PLAYOUT_GAIN).clamp(-1.0, 1.0) }

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> Vec<f32> { vec![0.1; frame_samples(FRAME_MS)] }

    #[test]
    fn the_tick_runs_at_half_a_frame_so_refill_beats_drain() {
        // "Run the tick at half a frame so it refills well before the cushion
        // drains." At one tick per frame the refill and drain RACE, which is
        // the bug.
        assert_eq!(tick_interval_ms(20), 10);
        assert_eq!(tick_interval_ms(40), 20);
        assert!(tick_interval_ms(5) >= 1, "a tick interval of zero would spin");
    }

    #[test]
    fn the_sink_is_topped_up_to_a_cushion_not_given_one_frame() {
        // One frame per tick has zero margin against timer-vs-audio-clock
        // drift — "independent of CPU governor", so no amount of speed fixes
        // it.
        let l = Latency::default();
        let mut p = Playout::default();
        for _ in 0..10 { p.push(frame(), &l); }
        let out = p.fill(0, &l);
        assert_eq!(out.len(), CUSHION_FRAMES, "wrote one frame where a cushion was needed");
    }

    #[test]
    fn a_partly_full_sink_is_only_topped_up_by_the_difference() {
        // Otherwise the cushion grows every tick and latency climbs without
        // bound.
        let l = Latency::default();
        let mut p = Playout::default();
        for _ in 0..10 { p.push(frame(), &l); }
        assert_eq!(p.fill(CUSHION_FRAMES - 1, &l).len(), 1);
        assert_eq!(p.fill(CUSHION_FRAMES, &l).len(), 0, "topped up an already-full sink");
    }

    #[test]
    fn playback_does_not_start_before_the_buffer_is_primed() {
        // Starting on the first frame means playing straight into the jitter
        // the buffer exists to absorb.
        let l = Latency::default();
        let mut p = Playout::default();
        p.push(frame(), &l);
        assert!(!p.started());
        assert!(p.fill(0, &l).is_empty());
        p.push(frame(), &l);
        assert!(p.started(), "never primed at the target depth");
        assert!(!p.fill(0, &l).is_empty());
    }

    #[test]
    fn a_full_buffer_drops_the_oldest_frame() {
        // In a live call the NEWEST audio is what the listener needs; dropping
        // it to keep stale frames increases the very delay the cap bounds.
        let l = Latency::from_settings(2, 3, 20);
        let mut p = Playout::default();
        for i in 0..3 { p.push(vec![i as f32], &l); }
        p.push(vec![99.0], &l);
        assert_eq!(p.dropped, 1);
        let out = p.fill(0, &l);
        assert_eq!(out.last().unwrap()[0], 99.0, "kept stale audio over fresh");
        assert_ne!(out[0][0], 0.0, "the oldest frame should have gone");
    }

    #[test]
    fn an_opus_frame_duration_is_snapped_to_a_legal_one() {
        // "So a stray setting can't produce un-decodable frames." Snapping
        // rather than rejecting: a bad setting costs a different frame size,
        // not a call that fails to start.
        assert_eq!(snap_frame_ms(20), 20);
        assert_eq!(snap_frame_ms(25), 20);
        assert_eq!(snap_frame_ms(30), 20, "ties resolve to the first, matching the C loop");
        assert_eq!(snap_frame_ms(0), 5);
        assert_eq!(snap_frame_ms(10_000), 60);
        assert!(LEGAL_FRAME_MS.contains(&snap_frame_ms(37)));
    }

    #[test]
    fn every_snapped_duration_is_actually_legal() {
        for req in 0..200u32 {
            assert!(LEGAL_FRAME_MS.contains(&snap_frame_ms(req)), "{req}");
        }
    }

    #[test]
    fn a_jitter_max_below_the_target_is_raised_to_it() {
        // A max below the target is a buffer that can never reach the depth it
        // is told to prime to — it would drop frames forever while waiting to
        // start.
        let l = Latency::from_settings(5, 2, 20);
        assert!(l.jitter_max >= l.jitter_target);
    }

    #[test]
    fn a_zero_jitter_target_is_floored_at_one() {
        // Zero means no buffering at all, which is the original bug restored.
        let l = Latency::from_settings(0, 10, 20);
        assert_eq!(l.jitter_target, 1);
    }

    #[test]
    fn the_configured_depth_is_capped_at_about_a_second() {
        // Beyond that it is not a jitter buffer, it is a delay line, and a call
        // becomes unusable before it becomes unreliable.
        let l = Latency::from_settings(10_000, 10_000, 20);
        assert_eq!(l.jitter_target, JITTER_MAX);
    }

    #[test]
    fn the_defaults_are_the_tuned_down_ones() {
        // Both were walked down from higher values — jitter 3 to 2, cushion 4
        // to 3 — and the next person to "optimise" latency needs to know
        // somebody already found where underruns start.
        assert_eq!(JITTER_TARGET, 2);
        assert_eq!(CUSHION_FRAMES, 3);
        assert_eq!(Latency::default().prime_ms(), 40);
    }

    #[test]
    fn frame_sizes_match_the_sample_rate() {
        assert_eq!(frame_samples(20), 320);
        assert_eq!(frame_samples(60), 960);
        assert!(frame_samples(120) <= MAX_DECODE_SAMPLES, "120 ms is the largest Opus frame");
    }

    #[test]
    fn the_makeup_gain_cannot_clip() {
        // A boost that clips turns quiet audio into DISTORTED audio, which is
        // worse than quiet.
        assert_eq!(apply_gain(1.0), 1.0);
        assert_eq!(apply_gain(-1.0), -1.0);
        assert!((apply_gain(0.4) - 0.6).abs() < 1e-5);
    }

    #[test]
    fn an_underrun_is_counted_only_when_the_sink_actually_runs_dry() {
        // A partially filled cushion is the normal state between frames;
        // counting it would make the metric useless for finding the real thing.
        let l = Latency::default();
        let mut p = Playout::default();
        for _ in 0..3 { p.push(frame(), &l); }
        p.fill(0, &l);
        assert_eq!(p.underruns, 0, "a normal drain counted as an underrun");
        p.fill(0, &l);
        assert_eq!(p.underruns, 1, "a dry sink was not counted");
    }
}
