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
//!
//! ## The link (the half that needs no sound card either)
//!
//! [`Link`] is the whole call minus the devices: one [`Jitter`] and one
//! decoder per peer, one encoder and sequence counter for us, a mute, and
//! the mix. It is fed 48 kHz mono samples from wherever the shell got them
//! and hands back 48 kHz mono frames to wherever the shell puts them —
//! the shell's only jobs are resampling to and from its devices and
//! keeping the sink topped up ([`Resampler`] is the first of those, here
//! because a voice link's resampler must be *streaming*: a fractional
//! position that survives the chunk boundary, or every 20 ms has a
//! click). The codec is a trait ([`Codec`]) so this crate stays free of
//! libopus: the shell supplies Opus from `studio-media`, the tests use
//! [`PcmCodec`], and a one-byte tag on every frame lets a receiver
//! *conceal* a frame it cannot decode rather than play it as noise — a
//! mismatch between two builds is a silent peer with a rising counter,
//! not a speaker full of static.
//!
//! Two rules the tests pin: a muted link keeps SENDING (encoded silence,
//! sequence still climbing) so nobody's loss meter reads a mute as
//! packet loss, and a frame arrives on the wire whole or not at all —
//! partial mic reads accumulate until there is a full 20 ms.

use std::collections::BTreeMap;

use crate::wire::Packet;
use crate::SiteId;

/// The link's sample rate. Opus's native rate and the project's, so the
/// common case resamples nothing.
pub const RATE: u32 = 48_000;
/// One frame: 20 ms at [`RATE`]. Opus's sweet spot for a conversation and
/// the period the jitter buffer's clock counts in.
pub const FRAME: usize = 960;
/// The same period as milliseconds, for the shell's clock arithmetic.
pub const FRAME_MS: u32 = 20;

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


// ── codec seam ──────────────────────────────────────────────────────────

/// One direction of a voice codec at [`RATE`] mono, [`FRAME`] samples per
/// call. `decode(None)` is packet-loss concealment: the codec fills the
/// slot with its best guess (Opus extrapolates; PCM goes quiet) and the
/// clock never stalls.
pub trait Codec {
    /// Written as the first byte of every frame on the wire. A receiver
    /// whose decoder carries a different tag conceals the frame.
    fn tag(&self) -> u8;
    /// `pcm.len() == FRAME`. Returns the payload (without the tag).
    fn encode(&mut self, pcm: &[f32]) -> Vec<u8>;
    /// `out.len() == FRAME`; overwritten, never accumulated into.
    fn decode(&mut self, payload: Option<&[u8]>, out: &mut [f32]);
}

/// 16-bit little-endian PCM: 30 KiB/s, no dependency, no concealment worth
/// the name. The test codec, and the fallback for a build without libopus.
#[derive(Debug, Default, Clone, Copy)]
pub struct PcmCodec;

impl PcmCodec {
    pub const TAG: u8 = 0;
}

impl Codec for PcmCodec {
    fn tag(&self) -> u8 { Self::TAG }
    fn encode(&mut self, pcm: &[f32]) -> Vec<u8> {
        let mut out = Vec::with_capacity(pcm.len() * 2);
        for &x in pcm {
            let v = (x.clamp(-1.0, 1.0) * 32767.0).round() as i16;
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }
    fn decode(&mut self, payload: Option<&[u8]>, out: &mut [f32]) {
        out.fill(0.0);
        let Some(p) = payload else { return };
        for (o, b) in out.iter_mut().zip(p.chunks_exact(2)) {
            *o = i16::from_le_bytes([b[0], b[1]]) as f32 / 32767.0;
        }
    }
}

// ── streaming resampler ─────────────────────────────────────────────────

/// Linear interpolation with state. Good enough for a voice link (the
/// codec band-limits harder than this aliases), and *streaming*: the
/// fractional read position and the last input sample carry over, so
/// feeding a signal in 20 ms pieces produces the same samples as feeding
/// it whole. That property is the test; a resampler without it clicks at
/// every chunk edge, which is exactly the cadence a call runs at.
#[derive(Debug, Clone)]
pub struct Resampler {
    from: u32,
    to: u32,
    /// Read position in INPUT samples, relative to `prev` at -1.
    pos: f64,
    /// The last input sample of the previous call, so the interpolation
    /// across the chunk boundary has both ends.
    prev: Option<f32>,
}

impl Resampler {
    pub fn new(from: u32, to: u32) -> Resampler {
        Resampler { from: from.max(1), to: to.max(1), pos: 0.0, prev: None }
    }
    pub fn from_rate(&self) -> u32 { self.from }
    pub fn to_rate(&self) -> u32 { self.to }

