//! wire.rs — the on-wire voice packet format.
//!
//! A talk-spurt is a stream of small frames. Each frame is serialized to
//! MessagePack here, then sealed to the group (the existing group seal) and
//! sent. On the far side it is opened, decoded here, and handed (by sequence
//! number) to the jitter buffer. Keeping the format in one place — Rust, with
//! serde — means a single source of truth for both ends.
//!
//! PROTOCOL VERSIONING IS BAKED IN FROM v1: every frame carries `ver`, and the
//! decoder rejects versions it doesn't understand instead of misparsing them.
//! When the layout changes incompatibly, bump `VOICE_PROTOCOL_VERSION` and teach
//! the decoder to accept the old version too. The `codec` field makes the stream
//! self-describing so a codec can be added/swapped WITHOUT a protocol break.

use serde::{Deserialize, Serialize};

/// Current voice wire-protocol version. Frames we emit carry this; the decoder
/// rejects frames whose `ver` it does not recognise.
pub const VOICE_PROTOCOL_VERSION: u8 = 1;

/// Codec identifier carried in every frame (self-describing stream).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VoiceCodec {
    /// Opus — the default (~16 kHz mono, ~4–6 kbps).
    Opus = 1,
    /// Raw little-endian 16-bit PCM — debugging / fallback only.
    Pcm16 = 2,
}

impl VoiceCodec {
    pub fn from_raw(v: u8) -> Option<Self> {
        match v {
            1 => Some(VoiceCodec::Opus),
            2 => Some(VoiceCodec::Pcm16),
            _ => None,
        }
    }
}

/// Per-frame flag bits.
pub mod frame_flags {
    /// Nothing special.
    pub const NONE: u8 = 0;
    /// First frame of a talk-spurt (follows silence). Lets the receiver reset
    /// its jitter buffer / play-out cleanly at the start of each burst.
    pub const TALKSPURT_START: u8 = 1 << 0;
    /// Opus DTX comfort-noise / silence frame.
    pub const DTX: u8 = 1 << 1;
}

/// One voice packet as it travels (sealed) between peers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceFrame {
    /// Protocol version (== `VOICE_PROTOCOL_VERSION` for frames we emit).
    pub ver: u8,
    /// Monotonic sequence number within a talk-spurt — ordering + loss detection.
    pub seq: u32,
    /// Sender capture timestamp (ms) — play-out timing / drift estimation.
    pub ts_ms: u32,
    /// Which codec `payload` is.
    pub codec: VoiceCodec,
    /// Flag bits (see `frame_flags`).
    pub flags: u8,
    /// Encoded audio bytes (an Opus packet, or raw PCM for `Pcm16`). Serialized
    /// as a MessagePack `bin` (compact — no per-byte overhead).
    #[serde(with = "serde_bytes")]
    pub payload: Vec<u8>,
}

impl VoiceFrame {
    /// Build a frame stamped with the current protocol version.
    pub fn new(seq: u32, ts_ms: u32, codec: VoiceCodec, flags: u8, payload: Vec<u8>) -> Self {
        Self { ver: VOICE_PROTOCOL_VERSION, seq, ts_ms, codec, flags, payload }
    }
}

/// What can go wrong decoding a frame off the wire.
#[derive(Debug, PartialEq, Eq)]
pub enum WireError {
    /// Not valid MessagePack, or not a VoiceFrame shape.
    Malformed,
    /// A version this build does not understand (the byte is reported back).
    UnsupportedVersion(u8),
}

/// Serialize a frame to MessagePack bytes. Infallible for a well-formed frame.
pub fn encode_voice_frame(frame: &VoiceFrame) -> Vec<u8> {
    rmp_serde::to_vec(frame).expect("VoiceFrame MessagePack encoding is infallible")
}

/// Parse a frame from MessagePack bytes, rejecting unknown protocol versions.
pub fn decode_voice_frame(bytes: &[u8]) -> Result<VoiceFrame, WireError> {
    let frame: VoiceFrame = rmp_serde::from_slice(bytes).map_err(|_| WireError::Malformed)?;
    if frame.ver != VOICE_PROTOCOL_VERSION {
        return Err(WireError::UnsupportedVersion(frame.ver));
    }
    Ok(frame)
}

