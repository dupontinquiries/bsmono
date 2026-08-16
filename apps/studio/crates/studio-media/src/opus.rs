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
