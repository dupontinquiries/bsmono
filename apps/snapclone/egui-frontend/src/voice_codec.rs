//! voice_codec — the seam between samples and the wire, now with a codec in it.
//!
//! Until this module a call sent RAW S16LE: `audio::f32_to_pcm` on the way out,
//! `audio::pcm_to_f32` on the way in, ~32 kB/s per peer where Opus needs ~2-3.
//! Fine on a LAN, not on mobile data. Worse, the binary told the host
//! `EncoderReady(true)` while doing it, so the tested "transmitting without a
//! codec" warning (`voice_send::Emit::NoEncoder`) could never fire (C14_TODO
//! section 14, "the app claims a codec it does not have").
//!
//! Now, with the `opus` feature (default on):
//! - OUTGOING: one Opus packet per mic frame (`Packet::VoiceOpus`), from
//!   `snapclone_audio::codec::VoiceEncoder`, when the frame length is one Opus
//!   accepts (2.5/5/10/20/40/60 ms at the capture rate). Any other length, or no
//!   `opus` feature, sends raw (`Packet::VoiceFrame`) as before -- and
//!   `encoder_ready()` says so, so the host's warning is truthful.
//! - INCOMING: raw or Opus, by the packet's kind. Opus is decoded by ONE
//!   decoder PER SPEAKER, made on that speaker's first packet: Opus carries
//!   state from packet to packet, and one decoder fed two speakers smears them.
//!   `forget` drops a speaker's decoder when they leave.
//!
//! Compatibility: a peer on an older build drops `VoiceOpus` (an unknown kind
//! to its `voice_bridge::decode`), so it hears silence from a newer peer, not
//! noise; a newer build plays both kinds.

use crate::voice_bridge::Packet;

/// What the bytes of a voice packet are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    RawS16,
    Opus,
}

/// Bits per second asked of the encoder: wideband speech at 16 kHz.
pub const OPUS_BITRATE: i32 = 24_000;

pub struct VoiceCodec {
    sample_rate: u32,
    frame_samples: usize,
    #[cfg(feature = "opus")]
    encoder: Option<snapclone_audio::codec::VoiceEncoder>,
    #[cfg(feature = "opus")]
    decoders: std::collections::HashMap<String, snapclone_audio::codec::VoiceDecoder>,
    /// Each speaker's last decoded frame length: what concealment makes.
    #[cfg(feature = "opus")]
    last_len: std::collections::HashMap<String, usize>,
}

