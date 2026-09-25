//! codec.rs — Opus encode/decode for group voice. FEATURE: `opus`.
//!
//! A thin opaque-handle wrapper over `audiopus` (libopus). It is FEATURE-GATED
//! and OFF by default: audiopus links a system libopus (libopus-dev / opus-dev)
//! or builds a vendored copy (needs autotools + cmake), so default `cargo test`
//! stays buildable on machines without it. Build the real thing with
//! `--features opus` (and have libopus available).
//!
//! NOTE: this module is written against the audiopus 0.2 API but is compiled
//! only when the feature is on, so it is validated on the device build, not in
//! the headless test sandbox.
//!
//! Frame sizing is the caller's responsibility: Opus wants a whole frame
//! (2.5/5/10/20/40/60 ms) of mono i16 PCM per `encode`, which is exactly what
//! the CaptureFramer produces. On decode, pass the encoded packet, or NULL/0 to
//! run packet-loss concealment for a frame the jitter buffer reported missing.

use audiopus::coder::{Decoder, Encoder};
use audiopus::{Application, Bandwidth, Bitrate, Channels, SampleRate, Signal};
use std::os::raw::c_int;

fn map_sample_rate(hz: u32) -> Option<SampleRate> {
    match hz {
        8000 => Some(SampleRate::Hz8000),
        12000 => Some(SampleRate::Hz12000),
        16000 => Some(SampleRate::Hz16000),
        24000 => Some(SampleRate::Hz24000),
        48000 => Some(SampleRate::Hz48000),
        _ => None,
    }
}

fn map_channels(n: u32) -> Option<Channels> {
    match n {
        1 => Some(Channels::Mono),
        2 => Some(Channels::Stereo),
        _ => None,
    }
}

/// Opaque Opus encoder handle.
pub struct OpusEncoder {
    enc: Encoder,
}

/// Opaque Opus decoder handle.
pub struct OpusDecoder {
    dec: Decoder,
}

/// Create a VOIP-mode Opus encoder. `bitrate` in bits/s (e.g. 5000 for ~5 kbps);
/// pass <= 0 to leave the Opus default. Returns null on an unsupported rate /
/// channel count or an Opus init error. Free with `sc_opus_encoder_free`.
#[no_mangle]
pub extern "C" fn sc_opus_encoder_new(
    sample_rate: u32,
    channels: u32,
    bitrate: i32,
) -> *mut OpusEncoder {
    let (sr, ch) = match (map_sample_rate(sample_rate), map_channels(channels)) {
        (Some(sr), Some(ch)) => (sr, ch),
        _ => return std::ptr::null_mut(),
    };
    let mut enc = match Encoder::new(sr, ch, Application::Voip) {
        Ok(e) => e,
        Err(_) => return std::ptr::null_mut(),
    };
    if bitrate > 0 {
        let _ = enc.set_bitrate(Bitrate::BitsPerSecond(bitrate));
    }
    Box::into_raw(Box::new(OpusEncoder { enc }))
}

/// Apply quality / efficiency settings to an existing encoder. Best-effort:
/// each setting is applied independently and individual failures are ignored
/// (matching `set_bitrate` in the constructor). Out-of-range values leave that
/// setting unchanged.
///
/// * `bitrate_bps`      target bitrate, bits/s (<= 0 leaves current)
/// * `complexity`       0..10, lower = faster / less CPU (other leaves current)
/// * `max_bandwidth`    0=NB 1=MB 2=WB 3=SWB 4=FB (other leaves current). At a
///                      16 kHz capture the real ceiling is WB regardless.
/// * `dtx`              1 = discontinuous transmission (near-zero on silence)
/// * `inband_fec`       1 = in-band forward error correction (loss-robust)
/// * `packet_loss_pct`  0..100, expected loss to tune FEC against
///
/// # Safety
/// `handle` from `sc_opus_encoder_new`, not already freed.
#[no_mangle]
pub unsafe extern "C" fn sc_opus_encoder_apply_quality(
    handle: *mut OpusEncoder,
    bitrate_bps: i32,
    complexity: i32,
    max_bandwidth: i32,
    dtx: i32,
    inband_fec: i32,
    packet_loss_pct: i32,
) {
    if handle.is_null() {
        return;
    }
    let enc = &mut (*handle).enc;
    // We always carry speech; hint the encoder so it favours SILK/VOIP tuning.
    let _ = enc.set_signal(Signal::Voice);
    if bitrate_bps > 0 {
        let _ = enc.set_bitrate(Bitrate::BitsPerSecond(bitrate_bps));
    }
    if (0..=10).contains(&complexity) {
        let _ = enc.set_complexity(complexity as u8);
    }
    let bw = match max_bandwidth {
        0 => Some(Bandwidth::Narrowband),
        1 => Some(Bandwidth::Mediumband),
        2 => Some(Bandwidth::Wideband),
        3 => Some(Bandwidth::Superwideband),
        4 => Some(Bandwidth::Fullband),
        _ => None,
    };
    if let Some(b) = bw {
        let _ = enc.set_max_bandwidth(b);
    }
    // audiopus 0.2 has no typed DTX setter — use the raw CTL.
    // OPUS_SET_DTX_REQUEST = 4016 (stable libopus ABI).
    let _ = enc.set_encoder_ctl_request(4016, if dtx != 0 { 1 } else { 0 });
    let _ = enc.set_inband_fec(inband_fec != 0);
    if (0..=100).contains(&packet_loss_pct) {
        let _ = enc.set_packet_loss_perc(packet_loss_pct as u8);
    }
}

