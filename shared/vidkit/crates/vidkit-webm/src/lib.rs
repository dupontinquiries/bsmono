//! Minimal WebM/Matroska demuxer.
//!
//! Exists because YouTube's VP9 renditions are WebM, and VP9 is the codec the
//! Fairphone's Venus block decodes in hardware. Without this the phone has no
//! path to its own fixed-function decoder and falls back to dav1d AV1, which
//! is the outcome the whole capability model exists to avoid.
//!
//! Scope: progressive VP8/VP9/AV1 video. No audio track extraction, no lacing,
//! no Cues-driven seeking (see `Mp4Demuxer` for the equivalent note). The
//! index is built by walking every cluster at open time, which for a local file
//! is fine and over HTTP means reading the whole thing — Cues would fix that
//! and is the obvious follow-up.

pub mod ebml;
pub mod segment;

use ebml::{id, ElementIter};
use segment::{find_segment, index_cluster, parse_info, parse_video_track, BlockRef, SegmentInfo, VideoTrack};
use std::time::Duration;
use vidkit_core::codec::VideoProfile;
use vidkit_core::traits::{self as core_traits, MediaSource, Packet};
use vidkit_core::io::{read_exact_at, ByteSource, Error, MemorySource, Result};

pub struct WebmDemuxer<S: ByteSource> {
    src: S,
    info: SegmentInfo,
    track: VideoTrack,
    blocks: Vec<BlockRef>,
    cursor: usize,
}

impl<S: ByteSource> WebmDemuxer<S> {
    pub fn open(mut src: S) -> Result<Self> {
        let file_end = src.size().unwrap_or(u64::MAX);
        let seg = find_segment(&mut src, file_end)?;
        let seg_end = seg.end_or(file_end);

        // Two passes over the segment's top-level children: the first finds
        // Info and Tracks, because block timestamps need TimestampScale and
        // block filtering needs the track number, and neither is guaranteed to
        // appear before the first cluster.
        let mut info = None;
        let mut track = None;
        let mut first_cluster = None;
        let mut it = ElementIter::new(seg.payload_offset, seg_end);
        while let Some(h) = it.next(&mut src)? {
            match h.id {
                id::INFO => info = Some(parse_info(&mut src, &h, seg_end)?),
                id::TRACKS => track = Some(parse_video_track(&mut src, &h, seg_end)?),
                id::CLUSTER => {
                    first_cluster = Some(h);
                    break;
                }
                _ => {}
            }
        }

        let info = info.unwrap_or(SegmentInfo {
            timestamp_scale_ns: segment::DEFAULT_TIMESTAMP_SCALE_NS,
            duration_scaled: None,
        });
        let track = track.ok_or_else(|| Error::Malformed("no Tracks element".into()))?;

        let mut blocks = Vec::new();
        if let Some(c) = first_cluster {
            let mut cluster = c;
            loop {
                let next = index_cluster(
                    &mut src,
                    &cluster,
                    seg_end,
                    info.timestamp_scale_ns,
                    track.number,
                    &mut blocks,
                )?;
                if next >= seg_end {
                    break;
                }
                let h = match ebml::read_element_header(&mut src, next) {
                    Ok(h) => h,
                    Err(Error::Truncated { .. }) => break,
                    Err(e) => return Err(e),
                };
                if h.id != id::CLUSTER {
                    // Cues, Tags, Chapters and so on follow the clusters.
                    break;
                }
                cluster = h;
            }
        }

        // Blocks are stored in decode order and their timestamps are
        // presentation times, so a file with B-frames is not sorted. Sorting
        // by timestamp would reorder decode, which breaks the decoder;
        // playback order is handled downstream by the frame queue.
        Ok(Self {
            src,
            info,
            track,
            blocks,
            cursor: 0,
        })
    }

    /// The underlying byte source.
    ///
    /// Exists so a network source can rebind an expired URL without rebuilding
    /// the demuxer — the cluster index and the read position are still valid,
    /// only the socket's address changed.
    pub fn source_mut(&mut self) -> &mut S {
        &mut self.src
    }

