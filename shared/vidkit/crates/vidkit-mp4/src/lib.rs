//! Minimal ISOBMFF demuxer.
//!
//! Enough to play a progressive MP4 — the shape yt-dlp hands back for AV1 and
//! H.264, and what most local files are. Not a general MP4 library: no
//! fragmented (`moof`) support, no edit lists, no audio track extraction yet.
//! Each of those is a real gap and is called out where it bites.

pub mod annexb;
pub mod boxes;
pub mod demux;
pub mod moov;

pub use boxes::{ByteSource, Error, MemorySource, Result};
pub use demux::Mp4Demuxer;
pub use moov::{Sample, VideoTrack};

/// A tiny MP4 writer, so tests exercise the parser against files whose every
/// byte is known. Building fixtures here rather than checking in a binary
/// means a failing test says which field is wrong.
///
/// Public behind the `fixtures` feature (off by default, so ordinary builds do
/// not carry it) because the cross-crate pipeline test needs a real MP4 too,
/// and a second copy of this would be free to drift from the parser it is
/// meant to exercise.
#[cfg(any(test, feature = "fixtures"))]
pub mod build {

    pub fn bx(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = Vec::with_capacity(payload.len() + 8);
        v.extend_from_slice(&((payload.len() + 8) as u32).to_be_bytes());
        v.extend_from_slice(kind);
        v.extend_from_slice(payload);
        v
    }

    /// A box using the 64-bit `largesize` form.
    pub fn bx64(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = Vec::with_capacity(payload.len() + 16);
        v.extend_from_slice(&1u32.to_be_bytes());
        v.extend_from_slice(kind);
        v.extend_from_slice(&((payload.len() + 16) as u64).to_be_bytes());
        v.extend_from_slice(payload);
        v
    }

    pub fn cat(parts: &[Vec<u8>]) -> Vec<u8> {
        parts.iter().flatten().copied().collect()
    }

    pub fn u32b(v: u32) -> Vec<u8> {
        v.to_be_bytes().to_vec()
    }

    /// tkhd v0. Width and height are 16.16 fixed point at payload offsets 76
    /// and 80 — after the 36-byte transform matrix, which is easy to miscount.
    pub fn tkhd(track_id: u32, width: u32, height: u32) -> Vec<u8> {
        let mut p = vec![0u8; 84];
        p[12..16].copy_from_slice(&track_id.to_be_bytes());
        p[76..80].copy_from_slice(&(width << 16).to_be_bytes());
        p[80..84].copy_from_slice(&(height << 16).to_be_bytes());
        bx(b"tkhd", &p)
    }

    pub fn mdhd(timescale: u32, duration: u32) -> Vec<u8> {
        let mut p = vec![0u8; 24];
        p[12..16].copy_from_slice(&timescale.to_be_bytes());
        p[16..20].copy_from_slice(&duration.to_be_bytes());
        bx(b"mdhd", &p)
    }

    pub fn hdlr(handler: &[u8; 4]) -> Vec<u8> {
        let mut p = vec![0u8; 24];
        p[8..12].copy_from_slice(handler);
        bx(b"hdlr", &p)
    }

    /// A VisualSampleEntry: 78 fixed bytes, then the codec config box.
    pub fn visual_entry(kind: &[u8; 4], w: u32, h: u32, config: Vec<u8>) -> Vec<u8> {
        let mut p = vec![0u8; 78];
        p[24..26].copy_from_slice(&(w as u16).to_be_bytes());
        p[26..28].copy_from_slice(&(h as u16).to_be_bytes());
        p.extend_from_slice(&config);
        bx(kind, &p)
    }

    pub fn stsd(entry: Vec<u8>) -> Vec<u8> {
        let mut p = vec![0u8; 4];
        p.extend_from_slice(&1u32.to_be_bytes());
        p.extend_from_slice(&entry);
        bx(b"stsd", &p)
    }

    pub fn table(kind: &[u8; 4], entries: &[Vec<u8>]) -> Vec<u8> {
        let mut p = vec![0u8; 4];
        p.extend_from_slice(&(entries.len() as u32).to_be_bytes());
        for e in entries {
            p.extend_from_slice(e);
        }
        bx(kind, &p)
    }

