//! Which stretch of the video to transcribe next.
//!
//! Transcribing an hour of lecture takes minutes even on a laptop and far
//! longer on a phone, so ORDER is the whole user-visible behaviour: the
//! stretch being watched first (so subtitles and "what is this section"
//! appear where the viewer is), then forward (where they are going), then the
//! start (so the chapter list completes).
//!
//! [`Mode`] is the steering wheel. `Ahead` bounds the work to a horizon past
//! the playhead — the battery setting, and the right one for watching rather
//! than navigating. `All` builds the whole chapter list. `Paused` stops between
//! chunks without losing what is done.

use std::collections::BTreeSet;
use std::time::Duration;

#[derive(Copy, Clone, PartialEq, Debug)]
pub enum Mode {
    /// Nothing new is started. The chunk in flight finishes.
    Paused,
    /// Only chunks starting before `playhead + horizon`, and never ones
    /// entirely behind the playhead.
    Ahead(Duration),
    /// Everything, playhead first.
    All,
}

/// One unit of work.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Chunk {
    pub index: u32,
    pub start: Duration,
    /// End of the span this chunk OWNS: cues starting in `start..end` are its.
    pub end: Duration,
    /// Extra audio decoded past `end`, so a sentence that starts before the
    /// boundary is heard to its finish rather than cut mid-word.
    pub post_roll: Duration,
}

impl Chunk {
    /// The audio to decode for this chunk: owned span plus post-roll.
    pub fn decode_span(&self) -> (Duration, Duration) {
        (self.start, self.end + self.post_roll)
    }
}

#[derive(Clone, Debug)]
pub struct Coverage {
    chunk: Duration,
    post_roll: Duration,
    /// Unknown for a live or unindexed stream; learned when a chunk comes
    /// back empty (see [`Coverage::mark_end`]).
    total: Option<Duration>,
    done: BTreeSet<u32>,
    in_flight: Option<u32>,
    pub mode: Mode,
}

impl Coverage {
    /// 30 s chunks match whisper's own window: shorter wastes its context,
    /// longer delays the first result. 2 s of post-roll covers a sentence
    /// tail without doubling the work.
    pub fn new(total: Option<Duration>) -> Self {
        Self::with_chunk(total, Duration::from_secs(30), Duration::from_secs(2))
    }

    pub fn with_chunk(total: Option<Duration>, chunk: Duration, post_roll: Duration) -> Self {
        Self {
            // A zero chunk would make every index the same instant and the
            // scheduler loop forever on chunk 0.
            chunk: chunk.max(Duration::from_secs(1)),
            post_roll,
            total,
            done: BTreeSet::new(),
            in_flight: None,
            mode: Mode::All,
        }
    }

    pub fn chunk_len(&self) -> Duration {
        self.chunk
    }

    pub fn total(&self) -> Option<Duration> {
        self.total
    }

    fn count(&self) -> Option<u32> {
        self.total.map(|t| {
            let n = t.as_secs_f64() / self.chunk.as_secs_f64();
            n.ceil() as u32
        })
    }

    fn index_at(&self, t: Duration) -> u32 {
        (t.as_secs_f64() / self.chunk.as_secs_f64()).floor() as u32
    }

    pub fn chunk(&self, index: u32) -> Chunk {
        let start = self.chunk * index;
        let mut end = start + self.chunk;
        if let Some(t) = self.total {
            end = end.min(t);
        }
        // No post-roll past the end of the media: ffmpeg would return less
        // audio than asked and that is fine, but asking is misleading.
        let post_roll = match self.total {
            Some(t) => self.post_roll.min(t.saturating_sub(end)),
            None => self.post_roll,
        };
        Chunk { index, start, end, post_roll }
    }

    fn eligible(&self, i: u32) -> bool {
        !self.done.contains(&i)
            && self.in_flight != Some(i)
            && self.count().map_or(true, |n| i < n)
    }

