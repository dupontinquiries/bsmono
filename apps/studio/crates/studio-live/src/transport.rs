//! transport — the clock everything else follows.
//!
//! A playhead is easy to get *approximately* right and surprisingly hard to get
//! exactly right, and the errors are the ones users report as "it feels wrong"
//! without being able to say why. Three of them matter enough to be designed
//! around rather than patched later:
//!
//! **1. The clock must count samples, not seconds.** Advancing a float by
//! `dt` every UI frame accumulates error, and worse, ties musical time to frame
//! rate — a dropped frame moves the playhead. [`Transport::advance`] takes the
//! number of frames the audio device actually consumed, so the playhead is
//! wherever the samples say it is. Video follows audio, never the reverse.
//!
//! **2. Recorded audio arrives late.** Input passes through the device buffer
//! before your callback sees it, so a take written at the position the playhead
//! reads is late by exactly the round-trip latency — the classic symptom being
//! that everything you record drifts behind the click by a consistent few
//! milliseconds. [`Transport::record_position`] subtracts it. This is the single
//! most common bug in home-grown recorders and it is invisible until you layer
//! two takes.
//!
//! **3. Loops must not drop samples at the seam.** When the playhead crosses
//! the loop end mid-buffer, the remainder belongs to the *start* of the loop,
//! not to the next buffer. [`Advance`] reports the split so a renderer can fill
//! both halves.

use studio_core::Ticks;

/// Where playback is and what it is doing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transport {
    pub playing: bool,
    pub recording: bool,
    /// Playhead, in samples at [`Transport::rate`] — the authoritative position.
    /// Exposed as ticks through [`Transport::position`]; kept in samples so it
    /// cannot drift.
    frame: i64,
    pub rate: u32,
    /// Loop range, if looping. Half-open: `[start, end)`.
    pub loop_range: Option<(Ticks, Ticks)>,
    /// Round-trip device latency. Subtracted when stamping recorded audio.
    pub latency: Ticks,
    /// Bars of click before recording actually punches in. Counted in ticks so
    /// the transport does not need to know the tempo.
    pub count_in: Ticks,
    /// Set while the count-in is running: the clock moves, the recorder does not
    /// arm, and the UI can show the countdown.
    counting_in: bool,
    /// Frame the count-in is counting toward. Stored rather than recomputed so
    /// the punch point cannot drift if the tempo or count length changes while
    /// the count is already running.
    punch_frame: i64,
}

impl Transport {
    pub fn new(rate: u32) -> Transport {
        Transport {
            playing: false, recording: false, frame: 0, rate,
            loop_range: None, latency: Ticks::ZERO, count_in: Ticks::ZERO,
            counting_in: false, punch_frame: 0,
        }
    }

    pub fn position(&self) -> Ticks { Ticks::from_samples(self.frame, self.rate) }
    pub fn set_position(&mut self, t: Ticks) { self.frame = t.to_samples(self.rate).max(0); }
    pub fn is_counting_in(&self) -> bool { self.counting_in }

    /// Start playing from the current position.
    pub fn play(&mut self) { self.playing = true; }

    /// Stop, leaving the playhead where it is (the behaviour editors expect;
    /// "return to start" is a separate, explicit action).
    pub fn stop(&mut self) {
        self.playing = false;
        self.recording = false;
        self.counting_in = false;
    }

    /// Begin recording. With a count-in configured, the clock rolls back so the
    /// count happens BEFORE the punch point — which is what a count-in is for.
    /// Without one, recording starts immediately at the playhead.
    pub fn start_recording(&mut self, punch_at: Ticks) {
        self.punch_frame = punch_at.to_samples(self.rate).max(0);
        self.set_position(punch_at.saturating_sub(self.count_in));
        self.counting_in = self.count_in.0 > 0;
        self.recording = !self.counting_in;
        self.playing = true;
    }

    /// Where a sample captured *now* actually belongs on the timeline.
    ///
    /// The device handed us audio that entered the microphone `latency` ago, so
    /// stamping it at the current playhead places it late by that much. Clamped
    /// at zero: a large configured latency near the start of a project must not
    /// produce a negative position.
    pub fn record_position(&self) -> Ticks {
        let p = self.position();
        Ticks(p.0.saturating_sub(self.latency.0).max(0))
    }

