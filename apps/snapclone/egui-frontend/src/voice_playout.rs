//! voice_playout — the receive half's scheduling, ported.
//!
//! The floor logic came across earlier; this is what happens to the packets
//! once you hold it. Decode, codec and sink stay with the host — this is the
//! part that decides, every frame interval, whether to **play, conceal, or go
//! silent**, and which stream each packet belongs to.
//!
//! ## One jitter buffer and one decoder PER SENDER
//!
//! The Qt comment is unambiguous and it is the single most important rule here:
//! Opus decoder state is per-stream, and feeding one decoder interleaved frames
//! from several senders **scrambles the audio**. So each peer gets its own
//! buffer, the tick pops one frame from each, and the results are summed into a
//! single play-out frame. For a one-to-one call the map holds one entry and the
//! mix is a pass-through — the same code path, not a special case.
//!
//! ## A gap is concealed, not skipped
//!
//! A missing sequence number is not the same as no audio. Opus PLC extrapolates
//! a frame from the last good one, which sounds like a slight smear; playing
//! silence instead sounds like the speaker stopped, and playing the NEXT frame
//! early makes them sound rushed and desynchronises everything after it. The
//! three outcomes are distinct states rather than a boolean.
//!
//! Concealment is capped. Extrapolating indefinitely turns a dead connection
//! into a ghost that keeps talking, so after a run of consecutive concealed
//! frames the stream goes silent and stays silent until real audio arrives.
//!
//! ## Preroll plays before the live buffer, not mixed into it
//!
//! Audio captured in the ring window BEFORE the user accepted is played first
//! and in order, so the head of an accepted message is not lost. It must not be
//! interleaved with live frames: those are the same speaker seconds apart, and
//! summing them is an echo rather than a recovery.

/// Frame interval at 16 kHz mono, matching the sink the Qt side opens.
pub const FRAME_MS: u32 = 20;

/// How many frames may be concealed in a row before the stream gives up.
///
/// About a fifth of a second. Long enough to ride out the packet loss that
/// actually happens, short enough that a dead connection stops talking.
pub const MAX_CONSECUTIVE_CONCEAL: u32 = 10;

/// Target depth of the jitter buffer, in frames.
///
/// Two frames of slack: enough to absorb ordinary reordering, small enough that
/// the added latency stays under a syllable. A deeper buffer hides more loss
/// and makes the conversation harder to hold.
pub const TARGET_DEPTH: usize = 2;

/// What the play-out tick decided for one stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Playout {
    /// A real frame, in arrival-independent sequence order.
    Play { seq: u64, payload_len: usize },
    /// The frame is missing: extrapolate from the last good one.
    Conceal,
    /// Nothing to play and nothing worth concealing.
    Silence,
}

/// One received packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub seq: u64,
    pub payload_len: usize,
    /// First frame of a talk spurt.
    ///
    /// The sender restarts sequence numbers at 0 every spurt, so without this
    /// flag a second spurt looks exactly like a flood of very late frames and
    /// the drop-late rule eats all of it. This scheduler was written without
    /// the flag and had precisely that bug; `talkspurt_gap` pins it.
    pub talkspurt_start: bool,
}

impl Frame {
    pub fn new(seq: u64, payload_len: usize) -> Frame {
        Frame { seq, payload_len, talkspurt_start: false }
    }
    /// The first frame of a spurt: resets the receiver's sequencing.
    pub fn spurt_start(seq: u64, payload_len: usize) -> Frame {
        Frame { seq, payload_len, talkspurt_start: true }
    }
}

/// A single sender's buffer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PeerStream {
    /// Ordered by sequence.
    queue: Vec<Frame>,
    /// Frames captured before the call was accepted, played first.
    preroll: Vec<Frame>,
    /// The next sequence expected.
    next_seq: Option<u64>,
    concealed_run: u32,
    started: bool,
}

impl PeerStream {
    pub fn depth(&self) -> usize { self.queue.len() }
    pub fn preroll_len(&self) -> usize { self.preroll.len() }
    pub fn concealed_run(&self) -> u32 { self.concealed_run }

