//! voice — frame-level Opus for the in-house call.
//!
//! [`crate::encode_opus`] and the `.opus` decoder work on whole files:
//! an Ogg container, pre-skip, a final granule. A call has none of that
//! — one 20 ms frame at a time, each on its own, the container's job done
//! by the wire's sequence numbers instead. So this is the codec at its
//! narrowest: [`VoiceEncoder::encode`] takes exactly [`FRAME`] samples
//! and returns the packet bytes; [`VoiceDecoder::decode`] takes a packet
//! (or `None` for a lost one) and fills exactly [`FRAME`] samples.
//!
//! Nothing here knows about the link, the jitter buffer or the sink;
//! `studio-collab`'s `voice::Codec` is the trait this is adapted to, in
//! the shell, so this crate stays what it is — codecs, not calls.
//!
//! Choices, and why:
//! * **VOIP application, 24 kbit/s VBR.** Opus's speech mode at a rate
//!   that survives a phone's uplink; music mode would spend bits on the
//!   room instead of the words.
//! * **Concealment is Opus's own.** A `None` packet reaches libopus as a
//!   null pointer, which is its documented request for packet-loss
//!   concealment: it extrapolates the last frame rather than dropping
//!   to silence. The tests pin that concealment is bounded and finite,
//!   not what it sounds like.
//! * **DTX is on.** Discontinuous transmission: during silence the
//!   encoder hands back a one-byte packet per frame and a ~25-byte
//!   comfort-noise refresh every 400 ms. This module returns those bytes
//!   as they are; the link sends them (a byte per frame keeps the far
//!   side's sequence and clock honest, and the decoder reads a one-byte
//!   packet as "keep the comfort noise going"). Without it, silence cost 41 bytes a frame — 16
//!   kbit/s for a muted microphone, which on a phone's uplink is not
//!   nothing. Measured, and the test pins the ratio.
//! * **In-band FEC is off.** It only pays when the decoder is handed the
//!   NEXT packet to recover the missing one from, which the link's
//!   playout does not do yet; paying the bitrate for it now would be
//!   paying for nothing. Named here so the day the link does, this is
//!   the line to change.
//! * **Errors are silence, never a panic.** An encode that fails returns
//!   an empty packet (the link sends it; the far side conceals); a decode
//!   that fails writes zeros. The audio path must not unwind.

/// Opus's native rate, and the call's.
pub const RATE: u32 = 48_000;
/// 20 ms at [`RATE`]. The one frame length this module speaks.
pub const FRAME: usize = 960;
/// Largest packet Opus will produce for one frame at any bitrate.
const MAX_PACKET: usize = 1275;

/// One direction out: mono 48 kHz frames in, Opus packets out.
pub struct VoiceEncoder {
    enc: opus::Encoder,
    buf: Vec<u8>,
}

impl VoiceEncoder {
    /// `bitrate_bps` — 24_000 is the intended default; the parameter
    /// exists so a metered link can be asked for less.
    pub fn new(bitrate_bps: i32) -> Result<VoiceEncoder, String> {
        let mut enc = opus::Encoder::new(RATE, opus::Channels::Mono, opus::Application::Voip)
            .map_err(|e| format!("opus encoder: {e}"))?;
        enc.set_bitrate(opus::Bitrate::Bits(bitrate_bps.max(6_000)))
            .map_err(|e| format!("opus bitrate: {e}"))?;
        enc.set_vbr(true).map_err(|e| format!("opus vbr: {e}"))?;
        enc.set_dtx(true).map_err(|e| format!("opus dtx: {e}"))?;
        Ok(VoiceEncoder { enc, buf: vec![0u8; MAX_PACKET] })
    }

