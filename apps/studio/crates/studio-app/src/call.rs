//! call — the in-house voice link, assembled.
//!
//! The pieces exist elsewhere and none of them knows the others: the
//! microphone's live tap ([`capture::Capture::tap_drain`]), the codec
//! (`studio_media::voice`), the link with its jitter buffers and mix
//! (`studio_collab::voice::Link`), and the sink's sequential stream
//! ([`audition::PlaybackSink::stream_push`]). This module is the pump
//! that connects them, run once per UI frame, and the policy that sits
//! on top: on/off, mute, push-to-talk, and what the status line says.
//!
//! ## The clock
//!
//! A jitter buffer must be popped once per frame period by the audio
//! clock, never by the network — and this shell is single-threaded
//! around the document, so the UI thread has to BE that clock. It is,
//! by proxy: the sink's stream is kept topped up to a fixed depth
//! ([`Call::target_ms`]) every frame, and each chunk pushed is one
//! playout slot popped. The device drains the stream at exactly its
//! rate, so the pops happen at exactly the frame rate on average, and
//! the depth is how long the UI may stall before the stream runs dry.
//!
//! The stream is topped up with SILENCE when no peer is playing, from
//! the moment the call starts. That is not waste: it is what keeps the
//! jitter buffer's depth honest. If the stream were left empty until a
//! peer spoke, the first pump after their first frame would fill the
//! whole depth at once — pop four or five slots in one go — and the
//! jitter buffer's sixty milliseconds of tolerance would be spent on
//! the sink's queue instead of on the network.
//!
//! ## What is not here
//!
//! Echo cancellation. A phone with its speaker on will feed the far
//! side its own voice back; headphones are the honest answer today, and
//! the status line does not pretend otherwise. Named, not built.

use std::sync::Arc;

use studio_collab::voice::{self, Codec, Link, Resampler, FRAME, RATE};
use studio_collab::wire::Packet;
use studio_collab::SiteId;

use crate::audition::PlaybackSink;
use crate::capture::Capture;

/// Frames of jitter buffering per peer: 60 ms, the floor for a
/// wide-area link.
pub const DEPTH: usize = 3;
/// Opus at this rate survives a phone's uplink.
pub const BITRATE: i32 = 24_000;

/// Opus, adapted to the link's codec seam.
struct OpusCodec {
    enc: Option<studio_media::voice::VoiceEncoder>,
    dec: Option<studio_media::voice::VoiceDecoder>,
}

impl OpusCodec {
    /// The tag every Opus build stamps its frames with; a PCM build's
    /// frames carry `PcmCodec::TAG` and the two conceal each other.
    const TAG: u8 = 1;
    fn encoder() -> Option<Box<dyn Codec>> {
        let enc = studio_media::voice::VoiceEncoder::new(BITRATE).ok()?;
        Some(Box::new(OpusCodec { enc: Some(enc), dec: None }))
    }
    fn decoder() -> Box<dyn Codec> {
        // A decoder that failed to construct conceals everything — a
        // silent peer with a rising counter, which the meter shows.
        Box::new(OpusCodec { enc: None, dec: studio_media::voice::VoiceDecoder::new().ok() })
    }
}

impl Codec for OpusCodec {
    fn tag(&self) -> u8 { Self::TAG }
    fn encode(&mut self, pcm: &[f32]) -> Vec<u8> {
        self.enc.as_mut().map(|e| e.encode(pcm)).unwrap_or_default()
    }
    fn decode(&mut self, payload: Option<&[u8]>, out: &mut [f32]) {
        match self.dec.as_mut() {
            Some(d) => d.decode(payload, out),
            None => out.fill(0.0),
        }
    }
}

