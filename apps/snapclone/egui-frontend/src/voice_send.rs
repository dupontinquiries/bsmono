//! voice_send — the transmit half's sequencing and gating, ported.
//!
//! The pair to `voice_playout`. The Opus encoder, the mic and the socket stay
//! with the host; what is ported is **when a frame is allowed out** and **what
//! sequence number it carries** — the two things the receiver depends on and
//! neither of which is audio processing.
//!
//! ## Talk-spurt sequencing, which the receiver was already relying on
//!
//! The Qt contract: *sequence numbers restart at 0 each talk-spurt and the
//! first frame is flagged TALKSPURT_START so the receiver resets its jitter
//! buffer cleanly.*
//!
//! Porting this found a bug in the receiver written last session. That
//! scheduler had no notion of the flag, and its drop-late-frames rule made a
//! second spurt — starting again at 0 — look exactly like a flood of very late
//! packets. It ate the whole spurt. `voice_playout::talkspurt_gap` pins it now.
//!
//! Worth naming the shape: the receiver was tested thoroughly and passed,
//! because every test fed it ONE spurt. The defect only existed in the
//! relationship between the two halves, which is exactly the kind a port that
//! carries one side at a time will produce.
//!
//! ## The gate is not the same as the mic being open
//!
//! Frames arrive from capture continuously — the VAD score is wanted even while
//! silent, because the floor logic uses it to decide whether to grant the floor
//! at all. So capture runs and the SENDER gates. Conflating the two means
//! either the VOX gate goes deaf between spurts, or the radio transmits the
//! room while nobody is talking.

/// Default encoder bitrate. 16 kbit/s mono voice — the Qt constructor's
/// default, low enough for a phone tether and high enough to stay intelligible.
pub const DEFAULT_BITRATE: u32 = 16_000;

/// What the sender decided for one captured frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Emit {
    /// Encode and send with this sequence number.
    Send { seq: u64, talkspurt_start: bool },
    /// Not transmitting: the frame is dropped, deliberately and silently.
    Gated,
    /// Transmitting, but the encoder is unavailable. Reported ONCE.
    NoEncoder,
}

/// The transmit half.
#[derive(Debug, Clone, PartialEq)]
pub struct VoiceSender {
    pub bitrate: u32,
    transmitting: bool,
    encoder_ready: bool,
    /// Next sequence within the current spurt.
    seq: u64,
    /// True until the first frame of a spurt has gone out.
    spurt_pending: bool,
    reported_no_encoder: bool,
    /// Latest voice-activity score from capture, kept even while gated.
    activity: f32,
}

impl Default for VoiceSender {
    fn default() -> Self {
        VoiceSender {
            bitrate: DEFAULT_BITRATE, transmitting: false, encoder_ready: true,
            seq: 0, spurt_pending: true, reported_no_encoder: false, activity: 0.0,
        }
    }
}

impl VoiceSender {
    pub fn is_transmitting(&self) -> bool { self.transmitting }
    pub fn encoder_ready(&self) -> bool { self.encoder_ready }
    pub fn activity(&self) -> f32 { self.activity }
    pub fn seq(&self) -> u64 { self.seq }

    /// The host reports whether an encoder exists.
    pub fn set_encoder_ready(&mut self, ready: bool) { self.encoder_ready = ready; }

    /// Floor state changed.
    ///
    /// Going ON starts a new spurt: sequence back to 0 and the next frame
    /// flagged. Going OFF does NOT reset — the reset belongs to the start of
    /// the next spurt, so a stop-start with no frames between cannot emit two
    /// starts and confuse the receiver into flushing twice.
    pub fn set_transmitting(&mut self, on: bool) {
        if on == self.transmitting { return; }
        self.transmitting = on;
        if on {
            self.seq = 0;
            self.spurt_pending = true;
        }
    }

    /// Voice-activity score from capture.
    ///
    /// Accepted whether or not we are transmitting: the floor logic's VOX gate
    /// needs it precisely while silent, to decide whether to ask for the floor.
    pub fn on_activity(&mut self, score: f32) { self.activity = score.clamp(0.0, 1.0); }