    pub fn track(&self) -> &VideoTrack {
        &self.track
    }

    pub fn info(&self) -> &SegmentInfo {
        &self.info
    }

    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    pub fn position(&self) -> usize {
        self.cursor
    }

    /// Index of the last keyframe at or before `ns`.
    pub fn keyframe_at_or_before(&self, ns: u64) -> usize {
        let mut best = 0usize;
        for (i, b) in self.blocks.iter().enumerate() {
            if b.timestamp_ns > ns {
                break;
            }
            if b.is_key {
                best = i;
            }
        }
        best
    }
}

impl<S: ByteSource + Send> MediaSource for WebmDemuxer<S> {
    fn video_profile(&self) -> Option<VideoProfile> {
        Some(self.track.profile)
    }

    fn dimensions(&self) -> (u32, u32) {
        (self.track.width, self.track.height)
    }

    fn nominal_fps(&self) -> u32 {
        match self.duration() {
            Some(d) if d.as_secs_f64() > 0.0 && self.blocks.len() > 1 => {
                ((self.blocks.len() as f64 / d.as_secs_f64()).round() as u32).clamp(1, 480)
            }
            _ => 30,
        }
    }

    fn duration(&self) -> Option<Duration> {
        self.info.duration_scaled.map(|d| {
            Duration::from_nanos((d * self.info.timestamp_scale_ns as f64) as u64)
        })
    }

    fn seekable(&self) -> bool {
        true
    }

    fn next_packet(&mut self) -> core_traits::Result<Option<Packet>> {
        let b = match self.blocks.get(self.cursor) {
            Some(b) => *b,
            None => return Ok(None),
        };
        self.cursor += 1;

        // VP8, VP9 and AV1 blocks hold the frame exactly as the decoder wants
        // it; unlike MP4's H.264 there is nothing to rewrite.
        let data = read_exact_at(&mut self.src, b.offset, b.size as usize)
            .map_err(|e| core_traits::Error::Backend(e.to_string()))?;

        Ok(Some(Packet {
            data,
            pts: Some(Duration::from_nanos(b.timestamp_ns)),
            dts: Some(Duration::from_nanos(b.timestamp_ns)),
            is_keyframe: b.is_key,
        }))
    }

    fn seek(&mut self, to: Duration) -> core_traits::Result<()> {
        self.cursor = self.keyframe_at_or_before(to.as_nanos() as u64);
        Ok(())
    }
}

/// Convenience for small files already in memory.
pub fn open_bytes(data: Vec<u8>) -> Result<WebmDemuxer<MemorySource>> {
    WebmDemuxer::open(MemorySource(data))
}

/// EBML writer, so tests run against files whose every byte is known.
///
/// Public behind the `fixtures` feature (off by default) for the same reason
/// as `vidkit_mp4::build`: the cross-crate pipeline test needs a real WebM,
/// and a second writer would be free to drift from the parser it exercises.
#[cfg(any(test, feature = "fixtures"))]
pub mod build {

    /// Encode a value as a size VINT of the narrowest sufficient width.
    pub fn vint(value: u64) -> Vec<u8> {
        for width in 1..=8usize {
            let bits = 7 * width;
            let max = (1u64 << bits) - 1;
            // All-ones is reserved for "unknown", so a value that would encode
            // as all-ones needs the next width up.
            if value < max {
                let marker = 1u64 << bits;
                let v = marker | value;
                return v.to_be_bytes()[8 - width..].to_vec();
            }
        }
        panic!("value too large for a VINT");
    }

    /// The all-ones "unknown size" encoding, one byte wide.
    pub fn vint_unknown() -> Vec<u8> {
        vec![0xFF]
    }

    pub fn id_bytes(element_id: u32) -> Vec<u8> {
        let b = element_id.to_be_bytes();
        let first = b.iter().position(|x| *x != 0).unwrap_or(3);
        b[first..].to_vec()
    }

