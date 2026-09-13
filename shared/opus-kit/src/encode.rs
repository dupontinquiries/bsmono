//! encode.rs — the missing half of this crate: writing an Ogg Opus file,
//! not just reading one. C14_TODO §4's one remaining "Open" item on the
//! voice pipeline: "Nothing in the tree encodes Opus; bo-suite's clips came
//! from an external tool. Needs libopus behind a feature."
//!
//! ## Why this is a separate module, and why `lib.rs` stays libopus-free
//!
//! `lib.rs`'s own docstring is explicit about why it has no libopus and no
//! `ogg` crate: the container arithmetic (pre_skip, the end-granule trim) is
//! pure bookkeeping, testable without a codec, and builds anywhere. Encoding
//! needs a real codec — there is no serious pure-Rust Opus encoder, encoding
//! is a different order of complexity from parsing a page header — so this
//! is real FFI to system libopus via `audiopus`, gated behind the `encode`
//! feature and isolated to this one file. Nothing outside this module, and
//! nothing that doesn't opt into the feature, links libopus at all.
//!
//! ## Encoding at a fixed 48 kHz, on purpose
//!
//! Opus's decoder output and an Ogg Opus container's granule positions are
//! ALWAYS reckoned in 48 kHz units regardless of what native rate the
//! ENCODER was configured for (8/12/16/24/48 kHz are all valid encoder
//! inputs) — but `pre_skip` is specifically "samples to discard at the
//! decoder's 48 kHz output", and the encoder's own `lookahead()` is reported
//! in samples at whatever rate the encoder itself was configured with. Using
//! a non-48 kHz encoder rate means scaling `lookahead()` by
//! `48000 / encoder_rate` to get a correct `pre_skip`, which is one more
//! place to get a factor wrong in a crate that exists specifically to stop
//! that class of bug. `lib.rs`'s own docstring already says "312 samples
//! (6.5 ms) in every file in this repository" — this project's own material
//! is uniformly 48 kHz already — so this module only supports encoding at
//! 48 kHz and asks the caller to resample first if their source is not.
//! That sidesteps the scaling arithmetic entirely rather than getting it
//! right by luck.
//!
//! ## One packet per page
//!
//! Real encoders pack multiple small packets into one Ogg page (fewer page
//! headers = less overhead) — worth doing later, not needed for this to be
//! correct. One packet per page is simpler, still RFC 7845-valid (any
//! encoder that reads pages generically, which is everything reasonable,
//! doesn't care), and was verified byte-for-byte decodable by real tools
//! (`opusdec`, `ffprobe`) before this was trusted, not assumed correct from
//! reading the RFC.

use crate::{Error, GRANULE_RATE};
use audiopus::coder::Encoder;
use audiopus::{Channels, SampleRate};
/// Re-exported so a caller can pick the tuning without depending on
/// audiopus itself — the point of the kit is that nothing outside it
/// names libopus.
pub use audiopus::Application;

/// 20ms frames at 48kHz: the frame size this project's material already
/// uses everywhere and a safe default complexity/latency tradeoff.
const FRAME_SAMPLES: usize = 960; // 48_000 * 20 / 1000

/// The two owned pieces of an encode job's outcome: the Ogg Opus bytes and
/// the container header actually written, so a caller can log/assert against
/// it without re-parsing the bytes it just produced.
#[derive(Debug)]
pub struct Encoded {
    pub bytes: Vec<u8>,
    pub head: crate::OpusHead,
}

/// Encode 48 kHz PCM (interleaved if `channels > 1`) into a complete,
/// self-contained Ogg Opus file.
///
/// `channels` must be 1 or 2 (mapping family 0 — the only family this
/// crate's own parser accepts, per [`Error::UnsupportedMapping`]).
/// `application` selects Opus's internal tuning; `Application::Audio` is
/// right for anything that isn't live voice-call latency.
pub fn encode_ogg_opus(
    pcm: &[i16],
    channels: u8,
    application: Application,
) -> Result<Encoded, Error> {
    encode_ogg_opus_at(pcm, channels, application, None)
}

