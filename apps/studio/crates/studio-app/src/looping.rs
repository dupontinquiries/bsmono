//! looping — a loop range on the arrangement, and takes per pass.
//!
//! The arrangement plays as one rendered buffer the sink runs to the
//! end; the sink can also loop a buffer, but only whole. So a loop range
//! is a different buffer: the range alone, rendered and set looped, with
//! the sink's frame counter mapped back into the timeline by [`Window`]
//! — `start + (frame mod len)`. The wrap happens in the callback, sample
//! exact, which matters for loop RECORDING: the singer hears the bar
//! come round on time, so what she sings lands on time.
//!
//! Loop-record is then a rule on top of that: each time the playhead
//! comes round, what the microphone caught since the last time is one
//! take, placed at the loop's start, on the next free lane (see
//! `studio_core`'s takes module). [`Passes`] is the detector — a wrap is
//! the mapped playhead going BACKWARDS while recording — kept pure so it
//! can be tested with numbers rather than a device.
//!
//! The pass boundary is where the UI saw the wrap, not the sample the
//! callback wrapped at; a frame's worth of the next pass's audio can end
//! up at the tail of this one. It is the tail of a take, past the loop's
//! end, and it is trimmed by the loop's length when the take is placed.
//! Not corrected further: a producer trims a take's edges anyway, and a
//! sample-exact split needs the capture and the sink on one clock, which
//! they are not (two devices, two clocks — the README's transport
//! section says why that is never assumed).

use studio_core::Ticks;

/// A loop range as the sink sees it: the rendered range's start and
/// length in the sink's frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub start: Ticks,
    pub end: Ticks,
    pub rate: u32,
}

impl Window {
    /// `None` for a range that cannot loop (end not after start).
    pub fn new(range: Option<(Ticks, Ticks)>, rate: u32) -> Option<Window> {
        let (start, end) = range?;
        (end > start && rate > 0).then_some(Window { start, end, rate })
    }
    pub fn frames(&self) -> usize {
        (self.end.to_samples(self.rate) - self.start.to_samples(self.rate)).max(1) as usize
    }
    /// The timeline position for a sink frame counter inside the looped
    /// buffer (the sink wraps its own counter, so `frame` is already in
    /// `0..frames()`; a larger one is wrapped here too, for safety).
    pub fn timeline(&self, frame: usize) -> Ticks {
        let f = frame % self.frames();
        Ticks::from_samples(self.start.to_samples(self.rate) + f as i64, self.rate)
    }
    /// The sink frame for a timeline position: inside the range it maps
    /// directly; outside, the start (a seek outside a loop lands on it).
    pub fn frame(&self, at: Ticks) -> usize {
        if at < self.start || at >= self.end { return 0; }
        (at.to_samples(self.rate) - self.start.to_samples(self.rate)).max(0) as usize
    }
}

/// The loop-record detector: fed the playhead each frame while
/// recording, says when a pass completed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Passes {
    last: Option<Ticks>,
    /// Passes completed since `start`.
    pub done: u32,
}

impl Passes {
    /// Arm at the start of a recording.
    pub fn start() -> Passes { Passes { last: None, done: 0 } }
    /// The playhead moved to `at`. Returns `true` on the frame it came
    /// round — moved backwards by more than a jitter's worth — which is
    /// the moment to cut the take so far.
    pub fn observe(&mut self, at: Ticks) -> bool {
        let wrapped = match self.last {
            Some(prev) => at < prev && prev.0 - at.0 > Ticks::from_seconds(0.05).0,
            None => false,
        };
        self.last = Some(at);
        if wrapped { self.done += 1; }
        wrapped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_maps_the_sinks_counter_back_into_the_loop() {
        let w = Window::new(Some((Ticks::from_seconds(2.0), Ticks::from_seconds(4.0))), 48_000).unwrap();
        assert_eq!(w.frames(), 96_000);
        assert_eq!(w.timeline(0), Ticks::from_seconds(2.0));
        assert_eq!(w.timeline(48_000), Ticks::from_seconds(3.0));
        assert_eq!(w.timeline(96_000), Ticks::from_seconds(2.0), "the counter wraps to the start");
        assert_eq!(w.frame(Ticks::from_seconds(3.5)), 72_000);
        assert_eq!(w.frame(Ticks::from_seconds(1.0)), 0, "a seek before the loop lands on its start");
        assert_eq!(w.frame(Ticks::from_seconds(4.0)), 0, "and at its end");
        assert!(Window::new(Some((Ticks(10), Ticks(10))), 48_000).is_none(), "an empty range cannot loop");
        assert!(Window::new(Some((Ticks(20), Ticks(10))), 48_000).is_none());
        assert!(Window::new(None, 48_000).is_none());
    }

    #[test]
    fn a_pass_is_the_playhead_coming_round_not_a_jitter() {
        let mut p = Passes::start();
        let s = |x: f64| Ticks::from_seconds(x);
        assert!(!p.observe(s(2.0)), "the first observation is not a wrap");
        assert!(!p.observe(s(2.5)));
        assert!(!p.observe(s(2.49)), "a millisecond backwards is a clock jitter, not a pass");
        assert!(!p.observe(s(3.9)));
        assert!(p.observe(s(2.02)), "back to the start: one pass done");
        assert_eq!(p.done, 1);
        assert!(!p.observe(s(3.0)));
        assert!(p.observe(s(2.0)));
        assert_eq!(p.done, 2);
        assert_eq!(Passes::start(), Passes::default());
    }
}
