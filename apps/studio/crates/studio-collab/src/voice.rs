//! voice — the in-house call, the half that needs no microphone.
//!
//! The decision this module records: the call lives INSIDE the app, over
//! the same carrier the ops use, by default. A small producer should not
//! need an intercom box or a second program to hear the singer; whoever
//! wants a Dante rig can mute this and route their own. So a
//! [`Packet::Voice`](crate::wire::Packet::Voice) is one encoded audio
//! frame — Opus by intent (the codec `studio-media` already ships), but
//! this module never looks inside the bytes: the shell encodes on the
//! way out and decodes on the way in, and what is tested here is the
//! part every voice link gets wrong first, the **jitter buffer**.
//!
//! A carrier delivers frames late, early, twice and not at all. The
//! buffer takes frames by sequence number and hands them out in order at
//! a steady rate: it holds `depth` frames before it starts (the playout
//! delay — 3 × 20 ms is the floor for a wide-area link, and 60 ms is
//! still a conversation), fills a missing frame with `None` so the
//! decoder can conceal it and the clock never stalls, drops a frame that
//! arrives after its slot has already played rather than playing it late
//! (a late word is worse than a lost one), and ignores duplicates. Rules
//! the tests pin because each was got wrong on a first draft somewhere:
//! reordering must not stall, a duplicate must not double-play, and the
//! first frame's sequence — not zero — is where playback starts.

use std::collections::BTreeMap;

/// One peer's inbound voice: frames keyed by sequence, played in order.
#[derive(Debug, Clone)]
pub struct Jitter {
    pending: BTreeMap<u32, Vec<u8>>,
    /// The next sequence to hand out, once started.
    next: Option<u32>,
    /// Frames held before playback begins.
    depth: usize,
    /// How many frames were dropped for arriving late, for the meter.
    pub late: u64,
    /// How many slots were concealed (no frame in time), for the meter.
    pub lost: u64,
}

impl Jitter {
    pub fn new(depth: usize) -> Jitter {
        Jitter { pending: BTreeMap::new(), next: None, depth: depth.max(1), late: 0, lost: 0 }
    }

    /// A frame arrived. Duplicates and frames whose slot already played
    /// are ignored (the latter counted as late).
    pub fn push(&mut self, seq: u32, frame: Vec<u8>) {
        if let Some(next) = self.next {
            if seq < next { self.late += 1; return; }
        }
        self.pending.entry(seq).or_insert(frame);
    }

    /// The next playout slot: `Ok(Some(frame))` to decode, `Ok(None)` for
    /// a slot to conceal, `Err(())` while still buffering (play silence,
    /// the clock has not started). Called once per frame period by the
    /// audio clock, never by the network.
    pub fn pop(&mut self) -> Result<Option<Vec<u8>>, ()> {
        let next = match self.next {
            Some(n) => n,
            None => {
                if self.pending.len() < self.depth { return Err(()); }
                let first = *self.pending.keys().next().expect("non-empty");
                self.next = Some(first);
                first
            }
        };
        self.next = Some(next.wrapping_add(1));
        match self.pending.remove(&next) {
            Some(f) => Ok(Some(f)),
            None => { self.lost += 1; Ok(None) }
        }
    }

    /// Frames waiting — the meter's "how far behind live are we".
    pub fn buffered(&self) -> usize { self.pending.len() }

    /// Forget everything: a seek, a reconnect, a peer that went quiet
    /// long enough that catching up would mean playing a paragraph late.
    pub fn reset(&mut self) { self.pending.clear(); self.next = None; }
}

/// Outbound sequencing: one counter per site.
#[derive(Debug, Clone, Default)]
pub struct Outbound { seq: u32 }

impl Outbound {
    /// The sequence to stamp the next frame with.
    pub fn next(&mut self) -> u32 { let s = self.seq; self.seq = self.seq.wrapping_add(1); s }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(n: u32) -> Vec<u8> { vec![n as u8; 4] }

    #[test]
    fn playback_waits_for_depth_then_runs_in_order_from_the_first_sequence_not_zero() {
        let mut j = Jitter::new(3);
        assert!(j.pop().is_err(), "nothing yet: keep buffering");
        j.push(40, f(40));
        j.push(41, f(41));
        assert!(j.pop().is_err(), "two of three: still buffering");
        j.push(42, f(42));
        assert_eq!(j.pop(), Ok(Some(f(40))), "starts at the first frame's sequence");
        assert_eq!(j.pop(), Ok(Some(f(41))));
        assert_eq!(j.pop(), Ok(Some(f(42))));
        assert_eq!(j.pop(), Ok(None), "underrun: a slot to conceal, and the clock still advances");
        assert_eq!(j.lost, 1);
    }

    #[test]
    fn reordering_does_not_stall_and_a_gap_is_concealed_not_waited_for() {
        let mut j = Jitter::new(2);
        j.push(1, f(1));
        j.push(0, f(0));
        assert_eq!(j.pop(), Ok(Some(f(0))));
        j.push(3, f(3)); // 2 is missing
        assert_eq!(j.pop(), Ok(Some(f(1))));
        assert_eq!(j.pop(), Ok(None), "slot 2 concealed");
        assert_eq!(j.pop(), Ok(Some(f(3))), "and 3 plays in its own slot, on time");
        assert_eq!((j.lost, j.late), (1, 0));
    }

    #[test]
    fn a_late_frame_is_dropped_and_a_duplicate_plays_once() {
        let mut j = Jitter::new(1);
        j.push(0, f(0));
        j.push(0, vec![9, 9, 9, 9]);
        assert_eq!(j.pop(), Ok(Some(f(0))), "the first copy wins; the duplicate does not replace it");
        j.push(2, f(2));
        assert_eq!(j.pop(), Ok(None), "1 missing");
        j.push(1, f(1)); // arrives after its slot played
        assert_eq!(j.late, 1);
        assert_eq!(j.pop(), Ok(Some(f(2))), "2 was not displaced by the late 1");
        assert_eq!(j.buffered(), 0);
    }

    #[test]
    fn reset_forgets_and_restarts_from_whatever_comes_next() {
        let mut j = Jitter::new(1);
        j.push(5, f(5));
        assert_eq!(j.pop(), Ok(Some(f(5))));
        j.reset();
        assert!(j.pop().is_err());
        j.push(900, f(900));
        assert_eq!(j.pop(), Ok(Some(f(900))), "a new stream after reset starts where it starts");
        let mut o = Outbound::default();
        assert_eq!((o.next(), o.next(), o.next()), (0, 1, 2));
    }
}