/// [`encode_ogg_opus`] with a target bitrate in bits per second (`None`
/// leaves libopus's own default). studio's bounce ladder and beatspace's
/// pick rungs with this.
pub fn encode_ogg_opus_at(
    pcm: &[i16],
    channels: u8,
    application: Application,
    bitrate_bps: Option<i32>,
) -> Result<Encoded, Error> {
    if channels != 1 && channels != 2 {
        return Err(Error::UnsupportedMapping(channels));
    }
    let ch = if channels == 1 { Channels::Mono } else { Channels::Stereo };
    let mut enc = Encoder::new(SampleRate::Hz48000, ch, application)
        .map_err(|_| Error::NotOpus)?;
    if let Some(bps) = bitrate_bps {
        enc.set_bitrate(audiopus::Bitrate::BitsPerSecond(bps))
            .map_err(|_| Error::NotOpus)?;
    }

    // Algorithmic delay, at the encoder's own 48kHz rate — see the module
    // doc for why fixing the rate at 48kHz is what makes this a direct
    // sample count instead of a scaled one.
    let pre_skip: u16 = enc_lookahead(&enc)?;

    let head = crate::OpusHead {
        version: 1,
        channels,
        pre_skip,
        input_rate: GRANULE_RATE, // informational per RFC 7845; honest since we require 48kHz in
        output_gain_q8: 0,
        mapping_family: 0,
    };

    let mut out = Vec::new();
    let serial: u32 = 0x4f70_7573; // "Opus" as a fixed serial — a single-stream file, uniqueness
                                    // across concurrently-muxed streams is not this crate's job.
    let mut seq: u32 = 0;

    write_page(&mut out, serial, seq, 0, &[&opus_head_packet(&head)], PageFlags::BOS);
    seq += 1;
    write_page(&mut out, serial, seq, 0, &[&opus_tags_packet()], PageFlags::NONE);
    seq += 1;

    let frame_len = FRAME_SAMPLES * channels as usize;
    let mut granule: u64 = 0;
    let mut packet_buf = vec![0u8; 4000]; // generous — Opus packets are well under this at any bitrate

    // `pre_skip` is the encoder's own algorithmic DELAY, not extra content: the last real samples
    // fed in don't finish propagating out through the encoder's internal lookahead buffer until
    // `pre_skip` MORE samples have been encoded after them. Stopping at exactly the real content's
    // length silently drops up to `pre_skip` samples (6.5ms here) off the true end — caught by
    // actually decoding the output and finding it short, not by reasoning about the RFC. Padding
    // the encoder's input with `pre_skip` extra zero samples flushes it; `real_end_granule` below
    // still trims the container to the true content length, the same "end-granule trim" this
    // crate's own top-level docstring already exists to make callers apply on the READ side.
    let real_samples_per_channel = pcm.len() / channels as usize;
    let flush_len = (real_samples_per_channel + pre_skip as usize) * channels as usize;
    let total_frames = flush_len.div_ceil(frame_len).max(1);

    for i in 0..total_frames {
        let start = i * frame_len;
        let end = (start + frame_len).min(pcm.len());
        let frame: std::borrow::Cow<[i16]> = if start >= pcm.len() {
            // Entirely past the real content: a silent flush frame.
            std::borrow::Cow::Owned(vec![0i16; frame_len])
        } else if end - start == frame_len {
            std::borrow::Cow::Borrowed(&pcm[start..end])
        } else {
            // Straddles the real/flush boundary: real samples then zero-pad to a full frame.
            let mut v = pcm[start..end].to_vec();
            v.resize(frame_len, 0);
            std::borrow::Cow::Owned(v)
        };
        let n = enc.encode(&frame, &mut packet_buf).map_err(|_| Error::NotOpus)?;
        granule += FRAME_SAMPLES as u64;
        let flags = if i + 1 == total_frames { PageFlags::EOS } else { PageFlags::NONE };
        write_page(&mut out, serial, seq, granule, &[&packet_buf[..n]], flags);
        seq += 1;
    }

    // The granule position on the LAST page is what `Stream::end_granule`
    // reads back — if we padded the final frame, the real end is BEFORE the
    // full-frame granule we just wrote, so patch it to the exact sample
    // count instead of the padded one. Both plus pre_skip is what
    // `Stream::body()` returns.
    let real_end_granule = pre_skip as u64 + (pcm.len() / channels as usize) as u64;
    patch_last_granule(&mut out, real_end_granule);

    Ok(Encoded { bytes: out, head })
}

/// `Encoder::lookahead()` isn't exposed by name on the trait objects audiopus
/// re-exports at the version this crate pins — called directly on the
/// concrete `Encoder` instead of routing through a trait, so a future
/// audiopus upgrade that reshuffles the trait surface can't silently change
/// which method this resolves to.
fn enc_lookahead(enc: &Encoder) -> Result<u16, Error> {
    let la = enc.lookahead().map_err(|_| Error::NotOpus)?;
    Ok(la.min(u16::MAX as u32) as u16)
}

