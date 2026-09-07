//! The audio-mastered clock.
//!
//! Video is presented against this, not against wall time. The reason is
//! physical: an audio device consumes samples at its own crystal's rate, which
//! is never exactly the nominal 48 kHz. Drive video off `Instant::now()` and
//! the two drift apart by tens of milliseconds over a few minutes — slowly
//! enough that it looks like bad encoding rather than a bug.
//!
//! # The part everyone gets wrong
//!
//! Frames *written* to the device are not frames *heard*. Between the two sits
//! the device buffer — commonly 10–50 ms, and much more on a phone with a
//! conservative PipeWire quantum. The audible position is
//! `written - buffered`, so the latency must be **subtracted**. Get the sign
//! wrong and video runs ahead of audio by twice the buffer depth, which is
//! exactly the "lips out of sync" symptom, and it is invisible on a desktop
//! with a 5 ms buffer.

use crate::traits::Clock;
use std::time::Duration;

/// Media position derived from how much audio the device has actually played.
///
/// The audio thread calls [`AudioClock::advance`] from its callback; the
/// player thread reads [`Clock::now`]. In a real player these are different
/// threads, so wrap this in a lock or drive it through an atomic snapshot —
/// this type is the arithmetic, not the synchronisation.
pub struct AudioClock {
    sample_rate: u32,
    /// Frames handed to the device since `base`.
    frames_written: u64,
    /// Frames sitting in the device buffer, not yet audible. Queried from the
    /// backend each callback (`snd_pcm_delay`, PipeWire's
    /// `pw_time.delay`, cpal's output timestamp).
    latency_frames: u64,
    /// Media time at the point `frames_written` was last reset — a seek, or
    /// the start of playback.
    base: Duration,
    paused: bool,
    /// Media time frozen at the moment of pause.
    paused_at: Duration,
    rate: f64,
}

impl AudioClock {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate: sample_rate.max(1),
            frames_written: 0,
            latency_frames: 0,
            base: Duration::ZERO,
            paused: true,
            paused_at: Duration::ZERO,
            rate: 1.0,
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Report frames submitted to the device and the device's current buffer
    /// depth. Call once per audio callback.
    ///
    /// `latency_frames` is re-read every call rather than cached because it
    /// genuinely varies: buffer depth changes with quantum renegotiation, and
    /// on mobile the compositor changes it under you when the screen blanks.
    pub fn advance(&mut self, frames: u64, latency_frames: u64) {
        self.frames_written += frames;
        self.latency_frames = latency_frames;
    }

    /// The device ran dry and emitted silence for `frames`.
    ///
    /// Those frames were written but never carried media, so the clock has run
    /// ahead of the content by that much. Without this correction every
    /// underrun permanently shifts A/V sync, and underruns are routine on a
    /// loaded phone.
    pub fn report_underrun(&mut self, frames: u64) {
        self.frames_written = self.frames_written.saturating_sub(frames);
    }

    /// Reposition after a seek. Resets the frame counter, so the caller must
    /// also have flushed the device buffer — otherwise the stale audio still
    /// in it will be counted against the new position.
    pub fn set_position(&mut self, pos: Duration) {
        self.base = pos;
        self.paused_at = pos;
        self.frames_written = 0;
        self.latency_frames = 0;
    }

    fn frames_to_duration(&self, frames: u64) -> Duration {
        Duration::from_nanos(frames.saturating_mul(1_000_000_000) / self.sample_rate as u64)
    }

    /// Audible frames: written minus what is still buffered.
    fn audible_frames(&self) -> u64 {
        self.frames_written.saturating_sub(self.latency_frames)
    }

    /// How far the clock is from the position the *written* data implies.
    /// Exposed for the stats overlay; a large steady value means the buffer is
    /// deeper than it needs to be.
    pub fn buffered(&self) -> Duration {
        self.frames_to_duration(self.latency_frames.min(self.frames_written))
    }
}

impl Clock for AudioClock {
    fn now(&self) -> Duration {
        if self.paused {
            return self.paused_at;
        }
        // While priming, `written < latency` and the audible count saturates to
        // zero, holding the clock at `base`. That is correct: nothing has been
        // heard yet, so no media time has passed. Returning a negative or
        // extrapolated position here would make the first frames present early
        // and then judder as the real clock caught up.
        self.base + self.frames_to_duration(self.audible_frames())
    }

    fn set_rate(&mut self, rate: f64) {
        // Deliberately does not scale the frame count.
        //
        // Rate changes are implemented by resampling the audio before it
        // reaches the device, so `frames_written` is already in *media* frames
        // and carries the rate implicitly. Multiplying here as well would apply
        // the speed-up twice. The field is kept only so callers can read back
        // what they asked for.
        self.rate = rate.clamp(0.25, 4.0);
    }

    fn pause(&mut self) {
        if !self.paused {
            self.paused_at = self.base + self.frames_to_duration(self.audible_frames());
            self.paused = true;
        }
    }

    fn resume(&mut self) {
        if self.paused {
            // Rebase onto the frozen position and restart counting, because the
            // device buffer was drained while paused and the old frame count no
            // longer describes anything.
            self.base = self.paused_at;
            self.frames_written = 0;
            self.latency_frames = 0;
            self.paused = false;
        }
    }