/// Free an encoder.
///
/// # Safety
/// `handle` from `sc_opus_encoder_new`, not already freed.
#[no_mangle]
pub unsafe extern "C" fn sc_opus_encoder_free(handle: *mut OpusEncoder) {
    if !handle.is_null() {
        drop(Box::from_raw(handle));
    }
}

/// Encode one frame of mono i16 PCM (`pcm_len` samples) into `out` (capacity
/// `out_cap` bytes). Returns the number of bytes written, or -1 on error.
///
/// # Safety
/// `pcm` valid for `pcm_len` i16; `out` writable for `out_cap` bytes; `handle`
/// from `sc_opus_encoder_new`.
#[no_mangle]
pub unsafe extern "C" fn sc_opus_encode(
    handle: *mut OpusEncoder,
    pcm: *const i16,
    pcm_len: usize,
    out: *mut u8,
    out_cap: usize,
) -> c_int {
    if handle.is_null() || pcm.is_null() || out.is_null() {
        return -1;
    }
    let input = std::slice::from_raw_parts(pcm, pcm_len);
    let output = std::slice::from_raw_parts_mut(out, out_cap);
    match (*handle).enc.encode(input, output) {
        Ok(n) => n as c_int,
        Err(_) => -1,
    }
}

/// Create an Opus decoder. Returns null on an unsupported rate / channel count
/// or an Opus init error. Free with `sc_opus_decoder_free`.
#[no_mangle]
pub extern "C" fn sc_opus_decoder_new(sample_rate: u32, channels: u32) -> *mut OpusDecoder {
    let (sr, ch) = match (map_sample_rate(sample_rate), map_channels(channels)) {
        (Some(sr), Some(ch)) => (sr, ch),
        _ => return std::ptr::null_mut(),
    };
    match Decoder::new(sr, ch) {
        Ok(d) => Box::into_raw(Box::new(OpusDecoder { dec: d })),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Free a decoder.
///
/// # Safety
/// `handle` from `sc_opus_decoder_new`, not already freed.
#[no_mangle]
pub unsafe extern "C" fn sc_opus_decoder_free(handle: *mut OpusDecoder) {
    if !handle.is_null() {
        drop(Box::from_raw(handle));
    }
}

/// Decode one frame into `out` mono i16 PCM (capacity `out_cap` samples).
/// Returns samples written, or -1 on error. If `data` is null or `data_len` is
/// 0, runs packet-loss concealment (for a frame the jitter buffer reported
/// missing). `fec` requests in-band forward-error-correction recovery.
///
/// # Safety
/// `data` valid for `data_len` bytes (or null); `out` writable for `out_cap`
/// i16; `handle` from `sc_opus_decoder_new`.
#[no_mangle]
pub unsafe extern "C" fn sc_opus_decode(
    handle: *mut OpusDecoder,
    data: *const u8,
    data_len: usize,
    out: *mut i16,
    out_cap: usize,
    fec: c_int,
) -> c_int {
    if handle.is_null() || out.is_null() {
        return -1;
    }
    let output = std::slice::from_raw_parts_mut(out, out_cap);
    let result = if data.is_null() || data_len == 0 {
        // packet-loss concealment
        (*handle).dec.decode(None::<&[u8]>, output, fec != 0)
    } else {
        let input = std::slice::from_raw_parts(data, data_len);
        (*handle).dec.decode(Some(input), output, fec != 0)
    };
    match result {
        Ok(n) => n as c_int,
        Err(_) => -1,
    }
}

// ── The same encoder and decoder for Rust callers ───────────────────────────
//
// The functions above are the C ABI the Qt app links. A Rust app (snapclone-egui)
// wants owned values, `Option` for failure and no raw pointers; these wrap the
// same audiopus handles, so both front doors run one implementation.

/// Opus frame durations, in tenths of a millisecond: 2.5, 5, 10, 20, 40, 60 ms.
const OPUS_FRAME_TENTHS_MS: [u32; 6] = [25, 50, 100, 200, 400, 600];

/// Is `samples` (per channel) one whole Opus frame at `sample_rate`? Opus
/// refuses every other length, so a caller whose frame size is a user setting
/// must check and fall back rather than fail every frame.
pub fn is_opus_frame(sample_rate: u32, samples: usize) -> bool {
    OPUS_FRAME_TENTHS_MS.iter().any(|t| sample_rate as usize * *t as usize == samples * 10_000)
}

/// The loss rate the encoder plans for (percent): what makes it embed FEC.
pub const EXPECTED_LOSS_PERC: u8 = 10;

/// A speech encoder (VOIP application, voice signal hint, in-band FEC on).
pub struct VoiceEncoder {
    inner: OpusEncoder,
    sample_rate: u32,
}

impl VoiceEncoder {
    /// `bitrate` in bits/s; <= 0 leaves the Opus default. None on an
    /// unsupported rate / channel count or an init error.
    pub fn new(sample_rate: u32, channels: u32, bitrate: i32) -> Option<VoiceEncoder> {
        let (sr, ch) = (map_sample_rate(sample_rate)?, map_channels(channels)?);
        let mut enc = Encoder::new(sr, ch, Application::Voip).ok()?;
        let _ = enc.set_signal(Signal::Voice);
        if bitrate > 0 {
            let _ = enc.set_bitrate(Bitrate::BitsPerSecond(bitrate));
        }
        let _ = enc.set_inband_fec(true);
        // In-band FEC is only EMBEDDED when the encoder expects loss: with the
        // default 0% it spends nothing on it and the receiver has nothing to
        // rebuild from. 10% is the common VoIP setting.
        let _ = enc.set_packet_loss_perc(EXPECTED_LOSS_PERC);
        Some(VoiceEncoder { inner: OpusEncoder { enc }, sample_rate })
    }

    /// One frame of mono i16 PCM to one packet. None if the frame is not an
    /// Opus frame length (`is_opus_frame`) or the encoder fails.
    pub fn encode(&mut self, pcm: &[i16]) -> Option<Vec<u8>> {
        if !is_opus_frame(self.sample_rate, pcm.len()) {
            return None;
        }
        let mut out = [0u8; 1275]; // the largest Opus packet for one frame
        let n = self.inner.enc.encode(pcm, &mut out).ok()?;
        Some(out[..n].to_vec())
    }
}

/// A speech decoder. Keep ONE per remote speaker: Opus is stateful across
/// packets, so feeding two speakers' packets into one decoder smears them.
pub struct VoiceDecoder {
    inner: OpusDecoder,
}

impl VoiceDecoder {
    pub fn new(sample_rate: u32, channels: u32) -> Option<VoiceDecoder> {
        let (sr, ch) = (map_sample_rate(sample_rate)?, map_channels(channels)?);
        Some(VoiceDecoder { inner: OpusDecoder { dec: Decoder::new(sr, ch).ok()? } })
    }

    /// Rebuild a LOST frame from the packet that followed it: Opus in-band FEC
    /// (a low-bitrate copy of the previous frame inside each packet). Better
    /// than `decode(None, ..)`'s extrapolation when that next packet is already
    /// here. Call it for the lost frame, then `decode(Some(next))` as usual.
    pub fn recover(&mut self, next_packet: &[u8], frame_samples: usize) -> Option<Vec<i16>> {
        if next_packet.is_empty() {
            return None;
        }
        let mut out = vec![0i16; frame_samples.max(1)];
        let n = self.inner.dec.decode(Some(next_packet), &mut out[..], true).ok()?;
        out.truncate(n);
        Some(out)
    }

    /// One packet to `frame_samples` of mono i16 PCM. `None` for `packet`
    /// conceals a LOST frame (packet-loss concealment); an empty or corrupt
    /// packet returns None rather than noise.
    pub fn decode(&mut self, packet: Option<&[u8]>, frame_samples: usize) -> Option<Vec<i16>> {
        let mut out = vec![0i16; frame_samples.max(1)];
        let n = match packet {
            Some(p) if p.is_empty() => return None,
            Some(p) => self.inner.dec.decode(Some(p), &mut out[..], false).ok()?,
            None => self.inner.dec.decode(None::<&[u8]>, &mut out[..], false).ok()?,
        };
        out.truncate(n);
        Some(out)
    }
}

#[cfg(test)]
mod voice_tests {
    use super::*;

    const RATE: u32 = 16_000;
    const FRAME: usize = 320; // 20 ms

    fn tone(frames: usize) -> Vec<Vec<i16>> {
        (0..frames).map(|f| (0..FRAME).map(|i| {
            let t = (f * FRAME + i) as f32 / RATE as f32;
            ((t * 440.0 * std::f32::consts::TAU).sin() * 0.3 * i16::MAX as f32
             + (t * 1300.0 * std::f32::consts::TAU).sin() * 0.1 * i16::MAX as f32) as i16
        }).collect()).collect()
    }
    fn rms(x: &[i16]) -> f32 { (x.iter().map(|&s| (s as f32).powi(2)).sum::<f32>() / x.len().max(1) as f32).sqrt() }

    #[test]
    fn frame_lengths_opus_accepts() {
        for (ms10, ok) in [(25u32, true), (50, true), (100, true), (200, true), (400, true), (600, true), (150, false), (300, false), (1000, false)] {
            let samples = RATE as usize * ms10 as usize / 10_000;
            assert_eq!(is_opus_frame(RATE, samples), ok, "{} ms", ms10 as f32 / 10.0);
        }
    }

    #[test]
    fn speech_band_audio_round_trips_at_a_fraction_of_the_raw_size() {
        let mut enc = VoiceEncoder::new(RATE, 1, 24_000).expect("encoder");
        let mut dec = VoiceDecoder::new(RATE, 1).expect("decoder");
        let frames = tone(50); // 1 s
        let (mut bytes, mut out) = (0usize, Vec::new());
        for f in &frames {
            let p = enc.encode(f).expect("a 20 ms frame encodes");
            bytes += p.len();
            out.extend(dec.decode(Some(&p), FRAME).expect("decodes"));
        }
        assert_eq!(out.len(), 50 * FRAME);
        let raw = 50 * FRAME * 2;
        assert!(bytes * 8 < raw, "Opus {bytes} B/s vs raw {raw} B/s");
        // Past the codec's start-up delay the level is the input's.
        let (a, b) = (rms(&frames.concat()[4000..]), rms(&out[4000..]));
        assert!((b / a - 1.0).abs() < 0.35, "level in {a:.0}, out {b:.0}");
    }

    /// Against what a decoder that got EVERY packet produced, a frame rebuilt
    /// from the next packet's FEC is closer than one extrapolated by PLC.
    #[test]
    fn fec_rebuilds_a_lost_frame_better_than_concealment_guesses_it() {
        let frames = tone(20);
        let mut enc = VoiceEncoder::new(RATE, 1, 24_000).unwrap();
        let packets: Vec<Vec<u8>> = frames.iter().map(|f| enc.encode(f).unwrap()).collect();
        let lost = 10;
        let mut reference = VoiceDecoder::new(RATE, 1).unwrap();
        let heard: Vec<Vec<i16>> = packets.iter().map(|p| reference.decode(Some(p), FRAME).unwrap()).collect();
        let err = |a: &[i16], b: &[i16]| (a.iter().zip(b).map(|(x, y)| ((*x as f32) - (*y as f32)).powi(2)).sum::<f32>() / a.len() as f32).sqrt();
        let run = |fec: bool| {
            let mut d = VoiceDecoder::new(RATE, 1).unwrap();
            for p in &packets[..lost] { d.decode(Some(p), FRAME).unwrap(); }
            let slot = if fec { d.recover(&packets[lost + 1], FRAME).unwrap() } else { d.decode(None, FRAME).unwrap() };
            assert_eq!(slot.len(), FRAME);
            err(&slot, &heard[lost])
        };
        let (with_fec, plc) = (run(true), run(false));
        assert!(with_fec < plc * 0.8, "FEC error {with_fec:.0} vs PLC {plc:.0} (against the full-decode reference)");
        assert!(VoiceDecoder::new(RATE, 1).unwrap().recover(&[], FRAME).is_none());
    }

    #[test]
    fn a_lost_frame_is_concealed_and_bad_input_is_refused() {
        let mut enc = VoiceEncoder::new(RATE, 1, 24_000).unwrap();
        let mut dec = VoiceDecoder::new(RATE, 1).unwrap();
        for f in tone(10) { let p = enc.encode(&f).unwrap(); dec.decode(Some(&p), FRAME).unwrap(); }
        let concealed = dec.decode(None, FRAME).expect("concealment");
        assert_eq!(concealed.len(), FRAME);
        assert!(rms(&concealed) > 100.0, "concealment continues the sound rather than dropping to silence");
        assert!(enc.encode(&vec![0i16; 300]).is_none(), "not an Opus frame length");
        assert!(dec.decode(Some(&[]), FRAME).is_none(), "an empty packet is not a frame");
        assert!(VoiceEncoder::new(44_100, 1, 0).is_none(), "44.1 kHz is not an Opus rate");
    }
}
