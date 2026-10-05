//! `sim::net::reliable` — the reliable-ordered channel world edits were documented to ride,
//! and did not.
//!
//! `world_cmd_wire` says world edits "ride the reliable-ordered channel"; the fullgame's
//! inline loop and `HostPump` sent each edit batch in ONE `send_to` and forgot it. A dropped
//! datagram -- Wi-Fi drops them -- was a block placed on one machine and not the other, and
//! nothing ever noticed: the two worlds diverged silently and stayed that way until a
//! join-sync. This is the smallest layer that makes a batch arrive, once, in order: a
//! sequence number per batch, a cumulative ack from the receiver, retransmission on a timer
//! until acked, in-order delivery with a bounded reorder buffer.
//!
//! One [`Link`] per peer, each direction inside it independent. The host does not relay a
//! reliable datagram verbatim (the sequence numbers are per link): it delivers, applies, and
//! re-sends on its other links -- see the fullgame loop and `HostPump`.
//!
//! Wire (the channel byte is the embedder's; these are the payloads after it):
//!
//! | channel         | payload                          |
//! |-----------------|----------------------------------|
//! | [`CH_RELIABLE`] | `[seq u32 LE][bytes]`            |
//! | [`CH_ACK`]      | `[cum u32 LE]`: all of `1..=cum` arrived |
//!
//! Pure: no sockets, no clock -- time is passed in. Every byte decoded is bounds-checked.

use std::collections::{BTreeMap, VecDeque};

/// A sequenced reliable batch.
pub const CH_RELIABLE: u8 = 6;
/// A cumulative acknowledgement.
pub const CH_ACK: u8 = 7;
/// Resend an unacked batch this often.
pub const RTO_MS: u64 = 250;
/// Resends before the link gives up on the peer (6 s at `RTO_MS`): a peer silent that long is
/// pruned by the peer table anyway.
pub const MAX_TRIES: u32 = 24;
/// Out-of-order batches held while waiting for the gap; past this, newer ones are dropped and
/// the sender's retransmission brings them back in order.
pub const MAX_BUFFERED: usize = 256;
/// Unacked batches a sender holds before refusing new ones (a peer that never acks).
pub const MAX_UNACKED: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Pending {
    seq: u32,
    bytes: Vec<u8>,
    /// `None` until first sent.
    sent_ms: Option<u64>,
    tries: u32,
}

/// One peer's reliable channel, both directions.
#[derive(Clone, Debug, Default)]
pub struct Link {
    next_seq: u32,
    unacked: VecDeque<Pending>,
    /// The next sequence number the far side owes us (1 at first).
    expected: u32,
    buffered: BTreeMap<u32, Vec<u8>>,
    ack_due: bool,
    /// Set once a batch has been resent `MAX_TRIES` times without an ack: the peer is gone
    /// or deaf, and the embedder should drop the link.
    pub given_up: bool,
    /// Counters for a status line.
    pub resent: u64,
    pub duplicates: u64,
}

impl Link {
    pub fn new() -> Link {
        Link { next_seq: 1, expected: 1, ..Default::default() }
    }