    /// `pcm.len()` must be [`FRAME`]; anything else (or an encoder error)
    /// returns an empty packet rather than panicking on the audio path.
    pub fn encode(&mut self, pcm: &[f32]) -> Vec<u8> {
        if pcm.len() != FRAME { return Vec::new(); }
        match self.enc.encode_float(pcm, &mut self.buf) {
            Ok(n) => self.buf[..n].to_vec(),
            Err(_) => Vec::new(),
        }
    }
}

/// One peer in: Opus packets (or their absence) in, mono 48 kHz frames out.
pub struct VoiceDecoder {
    dec: opus::Decoder,
}

impl VoiceDecoder {
    pub fn new() -> Result<VoiceDecoder, String> {
        let dec = opus::Decoder::new(RATE, opus::Channels::Mono)
            .map_err(|e| format!("opus decoder: {e}"))?;
        Ok(VoiceDecoder { dec })
    }

    /// Fill `out` (`out.len() == FRAME`) from `packet`, or conceal a lost
    /// one when `packet` is `None`. A packet that fails to decode, an
    /// empty packet, or a wrong-length `out` yields silence.
    pub fn decode(&mut self, packet: Option<&[u8]>, out: &mut [f32]) {
        if out.len() != FRAME { out.fill(0.0); return; }
        let input: &[u8] = match packet {
            Some(p) if !p.is_empty() => p,
            // Empty slice => null pointer inside the crate => libopus PLC.
            _ => &[],
        };
        match self.dec.decode_float(input, out, false) {
            Ok(n) if n == FRAME => {}
            Ok(n) => {
                // A packet of another duration: keep what came, silence
                // the rest — never leave the previous frame's samples.
                for o in out.iter_mut().skip(n) { *o = 0.0; }
            }
            Err(_) => out.fill(0.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(n: usize, hz: f32, amp: f32) -> Vec<f32> {
        (0..n).map(|i| amp * (2.0 * std::f32::consts::PI * hz * i as f32 / RATE as f32).sin())
            .collect()
    }

    /// Peak normalized cross-correlation over lags 0..max_lag — a codec
    /// is allowed a few samples of delay, not a different signal.
    fn best_corr(a: &[f32], b: &[f32], max_lag: usize) -> f32 {
        let mut best = 0.0f32;
        for lag in 0..max_lag {
            let n = a.len().min(b.len() - lag);
            let (mut xy, mut xx, mut yy) = (0.0f32, 0.0f32, 0.0f32);
            for i in 0..n {
                let (x, y) = (a[i], b[i + lag]);
                xy += x * y; xx += x * x; yy += y * y;
            }
            let c = xy / (xx.sqrt() * yy.sqrt()).max(1e-9);
            if c > best { best = c; }
        }
        best
    }

    #[test]
    fn a_tone_survives_the_round_trip_after_the_codec_warms_up() {
        let mut enc = VoiceEncoder::new(24_000).unwrap();
        let mut dec = VoiceDecoder::new().unwrap();
        let tone = sine(FRAME * 12, 440.0, 0.5);
        let mut back = Vec::with_capacity(tone.len());
        let mut sizes = Vec::new();
        for f in tone.chunks_exact(FRAME) {
            let pkt = enc.encode(f);
            assert!(!pkt.is_empty(), "a frame encodes to something");
            sizes.push(pkt.len());
            let mut out = vec![0.0; FRAME];
            dec.decode(Some(&pkt), &mut out);
            back.extend_from_slice(&out);
        }
        // Skip the first two frames: pre-echo and the encoder's lookahead
        // make them the codec's, not the tone's. Then the shape must match
        // within a few ms of delay.
        let from = FRAME * 2;
        let c = best_corr(&tone[from..tone.len() - 480], &back[from..], 480);
        assert!(c > 0.9, "decoded tone correlates {c} with the input");
        assert!(back.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
        // 24 kbit/s VBR: 60 bytes per 20 ms is the nominal; a voiced frame
        // sits near it, and never near the 1275-byte ceiling.
        let avg = sizes.iter().sum::<usize>() as f32 / sizes.len() as f32;
        assert!((20.0..=120.0).contains(&avg), "average packet {avg} bytes");
    }

    #[test]
    fn silence_costs_almost_nothing_on_the_wire() {
        // The link keeps sending while muted; this is what that costs.
        let mut enc = VoiceEncoder::new(24_000).unwrap();
        let mut tone_bytes = 0;
        for f in sine(FRAME * 4, 300.0, 0.8).chunks_exact(FRAME) { tone_bytes += enc.encode(f).len(); }
        // DTX needs a run of silence to engage (it hangs over for a few
        // frames after speech), so measure the second second, not the
        // first frames.
        let mut sizes = Vec::new();
        for _ in 0..100 { sizes.push(enc.encode(&vec![0.0; FRAME]).len()); }
        let silent_bytes: usize = sizes[50..].iter().sum();
        assert!(silent_bytes * 10 < tone_bytes * 50 / 4,
                "50 silent frames ({silent_bytes} B) should be a tenth of 50 voiced ones ({} B)",
                tone_bytes * 50 / 4);
        assert!(sizes[50..].iter().all(|&n| n > 0),
                "and every frame is still a packet, so the far side's clock runs: {:?}", &sizes[50..60]);
        // Measured: a 1-byte frame, with a ~25-byte comfort-noise refresh
        // every 400 ms (every 20th frame). Pin both halves.
        let tiny = sizes[50..].iter().filter(|&&n| n <= 2).count();
        assert!(tiny >= 45, "DTX: nearly every frame is a byte or two: {:?}", &sizes[50..70]);
        assert!(sizes[50..].iter().all(|&n| n <= 60), "and the refresh is small: {:?}", &sizes[50..70]);
    }

    #[test]
    fn concealment_is_bounded_finite_and_decays_rather_than_stalls() {
        let mut enc = VoiceEncoder::new(24_000).unwrap();
        let mut dec = VoiceDecoder::new().unwrap();
        for f in sine(FRAME * 6, 440.0, 0.6).chunks_exact(FRAME) {
            let pkt = enc.encode(f);
            let mut out = vec![0.0; FRAME];
            dec.decode(Some(&pkt), &mut out);
        }
        let mut peaks = Vec::new();
        for _ in 0..10 {
            let mut out = vec![9.0; FRAME];
            dec.decode(None, &mut out);
            assert!(out.iter().all(|x| x.is_finite() && x.abs() <= 1.0), "PLC output stays in range");
            peaks.push(out.iter().fold(0.0f32, |m, x| m.max(x.abs())));
        }
        assert!(peaks[0] > 0.05, "PLC extrapolates the tone at first: peak {}", peaks[0]);
        assert!(peaks[9] < peaks[0], "and fades over 200 ms rather than looping forever: {peaks:?}");
    }

    #[test]
    fn the_error_paths_are_silence_not_panics() {
        let mut enc = VoiceEncoder::new(24_000).unwrap();
        assert!(enc.encode(&[0.1; 100]).is_empty(), "wrong frame length: empty packet");
        assert!(enc.encode(&[]).is_empty());
        let mut dec = VoiceDecoder::new().unwrap();
        let mut out = vec![1.0; FRAME];
        dec.decode(Some(&[0xff; 7]), &mut out);
        assert!(out.iter().all(|&x| x.abs() <= 1.0), "garbage decodes to something in range or silence");
        let mut short = vec![1.0; 10];
        dec.decode(Some(&[1, 2, 3]), &mut short);
        assert!(short.iter().all(|&x| x == 0.0), "wrong out length: zeros");
        let mut out = vec![1.0; FRAME];
        dec.decode(Some(&[]), &mut out);
        assert!(out.iter().all(|x| x.is_finite()), "empty packet = conceal, never unwinds");
        assert!(VoiceEncoder::new(1).is_ok(), "an absurd bitrate is floored, not refused");
    }
}