    /// Accept a live frame.
    ///
    /// A frame older than the play-out point is DROPPED rather than played out
    /// of order: it arrived after its moment passed, and inserting it behind the
    /// current position would play the past.
    pub fn push(&mut self, f: Frame) -> bool {
        // A new talk spurt resets sequencing. Anything still queued belongs to
        // the previous spurt and is DISCARDED rather than played after it: the
        // speaker already stopped, and those frames are older than the silence
        // the listener just heard.
        if f.talkspurt_start {
            self.queue.clear();
            self.next_seq = None;
            self.concealed_run = 0;
            self.started = false;
            self.queue.push(f);
            return true;
        }
        if let Some(next) = self.next_seq {
            if f.seq < next { return false; }
        }
        if self.queue.iter().any(|q| q.seq == f.seq) { return false; }
        self.queue.push(f);
        self.queue.sort_by_key(|q| q.seq);
        true
    }

    /// Queue audio captured before the user accepted.
    ///
    /// Kept separate from the live queue on purpose — see the module docs.
    pub fn push_preroll(&mut self, f: Frame) { self.preroll.push(f); }

    /// Decide what to play this interval.
    pub fn tick(&mut self) -> Playout {
        // Preroll first, in order, before any live audio.
        if !self.preroll.is_empty() {
            let f = self.preroll.remove(0);
            self.concealed_run = 0;
            return Playout::Play { seq: f.seq, payload_len: f.payload_len };
        }

        if self.queue.is_empty() {
            // Nothing buffered. Conceal only if we were mid-speech, and only up
            // to the cap — otherwise a dead connection becomes a ghost that
            // keeps talking.
            if self.started && self.concealed_run < MAX_CONSECUTIVE_CONCEAL {
                self.concealed_run += 1;
                return Playout::Conceal;
            }
            return Playout::Silence;
        }

        // Wait for the buffer to reach target depth before the first frame, so
        // the stream does not start already starving.
        if !self.started && self.queue.len() < TARGET_DEPTH {
            return Playout::Silence;
        }

        let want = self.next_seq.unwrap_or(self.queue[0].seq);
        if self.queue[0].seq == want {
            let f = self.queue.remove(0);
            self.next_seq = Some(f.seq + 1);
            self.started = true;
            self.concealed_run = 0;
            return Playout::Play { seq: f.seq, payload_len: f.payload_len };
        }

        // A gap. Conceal it rather than playing the next frame early, which
        // would make the speaker sound rushed and desynchronise everything
        // after it.
        if self.concealed_run < MAX_CONSECUTIVE_CONCEAL {
            self.concealed_run += 1;
            self.next_seq = Some(want + 1);
            return Playout::Conceal;
        }
        // Given up concealing: skip to what we actually have.
        let f = self.queue.remove(0);
        self.next_seq = Some(f.seq + 1);
        self.concealed_run = 0;
        Playout::Play { seq: f.seq, payload_len: f.payload_len }
    }

    pub fn flush(&mut self) { *self = PeerStream::default(); }
}

/// The receive half: one stream per sender, summed each tick.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VoicePlayout {
    streams: Vec<(String, PeerStream)>,
    playing: bool,
    /// True once the host has reported the decoder missing. Reported ONCE, as
    /// the Qt side does: a per-frame error would flood the log with the same
    /// build problem.
    decoder_unavailable: bool,
}

impl VoicePlayout {
    pub fn start(&mut self) { self.playing = true; }

    /// Stop and flush. A call that ends must not leave frames to play into the
    /// next one.
    pub fn stop(&mut self) {
        self.playing = false;
        self.streams.clear();
    }

    pub fn is_playing(&self) -> bool { self.playing }
    pub fn senders(&self) -> usize { self.streams.len() }

    pub fn stream_for(&mut self, id: &str) -> &mut PeerStream {
        if let Some(i) = self.streams.iter().position(|(k, _)| k == id) {
            return &mut self.streams[i].1;
        }
        self.streams.push((id.to_string(), PeerStream::default()));
        let last = self.streams.len() - 1;
        &mut self.streams[last].1
    }

    pub fn peek(&self, id: &str) -> Option<&PeerStream> {
        self.streams.iter().find(|(k, _)| k == id).map(|(_, s)| s)
    }

