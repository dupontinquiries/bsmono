//! voice_rx — arriving Opus voice, played in order, with gaps concealed.
//!
//! The pieces existed and did not meet: `voice_playout` (the ported jitter
//! scheduler: per-sender ordering, drop-late, talk-spurt resets, capped
//! concealment) was started and stopped by the app but never fed, and decoded
//! audio went straight to the sink in ARRIVAL order -- a reordered packet played
//! out of order, a lost one was simply a hole. The scheduler works on sequence
//! numbers only ("the host owns the decoder"), so this is the assembly:
//!
//! - `arrive`: keep the payload under (peer, seq) and give the scheduler the
//!   frame's metadata; a frame it refuses (older than the play-out point, or a
//!   duplicate) has its payload dropped at once, so the store stays bounded;
//! - `tick`, once per frame interval: for each speaker, `Play { seq }` decodes
//!   the stored payload, `Conceal` runs Opus packet-loss concealment from their
//!   last good frame, `Silence` gives nothing.
//!
//! Raw S16 frames from older peers carry no sequence number and keep the
//! immediate path (`voice_codec::decode`); only `Packet::VoiceOpus` comes here.

use crate::voice_codec::{Codec, VoiceCodec};
use crate::voice_playout::{Frame, Playout, VoicePlayout};
use std::collections::{BTreeMap, HashMap};

pub struct VoiceRx {
    codec: VoiceCodec,
    playout: VoicePlayout,
    store: HashMap<String, BTreeMap<u64, Vec<u8>>>,
}

impl VoiceRx {
    pub fn new(sample_rate: u32, frame_samples: usize) -> VoiceRx {
        let mut playout = VoicePlayout::default();
        playout.start();
        VoiceRx { codec: VoiceCodec::new(sample_rate, frame_samples), playout, store: HashMap::new() }
    }

    /// One Opus packet from `peer`.
    pub fn arrive(&mut self, peer: &str, seq: u32, spurt_start: bool, data: Vec<u8>) {
        let seq = seq as u64;
        let frame = if spurt_start { Frame::spurt_start(seq, data.len()) } else { Frame::new(seq, data.len()) };
        let store = self.store.entry(peer.to_string()).or_default();
        if spurt_start {
            // A new spurt restarts sequence numbers: whatever is stored belongs
            // to the previous one (the scheduler discards those frames too).
            store.clear();
        }
        if self.playout.on_frame(peer, frame) {
            store.insert(seq, data);
        }
    }

    /// One play-out interval: `(speaker, samples)` for each speaker with
    /// something to play or conceal.
    pub fn tick(&mut self) -> Vec<(String, Vec<f32>)> {
        let mut out = Vec::new();
        for (peer, decision) in self.playout.tick() {
            let samples = match decision {
                Playout::Play { seq, .. } => {
                    let store = self.store.entry(peer.clone()).or_default();
                    let bytes = store.remove(&seq);
                    // Anything older than what just played can never play.
                    let stale: Vec<u64> = store.range(..seq).map(|(k, _)| *k).collect();
                    for k in stale { store.remove(&k); }
                    bytes.and_then(|b| self.codec.decode(&peer, Codec::Opus, &b))
                }
                Playout::Conceal => self.codec.conceal(&peer),
                Playout::Silence => None,
            };
            if let Some(s) = samples { out.push((peer, s)); }
        }
        out
    }

    /// A speaker left: drop their buffer, payloads and decoder.
    pub fn forget(&mut self, peer: &str) {
        self.store.remove(peer);
        self.codec.forget(peer);
        self.playout.retain_senders(|s| s != peer);
    }

    /// Payloads held for `peer` (bounded; for tests and diagnostics).
    pub fn held(&self, peer: &str) -> usize { self.store.get(peer).map_or(0, |s| s.len()) }
}

#[cfg(all(test, feature = "opus"))]
mod tests {
    use super::*;
    use crate::sink_cushion::{frame_samples, SAMPLE_RATE};
    use crate::voice_bridge::Packet;

    fn n() -> usize { frame_samples(20) }
    fn rms(x: &[f32]) -> f32 { (x.iter().map(|s| s * s).sum::<f32>() / x.len().max(1) as f32).sqrt() }

    /// `count` Opus packets of a tone whose loudness RISES frame by frame, so
    /// the decoded order is visible in the levels.
    fn ramp(count: usize) -> Vec<Vec<u8>> {
        let mut tx = VoiceCodec::new(SAMPLE_RATE, n());
        (0..count).map(|f| {
            let amp = 0.05 + 0.05 * f as f32;
            let frame: Vec<f32> = (0..n()).map(|i| ((f * n() + i) as f32 / SAMPLE_RATE as f32 * 300.0 * std::f32::consts::TAU).sin() * amp).collect();
            match tx.encode(&frame, f as u64, f == 0) { Packet::VoiceOpus { data, .. } => data, p => panic!("{p:?}") }
        }).collect()
    }

