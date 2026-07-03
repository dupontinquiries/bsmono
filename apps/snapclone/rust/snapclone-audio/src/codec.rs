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
