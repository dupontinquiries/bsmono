//! Segment structure: Info, Tracks, and the cluster/block index.

use crate::ebml::*;
use vidkit_core::codec::{Chroma, Codec, VideoProfile};
use vidkit_core::io::{read_exact_at, ByteSource, Error, Result};

/// Default `TimestampScale` when Info omits it: one millisecond in
/// nanoseconds. Assuming a different default silently scales the whole
/// timeline, which looks like the video playing at the wrong speed.
pub const DEFAULT_TIMESTAMP_SCALE_NS: u64 = 1_000_000;

#[derive(Clone, Debug)]
pub struct SegmentInfo {
    pub timestamp_scale_ns: u64,
    /// Duration in timestamp-scale units. Stored as a float in the file.
    pub duration_scaled: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct VideoTrack {
    pub number: u64,
    pub codec_id: String,
    pub width: u32,
    pub height: u32,
    pub codec_private: Vec<u8>,
    pub profile: VideoProfile,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BlockRef {
    /// Offset of the frame data itself, past the block header.
    pub offset: u64,
    pub size: u32,
    /// Absolute timestamp in nanoseconds.
    pub timestamp_ns: u64,
    pub is_key: bool,
}

fn codec_from_id(codec_id: &str, private: &[u8]) -> Result<VideoProfile> {
    let codec = match codec_id {
        "V_VP9" => Codec::Vp9,
        "V_VP8" => Codec::Vp8,
        "V_AV1" => Codec::Av1,
        // Legal in Matroska and essentially absent from YouTube. Supporting it
        // means the avcC -> Annex-B conversion from vidkit-mp4; refusing is
        // better than emitting length-prefixed NALs a V4L2 decoder will
        // silently ignore.
        "V_MPEG4/ISO/AVC" => {
            return Err(Error::Unsupported(
                "H.264 in WebM needs Annex-B conversion; use the MP4 rendition".into(),
            ))
        }
        "V_MPEGH/ISO/HEVC" => {
            return Err(Error::Unsupported("HEVC in WebM is not handled".into()))
        }
        other => return Err(Error::Unsupported(format!("codec {other}"))),
    };

    // VP9's CodecPrivate, when present, is a sequence of [id][len][value]
    // triples; feature id 3 is bit depth. It is frequently absent, and 8 bits
    // is the safe default in the same direction as elsewhere: under-reporting
    // depth falls back to software rather than rejecting a decodable stream.
    let mut bit_depth = 8u8;
    if codec == Codec::Vp9 {
        let mut i = 0usize;
        while i + 2 < private.len() {
            let fid = private[i];
            let len = private[i + 1] as usize;
            if i + 2 + len > private.len() {
                break;
            }
            if fid == 3 && len >= 1 {
                bit_depth = private[i + 2];
            }
            i += 2 + len;
        }
    }
    if codec == Codec::Av1 && private.len() >= 3 {
        let b2 = private[2];
        bit_depth = if (b2 >> 5) & 1 == 1 {
            12
        } else if (b2 >> 6) & 1 == 1 {
            10
        } else {
            8
        };
    }

    Ok(VideoProfile {
        codec,
        profile: None,
        bit_depth: if bit_depth == 0 { 8 } else { bit_depth },
        chroma: Chroma::Yuv420,
        level: None,
    })
}

/// Locate the Segment element.
pub fn find_segment<S: ByteSource + ?Sized>(src: &mut S, limit: u64) -> Result<ElementHeader> {
    let mut it = ElementIter::new(0, limit);
    while let Some(h) = it.next(src)? {
        if h.id == id::SEGMENT {
            return Ok(h);
        }
        if h.id != id::EBML_HEADER {
            return Err(Error::Malformed(format!(
                "expected EBML header or Segment, found {:#x}",
                h.id
            )));
        }
    }
    Err(Error::Malformed("no Segment element".into()))
}

pub fn parse_info<S: ByteSource + ?Sized>(src: &mut S, info: &ElementHeader, limit: u64) -> Result<SegmentInfo> {
    let mut scale = DEFAULT_TIMESTAMP_SCALE_NS;
    let mut duration = None;
    let mut it = ElementIter::new(info.payload_offset, info.end_or(limit));
    while let Some(h) = it.next(src)? {
        let len = h.payload_len.unwrap_or(0);
        match h.id {
            id::TIMESTAMP_SCALE => scale = read_uint(src, h.payload_offset, len)?,
            id::DURATION => duration = Some(read_float(src, h.payload_offset, len)?),
            _ => {}
        }
    }
    Ok(SegmentInfo {
        timestamp_scale_ns: scale.max(1),
        duration_scaled: duration,
    })
}

/// First video track in Tracks.
pub fn parse_video_track<S: ByteSource + ?Sized>(
    src: &mut S,
    tracks: &ElementHeader,
    limit: u64,
) -> Result<VideoTrack> {
    let mut it = ElementIter::new(tracks.payload_offset, tracks.end_or(limit));
    while let Some(entry) = it.next(src)? {
        if entry.id != id::TRACK_ENTRY {
            continue;
        }
        let entry_end = entry.end_or(limit);

        let mut number = 0u64;
        let mut track_type = 0u64;
        let mut codec_id = String::new();
        let mut codec_private = Vec::new();
        let mut width = 0u32;
        let mut height = 0u32;

        let mut ch = ElementIter::new(entry.payload_offset, entry_end);
        while let Some(f) = ch.next(src)? {
            let len = f.payload_len.unwrap_or(0);
            match f.id {
                id::TRACK_NUMBER => number = read_uint(src, f.payload_offset, len)?,
                id::TRACK_TYPE => track_type = read_uint(src, f.payload_offset, len)?,
                id::CODEC_ID => {
                    let b = read_exact_at(src, f.payload_offset, len as usize)?;
                    // CodecID is ASCII and sometimes NUL-padded to a fixed
                    // width by older muxers; trailing NULs would fail the
                    // string match.
                    codec_id = String::from_utf8_lossy(&b)
                        .trim_end_matches('\0')
                        .to_string();
                }
                id::CODEC_PRIVATE => {
                    codec_private = read_exact_at(src, f.payload_offset, len as usize)?
                }
                id::VIDEO => {
                    let mut v = ElementIter::new(f.payload_offset, f.end_or(entry_end));
                    while let Some(vf) = v.next(src)? {
                        let vlen = vf.payload_len.unwrap_or(0);
                        match vf.id {
                            id::PIXEL_WIDTH => {
                                width = read_uint(src, vf.payload_offset, vlen)? as u32
                            }
                            id::PIXEL_HEIGHT => {
                                height = read_uint(src, vf.payload_offset, vlen)? as u32
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }

        // TrackType 1 is video. Matching on the codec id instead would also
        // pick up a thumbnail or timed-metadata track.
        if track_type != 1 {
            continue;
        }

        let profile = codec_from_id(&codec_id, &codec_private)?;
        return Ok(VideoTrack {
            number,
            codec_id,
            width,
            height,
            codec_private,
            profile,
        });
    }
    Err(Error::Malformed("no video track in Tracks".into()))
}

fn is_cluster_child(element_id: u32) -> bool {
    matches!(
        element_id,
        id::TIMESTAMP | id::SIMPLE_BLOCK | id::BLOCK_GROUP | 0xA7 /* Position */ | 0xAB /* PrevSize */
    )
}

/// Parse a block header, returning (track number, relative timestamp, flags,
/// bytes consumed).
fn block_header<S: ByteSource + ?Sized>(src: &mut S, at: u64) -> Result<(u64, i16, u8, usize)> {
    // The track number is a size-style VINT: marker stripped.
    let (track, tlen) = read_size(src, at)?;
    let track = track.ok_or_else(|| Error::Malformed("block track number is unknown-size".into()))?;
    let rest = read_exact_at(src, at + tlen as u64, 3)?;
    let rel = i16::from_be_bytes([rest[0], rest[1]]);
    Ok((track, rel, rest[2], tlen + 3))
}

/// Index every block belonging to `track_number`, starting at `start`.
///
/// Returns the offset just past the cluster, so the caller can continue even
/// when the cluster declared an unknown size — which live-muxed WebM does.
pub fn index_cluster<S: ByteSource + ?Sized>(
    src: &mut S,
    cluster: &ElementHeader,
    file_end: u64,
    timestamp_scale_ns: u64,
    track_number: u64,
    out: &mut Vec<BlockRef>,
) -> Result<u64> {
    let hard_limit = cluster.end_or(file_end);
    let mut cluster_ts = 0u64;
    let mut cursor = cluster.payload_offset;

    while cursor < hard_limit {
        let h = match read_element_header(src, cursor) {
            Ok(h) => h,
            Err(Error::Truncated { .. }) => return Ok(file_end),
            Err(e) => return Err(e),
        };
        // An id that is not a cluster child terminates an unknown-size cluster.
        if !is_cluster_child(h.id) {
            return Ok(cursor);
        }
        let len = h.payload_len.ok_or_else(|| {
            Error::Malformed(format!("cluster child {:#x} has unknown size", h.id))
        })?;
        let end = h.payload_offset + len;
        if end > hard_limit {
            return Err(Error::Malformed(format!(
                "cluster child {:#x} at {cursor} overruns the cluster",
                h.id
            )));
        }

        match h.id {
            id::TIMESTAMP => cluster_ts = read_uint(src, h.payload_offset, len)?,
            id::SIMPLE_BLOCK => {
                let (track, rel, flags, hdr) = block_header(src, h.payload_offset)?;
                if track == track_number {
                    // Lacing packs several frames into one block. It is used
                    // for audio and effectively never for video; unpacking it
                    // wrongly would splice frames together, so refuse.
                    if flags & 0x06 != 0 {
                        return Err(Error::Unsupported(
                            "laced video block; unpacking lacing is not implemented".into(),
                        ));
                    }
                    out.push(BlockRef {
                        offset: h.payload_offset + hdr as u64,
                        size: (len - hdr as u64) as u32,
                        timestamp_ns: abs_ts(cluster_ts, rel, timestamp_scale_ns),
                        // Only SimpleBlock carries the keyframe flag.
                        is_key: flags & 0x80 != 0,
                    });
                }
            }
            id::BLOCK_GROUP => {
                let mut block: Option<(u64, u64, u64, i16)> = None;
                let mut has_reference = false;
                let mut g = ElementIter::new(h.payload_offset, end);
                while let Some(gf) = g.next(src)? {
                    let glen = gf.payload_len.unwrap_or(0);
                    match gf.id {
                        id::BLOCK => {
                            let (track, rel, _flags, hdr) = block_header(src, gf.payload_offset)?;
                            block = Some((
                                track,
                                gf.payload_offset + hdr as u64,
                                glen - hdr as u64,
                                rel,
                            ));
                        }
                        // A Block inside a BlockGroup has no keyframe flag.
                        // It is a keyframe iff no ReferenceBlock is present.
                        id::REFERENCE_BLOCK => has_reference = true,
                        _ => {}
                    }
                }
                if let Some((track, off, size, rel)) = block {
                    if track == track_number {
                        out.push(BlockRef {
                            offset: off,
                            size: size as u32,
                            timestamp_ns: abs_ts(cluster_ts, rel, timestamp_scale_ns),
                            is_key: !has_reference,
                        });
                    }
                }
            }
            _ => {}
        }
        cursor = end;
    }
    Ok(hard_limit)
}

/// Cluster timestamp plus the block's signed relative offset, scaled.
///
/// The relative value is genuinely signed: a block may precede its cluster's
/// timestamp. Reading it unsigned turns a small negative into ~65000 ticks and
/// throws one frame far into the future.
fn abs_ts(cluster_ts: u64, rel: i16, scale_ns: u64) -> u64 {
    let ticks = if rel >= 0 {
        cluster_ts.saturating_add(rel as u64)
    } else {
        cluster_ts.saturating_sub((-(rel as i32)) as u64)
    };
    ticks.saturating_mul(scale_ns)
}
