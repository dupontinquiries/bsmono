//! The frame queue that sits between decode and present.
//!
//! This is the thing that actually makes the UI smooth, and it is deliberately
//! pure so the behaviour can be tested without a GPU or a compositor.
//!
//! The core idea: the shell does **not** run a free-running repaint loop. It
//! asks the queue what to do, and the queue answers with either a frame to
//! show or a duration to sleep. On a 90 Hz phone panel playing 24 fps video
//! that is 24 wakeups per second instead of 90, which is most of the battery
//! win available here.

use crate::clock::{Action, SyncPolicy};
use crate::frame::{Frame, RecycleToken};
use std::collections::VecDeque;
use std::time::Duration;

#[derive(Debug)]
pub enum Poll {
    /// Show this now, then ask again.
    Show(Frame),
    /// Nothing due yet. Sleep at most this long, then ask again.
    ///
    /// The shell maps this onto its repaint scheduling — for egui that is
    /// `ctx.request_repaint_after(d)`, for GTK4 a `FrameClock` tick or a
    /// timeout source. Either way: schedule, do not spin.
    Wait(Duration),
    /// Queue is empty. Decode is behind, or the stream ended.
    Starved,
}

/// How the queue behaves when it fills up.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Pressure {
    /// Normal: hold `capacity` frames, refuse more.
    Normal,
    /// Decode is ahead and memory is tight (phones). Drop the *oldest*
    /// undisplayed frames rather than the newest — dropping new frames makes
    /// the picture freeze while audio runs on, which looks far worse than a
    /// brief judder.
    DropOldest,
}

pub struct FrameQueue {
    frames: VecDeque<Frame>,
    capacity: usize,
    policy: SyncPolicy,
    pressure: Pressure,
    dropped_late: u32,
    dropped_pressure: u32,
    /// Tokens from frames the queue discarded on its own — late drops,
    /// overflow drops, flushes.
    ///
    /// Without this they would vanish with the `Frame`, and with a hardware
    /// decoder that is a slow buffer leak: the pool shrinks by one every time
    /// playback stutters, and eventually the decoder blocks forever. Software
    /// frames have `recycle: None` and contribute nothing here, which is why
    /// this is invisible until you run on real hardware.
    reclaimed: Vec<RecycleToken>,
    shown: u64,
    /// Set once the first frame has been shown. Before that we present
    /// immediately regardless of PTS, so startup does not stall waiting for a
    /// clock that has not been started yet.
    started: bool,
}

impl FrameQueue {
    pub fn new(capacity: usize, policy: SyncPolicy) -> Self {
        Self {
            frames: VecDeque::with_capacity(capacity.max(1)),
            capacity: capacity.max(1),
            policy,
            pressure: Pressure::Normal,
            dropped_late: 0,
            dropped_pressure: 0,
            reclaimed: Vec::new(),
            shown: 0,
            started: false,
        }
    }