/// The call's controls and plumbing. One per shell.
pub struct Call {
    link: Link,
    on: bool,
    muted: bool,
    /// Push-to-talk: muted unless [`Call::talking`] is held.
    ptt: bool,
    talking: bool,
    /// The tap could not be opened: hearing, not speaking, and the
    /// reason, for the status line.
    listen_only: Option<String>,
    /// Mic rate → 48 kHz. `None` while listen-only.
    mic_rs: Option<Resampler>,
    /// 48 kHz → the sink's rate. Rebuilt if the sink's rate changes.
    out_rs: Resampler,
    /// Stream depth to keep queued on the sink, in ms. The UI's stall
    /// budget: `update` runs every 50 ms at most when idle on the wire,
    /// so this must comfortably exceed that.
    pub target_ms: u32,
    /// Pumps that found the stream EMPTY while the call was on — the
    /// UI stalled longer than `target_ms`, and the far side went quiet
    /// for the difference. Said on the status line.
    starved: u64,
    /// Whether the stream has been filled at least once since start, so
    /// the first pump does not count as a starve.
    primed: bool,
    /// A peer sent voice while the call was off: who, for the prompt.
    ringing: Option<SiteId>,
    /// The codec this build speaks, for the status line.
    codec: &'static str,
    frame: Vec<f32>,
    chunk: Vec<f32>,
}

impl Default for Call {
    fn default() -> Self { Call::new() }
}

impl Call {
    /// Opus if libopus is usable on this machine, else 16-bit PCM — and
    /// the status line says which, because a PCM peer and an Opus peer
    /// conceal each other.
    pub fn new() -> Call {
        match OpusCodec::encoder() {
            Some(enc) => Call::with_link(
                Link::new(DEPTH, enc, Box::new(OpusCodec::decoder)), "opus"),
            None => Call::with_link(Link::pcm(DEPTH), "pcm"),
        }
    }

    /// PCM both ways: the tests' constructor, so what goes in comes out
    /// to within 16-bit rounding and the plumbing can be checked exactly.
    pub fn pcm() -> Call { Call::with_link(Link::pcm(DEPTH), "pcm") }

    fn with_link(link: Link, codec: &'static str) -> Call {
        Call {
            link, on: false, muted: false, ptt: false, talking: false, listen_only: None,
            mic_rs: None, out_rs: Resampler::new(RATE, RATE), target_ms: 100,
            starved: 0, primed: false, ringing: None, codec,
            frame: vec![0.0; FRAME], chunk: Vec::with_capacity(FRAME * 2),
        }
    }

    pub fn on(&self) -> bool { self.on }
    pub fn muted(&self) -> bool { self.muted }
    pub fn push_to_talk(&self) -> bool { self.ptt }
    pub fn talking(&self) -> bool { self.talking }
    pub fn listen_only(&self) -> Option<&str> { self.listen_only.as_deref() }
    pub fn starved(&self) -> u64 { self.starved }
    pub fn meter(&self) -> voice::Meter { self.link.meter() }
    /// A peer who spoke while we were not on the call.
    pub fn ringing(&self) -> Option<SiteId> { self.ringing }

    /// Join: open the tap, start the clock. A tap that cannot open makes
    /// the call listen-only rather than refusing it — hearing the singer
    /// with no way to answer is still most of a call.
    pub fn start(&mut self, mic: &mut dyn Capture) {
        if self.on { return; }
        self.on = true;
        self.primed = false;
        self.starved = 0;
        self.ringing = None;
        self.link.reset();
        self.out_rs.reset();
        match mic.tap_start() {
            Ok(()) => {
                self.listen_only = None;
                self.mic_rs = Some(Resampler::new(mic.rate().max(1), RATE));
            }
            Err(why) => {
                self.listen_only = Some(why);
                self.mic_rs = None;
            }
        }
    }

    /// Leave: close the tap, forget every peer's buffer.
    pub fn stop(&mut self, mic: &mut dyn Capture) {
        if !self.on { return; }
        self.on = false;
        mic.tap_stop();
        self.mic_rs = None;
        self.link.reset();
    }

    pub fn toggle(&mut self, mic: &mut dyn Capture) {
        if self.on { self.stop(mic) } else { self.start(mic) }
    }