    /// Advance by `frames` of audio. Returns what happened, so the caller can
    /// render the pieces in the right order.
    pub fn advance(&mut self, frames: i64) -> Advance {
        if !self.playing || frames <= 0 {
            return Advance { from: self.position(), to: self.position(), wrapped: None, punched_in: false };
        }
        let from = self.position();
        let mut punched_in = false;

        // Count-in: the clock runs but the recorder stays disarmed until it
        // reaches the punch point. `punch_at` was stored by rolling the position
        // back, so the punch is simply "count_in past where we started".
        if self.counting_in {
            let punch = self.punch_frame;
            self.frame += frames;
            if self.frame >= punch {
                self.counting_in = false;
                self.recording = true;
                punched_in = true;
            }
            return Advance { from, to: self.position(), wrapped: None, punched_in };
        }

        self.frame += frames;
        let mut wrapped = None;
        if let Some((s, e)) = self.loop_range {
            if e.0 > s.0 && self.position() >= e {
                // The overshoot belongs to the START of the loop, not to the
                // next buffer — dropping it is what makes a loop click.
                let over = self.position().saturating_sub(e);
                self.set_position(s.saturating_add(over));
                wrapped = Some((e, s));
            }
        }
        Advance { from, to: self.position(), wrapped, punched_in }
    }
}

/// What one `advance` did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Advance {
    pub from: Ticks,
    pub to: Ticks,
    /// `(hit_end, resumed_at)` when the loop wrapped inside this buffer.
    pub wrapped: Option<(Ticks, Ticks)>,
    /// True on the buffer where a count-in ended and recording armed.
    pub punched_in: bool,
}

impl Advance {
    /// The spans of timeline this buffer covers, in order — one normally, two
    /// when the loop wrapped. A renderer walks these instead of assuming the
    /// buffer is contiguous, which it is not at a loop seam.
    pub fn spans(&self) -> Vec<(Ticks, Ticks)> {
        match self.wrapped {
            None => vec![(self.from, self.to)],
            Some((end, start)) => vec![(self.from, end), (start, self.to)],
        }
    }
}

/// Who drives the clock in a shared session.
///
/// In theater mode one site holds the transport and everyone follows its
/// position; in split mode every site runs its own. This is the transport-level
/// half of `studio_egui::touch::ViewMode` — kept here because the clock is not
/// a UI concern, and because a follower must be able to correct for network
/// delay without the UI knowing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Sync {
    /// We drive; our position is broadcast.
    Host,
    /// We follow the host's position.
    Follow { host_pos: Ticks, at_rtt: Ticks },
    /// Nobody is shared; local clock only.
    Local,
}

impl Sync {
    /// Where a follower should actually be, given what the host last said.
    ///
    /// The host's packet describes where it was one network hop ago, so a naive
    /// follower is always behind by the round trip. Adding half the RTT puts us
    /// where the host *is now* rather than where it was — the same correction
    /// clock sync has used for decades, and the difference between a shared
    /// view that feels live and one that feels laggy.
    pub fn corrected(&self, local: Ticks) -> Ticks {
        match self {
            Sync::Follow { host_pos, at_rtt } => Ticks(host_pos.0 + at_rtt.0 / 2),
            _ => local,
        }
    }

    /// Should a follower re-seek, or is it close enough to let its own clock
    /// run? Re-seeking every packet makes audio stutter; never re-seeking lets
    /// drift accumulate. Correct within a threshold, jump beyond it.
    pub fn should_reseek(&self, local: Ticks, threshold: Ticks) -> bool {
        match self {
            Sync::Follow { .. } => (self.corrected(local).0 - local.0).abs() > threshold.0,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    #[test]
    fn the_playhead_counts_samples_and_does_not_drift() {
        // 10,000 buffers of an awkward size. A float clock advanced by dt would
        // be visibly wrong by here; a sample counter cannot be.
        let mut t = Transport::new(RATE);
        t.play();
        for _ in 0..10_000 { t.advance(441); }
        let expect = Ticks::from_samples(4_410_000, RATE);
        assert_eq!(t.position(), expect, "the playhead is exactly where the samples say");
    }

    #[test]
    fn a_stopped_transport_stays_put() {
        let mut t = Transport::new(RATE);
        t.set_position(Ticks::from_seconds(5.0));
        let a = t.advance(4096);
        assert_eq!(t.position(), Ticks::from_seconds(5.0));
        assert_eq!(a.from, a.to);
        // and stopping leaves the playhead alone rather than rewinding it
        t.play();
        t.advance(RATE as i64);
        t.stop();
        assert!(t.position() > Ticks::from_seconds(5.0));
    }

    #[test]
    fn recorded_audio_is_stamped_back_by_the_device_latency() {
        // THE bug this exists to prevent: without compensation every take lands
        // a few milliseconds behind the click, which is invisible until you
        // layer two of them.
        let mut t = Transport::new(RATE);
        t.latency = Ticks::from_samples(512, RATE);
        t.set_position(Ticks::from_seconds(10.0));
        let expected = Ticks::from_seconds(10.0).saturating_sub(Ticks::from_samples(512, RATE));
        assert_eq!(t.record_position(), expected, "the take belongs where the sound happened");
        assert!(t.record_position() < t.position());
        // near zero it clamps instead of going negative
        t.set_position(Ticks::from_samples(10, RATE));
        assert_eq!(t.record_position(), Ticks::ZERO);
    }

    #[test]
    fn a_loop_wrap_reports_both_halves_of_the_buffer() {
        // The remainder past the loop end belongs to the START of the loop.
        // Dropping it is what makes a loop click.
        let mut t = Transport::new(RATE);
        t.loop_range = Some((Ticks::from_seconds(1.0), Ticks::from_seconds(2.0)));
        t.set_position(Ticks::from_seconds(2.0).saturating_sub(Ticks::from_samples(100, RATE)));
        t.play();
        let a = t.advance(500);
        let (end, start) = a.wrapped.expect("it should have wrapped");
        assert_eq!(end, Ticks::from_seconds(2.0));
        assert_eq!(start, Ticks::from_seconds(1.0));
        // 400 frames of overshoot land after the loop start
        assert_eq!(t.position(), Ticks::from_seconds(1.0).saturating_add(Ticks::from_samples(400, RATE)));
        // and the renderer is told to fill two spans, not one
        let spans = a.spans();
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].1, Ticks::from_seconds(2.0));
        assert_eq!(spans[1].0, Ticks::from_seconds(1.0));
    }