    /// The next chunk to transcribe, or `None` if there is nothing to do in
    /// this mode. Does not mark it in flight; [`Coverage::begin`] does.
    pub fn next(&self, playhead: Duration) -> Option<Chunk> {
        let here = self.index_at(playhead);
        match self.mode {
            Mode::Paused => None,
            Mode::Ahead(horizon) => {
                let last = self.index_at(playhead + horizon);
                (here..=last).find(|&i| self.eligible(i)).map(|i| self.chunk(i))
            }
            Mode::All => {
                // Forward from the playhead; for an unknown length, forward
                // is open-ended until mark_end closes it.
                let forward_limit = self.count().unwrap_or(u32::MAX);
                let fwd = (here..forward_limit).take(100_000).find(|&i| self.eligible(i));
                fwd.or_else(|| (0..here.min(forward_limit)).find(|&i| self.eligible(i)))
                    .map(|i| self.chunk(i))
            }
        }
    }

    pub fn begin(&mut self, c: &Chunk) {
        self.in_flight = Some(c.index);
    }

    pub fn in_flight(&self) -> Option<Chunk> {
        self.in_flight.map(|i| self.chunk(i))
    }

    /// The chunk finished (successfully or not: a chunk that FAILS is still
    /// done, or a model that crashes on one stretch of audio is asked about
    /// it forever and nothing after it is ever transcribed).
    pub fn finish(&mut self, index: u32) {
        self.done.insert(index);
        if self.in_flight == Some(index) {
            self.in_flight = None;
        }
    }

    /// A chunk decoded to no audio at all: the media ends at its start.
    /// Chunks past it are forgotten rather than attempted.
    pub fn mark_end(&mut self, index: u32) {
        let end = self.chunk * index;
        self.total = Some(match self.total {
            Some(t) => t.min(end),
            None => end,
        });
        if self.in_flight == Some(index) {
            self.in_flight = None;
        }
        self.done.retain(|&i| i < index);
    }

    pub fn is_done(&self, index: u32) -> bool {
        self.done.contains(&index)
    }