    /// Muted sends silence, not nothing — see `Link`.
    pub fn set_muted(&mut self, on: bool) { self.muted = on; }
    /// Push-to-talk mode: the microphone is muted except while
    /// [`Call::set_talking`] is held. Turning it on does not mute the
    /// call's own mute toggle — both must be off to be heard.
    pub fn set_push_to_talk(&mut self, on: bool) { self.ptt = on; if !on { self.talking = false; } }
    /// The hold-to-talk control, pressed or released. Ignored outside
    /// push-to-talk mode so a stray release cannot silence an open mic.
    pub fn set_talking(&mut self, held: bool) { if self.ptt { self.talking = held; } }

    /// A peer left, or timed out: drop its buffer so a reconnect from a
    /// fresh sequence does not replay its last words.
    pub fn forget(&mut self, site: SiteId) { self.link.forget(site); }

    /// A packet from the wire. Only `Packet::Voice` is looked at, and
    /// only while the call is on — a frame from a call we have not
    /// joined is dropped, and the peer is remembered as ringing.
    pub fn receive(&mut self, p: &Packet) {
        let Packet::Voice { site, .. } = p else { return };
        if !self.on { self.ringing = Some(*site); return; }
        self.link.receive(p);
    }

    /// The per-frame pump: the microphone out, the peers in. Returns the
    /// packets to send. Does nothing while off.
    pub fn pump(&mut self, mic: &mut dyn Capture, sink: &mut dyn PlaybackSink, site: SiteId)
        -> Vec<Packet>
    {
        if !self.on { return Vec::new(); }
        self.link.set_muted(self.muted || (self.ptt && !self.talking));

        // ── out: whatever the tap caught, at 48 kHz, in whole frames ──
        let mut packets = Vec::new();
        if let Some(rs) = self.mic_rs.as_mut() {
            let raw = mic.tap_drain();
            if !raw.is_empty() {
                if rs.from_rate() != mic.rate().max(1) {
                    // The device renegotiated under us; follow it.
                    *rs = Resampler::new(mic.rate().max(1), RATE);
                }
                self.chunk.clear();
                rs.push(&raw, &mut self.chunk);
                packets = self.link.feed_mic(&self.chunk, site);
            }
        }

        // ── in: keep the sink's stream at depth; every chunk is a slot ──
        let rate = sink.rate().max(1);
        if self.out_rs.to_rate() != rate {
            self.out_rs = Resampler::new(RATE, rate);
        }
        let target = (rate as u64 * self.target_ms as u64 / 1000) as usize;
        if !self.primed {
            // The first fill is silence pushed WITHOUT popping anyone: a
            // peer whose frames arrived in the same UI frame we joined
            // would otherwise have the whole depth popped from its
            // buffer at once — three concealed, three late, before a
            // word was heard. Found by the TCP test, where the burst
            // and the join land in the same turn.
            self.frame.fill(0.0);
            while sink.stream_queued() < target {
                self.chunk.clear();
                self.out_rs.push(&self.frame, &mut self.chunk);
                if self.chunk.is_empty() { break; }
                sink.stream_push(Arc::new(self.chunk.clone()));
            }
            self.primed = true;
            return packets;
        }
        if sink.stream_queued() == 0 {
            self.starved += 1;
        }
        while sink.stream_queued() < target {
            // Zero peers: `frame` is silence and no clock advances; it is
            // still pushed, so the depth is there when someone speaks.
            self.link.playout(&mut self.frame);
            self.chunk.clear();
            self.out_rs.push(&self.frame, &mut self.chunk);
            if self.chunk.is_empty() { break; }
            sink.stream_push(Arc::new(self.chunk.clone()));
        }
        packets
    }