    pub fn stsz(sizes: &[u32]) -> Vec<u8> {
        let mut p = vec![0u8; 4];
        p.extend_from_slice(&0u32.to_be_bytes()); // per-sample sizes follow
        p.extend_from_slice(&(sizes.len() as u32).to_be_bytes());
        for s in sizes {
            p.extend_from_slice(&s.to_be_bytes());
        }
        bx(b"stsz", &p)
    }

    /// A minimal but structurally correct `avcC` with one SPS and one PPS.
    pub fn avcc(length_size: usize, sps: &[u8], pps: &[u8]) -> Vec<u8> {
        let mut p = vec![1u8, 0x64, 0x00, 0x28];
        // Top six bits are reserved 1s. A parser that forgets to mask gets 255.
        p.push(0xFC | ((length_size - 1) as u8));
        p.push(0xE0 | 1); // reserved bits | numOfSequenceParameterSets = 1
        p.extend_from_slice(&(sps.len() as u16).to_be_bytes());
        p.extend_from_slice(sps);
        p.push(1); // numOfPictureParameterSets
        p.extend_from_slice(&(pps.len() as u16).to_be_bytes());
        p.extend_from_slice(pps);
        bx(b"avcC", &p)
    }
}

#[cfg(test)]
mod tests {
    /// Two AV1 samples, both sync, with the given av1C payload.
    fn av1_file(av1c_payload: &[u8]) -> Vec<u8> {
        use crate::build::*;
        let n = 2u32;
        let payloads: Vec<Vec<u8>> = (0..n).map(|_| vec![0xA0u8; 6]).collect();
        let sizes: Vec<u32> = payloads.iter().map(|p| p.len() as u32).collect();
        let mut mdat_body = Vec::new();
        let mut offs = Vec::new();
        for p in &payloads {
            offs.push(mdat_body.len());
            mdat_body.extend_from_slice(p);
        }
        let mdat = bx(b"mdat", &mdat_body);
        let ftyp = bx(b"ftyp", b"isom\0\0\x02\0isomiso2av01mp41");
        let av1c = bx(b"av1C", av1c_payload);
        let make = |at: u64| {
            let stco = table(b"stco", &offs.iter().map(|o| u32b((at + *o as u64) as u32)).collect::<Vec<_>>());
            let stbl = bx(b"stbl", &cat(&[
                stsd(visual_entry(b"av01", 64, 64, av1c.clone())),
                table(b"stts", &[cat(&[u32b(n), u32b(1000)])]),
                table(b"stsc", &[cat(&[u32b(1), u32b(1), u32b(1)])]),
                stsz(&sizes),
                stco,
            ]));
            let minf = bx(b"minf", &cat(&[bx(b"vmhd", &[0u8; 12]), stbl]));
            let mdia = bx(b"mdia", &cat(&[mdhd(30_000, n * 1000), hdlr(b"vide"), minf]));
            let trak = bx(b"trak", &cat(&[tkhd(1, 64, 64), mdia]));
            bx(b"moov", &cat(&[bx(b"mvhd", &[0u8; 100]), trak]))
        };
        let probe = make(0);
        let at = (ftyp.len() + probe.len() + 8) as u64;
        let moov = make(at);
        assert_eq!(moov.len(), probe.len());
        cat(&[ftyp, moov, mdat])
    }

    use super::build::*;
    use super::*;
    use crate::annexb::{parse_avcc, to_annex_b, START_CODE};
    use std::time::Duration;
    use vidkit_core::codec::Codec;
    use vidkit_core::traits::MediaSource;

    const SPS: &[u8] = &[0x67, 0x64, 0x00, 0x28, 0xAA];
    const PPS: &[u8] = &[0x68, 0xEE, 0x3C, 0xB0];

    struct Spec {
        /// (size, is_sync) per sample, in file order.
        samples: Vec<(u32, bool)>,
        /// stsc entries: (first_chunk, samples_per_chunk, desc_index)
        stsc: Vec<(u32, u32, u32)>,
        /// stts entries: (count, delta)
        stts: Vec<(u32, u32)>,
        ctts: Option<Vec<(u32, i32)>>,
        timescale: u32,
        codec: [u8; 4],
        config: Vec<u8>,
        use_co64: bool,
        /// Put `moov` after `mdat`, as a non-faststart recorder would.
        moov_last: bool,
    }

