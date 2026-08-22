//! opus — `.opus` (Ogg Opus, RFC 7845) into [`SourceAudio`].
//!
//! Symphonia has no Opus codec, so this file pairs the `ogg` crate
//! (container demux) with libopus (the `opus` crate) — the combination
//! `apps/beatspace` already proved in this monorepo. This is a rewrite for
//! this crate's contract rather than a copy, because the contracts differ in
//! three ways that matter for a sampler and for loops:
//!
//! * **Channels are preserved**, not downmixed: `SourceAudio` is planar and
//!   `mixdown` handles stereo, so a stereo melody loop stays stereo.
//!   (beatspace feeds a mono mel front-end; downmix is correct *there*.)
//! * **Float decode** (`decode_float`), skipping the i16 round-trip.
//! * **Both ends are trimmed.** RFC 7845 defines a `pre_skip` of encoder
//!   priming samples at the start AND a final-page granule position that
//!   marks where the real audio ends inside the last (padded) frame. A
//!   sampler one-shot survives sloppy trimming; a *loop* does not — trailing
//!   codec padding at the seam is an audible tick every bar, which is the
//!   difference between a melody loop and a metronome you didn't ask for.
//!
//! Channel-mapping family 0 only (mono/stereo — RFC 7845 §5.1.1's "vast
//! majority of music" case). Surround Opus needs the mapping-table decode
//! path libopus's plain `Decoder` doesn't do; refusing it loudly beats
//! decoding it wrong.

use std::io::{Cursor, Read, Seek};
use std::path::Path;

use studio_audio::SourceAudio;

use crate::DecodeError;

/// Opus always decodes at 48 kHz regardless of the input's original rate —
/// that is the codec's contract, not a resample this crate chose.
const OPUS_RATE: u32 = 48_000;
/// Longest legal Opus frame: 120 ms at 48 kHz.
const MAX_FRAME: usize = 5760;

pub(crate) fn decode_opus_file(path: &Path) -> Result<SourceAudio, DecodeError> {
    decode_opus_reader(std::fs::File::open(path)?)
}

pub(crate) fn decode_opus_bytes(bytes: Vec<u8>) -> Result<SourceAudio, DecodeError> {
    decode_opus_reader(Cursor::new(bytes))
}

/// Does this look like an Ogg stream whose first logical stream is Opus?
/// Checked against the first page only — `OpusHead` is required by RFC 7845
/// to be the first packet of the first page, so scanning further would only
/// ever *mis*-identify something.
pub(crate) fn sniff_opus(bytes: &[u8]) -> bool {
    bytes.starts_with(b"OggS")
        && bytes[..bytes.len().min(512)]
            .windows(8)
            .any(|w| w == b"OpusHead")
}

fn decode_opus_reader<R: Read + Seek>(reader: R) -> Result<SourceAudio, DecodeError> {
    use ogg::reading::PacketReader;

    let mut reader = PacketReader::new(reader);
    let mut channels = 0usize;
    let mut pre_skip = 0usize;
    let mut decoder: Option<opus::Decoder> = None;
    let mut header_seen = false;
    let mut tags_seen = false;
    // Interleaved decode scratch, planar accumulation.
    let mut planes: Vec<Vec<f32>> = Vec::new();
    let mut frame = vec![0f32; 0];
    // The last granule position seen: total valid 48k samples (including
    // pre_skip) up to the end of that page. The final page's value is the
    // stream's true length — everything past it is codec padding.
    let mut last_granule: Option<u64> = None;

    loop {
        let packet = reader
            .read_packet()
            .map_err(|e| DecodeError::Decode(format!("ogg: {e}")))?;
        let Some(packet) = packet else { break };
        let data = &packet.data;

        if !header_seen {
            if data.starts_with(b"OpusHead") && data.len() >= 19 {
                channels = data[9] as usize;
                pre_skip = u16::from_le_bytes([data[10], data[11]]) as usize;
                let family = data[18];
                if family != 0 || channels == 0 || channels > 2 {
                    return Err(DecodeError::Decode(format!(
                        "opus channel mapping family {family} with {channels} channel(s): \
                         only mono/stereo (family 0) is supported"
                    )));
                }
                let ch = if channels == 2 { opus::Channels::Stereo } else { opus::Channels::Mono };
                decoder = Some(
                    opus::Decoder::new(OPUS_RATE, ch)
                        .map_err(|e| DecodeError::Decode(format!("opus init: {e}")))?,
                );
                planes = vec![Vec::new(); channels];
                frame = vec![0f32; MAX_FRAME * channels];
                header_seen = true;
            }
            // Not OpusHead: a different codec's logical stream (e.g. Vorbis
            // muxed first) — keep scanning; the caller's Symphonia path
            // already handled pure-Vorbis files before we got here.
            continue;
        }
        if !tags_seen {
            if data.starts_with(b"OpusTags") {
                tags_seen = true;
                continue;
            }
            return Err(DecodeError::Decode("opus: audio before OpusTags".into()));
        }

        let dec = decoder.as_mut().expect("set when header_seen");
        let n = dec
            .decode_float(data, &mut frame, false)
            .map_err(|e| DecodeError::Decode(format!("opus decode: {e}")))?;
        for i in 0..n {
            for (c, plane) in planes.iter_mut().enumerate() {
                plane.push(frame[i * channels + c]);
            }
        }
        // ogg's granule for a page is carried on that page's last packet;
        // u64::MAX / -1 means "no packet ends on this page".
        let g = packet.absgp_page();
        if g != u64::MAX {
            last_granule = Some(g);
        }
    }

    if !header_seen {
        return Err(DecodeError::UnsupportedFormat);
    }

    // Both trims, in stream order: drop the encoder priming, then cut at the
    // granule-declared end. Guard against a corrupt granule CLAIMING more
    // audio than was decoded — truncate never extends.
    let valid = last_granule
        .map(|g| (g as usize).saturating_sub(pre_skip))
        .unwrap_or(usize::MAX);
    for plane in &mut planes {
        if pre_skip < plane.len() {
            plane.drain(0..pre_skip);
        } else {
            plane.clear();
        }
        plane.truncate(valid.min(plane.len()));
    }

    Ok(SourceAudio { sample_rate: OPUS_RATE, channels: planes })
}