    fn is_paused(&self) -> bool {
        self.paused
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Frames for `n` milliseconds at 48 kHz.
    fn f(n: u64) -> u64 {
        n * 48
    }

    fn started() -> AudioClock {
        let mut c = AudioClock::new(SR);
        c.resume();
        c
    }

    #[test]
    fn a_paused_clock_reports_its_frozen_position() {
        let mut c = AudioClock::new(SR);
        c.set_position(ms(5_000));
        assert_eq!(c.now(), ms(5_000));
        c.advance(f(100), 0);
        assert_eq!(c.now(), ms(5_000), "advancing while paused must not move it");
    }

    #[test]
    fn position_tracks_audible_frames() {
        let mut c = started();
        c.advance(f(1_000), 0);
        assert_eq!(c.now(), ms(1_000));
    }

    /// The sign check. Written 1000 ms, 200 ms still in the buffer, so 800 ms
    /// has been heard.
    #[test]
    fn buffered_audio_is_subtracted_not_added() {
        let mut c = started();
        c.advance(f(1_000), f(200));
        assert_eq!(
            c.now(),
            ms(800),
            "latency must be subtracted; adding it puts video ahead by 2x the buffer"
        );
    }

    #[test]
    fn priming_holds_the_clock_at_the_base_rather_than_going_negative() {
        let mut c = started();
        // 20 ms written, but the device wants 50 ms buffered before it starts.
        c.advance(f(20), f(50));
        assert_eq!(c.now(), Duration::ZERO);
    }

    #[test]
    fn latency_changes_are_picked_up_each_callback() {
        let mut c = started();
        c.advance(f(500), f(50));
        assert_eq!(c.now(), ms(450));
        // Quantum renegotiated to a deeper buffer.
        c.advance(f(500), f(100));
        assert_eq!(c.now(), ms(900));
        assert_eq!(c.buffered(), ms(100));
    }

    #[test]
    fn an_underrun_does_not_permanently_shift_sync() {
        let mut c = started();
        c.advance(f(1_000), 0);
        // 30 ms of that was silence the device invented.
        c.report_underrun(f(30));
        assert_eq!(c.now(), ms(970));
    }

    #[test]
    fn underrun_larger_than_the_written_count_saturates() {
        let mut c = started();
        c.advance(f(10), 0);
        c.report_underrun(f(1_000));
        assert_eq!(c.now(), Duration::ZERO);
    }

    #[test]
    fn seek_repositions_and_discards_the_old_frame_count() {
        let mut c = started();
        c.advance(f(2_000), f(50));
        c.set_position(ms(60_000));
        assert_eq!(c.now(), ms(60_000));
        c.advance(f(100), 0);
        assert_eq!(c.now(), ms(60_100));
    }

    #[test]
    fn pause_freezes_the_audible_position_and_resume_continues_from_it() {
        let mut c = started();
        c.advance(f(1_000), f(100));
        c.pause();
        assert_eq!(c.now(), ms(900));

        // Frames arriving after a pause are meaningless; must not move it.
        c.advance(f(500), 0);
        assert_eq!(c.now(), ms(900));

        c.resume();
        assert_eq!(c.now(), ms(900), "resume must not jump");
        c.advance(f(250), 0);
        assert_eq!(c.now(), ms(1_150));
    }

    /// Resume rebases because the device buffer was drained while paused; the
    /// pre-pause frame count no longer describes anything real.
    #[test]
    fn resume_does_not_replay_the_pre_pause_frame_count() {
        let mut c = started();
        c.advance(f(1_000), 0);
        c.pause();
        c.resume();
        c.advance(f(1), 0);
        let now = c.now();
        assert!(
            now < ms(1_010),
            "resume double-counted the pre-pause frames: {now:?}"
        );
    }

    /// Rate is carried by the resampler, so the clock must not apply it again.
    #[test]
    fn rate_does_not_scale_the_frame_count() {
        let mut c = started();
        c.set_rate(2.0);
        c.advance(f(1_000), 0);
        assert_eq!(
            c.now(),
            ms(1_000),
            "frames are already media frames; scaling here applies the rate twice"
        );
    }

    #[test]
    fn non_standard_sample_rates_are_exact_enough() {
        let mut c = AudioClock::new(44_100);
        c.resume();
        c.advance(44_100, 0);
        assert_eq!(c.now(), Duration::from_secs(1));
        // A rate that does not divide evenly must not accumulate visible error.
        let mut c = AudioClock::new(44_100);
        c.resume();
        for _ in 0..1_000 {
            c.advance(441, 0);
        }
        let drift = c.now().as_micros() as i64 - 10_000_000i64;
        assert!(drift.abs() < 1_000, "drift of {drift} us over 10 s");
    }

    #[test]
    fn zero_sample_rate_is_clamped_rather_than_dividing_by_zero() {
        let mut c = AudioClock::new(0);
        c.resume();
        c.advance(1, 0);
        let _ = c.now();
        assert_eq!(c.sample_rate(), 1);
    }
}