    impl Default for Spec {
        fn default() -> Self {
            Self {
                samples: (0..6).map(|i| (10 + i, i % 3 == 0)).collect(),
                stsc: vec![(1, 2, 1)],
                stts: vec![(6, 1000)],
                ctts: None,
                timescale: 30_000,
                codec: *b"avc1",
                config: avcc(4, SPS, PPS),
                use_co64: false,
                moov_last: false,
            }
        }
    }

    /// Build a complete file and return (bytes, per-sample payloads).
    fn build(spec: &Spec) -> (Vec<u8>, Vec<Vec<u8>>) {
        let ftyp = bx(b"ftyp", b"isom\0\0\x02\0isomiso2avc1mp41");

        // Sample payloads: a single length-prefixed NAL each, so the Annex-B
        // conversion has something real to chew on. Body is a repeated byte
        // keyed to the sample index, so a mis-mapped offset is obvious.
        let mut payloads = Vec::new();
        for (i, (size, _)) in spec.samples.iter().enumerate() {
            let body_len = (*size as usize).saturating_sub(4).max(1);
            let mut s = Vec::with_capacity(*size as usize);
            s.extend_from_slice(&(body_len as u32).to_be_bytes());
            s.extend(std::iter::repeat(0xA0 | (i as u8 & 0x0F)).take(body_len));
            s.truncate(*size as usize);
            payloads.push(s);
        }

        // Group samples into chunks exactly as stsc describes, so chunk offsets
        // are genuinely non-contiguous in the way a real muxer produces.
        let mut chunk_layout: Vec<Vec<usize>> = Vec::new();
        {
            let mut sample = 0usize;
            for (ix, (first, per, _)) in spec.stsc.iter().enumerate() {
                // Saturating throughout: the builder must be able to emit
                // deliberately malformed tables (first_chunk = 0) so the parser
                // gets a chance to reject them. Panicking here instead would
                // make the test pass for the wrong reason.
                let last = match spec.stsc.get(ix + 1) {
                    Some((nf, _, _)) => nf.saturating_sub(1),
                    None => {
                        let remaining = spec.samples.len().saturating_sub(sample);
                        first.saturating_sub(1) + (remaining as u32).div_ceil((*per).max(1))
                    }
                };
                for _ in *first.max(&1)..=last.max(1) {
                    let mut c = Vec::new();
                    for _ in 0..*per {
                        if sample < spec.samples.len() {
                            c.push(sample);
                            sample += 1;
                        }
                    }
                    if !c.is_empty() {
                        chunk_layout.push(c);
                    }
                }
            }
        }

        // mdat payload, with a deliberate gap between chunks so that a parser
        // assuming samples are contiguous across chunk boundaries fails.
        const GAP: usize = 7;
        let mut mdat_body = Vec::new();
        let mut chunk_rel_offsets = Vec::new();
        for chunk in &chunk_layout {
            chunk_rel_offsets.push(mdat_body.len());
            for &s in chunk {
                mdat_body.extend_from_slice(&payloads[s]);
            }
            mdat_body.extend(std::iter::repeat(0xFF).take(GAP));
        }
        let mdat = bx(b"mdat", &mdat_body);

        // Where mdat's payload starts depends on box order, so build moov twice
        // is avoided by computing the layout first.
        let sizes: Vec<u32> = spec.samples.iter().map(|(s, _)| *s).collect();
        let sync: Vec<u32> = spec
            .samples
            .iter()
            .enumerate()
            .filter(|(_, (_, k))| *k)
            .map(|(i, _)| i as u32 + 1)
            .collect();

        let make_moov = |mdat_payload_at: u64| -> Vec<u8> {
            let chunk_offsets: Vec<u64> = chunk_rel_offsets
                .iter()
                .map(|r| mdat_payload_at + *r as u64)
                .collect();

            let stco = if spec.use_co64 {
                table(
                    b"co64",
                    &chunk_offsets.iter().map(|o| o.to_be_bytes().to_vec()).collect::<Vec<_>>(),
                )
            } else {
                table(
                    b"stco",
                    &chunk_offsets
                        .iter()
                        .map(|o| u32b(*o as u32))
                        .collect::<Vec<_>>(),
                )
            };

            let mut stbl_parts = vec![
                stsd(visual_entry(
                    &spec.codec,
                    1920,
                    1080,
                    spec.config.clone(),
                )),
                table(
                    b"stts",
                    &spec
                        .stts
                        .iter()
                        .map(|(c, d)| cat(&[u32b(*c), u32b(*d)]))
                        .collect::<Vec<_>>(),
                ),
                table(
                    b"stsc",
                    &spec
                        .stsc
                        .iter()
                        .map(|(a, b, c)| cat(&[u32b(*a), u32b(*b), u32b(*c)]))
                        .collect::<Vec<_>>(),
                ),
                stsz(&sizes),
                stco,
            ];
            if !sync.is_empty() && sync.len() != sizes.len() {
                stbl_parts.push(table(
                    b"stss",
                    &sync.iter().map(|s| u32b(*s)).collect::<Vec<_>>(),
                ));
            }
            if let Some(ctts) = &spec.ctts {
                stbl_parts.push(table(
                    b"ctts",
                    &ctts
                        .iter()
                        .map(|(c, o)| cat(&[u32b(*c), u32b(*o as u32)]))
                        .collect::<Vec<_>>(),
                ));
            }

            let stbl = bx(b"stbl", &cat(&stbl_parts));
            let minf = bx(b"minf", &cat(&[bx(b"vmhd", &[0u8; 12]), stbl]));
            let total_ticks: u32 = spec.stts.iter().map(|(c, d)| c * d).sum();
            let mdia = bx(
                b"mdia",
                &cat(&[mdhd(spec.timescale, total_ticks), hdlr(b"vide"), minf]),
            );
            let trak = bx(b"trak", &cat(&[tkhd(1, 1920, 1080), mdia]));
            bx(b"moov", &cat(&[bx(b"mvhd", &[0u8; 100]), trak]))
        };

        let file = if spec.moov_last {
            let mdat_at = (ftyp.len() + 8) as u64;
            let moov = make_moov(mdat_at);
            cat(&[ftyp, mdat, moov])
        } else {
            // moov's own size depends on nothing that depends on moov, but its
            // *length* shifts mdat, so compute once with a placeholder and
            // again with the real offset.
            let probe = make_moov(0);
            let mdat_at = (ftyp.len() + probe.len() + 8) as u64;
            let moov = make_moov(mdat_at);
            assert_eq!(moov.len(), probe.len(), "moov length must be stable");
            cat(&[ftyp, moov, mdat])
        };

        (file, payloads)
    }