// ─── FFI ─────────────────────────────────────────────────────────────────────
//
// The C++ send path builds a packet from the Opus bytes + metadata
// (sc_voice_frame_encode), seals it, and sends it; the receive path opens it and
// parses it (sc_voice_frame_decode) to get the seq (for the jitter buffer) and
// the payload (for the decoder). `ver` is stamped/checked here so neither end
// hand-rolls the protocol version. Result is a c_int: 0 ok, negative = error.

use std::os::raw::c_int;

/// Encode a voice frame to MessagePack. `ver` is set to the current protocol
/// version internally. Writes bytes to `out` and the length to `out_len`.
/// Returns 0; or -1 (null `out_len`), -2 (buffer too small — `out_len` holds the
/// required size, nothing written), -3 (unknown `codec`).
///
/// # Safety
/// `payload` valid for `payload_len` (or null); `out` writable for `out_cap`;
/// `out_len` a valid `*mut usize`.
#[no_mangle]
pub unsafe extern "C" fn sc_voice_frame_encode(
    seq: u32,
    ts_ms: u32,
    codec: u8,
    flags: u8,
    payload: *const u8,
    payload_len: usize,
    out: *mut u8,
    out_cap: usize,
    out_len: *mut usize,
) -> c_int {
    if out_len.is_null() {
        return -1;
    }
    let codec = match VoiceCodec::from_raw(codec) {
        Some(c) => c,
        None => return -3,
    };
    let payload_vec = if payload.is_null() || payload_len == 0 {
        Vec::new()
    } else {
        std::slice::from_raw_parts(payload, payload_len).to_vec()
    };
    let frame = VoiceFrame::new(seq, ts_ms, codec, flags, payload_vec);
    let bytes = encode_voice_frame(&frame);
    *out_len = bytes.len();
    if out.is_null() || bytes.len() > out_cap {
        return -2;
    }
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len());
    0
}

/// Decode a voice frame from MessagePack. Writes the header fields (any out
/// pointer may be null to skip it) and the payload bytes. Returns 0; or -1 (null
/// `bytes`/`out_seq`/`out_payload_len`), -2 (payload buffer too small —
/// `out_payload_len` holds the required size), -4 (malformed), -5 (unsupported
/// version — `out_ver` holds the offending version). Size `out_payload` to the
/// input length and -2 cannot happen.
///
/// # Safety
/// `bytes` valid for `len`; the out pointers writable (or null where allowed);
/// `out_payload` writable for `out_payload_cap`.
#[no_mangle]
pub unsafe extern "C" fn sc_voice_frame_decode(
    bytes: *const u8,
    len: usize,
    out_ver: *mut u8,
    out_seq: *mut u32,
    out_ts_ms: *mut u32,
    out_codec: *mut u8,
    out_flags: *mut u8,
    out_payload: *mut u8,
    out_payload_cap: usize,
    out_payload_len: *mut usize,
) -> c_int {
    if bytes.is_null() || out_seq.is_null() || out_payload_len.is_null() {
        return -1;
    }
    let slice = std::slice::from_raw_parts(bytes, len);
    match decode_voice_frame(slice) {
        Ok(frame) => {
            if !out_ver.is_null() {
                *out_ver = frame.ver;
            }
            *out_seq = frame.seq;
            if !out_ts_ms.is_null() {
                *out_ts_ms = frame.ts_ms;
            }
            if !out_codec.is_null() {
                *out_codec = frame.codec as u8;
            }
            if !out_flags.is_null() {
                *out_flags = frame.flags;
            }
            *out_payload_len = frame.payload.len();
            if out_payload.is_null() || frame.payload.len() > out_payload_cap {
                return -2;
            }
            std::ptr::copy_nonoverlapping(
                frame.payload.as_ptr(),
                out_payload,
                frame.payload.len(),
            );
            0
        }
        Err(WireError::UnsupportedVersion(v)) => {
            if !out_ver.is_null() {
                *out_ver = v;
            }
            -5
        }
        Err(WireError::Malformed) => -4,
    }
}