    /// A captured PCM frame is ready.
    pub fn on_frame(&mut self) -> Emit {
        if !self.transmitting { return Emit::Gated; }
        if !self.encoder_ready { return Emit::NoEncoder; }
        let start = self.spurt_pending;
        self.spurt_pending = false;
        let seq = self.seq;
        self.seq += 1;
        Emit::Send { seq, talkspurt_start: start }
    }

    /// Report the missing encoder. True only the first time, so a caller can
    /// log once rather than fifty times a second.
    pub fn note_no_encoder(&mut self) -> bool {
        if self.reported_no_encoder { return false; }
        self.reported_no_encoder = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transmitting() -> VoiceSender {
        let mut s = VoiceSender::default();
        s.set_transmitting(true);
        s
    }

    #[test]
    fn a_spurt_starts_at_zero_with_the_flag_set() {
        // The receiver resets its jitter buffer on this flag; without it a
        // second spurt looks like a flood of late packets.
        let mut s = transmitting();
        assert_eq!(s.on_frame(), Emit::Send { seq: 0, talkspurt_start: true });
        assert_eq!(s.on_frame(), Emit::Send { seq: 1, talkspurt_start: false });
    }

    #[test]
    fn every_new_spurt_restarts_the_sequence() {
        let mut s = transmitting();
        s.on_frame();
        s.on_frame();
        s.set_transmitting(false);
        s.set_transmitting(true);
        assert_eq!(s.on_frame(), Emit::Send { seq: 0, talkspurt_start: true });
    }

    #[test]
    fn a_stop_start_with_no_frames_between_emits_one_start_not_two() {
        // Otherwise the receiver flushes twice for a single spurt, discarding
        // audio it had already buffered.
        let mut s = transmitting();
        s.set_transmitting(false);
        s.set_transmitting(true);
        s.set_transmitting(false);
        s.set_transmitting(true);
        assert_eq!(s.on_frame(), Emit::Send { seq: 0, talkspurt_start: true });
        assert_eq!(s.on_frame(), Emit::Send { seq: 1, talkspurt_start: false });
    }

    #[test]
    fn setting_the_same_transmit_state_twice_does_not_restart_a_spurt() {
        // A repeated signal must not renumber mid-sentence — the receiver would
        // flush and the listener would hear a cut.
        let mut s = transmitting();
        s.on_frame();
        s.set_transmitting(true);
        assert_eq!(s.on_frame(), Emit::Send { seq: 1, talkspurt_start: false });
    }

    #[test]
    fn frames_are_gated_while_not_transmitting() {
        // The radio must not send the room while nobody has the floor.
        let mut s = VoiceSender::default();
        assert_eq!(s.on_frame(), Emit::Gated);
        assert_eq!(s.seq(), 0, "and gated frames consume no sequence numbers");
    }

    #[test]
    fn voice_activity_is_accepted_while_gated() {
        // THE reason capture and gating are separate. The VOX gate needs the
        // score precisely while silent, to decide whether to ask for the floor;
        // a sender that ignored activity between spurts would go deaf exactly
        // when the decision is made.
        let mut s = VoiceSender::default();
        assert!(!s.is_transmitting());
        s.on_activity(0.8);
        assert_eq!(s.activity(), 0.8);
    }

    #[test]
    fn an_activity_score_is_clamped_rather_than_trusted() {
        let mut s = VoiceSender::default();
        s.on_activity(4.0);
        assert_eq!(s.activity(), 1.0);
        s.on_activity(-1.0);
        assert_eq!(s.activity(), 0.0);
    }

    #[test]
    fn a_missing_encoder_reports_rather_than_pretending_to_send() {
        // Dropping frames silently would look like a network problem and send
        // someone debugging the wrong layer.
        let mut s = transmitting();
        s.set_encoder_ready(false);
        assert_eq!(s.on_frame(), Emit::NoEncoder);
        assert_eq!(s.seq(), 0, "a frame that never encoded takes no sequence number");
    }

    #[test]
    fn the_missing_encoder_is_reported_once() {
        // Fifty frames a second, all with the same build problem.
        let mut s = transmitting();
        assert!(s.note_no_encoder());
        assert!(!s.note_no_encoder());
    }

    #[test]
    fn recovering_an_encoder_resumes_the_same_spurt() {
        // The listener heard a gap; renumbering would make the rest of the
        // sentence look like a new spurt and flush what survived.
        let mut s = transmitting();
        s.on_frame();
        s.set_encoder_ready(false);
        s.on_frame();
        s.set_encoder_ready(true);
        assert_eq!(s.on_frame(), Emit::Send { seq: 1, talkspurt_start: false });
    }

    #[test]
    fn the_default_bitrate_matches_the_qt_constructor() {
        assert_eq!(VoiceSender::default().bitrate, 16_000);
    }
}

#[cfg(test)]
mod round_trip {
    use super::*;
    use crate::voice_playout::{Frame, PeerStream, Playout};