    pub fn with_pressure(mut self, p: Pressure) -> Self {
        self.pressure = p;
        self
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn is_full(&self) -> bool {
        self.frames.len() >= self.capacity
    }

    /// Frames dropped for being too late to matter, since last read.
    pub fn take_late_drops(&mut self) -> u32 {
        std::mem::take(&mut self.dropped_late)
    }

    /// Frames dropped because the queue overflowed, since last read.
    pub fn take_pressure_drops(&mut self) -> u32 {
        std::mem::take(&mut self.dropped_pressure)
    }

    pub fn shown_count(&self) -> u64 {
        self.shown
    }

    /// Take the recycle tokens of every frame the queue discarded. The caller
    /// must hand these back to the decoder. Draining this is not optional.
    pub fn take_reclaimed(&mut self) -> Vec<RecycleToken> {
        std::mem::take(&mut self.reclaimed)
    }

    fn discard(&mut self, mut f: Frame) {
        if let Some(t) = f.recycle.take() {
            self.reclaimed.push(t);
        }
    }

    /// Returns the frame back to the caller if it did not fit, so the decoder
    /// can hold it rather than losing it.
    pub fn push(&mut self, frame: Frame) -> Option<Frame> {
        if self.frames.len() < self.capacity {
            self.frames.push_back(frame);
            return None;
        }
        match self.pressure {
            Pressure::Normal => Some(frame),
            Pressure::DropOldest => {
                if let Some(old) = self.frames.pop_front() {
                    self.discard(old);
                }
                self.dropped_pressure += 1;
                self.frames.push_back(frame);
                None
            }
        }
    }

    /// Ask what to do at media time `now`.
    ///
    /// Drains late frames in a loop rather than one per call: after a seek or
    /// a decode stall there can be many, and returning `Show` for each in turn
    /// would present a burst of stale frames at wall-clock speed.
    pub fn poll(&mut self, now: Duration) -> Poll {
        loop {
            let pts = match self.frames.front() {
                Some(f) => f.pts,
                None => return Poll::Starved,
            };

            if !self.started {
                self.started = true;
                self.shown += 1;
                return Poll::Show(self.frames.pop_front().expect("checked above"));
            }

            match self.policy.decide(pts, now) {
                Action::Show => {
                    self.shown += 1;
                    return Poll::Show(self.frames.pop_front().expect("checked above"));
                }
                Action::Hold(d) => return Poll::Wait(d),
                Action::Drop => {
                    if let Some(old) = self.frames.pop_front() {
                        self.discard(old);
                    }
                    self.dropped_late += 1;
                    // ...and re-examine the next one.
                }
            }
        }
    }

    /// Discard everything. Used on seek and on decoder reconfiguration.
    /// Does not count as drops — those numbers are for the stats overlay and a
    /// seek is not a performance problem.
    pub fn flush(&mut self) {
        while let Some(f) = self.frames.pop_front() {
            self.discard(f);
        }
        self.started = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{ColorInfo, CpuPlanes, FrameData, PlaneLayout};

    fn frame_with_token(ms: u64, token: u32) -> Frame {
        let mut f = frame_at(ms);
        f.recycle = Some(RecycleToken(token));
        f
    }

    fn frame_at(ms: u64) -> Frame {
        Frame {
            data: FrameData::Cpu(CpuPlanes {
                data: vec![vec![0u8; 16]],
                strides: vec![4],
            }),
            width: 4,
            height: 4,
            layout: PlaneLayout::I420,
            color: ColorInfo::default(),
            pts: Duration::from_millis(ms),
            duration: Some(Duration::from_millis(16)),
            recycle: None,
            grain: None,
        }
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn q() -> FrameQueue {
        FrameQueue::new(4, SyncPolicy::for_refresh_hz(60))
    }

    #[test]
    fn empty_queue_reports_starvation() {
        let mut q = q();
        assert!(matches!(q.poll(ms(0)), Poll::Starved));
    }

    #[test]
    fn first_frame_shows_immediately_regardless_of_pts() {
        let mut q = q();
        // A far-future PTS must not stall startup.
        q.push(frame_at(10_000));
        assert!(matches!(q.poll(ms(0)), Poll::Show(_)));
    }

    #[test]
    fn subsequent_future_frame_waits() {
        let mut q = q();
        q.push(frame_at(0));
        q.push(frame_at(500));
        let _ = q.poll(ms(0));
        match q.poll(ms(0)) {
            Poll::Wait(d) => assert!(d > ms(480), "unexpected wait {d:?}"),
            other => panic!("expected Wait, got {other:?}"),
        }
    }

    /// The behaviour that matters after a stall: many stale frames get dropped
    /// in one poll, not one per repaint.
    #[test]
    fn a_backlog_of_late_frames_drains_in_a_single_poll() {
        let mut q = FrameQueue::new(8, SyncPolicy::for_refresh_hz(60));
        q.push(frame_at(0));
        let _ = q.poll(ms(0)); // consume the startup frame

        for t in [100, 200, 300, 400, 990] {
            q.push(frame_at(t));
        }
        // Clock has jumped to 1s; the first four are far past the late limit.
        match q.poll(ms(1000)) {
            Poll::Show(f) => assert_eq!(f.pts, ms(990)),
            other => panic!("expected the fresh frame, got {other:?}"),
        }
        assert_eq!(q.take_late_drops(), 4);
        assert_eq!(q.take_late_drops(), 0, "drop count must reset when read");
    }

    #[test]
    fn full_queue_hands_the_frame_back_by_default() {
        let mut q = FrameQueue::new(2, SyncPolicy::for_refresh_hz(60));
        assert!(q.push(frame_at(0)).is_none());
        assert!(q.push(frame_at(16)).is_none());
        let rejected = q.push(frame_at(32));
        assert!(rejected.is_some(), "decoder must be able to retain the frame");
        assert_eq!(q.len(), 2);
    }

    #[test]
    fn drop_oldest_keeps_the_newest_frames() {
        let mut q = FrameQueue::new(2, SyncPolicy::for_refresh_hz(60))
            .with_pressure(Pressure::DropOldest);
        q.push(frame_at(0));
        q.push(frame_at(16));
        assert!(q.push(frame_at(32)).is_none(), "must accept under DropOldest");
        assert_eq!(q.take_pressure_drops(), 1);
        // Oldest went, newest stayed.
        let _ = q.poll(ms(0)); // startup frame, whatever it is
        match q.poll(ms(32)) {
            Poll::Show(f) => assert_eq!(f.pts, ms(32)),
            other => panic!("expected pts=32, got {other:?}"),
        }
    }

    #[test]
    fn flush_resets_the_startup_behaviour() {
        let mut q = q();
        q.push(frame_at(0));
        let _ = q.poll(ms(0));
        q.flush();
        assert!(q.is_empty());
        // After a seek the next frame must again show immediately rather than
        // waiting on a PTS from the new position.
        q.push(frame_at(60_000));
        assert!(matches!(q.poll(ms(0)), Poll::Show(_)));
    }

    /// The leak this field exists to prevent: a late-dropped frame's buffer
    /// must come back, or the hardware pool bleeds out over a long playback.
    #[test]
    fn late_dropped_frames_return_their_buffers() {
        let mut q = FrameQueue::new(8, SyncPolicy::for_refresh_hz(60));
        q.push(frame_with_token(0, 100));
        let _ = q.poll(ms(0));

        q.push(frame_with_token(100, 101));
        q.push(frame_with_token(200, 102));
        q.push(frame_with_token(990, 103));
        let _ = q.poll(ms(1000));

        let mut got: Vec<u32> = q.take_reclaimed().into_iter().map(|t| t.0).collect();
        got.sort();
        assert_eq!(got, vec![101, 102], "dropped buffers must be reclaimed");
        assert!(q.take_reclaimed().is_empty(), "reclaim list must drain once");
    }

    #[test]
    fn overflow_drops_return_their_buffers() {
        let mut q = FrameQueue::new(2, SyncPolicy::for_refresh_hz(60))
            .with_pressure(Pressure::DropOldest);
        q.push(frame_with_token(0, 7));
        q.push(frame_with_token(16, 8));
        q.push(frame_with_token(32, 9));
        let got: Vec<u32> = q.take_reclaimed().into_iter().map(|t| t.0).collect();
        assert_eq!(got, vec![7]);
    }

    #[test]
    fn flush_returns_every_held_buffer() {
        let mut q = q();
        q.push(frame_with_token(0, 1));
        q.push(frame_with_token(16, 2));
        q.push(frame_with_token(32, 3));
        q.flush();
        let mut got: Vec<u32> = q.take_reclaimed().into_iter().map(|t| t.0).collect();
        got.sort();
        assert_eq!(got, vec![1, 2, 3]);
    }

    /// A presented frame's token belongs to the caller, not the queue.
    #[test]
    fn shown_frames_are_not_reclaimed_by_the_queue() {
        let mut q = q();
        q.push(frame_with_token(0, 42));
        match q.poll(ms(0)) {
            Poll::Show(f) => assert_eq!(f.recycle, Some(RecycleToken(42))),
            other => panic!("expected Show, got {other:?}"),
        }
        assert!(q.take_reclaimed().is_empty());
    }

    #[test]
    fn flush_does_not_inflate_the_drop_counters() {
        let mut q = q();
        q.push(frame_at(0));
        q.push(frame_at(16));
        q.flush();
        assert_eq!(q.take_late_drops(), 0);
        assert_eq!(q.take_pressure_drops(), 0);
    }

    #[test]
    fn shown_count_tracks_presentations() {
        let mut q = q();
        q.push(frame_at(0));
        q.push(frame_at(16));
        let _ = q.poll(ms(0));
        let _ = q.poll(ms(16));
        assert_eq!(q.shown_count(), 2);
    }
}