    pub fn elem(element_id: u32, payload: &[u8]) -> Vec<u8> {
        let mut v = id_bytes(element_id);
        v.extend_from_slice(&vint(payload.len() as u64));
        v.extend_from_slice(payload);
        v
    }

    /// An element declaring unknown size; only legal for containers.
    pub fn elem_unknown(element_id: u32, payload: &[u8]) -> Vec<u8> {
        let mut v = id_bytes(element_id);
        v.extend_from_slice(&vint_unknown());
        v.extend_from_slice(payload);
        v
    }

    pub fn uint(element_id: u32, value: u64) -> Vec<u8> {
        let bytes = value.to_be_bytes();
        let first = bytes.iter().position(|b| *b != 0).unwrap_or(7);
        elem(element_id, &bytes[first..])
    }

    pub fn float64(element_id: u32, value: f64) -> Vec<u8> {
        elem(element_id, &value.to_be_bytes())
    }

    pub fn string(element_id: u32, s: &str) -> Vec<u8> {
        elem(element_id, s.as_bytes())
    }

    pub fn cat(parts: &[Vec<u8>]) -> Vec<u8> {
        parts.iter().flatten().copied().collect()
    }

    /// SimpleBlock payload: track VINT, signed int16 relative timestamp, flags.
    pub fn simple_block(track: u64, rel: i16, keyframe: bool, data: &[u8]) -> Vec<u8> {
        let mut p = vint(track);
        p.extend_from_slice(&rel.to_be_bytes());
        p.push(if keyframe { 0x80 } else { 0x00 });
        p.extend_from_slice(data);
        super::build::elem(super::ebml::id::SIMPLE_BLOCK, &p)
    }

    pub fn laced_block(track: u64, rel: i16, data: &[u8]) -> Vec<u8> {
        let mut p = vint(track);
        p.extend_from_slice(&rel.to_be_bytes());
        p.push(0x80 | 0x02); // keyframe + Xiph lacing
        p.extend_from_slice(data);
        elem(super::ebml::id::SIMPLE_BLOCK, &p)
    }

    /// A Block inside a BlockGroup: no keyframe flag in the header.
    pub fn block_group(track: u64, rel: i16, referenced: bool, data: &[u8]) -> Vec<u8> {
        let mut b = vint(track);
        b.extend_from_slice(&rel.to_be_bytes());
        b.push(0x00);
        b.extend_from_slice(data);
        let mut g = elem(super::ebml::id::BLOCK, &b);
        if referenced {
            g.extend_from_slice(&elem(super::ebml::id::REFERENCE_BLOCK, &[0x81]));
        }
        elem(super::ebml::id::BLOCK_GROUP, &g)
    }
}

#[cfg(test)]
mod tests {
    use super::build::*;
    use super::ebml::id;
    use super::*;
    use vidkit_core::codec::Codec;

    fn tracks_element(codec: &str, private: &[u8]) -> Vec<u8> {
        let video = elem(id::VIDEO, &cat(&[uint(id::PIXEL_WIDTH, 1920), uint(id::PIXEL_HEIGHT, 1080)]));
        let mut entry = cat(&[
            uint(id::TRACK_NUMBER, 1),
            uint(id::TRACK_TYPE, 1),
            string(id::CODEC_ID, codec),
            video,
        ]);
        if !private.is_empty() {
            entry.extend_from_slice(&elem(id::CODEC_PRIVATE, private));
        }
        elem(id::TRACKS, &elem(id::TRACK_ENTRY, &entry))
    }

    fn info_element(scale: u64, duration: Option<f64>) -> Vec<u8> {
        let mut p = uint(id::TIMESTAMP_SCALE, scale);
        if let Some(d) = duration {
            p.extend_from_slice(&float64(id::DURATION, d));
        }
        elem(id::INFO, &p)
    }

    /// Frame payloads keyed to their index, so a mis-read offset is obvious.
    fn frame(i: u8, len: usize) -> Vec<u8> {
        vec![0xB0 | (i & 0x0F); len]
    }