    /// Convert `input` and append the result to `out`.
    pub fn push(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if input.is_empty() { return; }
        if self.from == self.to {
            out.extend_from_slice(input);
            return;
        }
        let step = self.from as f64 / self.to as f64;
        // Virtual input: [prev, input...], where prev sits at index 0.
        // With no prev yet, the first output is the first input sample.
        let (base, len) = match self.prev {
            Some(_) => (1usize, input.len() + 1),
            None => { self.pos = 0.0; (0usize, input.len()) }
        };
        let at = |i: usize, prev: Option<f32>| -> f32 {
            if base == 1 {
                if i == 0 { prev.unwrap_or(0.0) } else { input[i - 1] }
            } else { input[i] }
        };
        let prev = self.prev;
        let mut pos = self.pos;
        // Interpolate only while both ends are in hand; a position that
        // lands on the last sample waits for the next chunk's first.
        while (pos as usize) + 1 < len {
            let i = pos as usize;
            let frac = (pos - i as f64) as f32;
            let a = at(i, prev);
            let b = at(i + 1, prev);
            out.push(a + (b - a) * frac);
            pos += step;
        }
        // Rebase so the last input sample becomes index 0 next time.
        self.pos = pos - (len as f64 - 1.0);
        self.prev = Some(*input.last().expect("non-empty"));
    }

    pub fn reset(&mut self) { self.pos = 0.0; self.prev = None; }
}

// ── the link ────────────────────────────────────────────────────────────

/// One peer's inbound side.
struct Inbound {
    jitter: Jitter,
    dec: Box<dyn Codec>,
    /// Frames concealed because their tag was not ours.
    unknown: u64,
}

/// The meter: what the status line says about the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Meter {
    /// Peers we have heard at least one frame from.
    pub peers: usize,
    /// Peers whose playout has started (buffered to depth at least once).
    pub playing: usize,
    /// Slots concealed for want of a frame, all peers.
    pub lost: u64,
    /// Frames dropped for arriving after their slot, all peers.
    pub late: u64,
    /// Frames concealed because the codec tag was not ours, all peers.
    pub unknown: u64,
    /// Frames sent.
    pub sent: u64,
}

/// The call, minus the devices. See the module docs.
pub struct Link {
    peers: BTreeMap<SiteId, Inbound>,
    depth: usize,
    make_decoder: Box<dyn Fn() -> Box<dyn Codec> + Send>,
    enc: Box<dyn Codec>,
    out: Outbound,
    muted: bool,
    /// Mic samples at [`RATE`] waiting to fill a frame.
    pending: Vec<f32>,
    sent: u64,
    /// Scratch for one peer's decode, so playout allocates nothing.
    scratch: Vec<f32>,
}

impl Link {
    /// `depth` frames of jitter buffering per peer (3 = 60 ms, the floor
    /// for a wide-area link). `enc` encodes what we send; `make_decoder`
    /// builds one decoder per peer that speaks, all with the same tag.
    pub fn new(
        depth: usize,
        enc: Box<dyn Codec>,
        make_decoder: Box<dyn Fn() -> Box<dyn Codec> + Send>,
    ) -> Link {
        Link {
            peers: BTreeMap::new(), depth: depth.max(1), make_decoder, enc,
            out: Outbound::default(), muted: false, pending: Vec::with_capacity(FRAME * 4),
            sent: 0, scratch: vec![0.0; FRAME],
        }
    }

    /// The test/fallback link: PCM both ways.
    pub fn pcm(depth: usize) -> Link {
        Link::new(depth, Box::new(PcmCodec), Box::new(|| Box::new(PcmCodec)))
    }

    pub fn muted(&self) -> bool { self.muted }
    /// Muted sends encoded SILENCE, not nothing — see the module docs.
    pub fn set_muted(&mut self, on: bool) { self.muted = on; }
    pub fn depth(&self) -> usize { self.depth }