// ── encoding ────────────────────────────────────────────────────────────────

/// Encode planar 48 kHz mono/stereo audio to an Ogg Opus stream (RFC 7845),
/// the mirror of [`decode_opus_bytes`] and held to the same standard it
/// holds others to: `pre_skip` is declared from the encoder's real
/// lookahead, and the final page's granule position marks the true end, so
/// OUR files trim cleanly in any player the way we demand others' files
/// trim in our sampler. One packet per `write_packet` call — the ogg crate
/// does the paging; the cumulative granule rides on every packet and the
/// page keeps its last packet's value, which is exactly the spec's rule.
pub(crate) fn encode_opus(
    audio: &studio_audio::SourceAudio,
    bitrate_bps: i32,
) -> Result<Vec<u8>, crate::EncodeError> {
    use crate::EncodeError;
    use ogg::writing::{PacketWriteEndInfo, PacketWriter};

    if audio.sample_rate != OPUS_RATE {
        // Opus encoders eat 48k (or a few fixed sub-rates); the render side
        // of this codebase already produces 48k, so a mismatch here is a
        // caller bug worth naming, not something to resample over quietly.
        return Err(EncodeError::Unsupported(format!(
            "opus encode wants 48000 Hz input, got {} — render at 48k",
            audio.sample_rate
        )));
    }
    let channels = audio.channels.len();
    if channels == 0 || channels > 2 {
        return Err(EncodeError::Unsupported(format!(
            "opus encode supports mono/stereo (mapping family 0), got {channels} channels"
        )));
    }
    let ch = if channels == 2 { opus::Channels::Stereo } else { opus::Channels::Mono };
    let mut enc = opus::Encoder::new(OPUS_RATE, ch, opus::Application::Audio)
        .map_err(|e| EncodeError::Encode(format!("encoder init: {e}")))?;
    enc.set_bitrate(opus::Bitrate::Bits(bitrate_bps))
        .map_err(|e| EncodeError::Encode(format!("set bitrate: {e}")))?;
    let pre_skip = enc.get_lookahead()
        .map_err(|e| EncodeError::Encode(format!("lookahead: {e}")))? as u64;

    let mut out = Vec::new();
    let mut w = PacketWriter::new(std::io::Cursor::new(&mut out));
    const SERIAL: u32 = 0x5354_5544; // "STUD"

    // OpusHead — its own page, granule 0, per RFC 7845 §3.
    let mut head = Vec::with_capacity(19);
    head.extend_from_slice(b"OpusHead");
    head.push(1); // version
    head.push(channels as u8);
    head.extend_from_slice(&(pre_skip as u16).to_le_bytes());
    head.extend_from_slice(&OPUS_RATE.to_le_bytes()); // original input rate, informational
    head.extend_from_slice(&0i16.to_le_bytes()); // output gain
    head.push(0); // mapping family 0
    w.write_packet(head, SERIAL, PacketWriteEndInfo::EndPage, 0)
        .map_err(|e| EncodeError::Encode(format!("ogg head: {e}")))?;

    // OpusTags — also its own page, granule 0.
    let vendor = b"studio-media";
    let mut tags = Vec::new();
    tags.extend_from_slice(b"OpusTags");
    tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    tags.extend_from_slice(vendor);
    tags.extend_from_slice(&0u32.to_le_bytes()); // no comments
    w.write_packet(tags, SERIAL, PacketWriteEndInfo::EndPage, 0)
        .map_err(|e| EncodeError::Encode(format!("ogg tags: {e}")))?;

    // Audio: 20ms frames, tail zero-padded; the final granule declares the
    // REAL length so the padding never reaches a listener. The encoder is a
    // pipeline `pre_skip` samples deep, so the input must be OVER-fed by
    // that much silence to flush the last real samples out — stop at
    // `frames_total` and the tail of the audio never leaves the encoder,
    // which a frame-aligned input exposes exactly (an unaligned one hides it
    // inside its own padding, which is how the first draft's tests missed it).
    const FRAME: usize = 960;
    let frames_total = audio.frames();
    let to_feed = frames_total + pre_skip as usize;
    let mut interleaved = vec![0f32; FRAME * channels];
    let mut packet = vec![0u8; 4000];
    let mut fed = 0usize;
    loop {
        interleaved.fill(0.0);
        for i in 0..FRAME {
            if fed + i >= frames_total { break; }
            for (c, plane) in audio.channels.iter().enumerate() {
                interleaved[i * channels + c] = plane[fed + i];
            }
        }
        let n = enc.encode_float(&interleaved, &mut packet)
            .map_err(|e| EncodeError::Encode(format!("encode: {e}")))?;
        fed += FRAME;
        let last = fed >= to_feed;
        // Granule counts DECODED samples including pre_skip, capped at the
        // input's true length — everything past the cap (frame padding AND
        // the lookahead flush) is what the cap exists to trim.
        let granule = pre_skip + (fed.min(frames_total)) as u64;
        let info = if last { PacketWriteEndInfo::EndStream } else { PacketWriteEndInfo::NormalPacket };
        w.write_packet(packet[..n].to_vec(), SERIAL, info, granule)
            .map_err(|e| EncodeError::Encode(format!("ogg audio: {e}")))?;
        if last { break; }
    }
    drop(w);
    Ok(out)
}