    fn file(children: Vec<u8>) -> Vec<u8> {
        let header = elem(id::EBML_HEADER, &[0x42, 0x86, 0x81, 0x01]);
        cat(&[header, elem(id::SEGMENT, &children)])
    }

    /// Two clusters, three frames each, only the first frame of each a keyframe.
    fn standard_file() -> (Vec<u8>, Vec<Vec<u8>>) {
        let mut frames = Vec::new();
        let mut clusters = Vec::new();
        for c in 0..2u8 {
            let mut body = uint(id::TIMESTAMP, (c as u64) * 100);
            for f in 0..3u8 {
                let ix = c * 3 + f;
                let data = frame(ix, 8 + ix as usize);
                frames.push(data.clone());
                body.extend_from_slice(&simple_block(1, (f as i16) * 33, f == 0, &data));
            }
            clusters.push(elem(id::CLUSTER, &body));
        }
        let body = cat(&[
            info_element(1_000_000, Some(200.0)),
            tracks_element("V_VP9", &[]),
            cat(&clusters),
        ]);
        (file(body), frames)
    }

    // ---- EBML primitives -------------------------------------------------

    #[test]
    fn vint_round_trips_through_the_reader() {
        for v in [0u64, 1, 126, 127, 128, 16382, 16383, 100_000, 1 << 40] {
            let encoded = vint(v);
            let mut src = MemorySource(encoded.clone());
            let (got, len) = super::ebml::read_size(&mut src, 0).unwrap();
            assert_eq!(got, Some(v), "value {v} encoded as {encoded:02x?}");
            assert_eq!(len, encoded.len());
        }
    }

    /// The classic mix-up: ids keep their marker bit, sizes strip theirs.
    #[test]
    fn ids_keep_the_marker_bit_and_sizes_strip_it() {
        let mut src = MemorySource(vec![0xA3, 0xA3]);
        let (element_id, id_len) = super::ebml::read_id(&mut src, 0).unwrap();
        assert_eq!(element_id, 0xA3, "id must keep its marker");
        assert_eq!(id_len, 1);

        let (size, _) = super::ebml::read_size(&mut src, 1).unwrap();
        assert_eq!(size, Some(0x23), "size must have its marker stripped");
    }

    #[test]
    fn all_ones_size_reads_as_unknown_not_a_huge_number() {
        let mut src = MemorySource(vec![0xFF]);
        assert_eq!(super::ebml::read_size(&mut src, 0).unwrap().0, None);
        let mut src = MemorySource(vec![0x7F, 0xFF]);
        assert_eq!(super::ebml::read_size(&mut src, 0).unwrap().0, None);
    }

    /// A zero first byte means "more than eight length bits", which is not a
    /// legal VINT. With enough bytes following, a parser that treats it as a
    /// width of 8 happily returns 0 instead of failing — so the fixture has to
    /// supply those bytes, or the test only ever sees a truncation error and
    /// proves nothing.
    #[test]
    fn a_zero_first_byte_is_rejected_even_with_data_following() {
        let mut src = MemorySource(vec![0x00; 16]);
        let e = super::ebml::read_size(&mut src, 0);
        assert!(
            matches!(e, Err(Error::Malformed(_))),
            "expected Malformed, got {e:?}"
        );
        let e = super::ebml::read_id(&mut src, 0);
        assert!(
            matches!(e, Err(Error::Malformed(_))),
            "expected Malformed, got {e:?}"
        );
    }

    #[test]
    fn signed_values_sign_extend_from_their_stored_width() {
        let mut src = MemorySource(vec![0xFF, 0xFE]);
        assert_eq!(super::ebml::read_int(&mut src, 0, 1).unwrap(), -1);
        assert_eq!(super::ebml::read_int(&mut src, 1, 1).unwrap(), -2);
        let mut src = MemorySource(vec![0xFF, 0x9C]);
        assert_eq!(super::ebml::read_int(&mut src, 0, 2).unwrap(), -100);
    }