    /// A voice frame from the wire. Any other packet is ignored, so a
    /// carrier's whole poll can be passed through.
    pub fn receive(&mut self, p: &Packet) {
        let Packet::Voice { site, seq, frame } = p else { return };
        let depth = self.depth;
        let make = &self.make_decoder;
        let peer = self.peers.entry(*site).or_insert_with(|| Inbound {
            jitter: Jitter::new(depth), dec: make(), unknown: 0,
        });
        peer.jitter.push(*seq, frame.clone());
    }

    /// Mic samples at [`RATE`] mono, any length. Returns one packet per
    /// completed frame, stamped with `site` and the next sequence.
    pub fn feed_mic(&mut self, pcm: &[f32], site: SiteId) -> Vec<Packet> {
        self.pending.extend_from_slice(pcm);
        let mut packets = Vec::new();
        let mut consumed = 0;
        while self.pending.len() - consumed >= FRAME {
            let frame = &self.pending[consumed..consumed + FRAME];
            let payload = if self.muted {
                self.scratch.fill(0.0);
                self.enc.encode(&self.scratch)
            } else {
                self.enc.encode(frame)
            };
            let mut bytes = Vec::with_capacity(payload.len() + 1);
            bytes.push(self.enc.tag());
            bytes.extend_from_slice(&payload);
            packets.push(Packet::Voice { site, seq: self.out.next(), frame: bytes });
            self.sent += 1;
            consumed += FRAME;
        }
        self.pending.drain(..consumed);
        packets
    }

    /// One playout slot: every started peer's next frame, decoded (or
    /// concealed) and summed into `out` (`out.len() == FRAME`, overwritten).
    /// Returns how many peers contributed a slot; zero means `out` is
    /// silence and no clock advanced. Call once per [`FRAME_MS`] of the
    /// sink's time — never per packet.
    pub fn playout(&mut self, out: &mut [f32]) -> usize {
        debug_assert_eq!(out.len(), FRAME);
        out.fill(0.0);
        let want = self.enc.tag();
        let mut n = 0;
        for peer in self.peers.values_mut() {
            let slot = match peer.jitter.pop() {
                Err(()) => continue,
                Ok(s) => s,
            };
            let payload = match slot.as_deref() {
                Some([tag, rest @ ..]) if *tag == want => Some(rest),
                Some(_) => { peer.unknown += 1; None }
                None => None,
            };
            peer.dec.decode(payload, &mut self.scratch);
            for (o, s) in out.iter_mut().zip(&self.scratch) { *o += *s; }
            n += 1;
        }
        if n > 1 {
            for o in out.iter_mut() { *o = o.clamp(-1.0, 1.0); }
        }
        n
    }

    /// A peer left: drop its buffer so its last words are not replayed
    /// on a reconnect from a fresh sequence.
    pub fn forget(&mut self, site: SiteId) { self.peers.remove(&site); }

    /// Drop every peer's buffer (a seek, a reconnect); we keep sending.
    pub fn reset(&mut self) { self.peers.clear(); self.pending.clear(); }

    pub fn meter(&self) -> Meter {
        let mut m = Meter { peers: self.peers.len(), sent: self.sent, ..Meter::default() };
        for p in self.peers.values() {
            m.lost += p.jitter.lost;
            m.late += p.jitter.late;
            m.unknown += p.unknown;
            if p.jitter.next.is_some() { m.playing += 1; }
        }
        m
    }

    /// Sites we have heard from.
    pub fn sites(&self) -> Vec<SiteId> { self.peers.keys().copied().collect() }
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

    // ── link ──────────────────────────────────────────────────────────

    fn sine(n: usize, hz: f32, amp: f32) -> Vec<f32> {
        (0..n).map(|i| amp * (2.0 * std::f32::consts::PI * hz * i as f32 / RATE as f32).sin())
            .collect()
    }