    /// A frame arrived from a sender.
    pub fn on_frame(&mut self, sender: &str, f: Frame) -> bool {
        if !self.playing { return false; }
        self.stream_for(sender).push(f)
    }

    pub fn enqueue_preroll(&mut self, sender: &str, f: Frame) {
        self.stream_for(sender).push_preroll(f);
    }

    /// One play-out interval: every stream decides, and the results are mixed.
    ///
    /// Returned per sender rather than pre-summed, because the host owns the
    /// decoder and only it can turn a `Play` into samples. Mixing here would
    /// mean mixing sequence numbers.
    pub fn tick(&mut self) -> Vec<(String, Playout)> {
        if !self.playing { return Vec::new(); }
        self.streams.iter_mut().map(|(k, s)| (k.clone(), s.tick())).collect()
    }

    /// Report the decoder missing. Returns true only the first time, so the
    /// caller can log once.
    pub fn note_decoder_unavailable(&mut self) -> bool {
        if self.decoder_unavailable { return false; }
        self.decoder_unavailable = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(seq: u64) -> Frame { Frame::new(seq, 40) }

    fn started_stream() -> PeerStream {
        let mut s = PeerStream::default();
        s.push(f(1));
        s.push(f(2));
        s.tick();          // reaches depth, plays 1
        s
    }

    #[test]
    fn each_sender_gets_its_own_stream() {
        // Opus decoder state is per-stream: feeding one decoder interleaved
        // frames from several senders scrambles the audio.
        let mut v = VoicePlayout::default();
        v.start();
        v.on_frame("ada", f(1));
        v.on_frame("bob", f(1));
        assert_eq!(v.senders(), 2);
        assert_eq!(v.peek("ada").unwrap().depth(), 1);
        assert_eq!(v.peek("bob").unwrap().depth(), 1);
    }

    #[test]
    fn a_one_to_one_call_uses_the_same_path_as_a_group() {
        // Not a special case: the map holds one entry and the mix is a
        // pass-through.
        let mut v = VoicePlayout::default();
        v.start();
        v.on_frame("ada", f(1));
        v.on_frame("ada", f(2));
        let out = v.tick();
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0].1, Playout::Play { seq: 1, .. }));
    }

    #[test]
    fn the_stream_waits_for_target_depth_before_its_first_frame() {
        // Starting on the first packet means starting already starving: the
        // next gap has nothing to ride out on.
        let mut s = PeerStream::default();
        s.push(f(1));
        assert_eq!(s.tick(), Playout::Silence, "one frame is not enough to start");
        s.push(f(2));
        assert!(matches!(s.tick(), Playout::Play { seq: 1, .. }));
    }

    #[test]
    fn a_gap_is_concealed_rather_than_skipped() {
        // Playing the next frame early makes the speaker sound rushed and
        // desynchronises everything after it.
        let mut s = started_stream();
        s.push(f(4));              // 3 never arrived
        assert!(matches!(s.tick(), Playout::Play { seq: 2, .. }));
        assert_eq!(s.tick(), Playout::Conceal, "3 is missing");
        assert!(matches!(s.tick(), Playout::Play { seq: 4, .. }));
    }

    #[test]
    fn concealment_is_capped_so_a_dead_connection_stops_talking() {
        // Extrapolating indefinitely turns a dropped call into a ghost.
        let mut s = started_stream();
        s.tick();                                   // plays 2, queue empty
        let mut concealed = 0;
        for _ in 0..(MAX_CONSECUTIVE_CONCEAL + 5) {
            if s.tick() == Playout::Conceal { concealed += 1; }
        }
        assert_eq!(concealed, MAX_CONSECUTIVE_CONCEAL);
        assert_eq!(s.tick(), Playout::Silence);
    }

    #[test]
    fn real_audio_resets_the_concealment_run() {
        // Otherwise a stream that lost ten frames early would give up on the
        // next gap however long it had been speaking cleanly since.
        let mut s = started_stream();
        s.tick();
        s.tick();                       // conceal
        assert!(s.concealed_run() > 0);
        s.push(f(3));
        s.push(f(4));
        s.tick();
        assert_eq!(s.concealed_run(), 0);
    }

    #[test]
    fn a_stream_that_never_started_is_silent_not_concealed() {
        // There is nothing to extrapolate FROM. Concealing here would invent
        // audio for a speaker who has not spoken.
        let mut s = PeerStream::default();
        for _ in 0..5 { assert_eq!(s.tick(), Playout::Silence); }
        assert_eq!(s.concealed_run(), 0);
    }

    #[test]
    fn a_late_frame_is_dropped_rather_than_played_out_of_order() {
        // It arrived after its moment passed; inserting it behind the play-out
        // point would play the past.
        let mut s = started_stream();
        s.tick();                       // plays 2, next_seq = 3
        assert!(!s.push(f(1)), "seq 1 is long gone");
        assert_eq!(s.depth(), 0);
    }

    #[test]
    fn a_duplicate_frame_is_ignored() {
        // Retransmits and multipath both deliver the same packet twice; playing
        // it twice is a stutter.
        let mut s = PeerStream::default();
        assert!(s.push(f(1)));
        assert!(!s.push(f(1)));
        assert_eq!(s.depth(), 1);
    }

    #[test]
    fn out_of_order_arrivals_are_played_in_sequence() {
        // That is the whole point of the buffer.
        let mut s = PeerStream::default();
        s.push(f(2));
        s.push(f(1));
        assert!(matches!(s.tick(), Playout::Play { seq: 1, .. }));
        assert!(matches!(s.tick(), Playout::Play { seq: 2, .. }));
    }

    #[test]
    fn preroll_plays_before_live_audio_and_in_order() {
        // The head of an accepted message would otherwise be lost.
        let mut s = PeerStream::default();
        s.push_preroll(Frame::new(100, 10));
        s.push_preroll(Frame::new(101, 10));
        s.push(f(1));
        s.push(f(2));
        assert!(matches!(s.tick(), Playout::Play { seq: 100, .. }));
        assert!(matches!(s.tick(), Playout::Play { seq: 101, .. }));
        assert!(matches!(s.tick(), Playout::Play { seq: 1, .. }),
                "then the live stream, not mixed with it");
    }

    #[test]
    fn preroll_is_not_mixed_into_the_live_queue() {
        // Those are the same speaker seconds apart — summing them is an echo,
        // not a recovery.
        let mut s = PeerStream::default();
        s.push_preroll(Frame::new(100, 10));
        s.push(f(1));
        assert_eq!(s.depth(), 1);
        assert_eq!(s.preroll_len(), 1);
    }

    #[test]
    fn frames_are_refused_while_stopped() {
        // A call that has ended must not accumulate audio to play into the next
        // one.
        let mut v = VoicePlayout::default();
        assert!(!v.on_frame("ada", f(1)));
        assert_eq!(v.tick().len(), 0);
    }

    #[test]
    fn stopping_flushes_so_the_next_call_starts_clean() {
        let mut v = VoicePlayout::default();
        v.start();
        v.on_frame("ada", f(1));
        v.stop();
        assert_eq!(v.senders(), 0);
        v.start();
        assert_eq!(v.tick().len(), 0);
    }

    #[test]
    fn the_missing_decoder_is_reported_once() {
        // A per-frame error would flood the log with the same build problem —
        // fifty times a second.
        let mut v = VoicePlayout::default();
        assert!(v.note_decoder_unavailable());
        assert!(!v.note_decoder_unavailable());
    }
}

#[cfg(test)]
mod talkspurt_gap {
    use super::*;

    /// A second talk spurt restarts sequence numbers at 0.
    ///
    /// The sender's contract says so explicitly: "sequence numbers restart at 0
    /// each talk-spurt and the first frame is flagged TALKSPURT_START so the
    /// receiver resets its jitter buffer cleanly." This scheduler was written
    /// without that flag, so the drop-late-frames rule silently eats the whole
    /// second spurt.
    #[test]
    fn a_second_talk_spurt_is_not_swallowed_as_late_frames() {
        let mut s = PeerStream::default();
        s.push(Frame::spurt_start(0, 40));
        s.push(Frame::new(1, 40));
        s.tick();
        s.tick();                      // played 0 and 1, next_seq = 2

        // Speaker stops, then starts again: sequence restarts at 0.
        let accepted = s.push(Frame::spurt_start(0, 40));
        assert!(accepted, "the new spurt's first frame was dropped as 'late'");
    }
}