    // ---- structure -------------------------------------------------------

    #[test]
    fn reads_track_metadata() {
        let (bytes, _) = standard_file();
        let d = open_bytes(bytes).unwrap();
        assert_eq!(d.dimensions(), (1920, 1080));
        assert_eq!(d.video_profile().unwrap().codec, Codec::Vp9);
        assert_eq!(d.track().number, 1);
        assert_eq!(d.block_count(), 6);
    }

    #[test]
    fn duration_is_scaled_by_the_timestamp_scale() {
        let (bytes, _) = standard_file();
        let d = open_bytes(bytes).unwrap();
        // 200 units at 1 ms per unit.
        assert_eq!(d.duration(), Some(Duration::from_millis(200)));
    }

    /// A missing TimestampScale means one millisecond. Assuming anything else
    /// scales the entire timeline and looks like the wrong playback speed.
    #[test]
    fn a_missing_timestamp_scale_defaults_to_one_millisecond() {
        let body = cat(&[
            tracks_element("V_VP9", &[]),
            elem(
                id::CLUSTER,
                &cat(&[uint(id::TIMESTAMP, 5), simple_block(1, 0, true, &frame(0, 4))]),
            ),
        ]);
        let mut d = open_bytes(file(body)).unwrap();
        assert_eq!(d.info().timestamp_scale_ns, 1_000_000);
        let p = d.next_packet().unwrap().unwrap();
        assert_eq!(p.pts, Some(Duration::from_millis(5)));
    }

    #[test]
    fn a_non_default_timestamp_scale_is_applied() {
        let body = cat(&[
            info_element(1_000, None), // microsecond ticks
            tracks_element("V_VP9", &[]),
            elem(
                id::CLUSTER,
                &cat(&[uint(id::TIMESTAMP, 5000), simple_block(1, 0, true, &frame(0, 4))]),
            ),
        ]);
        let mut d = open_bytes(file(body)).unwrap();
        let p = d.next_packet().unwrap().unwrap();
        assert_eq!(p.pts, Some(Duration::from_millis(5)));
    }

    // ---- blocks ----------------------------------------------------------

    #[test]
    fn frames_are_read_from_the_right_offsets() {
        let (bytes, frames) = standard_file();
        let mut d = open_bytes(bytes).unwrap();
        for (i, expected) in frames.iter().enumerate() {
            let p = d.next_packet().unwrap().unwrap();
            assert_eq!(&p.data, expected, "frame {i} read from the wrong offset");
        }
        assert!(d.next_packet().unwrap().is_none());
    }

    #[test]
    fn timestamps_combine_cluster_and_block_values() {
        let (bytes, _) = standard_file();
        let mut d = open_bytes(bytes).unwrap();
        let mut ts = Vec::new();
        while let Some(p) = d.next_packet().unwrap() {
            ts.push(p.pts.unwrap().as_millis() as u64);
        }
        assert_eq!(ts, vec![0, 33, 66, 100, 133, 166]);
    }

    /// The relative timestamp is signed; reading it unsigned turns a small
    /// negative into ~65000 ticks and throws one frame far into the future.
    #[test]
    fn a_negative_relative_timestamp_moves_backwards() {
        let body = cat(&[
            info_element(1_000_000, None),
            tracks_element("V_VP9", &[]),
            elem(
                id::CLUSTER,
                &cat(&[
                    uint(id::TIMESTAMP, 100),
                    simple_block(1, -20, true, &frame(0, 4)),
                ]),
            ),
        ]);
        let mut d = open_bytes(file(body)).unwrap();
        let p = d.next_packet().unwrap().unwrap();
        assert_eq!(p.pts, Some(Duration::from_millis(80)));
    }