    fn open(spec: &Spec) -> (Mp4Demuxer<MemorySource>, Vec<Vec<u8>>) {
        let (bytes, payloads) = build(spec);
        (
            Mp4Demuxer::open(MemorySource(bytes)).expect("open"),
            payloads,
        )
    }

    // ---- structure -----------------------------------------------------

    #[test]
    fn reads_dimensions_timescale_and_codec() {
        let (d, _) = open(&Spec::default());
        assert_eq!(d.dimensions(), (1920, 1080));
        assert_eq!(d.track().timescale, 30_000);
        assert_eq!(d.video_profile().unwrap().codec, Codec::H264);
        assert_eq!(d.sample_count(), 6);
    }

    #[test]
    fn duration_comes_from_the_media_header() {
        let (d, _) = open(&Spec::default());
        // 6 samples x 1000 ticks at 30000 ticks/s = 200 ms.
        assert_eq!(d.duration(), Some(Duration::from_millis(200)));
    }

    #[test]
    fn moov_after_mdat_still_opens() {
        let (d, _) = open(&Spec {
            moov_last: true,
            ..Default::default()
        });
        assert_eq!(d.sample_count(), 6);
    }

    #[test]
    fn co64_chunk_offsets_are_supported() {
        let (mut d, payloads) = open(&Spec {
            use_co64: true,
            ..Default::default()
        });
        let p = d.next_packet().unwrap().unwrap();
        assert!(p.data.len() >= payloads[0].len());
    }

    // ---- the stsc mapping ----------------------------------------------

    /// The bug that produces plausible garbage rather than an error. The
    /// fixture puts a gap between chunks, so a parser that walks samples
    /// contiguously reads padding instead of sample data.
    #[test]
    fn samples_are_located_through_the_chunk_mapping() {
        let (mut d, payloads) = open(&Spec::default());
        for (i, expected) in payloads.iter().enumerate() {
            let p = d.next_packet().unwrap().unwrap_or_else(|| panic!("sample {i} missing"));
            // Annex-B replaced the 4-byte length prefix with a start code, so
            // the body after it must match the original body exactly.
            assert_eq!(
                &p.data[p.data.len() - (expected.len() - 4)..],
                &expected[4..],
                "sample {i} read from the wrong offset"
            );
        }
        assert!(d.next_packet().unwrap().is_none());
    }