    #[test]
    fn no_loop_means_one_contiguous_span() {
        let mut t = Transport::new(RATE);
        t.play();
        let a = t.advance(1024);
        assert!(a.wrapped.is_none());
        assert_eq!(a.spans().len(), 1);
        assert_eq!(a.spans()[0], (Ticks::ZERO, t.position()));
    }

    #[test]
    fn a_count_in_rolls_back_and_arms_at_the_punch_point() {
        let mut t = Transport::new(RATE);
        t.count_in = Ticks::from_seconds(2.0);
        t.start_recording(Ticks::from_seconds(30.0));
        assert!(t.is_counting_in());
        assert!(!t.recording, "the recorder must not be armed during the count");
        assert_eq!(t.position(), Ticks::from_seconds(28.0), "the clock rolled back to count first");

        // run most of the count — still not armed
        t.advance(RATE as i64);
        assert!(!t.recording);
        // cross the punch point
        let a = t.advance(RATE as i64 + 10);
        assert!(a.punched_in, "the buffer that crosses the punch reports it");
        assert!(t.recording, "and recording is now armed");
        assert!(!t.is_counting_in());
        assert!(t.position() >= Ticks::from_seconds(30.0));
    }

    #[test]
    fn recording_without_a_count_in_starts_immediately() {
        let mut t = Transport::new(RATE);
        t.start_recording(Ticks::from_seconds(4.0));
        assert!(t.recording);
        assert!(!t.is_counting_in());
        assert_eq!(t.position(), Ticks::from_seconds(4.0));
    }

    #[test]
    fn a_follower_corrects_for_the_network_hop() {
        // The host's packet says where it WAS a hop ago; a naive follower is
        // permanently behind. Half the round trip puts us where it is now.
        let local = Ticks::from_seconds(10.0);
        let s = Sync::Follow { host_pos: Ticks::from_seconds(12.0), at_rtt: Ticks::from_seconds(0.08) };
        let c = s.corrected(local);
        assert_eq!(c, Ticks::from_seconds(12.04), "host position plus half the RTT");
        // host and local modes never rewrite the local clock
        assert_eq!(Sync::Host.corrected(local), local);
        assert_eq!(Sync::Local.corrected(local), local);
    }

    #[test]
    fn a_follower_only_reseeks_when_it_has_really_drifted() {
        // Re-seeking every packet stutters; never re-seeking drifts. Correct
        // within a threshold, jump beyond it.
        let local = Ticks::from_seconds(10.0);
        let tol = Ticks::from_seconds(0.25);
        let close = Sync::Follow { host_pos: Ticks::from_seconds(10.05), at_rtt: Ticks::ZERO };
        assert!(!close.should_reseek(local, tol), "a small offset is left to the local clock");
        let far = Sync::Follow { host_pos: Ticks::from_seconds(45.0), at_rtt: Ticks::ZERO };
        assert!(far.should_reseek(local, tol), "someone scrubbed — jump");
        assert!(!Sync::Local.should_reseek(local, tol), "a local transport never re-seeks");
    }
}