fn opus_head_packet(h: &crate::OpusHead) -> Vec<u8> {
    let mut v = Vec::with_capacity(19);
    v.extend_from_slice(b"OpusHead");
    v.push(h.version);
    v.push(h.channels);
    v.extend_from_slice(&h.pre_skip.to_le_bytes());
    v.extend_from_slice(&h.input_rate.to_le_bytes());
    v.extend_from_slice(&(h.output_gain_q8 as u16).to_le_bytes());
    v.push(h.mapping_family);
    debug_assert_eq!(v.len(), 19);
    v
}

fn opus_tags_packet() -> Vec<u8> {
    let vendor = b"opus-kit encode.rs";
    let mut v = Vec::new();
    v.extend_from_slice(b"OpusTags");
    v.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    v.extend_from_slice(vendor);
    v.extend_from_slice(&0u32.to_le_bytes()); // zero user comments
    v
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct PageFlags(u8);
impl PageFlags {
    const NONE: PageFlags = PageFlags(0);
    const BOS: PageFlags = PageFlags(0x02);
    const EOS: PageFlags = PageFlags(0x04);
}

/// Write one Ogg page carrying `packets` (usually one), with a correct
/// lacing table and CRC.
fn write_page(out: &mut Vec<u8>, serial: u32, seq: u32, granule: u64, packets: &[&[u8]], flags: PageFlags) {
    let start = out.len();
    out.extend_from_slice(b"OggS");
    out.push(0); // stream structure version
    out.push(flags.0);
    out.extend_from_slice(&granule.to_le_bytes());
    out.extend_from_slice(&serial.to_le_bytes());
    out.extend_from_slice(&seq.to_le_bytes());
    let crc_pos = out.len();
    out.extend_from_slice(&0u32.to_le_bytes()); // CRC placeholder, patched below

    // Lacing: each packet becomes a run of 255-byte segments plus one
    // terminator segment strictly less than 255 (a fresh 0-length segment if
    // the packet is an exact multiple of 255) — RFC 3533 §6, the same rule
    // this crate's own `pages()` relies on when it sums the segment table to
    // find a page's body length.
    let mut segs = Vec::new();
    for p in packets {
        let mut rem = p.len();
        while rem >= 255 {
            segs.push(255u8);
            rem -= 255;
        }
        segs.push(rem as u8);
    }
    out.push(segs.len() as u8);
    out.extend_from_slice(&segs);
    for p in packets {
        out.extend_from_slice(p);
    }

    let crc = ogg_crc32(&out[start..]);
    out[crc_pos..crc_pos + 4].copy_from_slice(&crc.to_le_bytes());
}

/// Overwrite the granule position of the LAST page written, in place. Used
/// once, after encoding, to record the true (unpadded) sample count on the
/// final page — which requires recomputing that page's CRC, since the CRC
/// covers the granule field.
fn patch_last_granule(out: &mut [u8], granule: u64) {
    // Walk pages the same way `pages()` does, to find the last one — rather
    // than assuming the caller's `seq` bookkeeping matches byte offsets,
    // which would be two implementations of the same walk that could drift.
    let mut last_start = None;
    let mut i = 0usize;
    while i + 27 <= out.len() {
        if &out[i..i + 4] != b"OggS" {
            break;
        }
        let nseg = out[i + 26] as usize;
        let table = i + 27;
        let body_len: usize = out[table..table + nseg].iter().map(|s| *s as usize).sum();
        let page_len = 27 + nseg + body_len;
        last_start = Some((i, page_len));
        i += page_len;
    }
    let Some((start, len)) = last_start else { return };
    out[start + 6..start + 14].copy_from_slice(&granule.to_le_bytes());
    // Zero the CRC field before recomputing, exactly as write_page does.
    out[start + 22..start + 26].copy_from_slice(&0u32.to_le_bytes());
    let crc = ogg_crc32(&out[start..start + len]);
    out[start + 22..start + 26].copy_from_slice(&crc.to_le_bytes());
}

/// Ogg's page CRC: CRC-32 with polynomial 0x04c11db7, MSB-first, no
/// reflection, no initial or final XOR — NOT the same algorithm as
/// zip/png/zlib's CRC-32 (which reflects both in and out). Verified against
/// real decoders (`opusdec`, `ffprobe`) rather than trusted from the spec
/// text alone: a subtly wrong CRC variant is exactly the kind of bug that
/// only shows up against an implementation that actually checks it, and
/// this crate's own `pages()` does not.
fn ogg_crc32(data: &[u8]) -> u32 {
    const POLY: u32 = 0x04c1_1db7;
    let mut table = [0u32; 256];
    let mut i = 0u32;
    while i < 256 {
        let mut c = i << 24;
        let mut j = 0;
        while j < 8 {
            c = if c & 0x8000_0000 != 0 { (c << 1) ^ POLY } else { c << 1 };
            j += 1;
        }
        table[i as usize] = c;
        i += 1;
    }
    let mut crc = 0u32;
    for &b in data {
        crc = (crc << 8) ^ table[(((crc >> 24) ^ b as u32) & 0xff) as usize];
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A few seconds of a real (if simple) waveform, not silence — silence
    /// can round-trip correctly through a bug that only shows up with actual
    /// signal (e.g. a channel-order swap is invisible on a mono sine but not
    /// on a stereo one with different tones per channel).
    fn tone(seconds: f32, hz: f32, channels: u8) -> Vec<i16> {
        let n = (48_000.0 * seconds) as usize;
        let mut v = Vec::with_capacity(n * channels as usize);
        for i in 0..n {
            let t = i as f32 / 48_000.0;
            let s = (t * hz * std::f32::consts::TAU).sin();
            v.push((s * 12_000.0) as i16);
            if channels == 2 {
                let s2 = (t * (hz * 1.5) * std::f32::consts::TAU).sin();
                v.push((s2 * 12_000.0) as i16);
            }
        }
        v
    }

    #[test]
    fn round_trips_through_this_crates_own_parser() {
        let pcm = tone(0.5, 440.0, 1);
        let enc = encode_ogg_opus(&pcm, 1, Application::Audio).expect("encode");
        let stream = crate::parse(&enc.bytes).expect("our own parser must accept our own output");
        assert_eq!(stream.head.channels, 1);
        assert_eq!(stream.head.mapping_family, 0);
        assert_eq!(stream.head.pre_skip, enc.head.pre_skip);
        // samples() is post-trim real audio length; must match the input exactly, not the
        // zero-padded final-frame length — this is the whole point of patch_last_granule.
        assert_eq!(stream.samples(), pcm.len() as u64);
    }

    #[test]
    fn stereo_round_trips_too() {
        let pcm = tone(0.3, 300.0, 2);
        let enc = encode_ogg_opus(&pcm, 2, Application::Audio).expect("encode");
        let stream = crate::parse(&enc.bytes).expect("parse");
        assert_eq!(stream.head.channels, 2);
        assert_eq!(stream.samples(), (pcm.len() / 2) as u64);
    }

    #[test]
    fn a_non_frame_aligned_length_still_round_trips_exactly() {
        // 0.37s at 48kHz is not a multiple of the 20ms/960-sample frame size,
        // so the final frame IS padded internally — this is the case
        // patch_last_granule exists for.
        let pcm = tone(0.37, 500.0, 1);
        assert_ne!(pcm.len() % FRAME_SAMPLES, 0, "test setup: need a non-aligned length");
        let enc = encode_ogg_opus(&pcm, 1, Application::Audio).expect("encode");
        let stream = crate::parse(&enc.bytes).expect("parse");
        assert_eq!(stream.samples(), pcm.len() as u64);
    }

    #[test]
    fn decodes_back_to_the_right_sample_count_with_a_real_decoder() {
        // Independent of this crate's own parser: decode with audiopus's own
        // Decoder, the same codec the encoder used, to confirm the PACKETS
        // themselves (not just the container framing around them) are valid
        // Opus — our parser checking the container correctly says nothing
        // about whether the audio payload inside it is well-formed.
        let pcm = tone(0.5, 440.0, 1);
        let enc = encode_ogg_opus(&pcm, 1, Application::Audio).expect("encode");
        let stream = crate::parse(&enc.bytes).expect("parse");

        let mut dec = audiopus::coder::Decoder::new(SampleRate::Hz48000, Channels::Mono)
            .expect("decoder");
        let mut total = 0u64;
        for p in &stream.pages[2..] {
            // skip OpusHead, OpusTags
            let packet = &enc.bytes[p.body..p.body + p.body_len];
            let mut pcm_out = vec![0i16; FRAME_SAMPLES];
            let n = dec.decode(Some(packet), &mut pcm_out, false).expect("decode");
            total += n as u64;
        }
        // Decoded length includes the pre_skip priming and any final-frame
        // padding — real audio is the trimmed window this crate's own
        // Stream::body() reports, which decodes_back only cross-checks the
        // upper bound of, not equality (padding means decoded >= real+skip).
        assert!(total >= stream.head.pre_skip as u64 + stream.samples());
    }

    #[test]
    fn rejects_a_bogus_channel_count() {
        assert_eq!(
            encode_ogg_opus(&[0i16; 10], 3, Application::Audio).unwrap_err(),
            Error::UnsupportedMapping(3)
        );
    }
}