    #[test]
    fn a_multi_entry_stsc_with_varying_chunk_sizes_maps_correctly() {
        // 3 samples/chunk for chunks 1-2, then 1 sample/chunk from chunk 3.
        let spec = Spec {
            samples: (0..8).map(|i| (12 + i, i == 0)).collect(),
            stsc: vec![(1, 3, 1), (3, 1, 1)],
            stts: vec![(8, 500)],
            ..Default::default()
        };
        let (mut d, payloads) = open(&spec);
        assert_eq!(d.sample_count(), 8);
        for (i, expected) in payloads.iter().enumerate() {
            let p = d.next_packet().unwrap().unwrap();
            assert_eq!(
                &p.data[p.data.len() - (expected.len() - 4)..],
                &expected[4..],
                "sample {i} misplaced by the stsc walk"
            );
        }
    }

    #[test]
    fn a_zero_first_chunk_is_rejected_rather_than_underflowing() {
        let spec = Spec {
            stsc: vec![(0, 2, 1)],
            ..Default::default()
        };
        let (bytes, _) = build(&spec);
        assert!(matches!(Mp4Demuxer::open(MemorySource(bytes)), Err(_)), "stsc first_chunk=0 must be rejected");
    }

    // ---- timing ---------------------------------------------------------

    #[test]
    fn decode_timestamps_come_from_stts_run_lengths() {
        let spec = Spec {
            samples: (0..4).map(|i| (10 + i, i == 0)).collect(),
            stsc: vec![(1, 2, 1)],
            stts: vec![(2, 1000), (2, 2000)],
            timescale: 1000,
            ..Default::default()
        };
        let (mut d, _) = open(&spec);
        let dts: Vec<u64> = (0..4)
            .map(|_| d.next_packet().unwrap().unwrap().dts.unwrap().as_millis() as u64)
            .collect();
        assert_eq!(dts, vec![0, 1000, 2000, 4000]);
    }

    /// Without `ctts`, a file with B-frames plays with the picture order
    /// visibly wrong while still looking like valid playback.
    #[test]
    fn composition_offsets_shift_presentation_ahead_of_decode() {
        let spec = Spec {
            samples: (0..4).map(|i| (10 + i, i == 0)).collect(),
            stsc: vec![(1, 4, 1)],
            stts: vec![(4, 1000)],
            ctts: Some(vec![(1, 0), (1, 2000), (1, 1000), (1, 1000)]),
            timescale: 1000,
            ..Default::default()
        };
        let (mut d, _) = open(&spec);
        let mut pts = Vec::new();
        let mut dts = Vec::new();
        while let Some(p) = d.next_packet().unwrap() {
            pts.push(p.pts.unwrap().as_millis() as u64);
            dts.push(p.dts.unwrap().as_millis() as u64);
        }
        assert_eq!(dts, vec![0, 1000, 2000, 3000]);
        assert_eq!(pts, vec![0, 3000, 3000, 4000]);
    }

    #[test]
    fn a_negative_composition_offset_saturates_instead_of_wrapping() {
        let spec = Spec {
            samples: (0..2).map(|i| (10 + i, i == 0)).collect(),
            stsc: vec![(1, 2, 1)],
            stts: vec![(2, 1000)],
            // Sample 0 at dts 0 with offset -500 would wrap to ~u64::MAX and
            // sort to the far future, and the frame queue would drop the rest.
            ctts: Some(vec![(1, -500), (1, 0)]),
            timescale: 1000,
            ..Default::default()
        };
        let (mut d, _) = open(&spec);
        let p = d.next_packet().unwrap().unwrap();
        assert_eq!(p.pts, Some(Duration::ZERO));
    }

    // ---- sync samples and seeking ---------------------------------------

    #[test]
    fn stss_marks_only_the_listed_samples_as_sync() {
        let (d, _) = open(&Spec::default());
        let flags: Vec<bool> = d.track().samples.iter().map(|s| s.is_sync).collect();
        // Default spec marks every third sample.
        assert_eq!(flags, vec![true, false, false, true, false, false]);
    }

