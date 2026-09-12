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

/// The sequence header held in `av1C`.
///
/// AV1 needs no start codes — samples are already OBUs — but it has the same
/// out-of-band problem in a different shape. An `av1C` box carries
/// `configOBUs` after its four-byte header, and for MP4 that is very often the
/// **only** place the sequence header appears: encoders are permitted to omit
/// it from the samples because the container is supposed to supply it.
///
/// A decoder handed frames with no sequence header produces nothing and, on
/// dav1d, no error either — the same silent black screen as a missing start
/// code, arriving by a different route. So the config OBUs are prepended to
/// the first keyframe and to the first keyframe after a seek, exactly as the
/// parameter sets are for H.264.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Av1Config {
    /// `configOBUs`, verbatim. Empty when the encoder put the sequence header
    /// in the samples instead, which is legal and needs no help from us.
    pub config_obus: Vec<u8>,
}

/// Parse `av1C`.
///
/// Layout: `marker(1)|version(7)`, `seq_profile(3)|seq_level_idx_0(5)`,
/// `seq_tier_0(1)|high_bitdepth(1)|twelve_bit(1)|monochrome(1)|
/// chroma_subsampling_x(1)|_y(1)|chroma_sample_position(2)`,
/// then a reserved/delay byte, then `configOBUs` to the end of the box.
pub fn parse_av1c(data: &[u8]) -> Result<Av1Config> {
    if data.len() < 4 {
        return Err(Error::Malformed(format!("av1C is {} bytes", data.len())));
    }
    // marker must be 1 and version 1; anything else is a box we do not
    // understand, and guessing at its layout would hand the decoder garbage
    // that looks like a sequence header.
    let marker = data[0] >> 7;
    let version = data[0] & 0x7F;
    if marker != 1 || version != 1 {
        return Err(Error::Malformed(format!(
            "av1C marker/version {marker}/{version}, expected 1/1"
        )));
    }
    Ok(Av1Config {
        config_obus: data[4..].to_vec(),
    })
}

/// Prepend the config OBUs to a sample.
///
/// Returns the sample unchanged when there is nothing to add, so the common
/// case allocates nothing extra.
pub fn with_av1_config(sample: &[u8], cfg: &Av1Config, include: bool) -> Vec<u8> {
    if !include || cfg.config_obus.is_empty() {
        return sample.to_vec();
    }
    let mut out = Vec::with_capacity(cfg.config_obus.len() + sample.len());
    out.extend_from_slice(&cfg.config_obus);
    out.extend_from_slice(sample);
    out
}

#[cfg(test)]
mod av1_tests {
    use super::*;

    fn av1c(obus: &[u8]) -> Vec<u8> {
        let mut v = vec![0x81, 0x00, 0x00, 0x00];
        v.extend_from_slice(obus);
        v
    }

    #[test]
    fn config_obus_are_everything_after_the_header() {
        let c = parse_av1c(&av1c(&[0x0A, 0x0B, 0x00])).unwrap();
        assert_eq!(c.config_obus, vec![0x0A, 0x0B, 0x00]);
    }

    /// An encoder that put the sequence header in the samples leaves this
    /// empty, which is legal and needs no help.
    #[test]
    fn an_empty_config_is_not_an_error() {
        let c = parse_av1c(&av1c(&[])).unwrap();
        assert!(c.config_obus.is_empty());
    }

    /// Guessing at the layout of a box we do not understand would hand the
    /// decoder garbage that looks like a sequence header.
    #[test]
    fn a_wrong_marker_or_version_is_refused() {
        assert!(parse_av1c(&[0x01, 0, 0, 0]).is_err(), "marker 0 accepted");
        assert!(parse_av1c(&[0x82, 0, 0, 0]).is_err(), "version 2 accepted");
        assert!(parse_av1c(&[0x81, 0x00]).is_err(), "truncated box accepted");
    }

    #[test]
    fn the_config_goes_in_front_of_the_frame() {
        let cfg = Av1Config { config_obus: vec![0xAA, 0xBB] };
        let out = with_av1_config(&[0x01, 0x02], &cfg, true);
        assert_eq!(out, vec![0xAA, 0xBB, 0x01, 0x02]);
    }

    /// Prepending on every frame would corrupt the stream, not merely waste
    /// bytes: a sequence header mid-sequence resets the decoder.
    #[test]
    fn nothing_is_prepended_when_not_asked_for() {
        let cfg = Av1Config { config_obus: vec![0xAA] };
        assert_eq!(with_av1_config(&[0x01], &cfg, false), vec![0x01]);
        let empty = Av1Config::default();
        assert_eq!(with_av1_config(&[0x01], &empty, true), vec![0x01]);
    }
}