    /// Queue a batch for the far side. Returns its sequence number; `None` when the
    /// unacked window is full (the batch is NOT queued: the caller keeps it or drops it
    /// knowingly, rather than the queue growing without bound against a dead peer).
    pub fn send(&mut self, bytes: &[u8]) -> Option<u32> {
        if self.unacked.len() >= MAX_UNACKED {
            return None;
        }
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1).max(1);
        self.unacked.push_back(Pending { seq, bytes: bytes.to_vec(), sent_ms: None, tries: 0 });
        Some(seq)
    }

    /// The datagram payloads to send now: every never-sent batch, and every unacked one
    /// whose last send is `RTO_MS` or more ago. Each is `[seq][bytes]`, to go out under
    /// `CH_RELIABLE`. Marks them sent at `now_ms`.
    pub fn due(&mut self, now_ms: u64) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        for p in self.unacked.iter_mut() {
            let due = match p.sent_ms {
                None => true,
                Some(t) => now_ms.saturating_sub(t) >= RTO_MS,
            };
            if !due {
                continue;
            }
            if p.tries >= MAX_TRIES {
                self.given_up = true;
                continue;
            }
            if p.sent_ms.is_some() {
                self.resent += 1;
            }
            p.sent_ms = Some(now_ms);
            p.tries += 1;
            out.push(frame(p.seq, &p.bytes));
        }
        out
    }

    /// A `CH_RELIABLE` payload arrived: the batches now deliverable, in order (empty for a
    /// duplicate, a gap, or a malformed payload). An ack becomes due in every case but the
    /// malformed one.
    pub fn receive(&mut self, payload: &[u8]) -> Vec<Vec<u8>> {
        let Some((seq, bytes)) = unframe(payload) else { return Vec::new() };
        self.ack_due = true;
        if seq < self.expected {
            self.duplicates += 1;
            return Vec::new();
        }
        if seq != self.expected {
            if self.buffered.len() < MAX_BUFFERED {
                self.buffered.entry(seq).or_insert_with(|| bytes.to_vec());
            }
            return Vec::new();
        }
        let mut out = vec![bytes.to_vec()];
        self.expected = self.expected.wrapping_add(1).max(1);
        while let Some(next) = self.buffered.remove(&self.expected) {
            out.push(next);
            self.expected = self.expected.wrapping_add(1).max(1);
        }
        out
    }

    /// A `CH_ACK` payload arrived: everything up to and including `cum` is off the queue.
    pub fn on_ack(&mut self, payload: &[u8]) {
        if payload.len() < 4 {
            return;
        }
        let cum = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        while self.unacked.front().is_some_and(|p| p.seq <= cum) {
            self.unacked.pop_front();
        }
    }

    /// The `CH_ACK` payload to send, if one is due since the last call: the highest
    /// contiguous sequence received.
    pub fn ack_payload(&mut self) -> Option<Vec<u8>> {
        if !self.ack_due {
            return None;
        }
        self.ack_due = false;
        Some((self.expected - 1).to_le_bytes().to_vec())
    }

    /// Batches sent and not yet acknowledged.
    pub fn unacked(&self) -> usize {
        self.unacked.len()
    }

    /// The sequence number the far side owes us next.
    pub fn expected(&self) -> u32 {
        self.expected
    }
}

/// `[seq u32 LE][bytes]`.
pub fn frame(seq: u32, bytes: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(4 + bytes.len());
    v.extend_from_slice(&seq.to_le_bytes());
    v.extend_from_slice(bytes);
    v
}