    #[test]
    fn no_stss_means_every_sample_is_a_sync_sample() {
        let spec = Spec {
            samples: (0..4).map(|i| (10 + i, true)).collect(),
            stsc: vec![(1, 2, 1)],
            stts: vec![(4, 1000)],
            ..Default::default()
        };
        let (d, _) = open(&spec);
        assert!(d.track().samples.iter().all(|s| s.is_sync));
    }

    #[test]
    fn seek_lands_on_the_preceding_sync_sample() {
        let (mut d, _) = open(&Spec::default());
        // Samples at dts 0,1000,...; sync at indices 0 and 3.
        d.seek(Duration::from_micros(
            // just past sample 4's dts of 4000 ticks at 30000/s
            (4200f64 / 30_000.0 * 1_000_000.0) as u64,
        ))
        .unwrap();
        assert_eq!(d.position(), 3, "must rewind to the keyframe, not the nearest sample");
    }

    #[test]
    fn seeking_before_the_start_lands_on_the_first_sample() {
        let (mut d, _) = open(&Spec::default());
        d.seek(Duration::ZERO).unwrap();
        assert_eq!(d.position(), 0);
    }

    // ---- Annex-B --------------------------------------------------------

    #[test]
    fn avcc_length_size_masks_the_reserved_bits() {
        for ls in [1usize, 2, 4] {
            let raw = avcc(ls, SPS, PPS);
            // Strip the 8-byte box header the helper added.
            let cfg = parse_avcc(&raw[8..]).unwrap();
            assert_eq!(cfg.length_size, ls);
        }
    }

    #[test]
    fn avcc_parameter_sets_are_emitted_with_start_codes() {
        let raw = avcc(4, SPS, PPS);
        let cfg = parse_avcc(&raw[8..]).unwrap();
        let mut expected = Vec::new();
        expected.extend_from_slice(&START_CODE);
        expected.extend_from_slice(SPS);
        expected.extend_from_slice(&START_CODE);
        expected.extend_from_slice(PPS);
        assert_eq!(cfg.parameter_sets, expected);
    }

    /// The black-screen bug: a V4L2 decoder fed MP4-format samples finds no
    /// start code, emits nothing, and reports no error.
    #[test]
    fn samples_are_converted_to_annex_b() {
        let (mut d, _) = open(&Spec::default());
        let p = d.next_packet().unwrap().unwrap();
        assert_eq!(&p.data[0..4], &START_CODE, "no start code: decoder will output nothing");
        assert!(
            p.data.windows(SPS.len()).any(|w| w == SPS),
            "keyframe must carry the SPS"
        );
        assert!(p.data.windows(PPS.len()).any(|w| w == PPS));
    }

    #[test]
    fn non_keyframes_do_not_repeat_the_parameter_sets() {
        let (mut d, _) = open(&Spec::default());
        let _key = d.next_packet().unwrap().unwrap();
        let inter = d.next_packet().unwrap().unwrap();
        assert!(
            !inter.data.windows(SPS.len()).any(|w| w == SPS),
            "SPS repeated on every frame wastes bandwidth"
        );
        assert!(!inter.is_keyframe);
    }

    /// After a seek the decoder is flushed and has forgotten the parameter
    /// sets, so the next keyframe must carry them again or it decodes to
    /// nothing.
    #[test]
    fn parameter_sets_are_re_emitted_after_a_seek() {
        let (mut d, _) = open(&Spec::default());
        while d.next_packet().unwrap().is_some() {}
        d.seek(Duration::ZERO).unwrap();
        let p = d.next_packet().unwrap().unwrap();
        assert!(
            p.data.windows(SPS.len()).any(|w| w == SPS),
            "post-seek keyframe lost its SPS"
        );
    }

    #[test]
    fn multiple_nals_in_one_sample_each_get_a_start_code() {
        let cfg = parse_avcc(&avcc(4, SPS, PPS)[8..]).unwrap();
        let mut sample = Vec::new();
        for body in [&[1u8, 2, 3][..], &[4, 5][..]] {
            sample.extend_from_slice(&(body.len() as u32).to_be_bytes());
            sample.extend_from_slice(body);
        }
        let out = to_annex_b(&sample, &cfg, false).unwrap();
        assert_eq!(out, vec![0, 0, 0, 1, 1, 2, 3, 0, 0, 0, 1, 4, 5]);
    }