    pub fn done_indices(&self) -> impl Iterator<Item = u32> + '_ {
        self.done.iter().copied()
    }

    /// Restore from a cache. Indices past a known end are ignored.
    pub fn restore_done(&mut self, done: impl IntoIterator<Item = u32>) {
        let n = self.count();
        self.done.extend(done.into_iter().filter(|&i| n.map_or(true, |n| i < n)));
    }

    /// Fraction of the known length transcribed, 0..1. `None` while the
    /// length is unknown — a percentage of an unknown total is invented.
    pub fn progress(&self) -> Option<f32> {
        let n = self.count()?;
        if n == 0 {
            return Some(1.0);
        }
        Some(self.done.iter().filter(|&&i| i < n).count() as f32 / n as f32)
    }

    pub fn complete(&self) -> bool {
        self.count().is_some_and(|n| (0..n).all(|i| self.done.contains(&i)))
    }

    /// Transcribed time ranges, merged, for drawing on the timeline.
    pub fn spans(&self) -> Vec<(Duration, Duration)> {
        let mut out: Vec<(Duration, Duration)> = Vec::new();
        for &i in &self.done {
            let c = self.chunk(i);
            match out.last_mut() {
                Some(last) if last.1 >= c.start => last.1 = last.1.max(c.end),
                _ => out.push((c.start, c.end)),
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn cov(total: u64) -> Coverage {
        Coverage::with_chunk(Some(s(total)), s(30), s(2))
    }

    /// Drain the scheduler in the order it chooses.
    fn order(c: &mut Coverage, playhead: Duration) -> Vec<u32> {
        let mut out = vec![];
        while let Some(ch) = c.next(playhead) {
            c.begin(&ch);
            out.push(ch.index);
            c.finish(ch.index);
            assert!(out.len() < 1000, "scheduler does not terminate");
        }
        out
    }

    #[test]
    fn the_playhead_chunk_comes_first_then_forward_then_the_start() {
        let mut c = cov(300); // 10 chunks
        let got = order(&mut c, s(125)); // chunk 4
        assert_eq!(got, vec![4, 5, 6, 7, 8, 9, 0, 1, 2, 3]);
        assert!(c.complete());
        assert_eq!(c.progress(), Some(1.0));
    }

    #[test]
    fn ahead_mode_stops_at_the_horizon_and_never_goes_back() {
        let mut c = cov(600);
        c.mode = Mode::Ahead(s(60));
        let got = order(&mut c, s(95)); // chunk 3; horizon 155 s is chunk 5
        assert_eq!(got, vec![3, 4, 5]);
        assert!(!c.complete());
        // Every chunk it did lies at or after the playhead's chunk.
        assert!(!got.is_empty() && got.iter().all(|&i| i >= 3));
    }

    #[test]
    fn paused_starts_nothing_but_keeps_what_is_done() {
        let mut c = cov(120);
        c.finish(0);
        c.mode = Mode::Paused;
        assert_eq!(c.next(s(0)), None);
        assert!(c.is_done(0));
        c.mode = Mode::All;
        assert_eq!(c.next(s(0)).map(|c| c.index), Some(1));
    }

    #[test]
    fn a_chunk_in_flight_is_not_handed_out_twice() {
        let mut c = cov(120);
        let first = c.next(s(0)).unwrap();
        c.begin(&first);
        let second = c.next(s(0)).unwrap();
        assert_ne!(first.index, second.index);
    }

    /// A failed chunk is finished, not retried forever.
    #[test]
    fn failure_is_still_progress() {
        let mut c = cov(60);
        let ch = c.next(s(0)).unwrap();
        c.begin(&ch);
        c.finish(ch.index); // the caller does this on Err too
        assert_eq!(c.next(s(0)).map(|c| c.index), Some(1));
    }

    #[test]
    fn the_last_chunk_is_short_and_has_no_post_roll_past_the_end() {
        let c = cov(75); // chunks 0..30, 30..60, 60..75
        let last = c.chunk(2);
        assert_eq!((last.start, last.end), (s(60), s(75)));
        assert_eq!(last.post_roll, Duration::ZERO);
        let mid = c.chunk(1);
        assert_eq!(mid.decode_span(), (s(30), s(62)));
        assert_eq!(c.next(s(74)).map(|c| c.index), Some(2));
        // No chunk 3 exists.
        let mut c = c;
        assert_eq!(order(&mut c, s(0)), vec![0, 1, 2]);
    }

    /// Unknown length: forward is open-ended until the source finds the end.
    #[test]
    fn an_unknown_length_is_learned_from_an_empty_chunk() {
        let mut c = Coverage::with_chunk(None, s(30), s(2));
        assert_eq!(c.progress(), None, "no percentage of an unknown total");
        for i in 0..3 {
            let ch = c.next(s(0)).unwrap();
            assert_eq!(ch.index, i);
            c.begin(&ch);
            c.finish(i);
        }
        let ch = c.next(s(0)).unwrap();
        assert_eq!(ch.index, 3);
        c.begin(&ch);
        c.mark_end(3);
        assert_eq!(c.total(), Some(s(90)));
        assert_eq!(c.next(s(0)), None);
        assert!(c.complete());
        assert_eq!(c.progress(), Some(1.0));
    }

    #[test]
    fn spans_merge_adjacent_chunks_and_keep_gaps() {
        let mut c = cov(300);
        for i in [0, 1, 3, 4, 5, 9] {
            c.finish(i);
        }
        assert_eq!(
            c.spans(),
            vec![(s(0), s(60)), (s(90), s(180)), (s(270), s(300))]
        );
        assert_eq!(c.progress(), Some(0.6));
    }

    #[test]
    fn a_zero_chunk_length_cannot_hang_the_scheduler() {
        let mut c = Coverage::with_chunk(Some(s(3)), Duration::ZERO, Duration::ZERO);
        assert_eq!(order(&mut c, s(0)), vec![0, 1, 2]);
    }

    #[test]
    fn restoring_ignores_indices_past_the_end() {
        let mut c = cov(60);
        c.restore_done([0, 1, 7]);
        assert!(c.complete());
        assert_eq!(c.done_indices().collect::<Vec<_>>(), vec![0, 1]);
    }
}