/// The current voice protocol version (so C can sanity-check / log it).
#[no_mangle]
pub extern "C" fn sc_voice_protocol_version() -> u8 {
    VOICE_PROTOCOL_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_preserves_fields() {
        let f = VoiceFrame::new(
            42,
            1234,
            VoiceCodec::Opus,
            frame_flags::TALKSPURT_START,
            vec![1, 2, 3, 4, 5],
        );
        let bytes = encode_voice_frame(&f);
        let back = decode_voice_frame(&bytes).expect("decodes");
        assert_eq!(back, f);
        assert_eq!(back.ver, VOICE_PROTOCOL_VERSION);
        assert_eq!(back.seq, 42);
        assert_eq!(back.codec, VoiceCodec::Opus);
        assert_eq!(back.payload, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn unknown_version_is_rejected() {
        let mut f = VoiceFrame::new(1, 0, VoiceCodec::Opus, 0, vec![9]);
        f.ver = 99; // a future version this build doesn't know
        let bytes = encode_voice_frame(&f);
        match decode_voice_frame(&bytes) {
            Err(WireError::UnsupportedVersion(99)) => {}
            other => panic!("expected UnsupportedVersion(99), got {:?}", other),
        }
    }

    #[test]
    fn garbage_is_malformed_not_panic() {
        assert_eq!(decode_voice_frame(&[0xff, 0x00, 0x13, 0x37]), Err(WireError::Malformed));
        assert_eq!(decode_voice_frame(&[]), Err(WireError::Malformed));
    }

    #[test]
    fn payload_is_compact_bin() {
        // 100-byte payload: with serde_bytes (msgpack bin) the whole packet stays
        // close to 100 bytes + small header, NOT ~200 (which a u8 array would be).
        let f = VoiceFrame::new(0, 0, VoiceCodec::Opus, 0, vec![0xAB; 100]);
        let bytes = encode_voice_frame(&f);
        assert!(bytes.len() < 130, "packet unexpectedly large: {} bytes", bytes.len());
    }

    #[test]
    fn codec_from_raw() {
        assert_eq!(VoiceCodec::from_raw(1), Some(VoiceCodec::Opus));
        assert_eq!(VoiceCodec::from_raw(2), Some(VoiceCodec::Pcm16));
        assert_eq!(VoiceCodec::from_raw(7), None);
    }

    #[test]
    fn ffi_encode_decode_roundtrip() {
        unsafe {
            let payload = [10u8, 20, 30, 40];
            let mut buf = [0u8; 256];
            let mut wlen: usize = 0;
            let rc = sc_voice_frame_encode(
                7, 999, VoiceCodec::Opus as u8, frame_flags::TALKSPURT_START,
                payload.as_ptr(), payload.len(),
                buf.as_mut_ptr(), buf.len(), &mut wlen,
            );
            assert_eq!(rc, 0);
            assert!(wlen > 0 && wlen <= buf.len());

            let (mut ver, mut seq, mut ts, mut codec, mut flags) = (0u8, 0u32, 0u32, 0u8, 0u8);
            let mut pbuf = [0u8; 256];
            let mut plen: usize = 0;
            let rc2 = sc_voice_frame_decode(
                buf.as_ptr(), wlen,
                &mut ver, &mut seq, &mut ts, &mut codec, &mut flags,
                pbuf.as_mut_ptr(), pbuf.len(), &mut plen,
            );
            assert_eq!(rc2, 0);
            assert_eq!(ver, VOICE_PROTOCOL_VERSION);
            assert_eq!(seq, 7);
            assert_eq!(ts, 999);
            assert_eq!(codec, VoiceCodec::Opus as u8);
            assert_eq!(flags, frame_flags::TALKSPURT_START);
            assert_eq!(&pbuf[..plen], &payload);

            // unknown codec → -3
            assert_eq!(
                sc_voice_frame_encode(0, 0, 200, 0, std::ptr::null(), 0,
                    buf.as_mut_ptr(), buf.len(), &mut wlen),
                -3
            );
            // output buffer too small → -2 with required size reported
            let mut tiny = [0u8; 2];
            let mut need: usize = 0;
            assert_eq!(
                sc_voice_frame_encode(0, 0, VoiceCodec::Opus as u8, 0,
                    payload.as_ptr(), payload.len(),
                    tiny.as_mut_ptr(), tiny.len(), &mut need),
                -2
            );
            assert!(need > tiny.len());
            // malformed input → -4
            let junk = [0xffu8, 0x00];
            assert_eq!(
                sc_voice_frame_decode(junk.as_ptr(), junk.len(),
                    std::ptr::null_mut(), &mut seq, std::ptr::null_mut(),
                    std::ptr::null_mut(), std::ptr::null_mut(),
                    pbuf.as_mut_ptr(), pbuf.len(), &mut plen),
                -4
            );
            assert_eq!(sc_voice_protocol_version(), VOICE_PROTOCOL_VERSION);
        }
    }
}