    #[test]
    fn a_nal_length_running_past_the_sample_is_an_error() {
        let cfg = parse_avcc(&avcc(4, SPS, PPS)[8..]).unwrap();
        let sample = [0u8, 0, 0, 200, 1, 2, 3];
        assert!(to_annex_b(&sample, &cfg, false).is_err());
    }

    #[test]
    fn a_zero_length_nal_is_an_error_not_an_infinite_loop() {
        let cfg = parse_avcc(&avcc(4, SPS, PPS)[8..]).unwrap();
        assert!(to_annex_b(&[0u8, 0, 0, 0], &cfg, false).is_err());
    }

    /// AV1 samples are already OBUs; touching them would corrupt the stream.
    /// The AV1 sequence header lives in `av1C` and, for a great many MP4
    /// encoders, nowhere else. A decoder handed frames without it produces
    /// nothing and no error -- the same silent black screen as a missing start
    /// code, by a different route.
    #[test]
    fn the_av1_sequence_header_reaches_the_first_keyframe() {
        let seq: &[u8] = &[0x0A, 0x0E, 0x00];
        let mut av1c = vec![0x81, 0x00, 0x00, 0x00];
        av1c.extend_from_slice(seq);
        let file = av1_file(&av1c);

        let mut d = Mp4Demuxer::open(MemorySource(file)).expect("open av1");
        let first = d.next_packet().unwrap().unwrap();
        assert!(first.is_keyframe);
        assert_eq!(
            &first.data[..seq.len()],
            seq,
            "the sequence header never reached the decoder"
        );

        // ...and NOT on every frame: a sequence header mid-sequence resets the
        // decoder, so repeating it corrupts the stream rather than wasting
        // bytes.
        let second = d.next_packet().unwrap().unwrap();
        assert_ne!(&second.data[..seq.len().min(second.data.len())], seq);
    }

    /// After a seek the decoder has been flushed and needs it again.
    #[test]
    fn the_sequence_header_is_re_emitted_after_a_seek() {
        let seq: &[u8] = &[0x0A, 0x0E, 0x00];
        let mut av1c = vec![0x81, 0x00, 0x00, 0x00];
        av1c.extend_from_slice(seq);
        let mut d = Mp4Demuxer::open(MemorySource(av1_file(&av1c))).unwrap();
        while d.next_packet().unwrap().is_some() {}

        d.seek(std::time::Duration::ZERO).unwrap();
        let after = d.next_packet().unwrap().unwrap();
        assert_eq!(
            &after.data[..seq.len()],
            seq,
            "a flushed decoder was given frames with no sequence header"
        );
    }

    /// An encoder that put the sequence header in the samples leaves configOBUs
    /// empty; the samples must then pass through untouched.
    #[test]
    fn an_empty_av1_config_leaves_samples_alone() {
        let mut d = Mp4Demuxer::open(MemorySource(av1_file(&[0x81, 0x00, 0x00, 0x00]))).unwrap();
        let p = d.next_packet().unwrap().unwrap();
        assert_eq!(p.data, vec![0xA0; 6], "an empty config still altered the frame");
    }

    /// A malformed av1C must not make the file unopenable: the samples may
    /// carry their own header, and refusing turns a recoverable oddity into an
    /// unplayable file.
    #[test]
    fn a_broken_av1c_still_opens() {
        let d = Mp4Demuxer::open(MemorySource(av1_file(&[0x00, 0x00])));
        assert!(d.is_ok(), "a bad av1C made the file unopenable");
    }

    #[test]
    fn av1_samples_pass_through_unconverted() {
        // av1C: marker/version, seq_profile|level, then the bit-depth flags.
        let av1c = bx(b"av1C", &[0x81, 0x00, 0x00, 0x00]);
        let spec = Spec {
            codec: *b"av01",
            config: av1c,
            ..Default::default()
        };
        let (mut d, payloads) = open(&spec);
        assert_eq!(d.video_profile().unwrap().codec, Codec::Av1);
        let p = d.next_packet().unwrap().unwrap();
        assert_eq!(p.data, payloads[0], "AV1 payload must be byte-identical");
    }