    /// One line for the status bar.
    pub fn describe(&self, mic: &dyn Capture) -> String {
        if !self.on {
            return match self.ringing {
                Some(s) => format!("site {} is talking — join the call to hear them", s.0),
                None => String::new(),
            };
        }
        let m = self.link.meter();
        let mut s = format!("call ({}): {} peer{}", self.codec, m.peers,
                            if m.peers == 1 { "" } else { "s" });
        if m.playing < m.peers { s.push_str(&format!(", {} buffering", m.peers - m.playing)); }
        if m.lost > 0 { s.push_str(&format!(", {} lost", m.lost)); }
        if m.late > 0 { s.push_str(&format!(", {} late", m.late)); }
        if m.unknown > 0 { s.push_str(&format!(", {} in a codec this build lacks", m.unknown)); }
        if self.starved > 0 { s.push_str(&format!(", {} UI stalls", self.starved)); }
        if let Some(why) = &self.listen_only {
            s.push_str(&format!(" — listen-only: {why}"));
        } else {
            if self.muted { s.push_str(" — muted"); }
            else if self.ptt { s.push_str(if self.talking { " — talking" } else { " — hold to talk" }); }
            let d = mic.tap_dropped();
            if d > 0 { s.push_str(&format!(", {d} mic block(s) dropped")); }
        }
        s
    }
}

/// A sink and a microphone the tests drive by hand, so the clock is the
/// test's. `pub(crate)` so the shell's TCP test can put a real socket
/// between two of them.
#[cfg(test)]
pub(crate) mod fakes {
    use super::*;
    use crate::capture::Take;

    /// A sink the test drains by hand, so the clock is the test's.
    pub(crate) struct FakeSink {
        pub(crate) rate: u32,
        pub(crate) queued: Vec<f32>,
        pub(crate) played: Vec<f32>,
    }
    impl FakeSink {
        pub(crate) fn new(rate: u32) -> FakeSink { FakeSink { rate, queued: Vec::new(), played: Vec::new() } }
        /// The device consumed `n` frames.
        pub(crate) fn consume(&mut self, n: usize) {
            let n = n.min(self.queued.len());
            self.played.extend(self.queued.drain(..n));
        }
    }
    impl PlaybackSink for FakeSink {
        fn set_buffer(&mut self, _: Arc<Vec<f32>>, _: u16, _: bool) {}
        fn set_playing(&mut self, _: bool) {}
        fn playing(&self) -> bool { false }
        fn seek(&mut self, _: usize) {}
        fn trigger(&mut self, _: Arc<Vec<f32>>) {}
        fn voices(&self) -> usize { 0 }
        fn stream_push(&mut self, chunk: Arc<Vec<f32>>) { self.queued.extend_from_slice(&chunk); }
        fn stream_queued(&self) -> usize { self.queued.len() }
        fn position(&self) -> Option<usize> { None }
        fn rate(&self) -> u32 { self.rate }
        fn describe(&self) -> String { "fake".into() }
    }

    /// A microphone the test speaks into.
    pub(crate) struct FakeMic {
        pub(crate) rate: u32,
        pub(crate) tapping: bool,
        pub(crate) script: Vec<f32>,
        pub(crate) refuse: bool,
    }
    impl FakeMic {
        pub(crate) fn new(rate: u32) -> FakeMic { FakeMic { rate, tapping: false, script: Vec::new(), refuse: false } }
        pub(crate) fn say(&mut self, pcm: &[f32]) { self.script.extend_from_slice(pcm); }
    }
    impl Capture for FakeMic {
        fn start(&mut self) -> Result<(), String> { Ok(()) }
        fn stop(&mut self) -> Option<Take> { None }
        fn split_take(&mut self) -> Option<Take> { None }
        fn recording(&self) -> bool { false }
        fn elapsed(&self) -> f32 { 0.0 }
        fn device_name(&self) -> &str { "fake" }
        fn rate(&self) -> u32 { self.rate }
        fn tap_start(&mut self) -> Result<(), String> {
            if self.refuse { return Err("no mic".into()); }
            self.tapping = true; Ok(())
        }
        fn tap_stop(&mut self) { self.tapping = false; }
        fn tapping(&self) -> bool { self.tapping }
        fn tap_drain(&mut self) -> Vec<f32> {
            if self.tapping { std::mem::take(&mut self.script) } else { Vec::new() }
        }
        fn tap_dropped(&self) -> usize { 0 }
    }