    fn max_abs_diff(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max)
    }

    #[test]
    fn what_a_speaks_b_hears_after_depth_frames_and_in_order() {
        let (a, b) = (SiteId(1), SiteId(2));
        let mut la = Link::pcm(3);
        let mut lb = Link::pcm(3);
        let voice = sine(FRAME * 5, 440.0, 0.5);
        let packets = la.feed_mic(&voice, a);
        assert_eq!(packets.len(), 5, "five full frames in, five packets out");
        for (i, p) in packets.iter().enumerate() {
            match p {
                Packet::Voice { site, seq, frame } => {
                    assert_eq!((*site, *seq), (a, i as u32));
                    assert_eq!(frame[0], PcmCodec::TAG);
                    assert_eq!(frame.len(), 1 + FRAME * 2);
                }
                other => panic!("{other:?}"),
            }
        }
        let mut out = vec![0.0; FRAME];
        assert_eq!(lb.playout(&mut out), 0, "nothing heard yet: silence, no clock");
        lb.receive(&packets[0]);
        lb.receive(&packets[1]);
        assert_eq!(lb.playout(&mut out), 0, "two of three: still buffering");
        assert_eq!(lb.meter(), Meter { peers: 1, playing: 0, sent: 0, ..Meter::default() });
        for p in &packets[2..] { lb.receive(p); }
        for i in 0..5 {
            assert_eq!(lb.playout(&mut out), 1, "frame {i}");
            let d = max_abs_diff(&out, &voice[i * FRAME..(i + 1) * FRAME]);
            assert!(d < 1.0 / 32000.0, "frame {i} differs by {d} — more than 16-bit rounding");
        }
        assert_eq!(lb.playout(&mut out), 1, "underrun: a concealed slot, clock still runs");
        assert!(out.iter().all(|&x| x == 0.0), "PCM conceals with silence");
        let m = lb.meter();
        assert_eq!((m.peers, m.playing, m.lost, m.late, m.unknown), (1, 1, 1, 0, 0));
        assert_eq!(la.meter().sent, 5);
        let _ = b;
    }

    #[test]
    fn a_partial_mic_read_waits_for_the_rest_of_its_frame() {
        let mut l = Link::pcm(1);
        assert!(l.feed_mic(&[0.1; 500], SiteId(1)).is_empty(), "500 of 960: nothing yet");
        let p = l.feed_mic(&[0.1; 500], SiteId(1));
        assert_eq!(p.len(), 1, "1000 seen: one frame, 40 held");
        assert_eq!(l.pending.len(), 40);
        let p = l.feed_mic(&[0.1; FRAME * 2 - 40], SiteId(1));
        assert_eq!(p.len(), 2, "exactly two more frames, nothing held");
        assert_eq!(l.pending.len(), 0);
    }

    #[test]
    fn a_muted_link_keeps_sending_silence_so_nobody_reads_a_mute_as_loss() {
        let mut la = Link::pcm(1);
        let mut lb = Link::pcm(1);
        la.set_muted(true);
        let packets = la.feed_mic(&sine(FRAME * 2, 300.0, 0.9), SiteId(7));
        assert_eq!(packets.len(), 2, "still one packet per frame while muted");
        for p in &packets { lb.receive(p); }
        let mut out = vec![1.0; FRAME];
        assert_eq!(lb.playout(&mut out), 1);
        assert!(out.iter().all(|&x| x == 0.0), "the muted frame decodes to silence");
        assert_eq!(lb.playout(&mut out), 1);
        assert_eq!(lb.meter().lost, 0, "a mute costs the far side no concealed slots");
        la.set_muted(false);
        let p = la.feed_mic(&[0.25; FRAME], SiteId(7));
        lb.receive(&p[0]);
        lb.playout(&mut out);
        assert!((out[10] - 0.25).abs() < 1e-3, "unmuted: sound again, {}", out[10]);
    }

    #[test]
    fn a_frame_in_a_codec_we_do_not_speak_is_concealed_not_played() {
        let mut lb = Link::pcm(1);
        // Tag 200: nothing this build knows. Payload is loud garbage.
        lb.receive(&Packet::Voice { site: SiteId(3), seq: 0, frame: vec![200; 1 + FRAME * 2] });
        let mut out = vec![1.0; FRAME];
        assert_eq!(lb.playout(&mut out), 1, "the slot still plays — as concealment");
        assert!(out.iter().all(|&x| x == 0.0), "never decoded as PCM");
        assert_eq!(lb.meter().unknown, 1);
        // The next frame in our codec plays normally: the peer is not marked bad.
        let mut la = Link::pcm(1);
        let p = la.feed_mic(&[0.5; FRAME], SiteId(3));
        let Packet::Voice { frame, .. } = &p[0] else { panic!() };
        lb.receive(&Packet::Voice { site: SiteId(3), seq: 1, frame: frame.clone() });
        lb.playout(&mut out);
        assert!((out[0] - 0.5).abs() < 1e-3);
        assert_eq!(lb.meter().unknown, 1, "not counted twice");
    }

    #[test]
    fn two_peers_sum_and_a_hot_sum_is_clamped() {
        let mut l = Link::pcm(1);
        let mut p1 = Link::pcm(1);
        let mut p2 = Link::pcm(1);
        let a = p1.feed_mic(&[0.25; FRAME], SiteId(1)).remove(0);
        let b = p2.feed_mic(&[0.25; FRAME], SiteId(2)).remove(0);
        l.receive(&a);
        l.receive(&b);
        let mut out = vec![0.0; FRAME];
        assert_eq!(l.playout(&mut out), 2);
        assert!((out[0] - 0.5).abs() < 1e-3, "additive: {}", out[0]);
        let a = p1.feed_mic(&[0.9; FRAME], SiteId(1)).remove(0);
        let b = p2.feed_mic(&[0.9; FRAME], SiteId(2)).remove(0);
        l.receive(&a);
        l.receive(&b);
        l.playout(&mut out);
        assert_eq!(out[0], 1.0, "1.8 clamps to full scale, not wrapped or scaled");
        assert_eq!(l.sites(), vec![SiteId(1), SiteId(2)]);
        l.forget(SiteId(1));
        assert_eq!(l.playout(&mut out), 1, "the forgotten peer no longer counts (and 2 conceals)");
        assert_eq!(l.meter().peers, 1);
    }

    #[test]
    fn a_non_voice_packet_is_ignored_by_receive() {
        let mut l = Link::pcm(1);
        l.receive(&Packet::Hello { site: SiteId(9), since: 0 });
        assert_eq!(l.meter().peers, 0);
    }

    // ── resampler ─────────────────────────────────────────────────────

    fn zero_crossings(x: &[f32]) -> usize {
        x.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count()
    }

    #[test]
    fn resampling_in_chunks_matches_resampling_whole() {
        let sig = sine(FRAME * 10, 440.0, 0.8);
        for (from, to) in [(48_000u32, 44_100u32), (44_100, 48_000), (48_000, 24_000), (16_000, 48_000)] {
            let mut whole = Vec::new();
            Resampler::new(from, to).push(&sig, &mut whole);
            let mut chunked = Vec::new();
            let mut r = Resampler::new(from, to);
            // Uneven chunk sizes on purpose: a frame boundary is the easy case.
            let mut i = 0;
            for n in [FRAME, 333, 1, 2000, 959, FRAME].iter().cycle() {
                if i >= sig.len() { break; }
                let end = (i + n).min(sig.len());
                r.push(&sig[i..end], &mut chunked);
                i = end;
            }
            assert_eq!(whole.len(), chunked.len(), "{from}->{to}: sample count");
            // Rebasing the fractional position each chunk costs one f32
            // ulp of drift, not a click: the bound is rounding, not tolerance.
            let d = max_abs_diff(&whole, &chunked);
            assert!(d < 1e-6, "{from}->{to}: chunked differs from whole by {d}");
            let expect = sig.len() as f64 * to as f64 / from as f64;
            assert!((whole.len() as f64 - expect).abs() <= 2.0, "{from}->{to}: {} vs {expect}", whole.len());
        }
    }

    #[test]
    fn resampling_keeps_the_pitch_and_equal_rates_are_the_identity() {
        let sig = sine(RATE as usize, 440.0, 0.8); // one second: 880 crossings
        let mut out = Vec::new();
        Resampler::new(48_000, 44_100).push(&sig, &mut out);
        let zc = zero_crossings(&out);
        assert!((878..=882).contains(&zc), "440 Hz should survive at 44.1k: {zc} crossings");
        let mut same = Vec::new();
        let mut r = Resampler::new(48_000, 48_000);
        r.push(&sig[..100], &mut same);
        r.push(&sig[100..], &mut same);
        assert_eq!(same, sig);
        let mut r = Resampler::new(48_000, 24_000);
        let mut empty = Vec::new();
        r.push(&[], &mut empty);
        assert!(empty.is_empty(), "no input, no output, no state change");
    }
}