    #[test]
    fn blocks_for_other_tracks_are_skipped() {
        let body = cat(&[
            info_element(1_000_000, None),
            tracks_element("V_VP9", &[]),
            elem(
                id::CLUSTER,
                &cat(&[
                    uint(id::TIMESTAMP, 0),
                    simple_block(1, 0, true, &frame(0, 6)),
                    // Track 2 is the audio track; its blocks must not enter the
                    // video index.
                    simple_block(2, 0, true, &frame(9, 30)),
                    simple_block(1, 33, false, &frame(1, 6)),
                ]),
            ),
        ]);
        let d = open_bytes(file(body)).unwrap();
        assert_eq!(d.block_count(), 2);
    }

    /// A Block inside a BlockGroup has no keyframe flag; it is a keyframe iff
    /// no ReferenceBlock accompanies it.
    #[test]
    fn block_group_keyframes_are_inferred_from_the_reference_block() {
        let body = cat(&[
            info_element(1_000_000, None),
            tracks_element("V_VP9", &[]),
            elem(
                id::CLUSTER,
                &cat(&[
                    uint(id::TIMESTAMP, 0),
                    block_group(1, 0, false, &frame(0, 6)),
                    block_group(1, 33, true, &frame(1, 6)),
                ]),
            ),
        ]);
        let mut d = open_bytes(file(body)).unwrap();
        assert_eq!(d.block_count(), 2);
        // No ReferenceBlock -> keyframe; one present -> not.
        assert!(d.next_packet().unwrap().unwrap().is_keyframe);
        assert!(!d.next_packet().unwrap().unwrap().is_keyframe);
    }

    #[test]
    fn a_laced_video_block_is_refused_rather_than_spliced() {
        let body = cat(&[
            info_element(1_000_000, None),
            tracks_element("V_VP9", &[]),
            elem(
                id::CLUSTER,
                &cat(&[uint(id::TIMESTAMP, 0), laced_block(1, 0, &frame(0, 20))]),
            ),
        ]);
        let e = match open_bytes(file(body)) {
            Err(e) => e,
            Ok(_) => panic!("accepted a laced video block"),
        };
        assert!(format!("{e}").contains("lacing"), "unhelpful error: {e}");
    }

    // ---- unknown sizes ---------------------------------------------------

    /// Live-muxed WebM writes an unknown-size Segment. It is not corrupt.
    #[test]
    fn an_unknown_size_segment_is_read_to_end_of_file() {
        let body = cat(&[
            info_element(1_000_000, None),
            tracks_element("V_VP9", &[]),
            elem(
                id::CLUSTER,
                &cat(&[
                    uint(id::TIMESTAMP, 0),
                    simple_block(1, 0, true, &frame(0, 6)),
                    simple_block(1, 33, false, &frame(1, 6)),
                ]),
            ),
        ]);
        let header = elem(id::EBML_HEADER, &[0x42, 0x86, 0x81, 0x01]);
        let f = cat(&[header, elem_unknown(id::SEGMENT, &body)]);
        let d = open_bytes(f).unwrap();
        assert_eq!(d.block_count(), 2);
    }

    #[test]
    fn an_unknown_size_cluster_terminates_at_the_next_sibling() {
        let c1 = elem_unknown(
            id::CLUSTER,
            &cat(&[
                uint(id::TIMESTAMP, 0),
                simple_block(1, 0, true, &frame(0, 6)),
            ]),
        );
        let c2 = elem(
            id::CLUSTER,
            &cat(&[
                uint(id::TIMESTAMP, 100),
                simple_block(1, 0, true, &frame(1, 6)),
            ]),
        );
        let body = cat(&[
            info_element(1_000_000, None),
            tracks_element("V_VP9", &[]),
            c1,
            c2,
        ]);
        let header = elem(id::EBML_HEADER, &[0x42, 0x86, 0x81, 0x01]);
        let mut d = open_bytes(cat(&[header, elem_unknown(id::SEGMENT, &body)])).unwrap();
        assert_eq!(d.block_count(), 2, "second cluster was not reached");
        let ts: Vec<u64> = (0..2)
            .map(|_| d.next_packet().unwrap().unwrap().pts.unwrap().as_millis() as u64)
            .collect();
        assert_eq!(ts, vec![0, 100]);
    }

