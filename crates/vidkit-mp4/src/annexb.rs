//! Length-prefixed NAL units to Annex-B.
//!
//! H.264 and HEVC inside MP4 are stored as length-prefixed NAL units, with the
//! SPS/PPS held out-of-band in the `avcC`/`hvcC` box. Every V4L2 stateless and
//! stateful decoder, and ffmpeg's raw parsers, want Annex-B: start-code
//! separated NALs with the parameter sets inline in the stream.
//!
//! Feeding MP4-format samples straight to a V4L2 decoder produces no output and
//! no error — the driver simply never finds a start code. It is one of the
//! most common reasons a "working" demuxer produces a black screen.
//!
//! AV1 and VP9 need none of this: `av01` samples are already OBUs and `vp09`
//! samples are raw VP9 frames, so those pass through untouched.

use crate::boxes::{be_u16, Error, Result};
use vidkit_core::codec::Codec;

pub const START_CODE: [u8; 4] = [0, 0, 0, 1];

/// Parsed `avcC` / `hvcC`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NalConfig {
    /// Bytes in each NAL's length prefix: 1, 2 or 4.
    pub length_size: usize,
    /// SPS/PPS (and VPS for HEVC) already wrapped in start codes, ready to
    /// prepend to a keyframe.
    pub parameter_sets: Vec<u8>,
}

/// Parse `avcC`.
///
/// Layout: version(1) profile(1) compat(1) level(1),
/// `0b111111xx` lengthSizeMinusOne, `0b111xxxxx` numSPS, then
/// `[u16 len + data]` per SPS, then numPPS(1) and the same for PPS.
pub fn parse_avcc(data: &[u8]) -> Result<NalConfig> {
    if data.len() < 7 {
        return Err(Error::Malformed(format!("avcC is {} bytes", data.len())));
    }
    // The top six bits are reserved 1s; masking them off is required, and
    // forgetting to gives a length_size of 255.
    let length_size = ((data[4] & 0x03) + 1) as usize;
    if !matches!(length_size, 1 | 2 | 4) {
        return Err(Error::Malformed(format!(
            "avcC lengthSizeMinusOne yields {length_size}"
        )));
    }

    let mut out = Vec::new();
    let mut at = 5usize;

    let num_sps = (data[at] & 0x1F) as usize;
    at += 1;
    for _ in 0..num_sps {
        let len = be_u16(data, at)? as usize;
        at += 2;
        let nal = data
            .get(at..at + len)
            .ok_or_else(|| Error::Malformed("avcC SPS runs past end".into()))?;
        out.extend_from_slice(&START_CODE);
        out.extend_from_slice(nal);
        at += len;
    }

    let num_pps = *data
        .get(at)
        .ok_or_else(|| Error::Malformed("avcC truncated before PPS count".into()))?
        as usize;
    at += 1;
    for _ in 0..num_pps {
        let len = be_u16(data, at)? as usize;
        at += 2;
        let nal = data
            .get(at..at + len)
            .ok_or_else(|| Error::Malformed("avcC PPS runs past end".into()))?;
        out.extend_from_slice(&START_CODE);
        out.extend_from_slice(nal);
        at += len;
    }

    Ok(NalConfig {
        length_size,
        parameter_sets: out,
    })
}

/// Parse `hvcC`.
///
/// Layout differs from `avcC`: 22 fixed bytes, then `numOfArrays(1)`, then per
/// array `completeness|type(1)`, `numNalus(2)`, then `[u16 len + data]`.
pub fn parse_hvcc(data: &[u8]) -> Result<NalConfig> {
    if data.len() < 23 {
        return Err(Error::Malformed(format!("hvcC is {} bytes", data.len())));
    }
    let length_size = ((data[21] & 0x03) + 1) as usize;
    if !matches!(length_size, 1 | 2 | 4) {
        return Err(Error::Malformed(format!(
            "hvcC lengthSizeMinusOne yields {length_size}"
        )));
    }

    let mut out = Vec::new();
    let num_arrays = data[22] as usize;
    let mut at = 23usize;

    for _ in 0..num_arrays {
        // Skip the completeness/type byte; VPS, SPS and PPS all need emitting
        // and we emit them in the order the file lists them.
        at += 1;
        let count = be_u16(data, at)? as usize;
        at += 2;
        for _ in 0..count {
            let len = be_u16(data, at)? as usize;
            at += 2;
            let nal = data
                .get(at..at + len)
                .ok_or_else(|| Error::Malformed("hvcC NAL runs past end".into()))?;
            out.extend_from_slice(&START_CODE);
            out.extend_from_slice(nal);
            at += len;
        }
    }

    Ok(NalConfig {
        length_size,
        parameter_sets: out,
    })
}

pub fn parse_config(codec: Codec, data: &[u8]) -> Result<Option<NalConfig>> {
    match codec {
        Codec::H264 => parse_avcc(data).map(Some),
        Codec::Hevc => parse_hvcc(data).map(Some),
        // Already in a start-code-free format the decoders accept as-is.
        Codec::Av1 | Codec::Vp8 | Codec::Vp9 => Ok(None),
    }
}

/// Rewrite one length-prefixed sample as Annex-B, optionally prefixing the
/// parameter sets.
///
/// Parameter sets are prepended to **every** sync sample, not just the first.
/// After a seek or a decoder flush the decoder has forgotten them, and a
/// keyframe without an SPS decodes to nothing. Repeating them costs a few
/// dozen bytes per keyframe.
pub fn to_annex_b(sample: &[u8], cfg: &NalConfig, prepend_parameter_sets: bool) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(sample.len() + 16 + cfg.parameter_sets.len());
    if prepend_parameter_sets {
        out.extend_from_slice(&cfg.parameter_sets);
    }

    let mut at = 0usize;
    while at < sample.len() {
        if at + cfg.length_size > sample.len() {
            return Err(Error::Malformed(format!(
                "NAL length prefix truncated at {at} of {}",
                sample.len()
            )));
        }
        let mut len = 0usize;
        for i in 0..cfg.length_size {
            len = (len << 8) | sample[at + i] as usize;
        }
        at += cfg.length_size;

        // A zero-length NAL would loop forever.
        if len == 0 {
            return Err(Error::Malformed(format!("zero-length NAL at {at}")));
        }
        let end = at
            .checked_add(len)
            .ok_or_else(|| Error::Malformed("NAL length overflows".into()))?;
        if end > sample.len() {
            return Err(Error::Malformed(format!(
                "NAL of {len} bytes at {at} runs past the {}-byte sample",
                sample.len()
            )));
        }

        out.extend_from_slice(&START_CODE);
        out.extend_from_slice(&sample[at..end]);
        at = end;
    }
    Ok(out)
}