fn unframe(payload: &[u8]) -> Option<(u32, &[u8])> {
    if payload.len() < 4 {
        return None;
    }
    let seq = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
    if seq == 0 {
        return None;
    }
    Some((seq, &payload[4..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Carry one side's `due()` output to the other side's `receive`, acks back; returns what
    /// the far side delivered.
    fn carry(from: &mut Link, to: &mut Link, now_ms: u64, drop: &dyn Fn(usize) -> bool) -> Vec<Vec<u8>> {
        let mut delivered = Vec::new();
        for (i, dg) in from.due(now_ms).into_iter().enumerate() {
            if drop(i) {
                continue;
            }
            delivered.extend(to.receive(&dg));
        }
        if let Some(ack) = to.ack_payload() {
            from.on_ack(&ack);
        }
        delivered
    }

    /// The happy path: three batches, in order, acked, nothing left unacked.
    #[test]
    fn batches_arrive_once_in_order_and_are_acked() {
        let (mut a, mut b) = (Link::new(), Link::new());
        assert_eq!(a.send(b"one"), Some(1));
        assert_eq!(a.send(b"two"), Some(2));
        assert_eq!(a.send(b"three"), Some(3));
        let got = carry(&mut a, &mut b, 1000, &|_| false);
        assert_eq!(got, vec![b"one".to_vec(), b"two".to_vec(), b"three".to_vec()]);
        assert_eq!(a.unacked(), 0, "the cumulative ack cleared all three");
        assert!(carry(&mut a, &mut b, 2000, &|_| false).is_empty(), "nothing to resend");
        assert_eq!(a.resent, 0);
        assert_eq!(b.expected(), 4);
    }

    /// The point of the module: the second of three datagrams is lost; the receiver holds
    /// the third, the sender resends the second after RTO, and delivery is still in order.
    #[test]
    fn a_lost_datagram_is_resent_and_delivery_stays_in_order() {
        let (mut a, mut b) = (Link::new(), Link::new());
        for s in ["one", "two", "three"] {
            a.send(s.as_bytes());
        }
        let got = carry(&mut a, &mut b, 1000, &|i| i == 1);
        assert_eq!(got, vec![b"one".to_vec()], "three is held back behind the gap");
        assert_eq!(a.unacked(), 2, "the ack covered one only");
        assert!(carry(&mut a, &mut b, 1100, &|_| false).is_empty(), "before RTO nothing is resent");
        let got = carry(&mut a, &mut b, 1000 + RTO_MS, &|_| false);
        assert_eq!(got, vec![b"two".to_vec(), b"three".to_vec()], "the gap fills and the held one follows");
        assert_eq!(a.unacked(), 0);
        assert_eq!(a.resent, 2, "two and three were both resent (three's ack was lost with the gap)");
    }

    /// A duplicate (the ack was lost, so the sender resent) is delivered once and re-acked.
    #[test]
    fn a_duplicate_is_dropped_and_re_acked() {
        let (mut a, mut b) = (Link::new(), Link::new());
        a.send(b"x");
        let dgs = a.due(1000);
        assert_eq!(b.receive(&dgs[0]), vec![b"x".to_vec()]);
        let _lost_ack = b.ack_payload();
        let again = a.due(1000 + RTO_MS);
        assert_eq!(again.len(), 1, "resent, the ack never came");
        assert!(b.receive(&again[0]).is_empty(), "delivered once");
        assert_eq!(b.duplicates, 1);
        let ack = b.ack_payload().expect("re-acked");
        a.on_ack(&ack);
        assert_eq!(a.unacked(), 0);
    }

    /// Out-of-order arrival is reordered; a malformed payload (short, seq 0) is ignored and
    /// acks nothing.
    #[test]
    fn out_of_order_is_reordered_and_junk_is_ignored() {
        let (mut a, mut b) = (Link::new(), Link::new());
        a.send(b"1");
        a.send(b"2");
        a.send(b"3");
        let dgs = a.due(0);
        assert!(b.receive(&dgs[2]).is_empty());
        assert!(b.receive(&dgs[1]).is_empty());
        assert_eq!(b.receive(&dgs[0]), vec![b"1".to_vec(), b"2".to_vec(), b"3".to_vec()]);
        let mut c = Link::new();
        assert!(c.receive(&[1, 2]).is_empty());
        assert!(c.receive(&frame(0, b"zero")).is_empty());
        assert_eq!(c.ack_payload(), None, "junk acks nothing");
        c.on_ack(&[9]); // short: ignored
        assert_eq!(c.expected(), 1);
    }

    /// A peer that never acks: the sender resends MAX_TRIES times, then gives up, and the
    /// unacked window refuses new batches past MAX_UNACKED.
    #[test]
    fn a_deaf_peer_is_given_up_on_and_the_window_is_bounded() {
        let mut a = Link::new();
        a.send(b"hello?");
        let mut t = 0;
        let mut sends = 0;
        for _ in 0..(MAX_TRIES + 5) {
            sends += a.due(t).len();
            t += RTO_MS;
        }
        assert_eq!(sends as u32, MAX_TRIES);
        assert!(a.given_up);
        let mut full = Link::new();
        for i in 0..MAX_UNACKED {
            assert!(full.send(&[i as u8]).is_some());
        }
        assert_eq!(full.send(b"one more"), None, "the window is full");
        assert_eq!(full.unacked(), MAX_UNACKED);
    }

    /// The reorder buffer is bounded: past MAX_BUFFERED held batches, newer ones are dropped
    /// (the sender's retransmission brings them back once the gap fills).
    #[test]
    fn the_reorder_buffer_is_bounded() {
        let mut b = Link::new();
        for seq in 2..(2 + MAX_BUFFERED as u32 + 10) {
            assert!(b.receive(&frame(seq, b"late")).is_empty());
        }
        assert_eq!(b.buffered.len(), MAX_BUFFERED);
        let got = b.receive(&frame(1, b"first"));
        assert_eq!(got.len(), 1 + MAX_BUFFERED, "the gap fills and the buffer drains in order");
        assert_eq!(b.expected(), 2 + MAX_BUFFERED as u32);
    }
}