fn to_i16(frame: &[f32]) -> Vec<i16> {
    frame.iter().map(|&s| (s.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16).collect()
}

fn to_f32(pcm: &[i16]) -> Vec<f32> {
    pcm.iter().map(|&s| s as f32 / i16::MAX as f32).collect()
}

impl VoiceCodec {
    /// For mic frames of `frame_samples` mono samples at `sample_rate`.
    pub fn new(sample_rate: u32, frame_samples: usize) -> VoiceCodec {
        VoiceCodec {
            sample_rate,
            frame_samples,
            #[cfg(feature = "opus")]
            encoder: if snapclone_audio::codec::is_opus_frame(sample_rate, frame_samples) {
                snapclone_audio::codec::VoiceEncoder::new(sample_rate, 1, OPUS_BITRATE)
            } else {
                None
            },
            #[cfg(feature = "opus")]
            decoders: std::collections::HashMap::new(),
            #[cfg(feature = "opus")]
            last_len: std::collections::HashMap::new(),
        }
    }

    /// Is outgoing voice compressed? This is what the host is told
    /// (`HostEvent::EncoderReady`), so "transmitting without a codec" fires
    /// exactly when it is true.
    pub fn encoder_ready(&self) -> bool {
        #[cfg(feature = "opus")]
        { self.encoder.is_some() }
        #[cfg(not(feature = "opus"))]
        { false }
    }

    /// One mic frame to the packet that carries it. `seq` and `spurt_start` are
    /// the sender's (`voice_send::Emit::Send`); an Opus packet carries them for
    /// the receiver's jitter scheduler, a raw one (the old format) cannot.
    pub fn encode(&mut self, frame: &[f32], seq: u64, spurt_start: bool) -> Packet {
        #[cfg(feature = "opus")]
        if let Some(enc) = self.encoder.as_mut() {
            if frame.len() == self.frame_samples {
                if let Some(p) = enc.encode(&to_i16(frame)) {
                    return Packet::VoiceOpus { seq: seq as u32, spurt_start, data: p };
                }
            }
        }
        let _ = (self.frame_samples, seq, spurt_start);
        Packet::VoiceFrame(crate::audio::f32_to_pcm(frame))
    }

    /// A speaker's packet to samples. None for a payload that is not a whole
    /// frame of its codec (a half-sample of raw, a corrupt or empty Opus
    /// packet), or for Opus in a build without the codec -- refused rather than
    /// played as noise.
    pub fn decode(&mut self, peer: &str, codec: Codec, bytes: &[u8]) -> Option<Vec<f32>> {
        match codec {
            Codec::RawS16 => {
                if bytes.is_empty() || bytes.len() % 2 != 0 { return None; }
                Some(crate::audio::pcm_to_f32(bytes))
            }
            Codec::Opus => {
                #[cfg(feature = "opus")]
                {
                    if !self.decoders.contains_key(peer) {
                        let d = snapclone_audio::codec::VoiceDecoder::new(self.sample_rate, 1)?;
                        self.decoders.insert(peer.to_string(), d);
                    }
                    let d = self.decoders.get_mut(peer)?;
                    // Up to 60 ms: the longest frame a sender may have chosen.
                    let max = self.sample_rate as usize * 60 / 1000;
                    let out = d.decode(Some(bytes), max).map(|s| to_f32(&s));
                    if let Some(o) = &out { self.last_len.insert(peer.to_string(), o.len()); }
                    return out;
                }
                #[cfg(not(feature = "opus"))]
                {
                    let _ = (peer, bytes);
                    None
                }
            }
        }
    }

    /// A frame this speaker should have sent is missing: Opus packet-loss
    /// concealment extrapolates one from their last good frame (a slight
    /// smear, where silence would sound like they stopped). None if nothing
    /// has been heard from them yet, or without the codec.
    pub fn conceal(&mut self, peer: &str) -> Option<Vec<f32>> {
        #[cfg(feature = "opus")]
        {
            let n = *self.last_len.get(peer)?;
            return self.decoders.get_mut(peer)?.decode(None, n).map(|s| to_f32(&s));
        }
        #[cfg(not(feature = "opus"))]
        {
            let _ = peer;
            None
        }
    }

    /// The call ended: every speaker's decoder state goes with it.
    pub fn reset(&mut self) {
        *self = VoiceCodec::new(self.sample_rate, self.frame_samples);
    }

    /// A speaker left: drop their decoder state.
    pub fn forget(&mut self, peer: &str) {
        #[cfg(feature = "opus")]
        { self.decoders.remove(peer); self.last_len.remove(peer); }
        let _ = peer;
    }

    /// How many speakers have decoder state (for tests and diagnostics).
    pub fn speakers(&self) -> usize {
        #[cfg(feature = "opus")]
        { self.decoders.len() }
        #[cfg(not(feature = "opus"))]
        { 0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sink_cushion::{frame_samples, SAMPLE_RATE};

    fn tone(freq: f32, frames: usize, n: usize) -> Vec<Vec<f32>> {
        (0..frames).map(|f| (0..n).map(|i| ((f * n + i) as f32 / SAMPLE_RATE as f32 * freq * std::f32::consts::TAU).sin() * 0.4).collect()).collect()
    }
    fn rms(x: &[f32]) -> f32 { (x.iter().map(|s| s * s).sum::<f32>() / x.len().max(1) as f32).sqrt() }

    #[test]
    fn raw_frames_still_decode_and_half_samples_are_refused() {
        let mut c = VoiceCodec::new(SAMPLE_RATE, frame_samples(20));
        let raw = crate::audio::f32_to_pcm(&[0.5, -0.5]);
        assert_eq!(c.decode("a", Codec::RawS16, &raw).map(|v| v.len()), Some(2));
        assert!(c.decode("a", Codec::RawS16, &[1, 2, 3]).is_none());
        assert!(c.decode("a", Codec::RawS16, &[]).is_none());
    }

    #[cfg(feature = "opus")]
    #[test]
    fn with_opus_a_call_sends_compressed_packets_and_says_so() {
        let n = frame_samples(20);
        let mut tx = VoiceCodec::new(SAMPLE_RATE, n);
        let mut rx = VoiceCodec::new(SAMPLE_RATE, n);
        assert!(tx.encoder_ready(), "20 ms at 16 kHz is an Opus frame");
        let (mut bytes, mut heard) = (0usize, Vec::new());
        for f in tone(330.0, 50, n) {
            let p = tx.encode(&f, 0, false);
            let Packet::VoiceOpus { data: b, .. } = &p else { panic!("expected Opus, got {p:?}") };
            bytes += b.len();
            // Through the real wire encoding, as a peer would receive it.
            let back = crate::voice_bridge::decode(&crate::voice_bridge::encode(&p)).unwrap();
            let Packet::VoiceOpus { data: b2, .. } = back else { panic!() };
            heard.extend(rx.decode("alice", Codec::Opus, &b2).expect("decodes"));
        }
        assert_eq!(heard.len(), 50 * n);
        let raw = 50 * n * 2;
        assert!(bytes * 8 < raw, "{bytes} B for 1 s of speech vs {raw} B raw");
        let level = rms(&heard[4000..]) / rms(&tone(330.0, 50, n).concat()[4000..]);
        assert!((level - 1.0).abs() < 0.35, "level ratio {level:.2}");
    }

    #[cfg(feature = "opus")]
    #[test]
    fn each_speaker_has_their_own_decoder_and_leaving_drops_it() {
        let n = frame_samples(20);
        let mut a = VoiceCodec::new(SAMPLE_RATE, n);
        let mut b = VoiceCodec::new(SAMPLE_RATE, n);
        let mut rx = VoiceCodec::new(SAMPLE_RATE, n);
        let (ta, tb) = (tone(220.0, 30, n), tone(880.0, 30, n));
        let (mut ha, mut hb) = (Vec::new(), Vec::new());
        for i in 0..30 {
            // Interleaved, as two speakers arrive.
            if let Packet::VoiceOpus { data: p, .. } = a.encode(&ta[i], i as u64, i == 0) { ha.extend(rx.decode("a", Codec::Opus, &p).unwrap()); }
            if let Packet::VoiceOpus { data: p, .. } = b.encode(&tb[i], i as u64, i == 0) { hb.extend(rx.decode("b", Codec::Opus, &p).unwrap()); }
        }
        assert_eq!(rx.speakers(), 2);
        // Each decoded stream is its own speaker's tone, not a smear of both:
        // correlate the tail with each source.
        let corr = |x: &[f32], y: &[f32]| { let n = x.len().min(y.len()); let (xs, ys) = (&x[n - 2000..n], &y[n - 2000..n]);
            let best = (0..200).map(|lag| xs[lag..].iter().zip(ys).map(|(p, q)| p * q).sum::<f32>()).fold(0.0, f32::max);
            best / (rms(xs) * rms(ys) * 1800.0).max(1e-6) };
        let (ta_all, tb_all) = (ta.concat(), tb.concat());
        assert!(corr(&ha, &ta_all) > 0.7 && corr(&hb, &tb_all) > 0.7, "a {:.2} b {:.2}", corr(&ha, &ta_all), corr(&hb, &tb_all));
        rx.forget("a");
        assert_eq!(rx.speakers(), 1);
        assert!(rx.decode("a", Codec::Opus, &[]).is_none(), "an empty packet is refused");
    }

    #[cfg(feature = "opus")]
    #[test]
    fn a_frame_length_opus_refuses_falls_back_to_raw_and_says_so() {
        let n = frame_samples(30); // 30 ms: not an Opus frame
        let mut tx = VoiceCodec::new(SAMPLE_RATE, n);
        assert!(!tx.encoder_ready(), "the host must hear 'no encoder'");
        assert!(matches!(tx.encode(&vec![0.1; n], 0, true), Packet::VoiceFrame(_)));
    }

    #[cfg(not(feature = "opus"))]
    #[test]
    fn without_the_codec_it_is_raw_and_honest() {
        let mut tx = VoiceCodec::new(SAMPLE_RATE, frame_samples(20));
        assert!(!tx.encoder_ready());
        assert!(matches!(tx.encode(&vec![0.1; frame_samples(20)], 0, true), Packet::VoiceFrame(_)));
        assert!(tx.decode("a", Codec::Opus, &[1, 2, 3]).is_none(), "Opus without the codec is refused, not played");
    }
}