    #[test]
    fn a_non_container_with_unknown_size_is_rejected() {
        let body = cat(&[
            tracks_element("V_VP9", &[]),
            elem_unknown(id::TIMESTAMP, &[0x00]),
        ]);
        assert!(open_bytes(file(body)).is_err());
    }

    // ---- seeking ---------------------------------------------------------

    #[test]
    fn seek_lands_on_the_preceding_keyframe() {
        let (bytes, _) = standard_file();
        let mut d = open_bytes(bytes).unwrap();
        // Keyframes at 0 ms and 100 ms; ask for 140 ms.
        d.seek(Duration::from_millis(140)).unwrap();
        assert_eq!(d.position(), 3);
        assert!(d.next_packet().unwrap().unwrap().is_keyframe);
    }

    #[test]
    fn seeking_before_the_first_keyframe_stays_at_zero() {
        let (bytes, _) = standard_file();
        let mut d = open_bytes(bytes).unwrap();
        d.seek(Duration::ZERO).unwrap();
        assert_eq!(d.position(), 0);
    }

    // ---- codecs ----------------------------------------------------------

    #[test]
    fn vp9_bit_depth_comes_from_codec_private_when_present() {
        // Feature id 3, length 1, value 10.
        let (_, _) = standard_file();
        let body = cat(&[
            info_element(1_000_000, None),
            tracks_element("V_VP9", &[3, 1, 10]),
            elem(
                id::CLUSTER,
                &cat(&[uint(id::TIMESTAMP, 0), simple_block(1, 0, true, &frame(0, 4))]),
            ),
        ]);
        let d = open_bytes(file(body)).unwrap();
        assert_eq!(d.video_profile().unwrap().bit_depth, 10);
    }

    #[test]
    fn a_missing_codec_private_defaults_to_eight_bit() {
        let (bytes, _) = standard_file();
        let d = open_bytes(bytes).unwrap();
        assert_eq!(d.video_profile().unwrap().bit_depth, 8);
    }

    #[test]
    fn av1_in_webm_is_recognised() {
        let body = cat(&[
            info_element(1_000_000, None),
            tracks_element("V_AV1", &[0x81, 0x00, 0x00]),
            elem(
                id::CLUSTER,
                &cat(&[uint(id::TIMESTAMP, 0), simple_block(1, 0, true, &frame(0, 4))]),
            ),
        ]);
        let d = open_bytes(file(body)).unwrap();
        assert_eq!(d.video_profile().unwrap().codec, Codec::Av1);
    }

    #[test]
    fn h264_in_webm_is_refused_with_a_useful_message() {
        let body = cat(&[
            info_element(1_000_000, None),
            tracks_element("V_MPEG4/ISO/AVC", &[1, 0x64, 0, 0x28, 0xFF, 0xE0]),
        ]);
        let e = match open_bytes(file(body)) {
            Err(e) => e,
            Ok(_) => panic!("accepted H.264 in WebM"),
        };
        assert!(format!("{e}").contains("Annex-B"), "unhelpful error: {e}");
    }

    #[test]
    fn a_nul_padded_codec_id_still_matches() {
        let video = elem(
            id::VIDEO,
            &cat(&[uint(id::PIXEL_WIDTH, 640), uint(id::PIXEL_HEIGHT, 480)]),
        );
        let entry = cat(&[
            uint(id::TRACK_NUMBER, 1),
            uint(id::TRACK_TYPE, 1),
            elem(id::CODEC_ID, b"V_VP9\0\0\0"),
            video,
        ]);
        let body = cat(&[
            info_element(1_000_000, None),
            elem(id::TRACKS, &elem(id::TRACK_ENTRY, &entry)),
            elem(
                id::CLUSTER,
                &cat(&[uint(id::TIMESTAMP, 0), simple_block(1, 0, true, &frame(0, 4))]),
            ),
        ]);
        let d = open_bytes(file(body)).unwrap();
        assert_eq!(d.video_profile().unwrap().codec, Codec::Vp9);
    }