    #[test]
    fn av1_high_bitdepth_flag_is_read() {
        // bit 6 of byte 2 is high_bitdepth.
        let av1c = bx(b"av1C", &[0x81, 0x00, 0x40, 0x00]);
        let spec = Spec {
            codec: *b"av01",
            config: av1c,
            ..Default::default()
        };
        let (d, _) = open(&spec);
        assert_eq!(d.video_profile().unwrap().bit_depth, 10);
    }

    // ---- malformed input -------------------------------------------------

    #[test]
    fn a_zero_advance_box_does_not_hang() {
        // size 8 header claiming to be inside a container it overruns.
        let mut f = bx(b"ftyp", b"isom");
        f.extend_from_slice(&[0, 0, 0, 4, b'j', b'u', b'n', b'k']);
        assert!(matches!(Mp4Demuxer::open(MemorySource(f)), Err(_)));
    }

    /// A box may declare any size it likes; nothing stops it claiming more
    /// than its parent contains. Trusting it turns a corrupt file into reads
    /// far outside the data.
    #[test]
    fn a_box_declaring_more_size_than_its_container_is_rejected() {
        let mut f = bx(b"ftyp", b"isom");
        // A moov header claiming 256 MiB, in a file of about twenty bytes.
        f.extend_from_slice(&0x1000_0000u32.to_be_bytes());
        f.extend_from_slice(b"moov");
        let e = match Mp4Demuxer::open(MemorySource(f)) {
            Err(e) => e,
            Ok(_) => panic!("accepted a box that overruns the file"),
        };
        assert!(
            format!("{e}").contains("past its container limit"),
            "unhelpful error: {e}"
        );
    }

    /// Same check one level down: a child box inside a well-formed parent.
    #[test]
    fn a_child_box_overrunning_its_parent_is_rejected() {
        let inner = {
            let mut v = Vec::new();
            // A trak claiming far more than the moov that holds it.
            v.extend_from_slice(&0x0001_0000u32.to_be_bytes());
            v.extend_from_slice(b"trak");
            v.extend_from_slice(&[0u8; 8]);
            v
        };
        let mut f = bx(b"ftyp", b"isom");
        f.extend_from_slice(&bx(b"moov", &inner));
        assert!(
            matches!(Mp4Demuxer::open(MemorySource(f)), Err(_)),
            "child box overrunning its parent must be rejected"
        );
    }

    #[test]
    fn a_file_without_moov_reports_that_clearly() {
        let f = bx(b"ftyp", b"isom");
        let e = match Mp4Demuxer::open(MemorySource(f)) {
            Err(e) => e,
            Ok(_) => panic!("opened a file with no moov"),
        };
        assert!(format!("{e}").contains("moov"), "unhelpful error: {e}");
    }

    #[test]
    fn a_truncated_file_errors_rather_than_reading_garbage() {
        let (mut bytes, _) = build(&Spec::default());
        bytes.truncate(bytes.len() / 2);
        // Either open fails, or reading a sample does; both are acceptable,
        // silently returning wrong bytes is not.
        match Mp4Demuxer::open(MemorySource(bytes)) {
            Err(_) => {}
            Ok(mut d) => {
                let mut saw_err = false;
                loop {
                    match d.next_packet() {
                        Ok(Some(_)) => {}
                        Ok(None) => break,
                        Err(_) => {
                            saw_err = true;
                            break;
                        }
                    }
                }
                assert!(saw_err, "truncated file read to completion without error");
            }
        }
    }

    #[test]
    fn a_64_bit_box_header_is_parsed() {
        // Wrap the whole moov in the largesize form.
        let (bytes, _) = build(&Spec::default());
        let mut src = MemorySource(bytes);
        let total = src.size().unwrap();
        let moov = crate::boxes::find_box(&mut src, 0, total, b"moov")
            .unwrap()
            .unwrap();
        let body =
            crate::boxes::read_exact_at(&mut src, moov.payload_offset, moov.payload_len as usize)
                .unwrap();
        let big = bx64(b"moov", &body);
        let mut rebuilt = bx(b"ftyp", b"isom");
        rebuilt.extend_from_slice(&big);
        let mut s2 = MemorySource(rebuilt);
        let total2 = s2.size().unwrap();
        let h = crate::boxes::find_box(&mut s2, 0, total2, b"moov")
            .unwrap()
            .unwrap();
        assert_eq!(h.payload_len as usize, body.len());
    }
}