    /// Send a spurt of `n` frames through the sender into a receiver stream.
    fn spurt(s: &mut VoiceSender, r: &mut PeerStream, n: usize) {
        s.set_transmitting(true);
        for _ in 0..n {
            if let Emit::Send { seq, talkspurt_start } = s.on_frame() {
                let f = if talkspurt_start { Frame::spurt_start(seq, 40) }
                        else { Frame::new(seq, 40) };
                r.push(f);
            }
        }
        s.set_transmitting(false);
    }

    /// The test the two halves needed and neither had.
    ///
    /// Both sides passed their own suites while disagreeing about what a second
    /// talk spurt means — the receiver had no notion of the flag and dropped
    /// the spurt as late frames. A defect that lives in the RELATIONSHIP
    /// between two ported halves is invisible to tests of either one, and this
    /// port carries one side at a time, so it is the expected shape of bug.
    #[test]
    fn two_talk_spurts_both_arrive() {
        let (mut s, mut r) = (VoiceSender::default(), PeerStream::default());

        spurt(&mut s, &mut r, 3);
        let mut first = 0;
        for _ in 0..3 {
            if matches!(r.tick(), Playout::Play { .. }) { first += 1; }
        }
        assert_eq!(first, 3, "the first spurt");

        // Speaker pauses, then talks again. Sequence numbers restart at 0.
        spurt(&mut s, &mut r, 3);
        let mut second = 0;
        for _ in 0..3 {
            if matches!(r.tick(), Playout::Play { .. }) { second += 1; }
        }
        assert_eq!(second, 3, "the second spurt, which used to be swallowed whole");
    }

    #[test]
    fn a_new_spurt_discards_what_the_old_one_left_buffered() {
        // Those frames are older than the silence the listener just heard.
        // Playing them after the new spurt starts would be the previous
        // sentence arriving late, on top of the current one.
        let (mut s, mut r) = (VoiceSender::default(), PeerStream::default());
        s.set_transmitting(true);
        for _ in 0..4 {
            if let Emit::Send { seq, talkspurt_start } = s.on_frame() {
                let f = if talkspurt_start { Frame::spurt_start(seq, 40) }
                        else { Frame::new(seq, 40) };
                r.push(f);
            }
        }
        s.set_transmitting(false);
        r.tick();                       // play one, three still queued

        s.set_transmitting(true);
        if let Emit::Send { seq, .. } = s.on_frame() { r.push(Frame::spurt_start(seq, 40)); }
        assert_eq!(r.depth(), 1, "the old spurt's leftovers are gone");
    }

    #[test]
    fn gated_frames_never_reach_the_receiver() {
        let (mut s, mut r) = (VoiceSender::default(), PeerStream::default());
        for _ in 0..5 {
            assert_eq!(s.on_frame(), Emit::Gated);
        }
        assert_eq!(r.depth(), 0);
    }
}