    /// Matching on the codec id alone would pick up a non-video track that
    /// happens to name a video codec.
    #[test]
    fn a_non_video_track_type_is_skipped() {
        let video = elem(id::VIDEO, &cat(&[uint(id::PIXEL_WIDTH, 1), uint(id::PIXEL_HEIGHT, 1)]));
        let decoy = cat(&[
            uint(id::TRACK_NUMBER, 1),
            uint(id::TRACK_TYPE, 2), // audio
            string(id::CODEC_ID, "V_VP9"),
            video.clone(),
        ]);
        let real = cat(&[
            uint(id::TRACK_NUMBER, 7),
            uint(id::TRACK_TYPE, 1),
            string(id::CODEC_ID, "V_VP9"),
            elem(
                id::VIDEO,
                &cat(&[uint(id::PIXEL_WIDTH, 1280), uint(id::PIXEL_HEIGHT, 720)]),
            ),
        ]);
        let tracks = elem(
            id::TRACKS,
            &cat(&[elem(id::TRACK_ENTRY, &decoy), elem(id::TRACK_ENTRY, &real)]),
        );
        let body = cat(&[
            info_element(1_000_000, None),
            tracks,
            elem(
                id::CLUSTER,
                &cat(&[uint(id::TIMESTAMP, 0), simple_block(7, 0, true, &frame(0, 4))]),
            ),
        ]);
        let d = open_bytes(file(body)).unwrap();
        assert_eq!(d.track().number, 7);
        assert_eq!(d.dimensions(), (1280, 720));
        assert_eq!(d.block_count(), 1);
    }

    // ---- malformed -------------------------------------------------------

    #[test]
    fn a_file_without_a_segment_reports_that() {
        let f = elem(id::EBML_HEADER, &[0x42, 0x86, 0x81, 0x01]);
        assert!(open_bytes(f).is_err());
    }

    /// A child declaring more size than its parent holds must be reported as
    /// exactly that. Asserting only `is_err()` is not enough: dropping the
    /// bounds check still errors, just later and with a misleading "no Tracks
    /// element" — so the message is the assertion.
    #[test]
    fn an_element_overrunning_its_parent_is_rejected() {
        let mut inner = id_bytes(id::TRACKS);
        inner.extend_from_slice(&vint(10_000)); // far larger than the segment
        inner.extend_from_slice(&[0u8; 4]);
        let f = cat(&[
            elem(id::EBML_HEADER, &[0x42, 0x86, 0x81, 0x01]),
            elem(id::SEGMENT, &inner),
        ]);
        let e = match open_bytes(f) {
            Err(e) => e,
            Ok(_) => panic!("accepted an element overrunning its parent"),
        };
        assert!(
            format!("{e}").contains("past its container limit"),
            "wrong diagnosis: {e}"
        );
    }

    /// Cutting the tail off must not silently yield a short frame full of
    /// whatever happened to be there. Either the open fails, or the affected
    /// read fails; returning fewer bytes than the block header promised and
    /// calling it a frame is the failure mode this guards.
    #[test]
    fn a_truncated_file_does_not_read_short_frames_silently() {
        let (full, frames) = standard_file();
        let mut bytes = full.clone();
        bytes.truncate(bytes.len() - 5);

        let mut d = match open_bytes(bytes) {
            Err(_) => return, // rejected at open; also acceptable
            Ok(d) => d,
        };

        let mut read = 0usize;
        let mut errored = false;
        loop {
            match d.next_packet() {
                Ok(Some(p)) => {
                    assert_eq!(
                        p.data.len(),
                        frames[read].len(),
                        "frame {read} came back short instead of erroring"
                    );
                    read += 1;
                }
                Ok(None) => break,
                Err(_) => {
                    errored = true;
                    break;
                }
            }
        }
        assert!(
            errored || read < frames.len(),
            "a truncated file read to completion with every frame intact"
        );
    }
}