    /// Drive: packets arrive in `order` (index = seq), one per tick, then drain.
    fn run(order: &[usize], packets: &[Vec<u8>], lose: &[usize]) -> Vec<Vec<f32>> {
        let mut rx = VoiceRx::new(SAMPLE_RATE, n());
        let mut heard = Vec::new();
        for (k, &seq) in order.iter().enumerate() {
            if !lose.contains(&seq) { rx.arrive("ann", seq as u32, seq == 0, packets[seq].clone()); }
            if k >= 1 { for (_, s) in rx.tick() { heard.push(s); } }
        }
        for _ in 0..4 { for (_, s) in rx.tick() { heard.push(s); } }
        heard
    }

    #[test]
    fn reordered_packets_play_in_sequence_order() {
        let p = ramp(12);
        // Pairs swapped in transit: 1 0 3 2 5 4 ... (seq 0 still flags the spurt).
        let order: Vec<usize> = (0..12).map(|i| i ^ 1).collect();
        let heard = run(&order, &p, &[]);
        assert!(heard.len() >= 11, "{} frames played", heard.len());
        let levels: Vec<f32> = heard.iter().map(|f| rms(f)).collect();
        // The twelve real frames' slots (after them the drain ticks conceal: the
        // scheduler extrapolates after a talker stops, up to its cap, since
        // nothing marks the end of a spurt). Past the codec's start-up frames,
        // loudness rises frame by frame: sequence order, not arrival order.
        for w in levels[2..12].windows(2) {
            assert!(w[1] > w[0] * 0.95, "levels not in order: {levels:?}");
        }
    }

    #[test]
    fn a_lost_packet_is_concealed_in_its_slot_and_playback_goes_on() {
        let p = ramp(12);
        let order: Vec<usize> = (0..12).collect();
        let full = run(&order, &p, &[]);
        let lossy = run(&order, &p, &[6]);
        assert_eq!(lossy.len(), full.len(), "the lost frame's slot is filled, not skipped");
        assert!(lossy.iter().all(|f| f.len() == n()));
        let slot = lossy.iter().position(|f| rms(f) > 0.0).map(|start| start + 6).unwrap();
        assert!(rms(&lossy[slot]) > 0.3 * rms(&full[slot]), "concealment continues the sound ({:.3} vs {:.3})", rms(&lossy[slot]), rms(&full[slot]));
        // The frame after the gap is the real one again (the last frames of both
        // runs are the post-stream concealment tail, so they are not the test).
        // The real audio after the gap comes back GRADUALLY -- the decoder's
        // state re-converges after concealment (measured: 0.60, 0.71, 0.80 of
        // the true level over the next three frames) -- so the test is a clear
        // return that keeps rising, not an instant one the codec does not give.
        // (Better, later: the encoder sends in-band FEC, so with one packet of
        // lookahead the lost frame could be REBUILT from the next packet rather
        // than extrapolated.)
        let ratio = |k: usize| rms(&lossy[slot + k]) / rms(&full[slot + k]).max(1e-6);
        assert!(ratio(1) > 0.4 && ratio(2) > ratio(1) && ratio(3) > ratio(2) && ratio(3) > 0.7,
                "real audio returns after the gap: {:.2} {:.2} {:.2}", ratio(1), ratio(2), ratio(3));
    }

    #[test]
    fn late_frames_are_not_kept_and_a_new_spurt_is_not_eaten() {
        let p = ramp(10);
        let mut rx = VoiceRx::new(SAMPLE_RATE, n());
        for s in 0..6 { rx.arrive("bo", s, s == 0, p[s as usize].clone()); rx.tick(); }
        for _ in 0..4 { rx.tick(); }
        // Stragglers from long ago: refused, and their payloads not held.
        for s in 0..4 { rx.arrive("bo", s, false, p[s as usize].clone()); }
        assert!(rx.held("bo") <= 2, "late payloads dropped ({} held)", rx.held("bo"));
        // A second spurt restarts at 0 and must play.
        let mut played = 0;
        for s in 0..6 { rx.arrive("bo", s, s == 0, p[s as usize].clone()); played += rx.tick().len(); }
        for _ in 0..3 { played += rx.tick().len(); }
        assert!(played >= 5, "the second spurt played ({played} frames)");
        rx.forget("bo");
        assert_eq!(rx.held("bo"), 0);
    }
}