    pub(crate) fn sine(n: usize, hz: f32, rate: u32, amp: f32) -> Vec<f32> {
        (0..n).map(|i| amp * (2.0 * std::f32::consts::PI * hz * i as f32 / rate as f32).sin())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::fakes::*;

    fn sine(n: usize, hz: f32, rate: u32, amp: f32) -> Vec<f32> {
        (0..n).map(|i| amp * (2.0 * std::f32::consts::PI * hz * i as f32 / rate as f32).sin())
            .collect()
    }
    fn zero_crossings(x: &[f32]) -> usize {
        x.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count()
    }
    const A: SiteId = SiteId(1);
    const B: SiteId = SiteId(2);

    #[test]
    fn what_a_says_into_the_tap_comes_out_of_b_s_stream_after_the_depth() {
        let (mut a, mut b) = (Call::pcm(), Call::pcm());
        let (mut mic_a, mut sink_a) = (FakeMic::new(48_000), FakeSink::new(48_000));
        let (mut mic_b, mut sink_b) = (FakeMic::new(48_000), FakeSink::new(48_000));
        a.start(&mut mic_a);
        b.start(&mut mic_b);
        assert!(mic_a.tapping() && mic_b.tapping(), "joining opens the tap");
        assert!(a.listen_only().is_none());

        // B's first pump, nobody talking: the stream fills with silence
        // to the target depth — the clock starts running now.
        assert!(b.pump(&mut mic_b, &mut sink_b, B).is_empty(), "B has nothing to say");
        let target = 48_000 * 100 / 1000;
        assert!(sink_b.stream_queued() >= target && sink_b.stream_queued() < target + FRAME,
                "filled to depth, not past it by more than a chunk: {}", sink_b.stream_queued());
        assert!(sink_b.queued.iter().all(|&x| x == 0.0), "silence while no peer plays");

        // A speaks five frames. One pump: five packets.
        let tone = sine(FRAME * 5, 440.0, 48_000, 0.5);
        mic_a.say(&tone);
        let packets = a.pump(&mut mic_a, &mut sink_a, A);
        assert_eq!(packets.len(), 5);
        for p in &packets { b.receive(p); }
        assert_eq!(b.meter().peers, 1);

        // B's device plays some silence out; each pump refills exactly
        // the slots consumed — four, then one: A's five frames, in order.
        sink_b.consume(FRAME * 4);
        b.pump(&mut mic_b, &mut sink_b, B);
        assert_eq!(b.meter().playing, 1, "A's buffer reached depth and started");
        sink_b.consume(FRAME);
        b.pump(&mut mic_b, &mut sink_b, B);
        assert_eq!(b.meter().lost, 0, "and nothing was concealed on the way");
        sink_b.consume(FRAME * 5);
        let heard = &sink_b.played[sink_b.played.len() - FRAME * 5..];
        let d = heard.iter().zip(&tone).map(|(x, y)| (x - y).abs()).fold(0.0f32, f32::max);
        assert!(d < 1.0 / 32000.0, "B heard A to within 16-bit rounding: {d}");
        assert_eq!(b.starved(), 0);

        // Leaving closes the tap and forgets the peer.
        b.stop(&mut mic_b);
        assert!(!mic_b.tapping());
        assert_eq!(b.meter().peers, 0);
        assert!(b.pump(&mut mic_b, &mut sink_b, B).is_empty() && !b.on());
    }

    #[test]
    fn the_pitch_survives_a_44_1k_microphone_and_a_44_1k_sink() {
        // A's mic and B's speaker are both 44.1k; the link is 48k between.
        let (mut a, mut b) = (Call::pcm(), Call::pcm());
        let (mut mic_a, mut sink_a) = (FakeMic::new(44_100), FakeSink::new(44_100));
        let (mut mic_b, mut sink_b) = (FakeMic::new(44_100), FakeSink::new(44_100));
        a.start(&mut mic_a);
        b.start(&mut mic_b);
        b.pump(&mut mic_b, &mut sink_b, B);
        // One second of 440 Hz at 44.1k: 880 crossings.
        mic_a.say(&sine(44_100, 440.0, 44_100, 0.5));
        let packets = a.pump(&mut mic_a, &mut sink_a, A);
        // 44100 samples → 48000 at the link → 50 frames.
        // The resampler holds its last sample for the next chunk, so a
        // one-off second lands one sample short of the 50th frame's end.
        assert!((49..=50).contains(&packets.len()),
                "a second of speech is fifty frames whatever the mic's rate: {}", packets.len());
        for p in &packets { b.receive(p); }
        // Drain and refill, four slots at a time (a 48k frame is 882
        // frames at 44.1k), until exactly as many slots as packets have
        // been popped — popping more would conceal, and the point is
        // that nothing needs concealing.
        let slot = 882;
        let mut popped = 0;
        while popped < packets.len() {
            let n = 4.min(packets.len() - popped);
            sink_b.consume(slot * n);
            b.pump(&mut mic_b, &mut sink_b, B);
            popped += n;
        }
        sink_b.consume(sink_b.queued.len());
        let played = &sink_b.played;
        let zc = zero_crossings(played);
        // Some of B's stream is the silence before A spoke; the crossings
        // count only where the tone is. 880 ± the resampler's edge.
        assert!((850..=890).contains(&zc), "440 Hz in, 440 Hz out at 44.1k: {zc} crossings");
        assert_eq!(b.meter().lost, 0);
        let peak = played.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!((0.45..=0.55).contains(&peak), "amplitude kept: {peak}");
    }

    #[test]
    fn push_to_talk_sends_silence_until_held_and_mute_sends_silence_regardless() {
        let (mut a, mut b) = (Call::pcm(), Call::pcm());
        let (mut mic_a, mut sink_a) = (FakeMic::new(48_000), FakeSink::new(48_000));
        let (mut mic_b, mut sink_b) = (FakeMic::new(48_000), FakeSink::new(48_000));
        a.start(&mut mic_a);
        b.start(&mut mic_b);
        a.set_push_to_talk(true);
        b.pump(&mut mic_b, &mut sink_b, B); // prime the depth
        // Pop exactly one slot per packet — the queue's depth never
        // changes, so nothing is over-popped into concealment — and
        // measure the slots just queued at the tail.
        let hear = |b: &mut Call, sink_b: &mut FakeSink, mic_b: &mut FakeMic, packets: &[Packet]| -> f32 {
            for p in packets { b.receive(p); }
            for _ in 0..packets.len() { sink_b.consume(FRAME); b.pump(mic_b, sink_b, B); }
            let n = sink_b.queued.len();
            sink_b.queued[n - FRAME * packets.len()..].iter().fold(0.0f32, |m, x| m.max(x.abs()))
        };
        mic_a.say(&[0.5; FRAME * 4]);
        let p = a.pump(&mut mic_a, &mut sink_a, A);
        assert_eq!(p.len(), 4, "still four packets: a released key is silence on the wire, not absence");
        assert_eq!(hear(&mut b, &mut sink_b, &mut mic_b, &p), 0.0, "not held: silence");
        a.set_talking(true);
        mic_a.say(&[0.5; FRAME * 4]);
        let p = a.pump(&mut mic_a, &mut sink_a, A);
        assert!((hear(&mut b, &mut sink_b, &mut mic_b, &p) - 0.5).abs() < 1e-3, "held: heard");
        a.set_muted(true);
        mic_a.say(&[0.5; FRAME * 4]);
        let p = a.pump(&mut mic_a, &mut sink_a, A);
        assert_eq!(hear(&mut b, &mut sink_b, &mut mic_b, &p), 0.0, "muted wins even while held");
        a.set_push_to_talk(false);
        assert!(!a.talking(), "leaving push-to-talk releases the key");
        assert_eq!(b.meter().lost, 0, "none of that read as loss on B's meter");
        assert!(a.describe(&mic_a).contains("muted"), "{}", a.describe(&mic_a));
    }

    #[test]
    fn a_frame_for_a_call_not_joined_is_dropped_and_the_caller_is_remembered() {
        let mut b = Call::pcm();
        let mut a = Call::pcm();
        let mut mic_a = FakeMic::new(48_000);
        a.start(&mut mic_a);
        mic_a.say(&[0.3; FRAME]);
        let p = a.pump(&mut mic_a, &mut FakeSink::new(48_000), A).remove(0);
        b.receive(&p);
        assert_eq!(b.meter().peers, 0, "nothing buffered for a call we are not on");
        assert_eq!(b.ringing(), Some(A));
        let mic_b = FakeMic::new(48_000);
        assert!(b.describe(&mic_b).contains("site 1 is talking"), "{}", b.describe(&mic_b));
        let mut off = Call::pcm();
        assert_eq!(off.describe(&mic_b), "", "off and nobody calling: the status line says nothing");
        off.receive(&Packet::Hello { site: A, since: 0 });
        assert_eq!(off.ringing(), None, "only voice rings");
    }

    #[test]
    fn a_stall_longer_than_the_depth_is_counted_not_hidden() {
        let mut b = Call::pcm();
        let (mut mic_b, mut sink_b) = (FakeMic::new(48_000), FakeSink::new(48_000));
        b.start(&mut mic_b);
        b.pump(&mut mic_b, &mut sink_b, B);
        assert_eq!(b.starved(), 0, "the first fill is not a stall");
        sink_b.consume(FRAME * 2);
        b.pump(&mut mic_b, &mut sink_b, B);
        assert_eq!(b.starved(), 0, "consumed less than the depth: fine");
        sink_b.consume(sink_b.queued.len());
        b.pump(&mut mic_b, &mut sink_b, B);
        assert_eq!(b.starved(), 1, "found the stream empty: the UI stalled past the depth");
        assert!(b.describe(&mic_b).contains("1 UI stalls"), "{}", b.describe(&mic_b));
    }

    #[test]
    fn no_microphone_means_listen_only_not_no_call() {
        let mut b = Call::pcm();
        let mut mic_b = FakeMic::new(48_000);
        mic_b.refuse = true;
        let mut sink_b = FakeSink::new(48_000);
        b.start(&mut mic_b);
        assert!(b.on());
        assert_eq!(b.listen_only(), Some("no mic"));
        // A still comes through.
        let mut a = Call::pcm();
        let mut mic_a = FakeMic::new(48_000);
        a.start(&mut mic_a);
        mic_a.say(&[0.4; FRAME * 4]);
        for p in a.pump(&mut mic_a, &mut FakeSink::new(48_000), A) { b.receive(&p); }
        for _ in 0..6 { b.pump(&mut mic_b, &mut sink_b, B); sink_b.consume(FRAME * 5); }
        assert!(sink_b.played.iter().any(|&x| (x - 0.4).abs() < 1e-3), "heard A with no mic of our own");
        assert!(b.pump(&mut mic_b, &mut sink_b, B).is_empty(), "and sent nothing");
        assert!(b.describe(&mic_b).contains("listen-only: no mic"), "{}", b.describe(&mic_b));
    }

    #[test]
    fn the_real_constructor_names_its_codec_and_speaks_it_both_ways() {
        // Opus if this build has it (studio-media's default), and a round
        // trip through it must carry a tone — not just not crash.
        let (mut a, mut b) = (Call::new(), Call::new());
        let (mut mic_a, mut sink_a) = (FakeMic::new(48_000), FakeSink::new(48_000));
        let (mut mic_b, mut sink_b) = (FakeMic::new(48_000), FakeSink::new(48_000));
        a.start(&mut mic_a);
        b.start(&mut mic_b);
        b.pump(&mut mic_b, &mut sink_b, B);
        mic_a.say(&sine(FRAME * 20, 440.0, 48_000, 0.5));
        let packets = a.pump(&mut mic_a, &mut sink_a, A);
        assert_eq!(packets.len(), 20);
        for p in &packets { b.receive(p); }
        for _ in 0..30 { sink_b.consume(FRAME * 5); b.pump(&mut mic_b, &mut sink_b, B); }
        sink_b.consume(sink_b.queued.len());
        let peak = sink_b.played.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(peak > 0.3, "a tone came through the {} codec: peak {peak}", a.codec);
        assert_eq!(b.meter().unknown, 0, "both sides speak the same codec");
        assert!(a.describe(&mic_a).starts_with(&format!("call ({})", a.codec)));
    }
}
