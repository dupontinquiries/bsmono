//! `moov` parsing: turning the sample tables into a flat list of samples.
//!
//! The sample tables are four interlocking arrays that between them say where
//! every sample lives and when it should be shown. They are compact and
//! awkward, and getting `stsc` wrong is the classic MP4 bug — it produces
//! plausible-looking garbage rather than an error, because the offsets are
//! still inside the file.
//!
//! This flattens them once, up front. A two-hour 30 fps video is ~216k
//! samples at 32 bytes each, so about 7 MB — acceptable even on a phone, and
//! it makes seeking and playback O(1) per sample instead of re-walking the
//! chunk mapping. If that ever becomes a problem the fix is a lazy chunk
//! cursor, not a smaller struct.

use crate::boxes::*;
use vidkit_core::codec::{Chroma, Codec, VideoProfile};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Sample {
    pub offset: u64,
    pub size: u32,
    /// Decode timestamp in track timescale ticks.
    pub dts: u64,
    /// Composition (presentation) timestamp in ticks. Differs from `dts` when
    /// B-frames are present; ignoring `ctts` makes such files play with the
    /// picture order visibly wrong.
    pub pts: u64,
    pub is_sync: bool,
}

#[derive(Clone, Debug)]
pub struct VideoTrack {
    pub track_id: u32,
    pub timescale: u32,
    pub duration_ticks: u64,
    pub width: u32,
    pub height: u32,
    pub profile: VideoProfile,
    /// Codec-private configuration: `avcC`, `hvcC`, `av1C`, `vpcC` contents.
    pub codec_config: Vec<u8>,
    pub samples: Vec<Sample>,
}

impl VideoTrack {
    pub fn ticks_to_duration(&self, ticks: u64) -> std::time::Duration {
        std::time::Duration::from_nanos(
            ticks.saturating_mul(1_000_000_000) / self.timescale.max(1) as u64,
        )
    }

    /// Index of the last sync sample at or before `ticks`.
    ///
    /// Seeking to a non-sync sample gives a corrupt picture until the next
    /// keyframe, so every seek lands on a sync sample and the player discards
    /// the difference. Returns 0 when the table has no sync list, which by
    /// convention means every sample is a sync sample.
    pub fn sync_sample_at_or_before(&self, ticks: u64) -> usize {
        let mut best = 0usize;
        for (i, s) in self.samples.iter().enumerate() {
            if s.dts > ticks {
                break;
            }
            if s.is_sync {
                best = i;
            }
        }
        best
    }

    pub fn nominal_fps(&self) -> u32 {
        if self.samples.len() < 2 || self.duration_ticks == 0 {
            return 30;
        }
        let secs = self.duration_ticks as f64 / self.timescale.max(1) as f64;
        if secs <= 0.0 {
            return 30;
        }
        ((self.samples.len() as f64 / secs).round() as u32).clamp(1, 480)
    }
}

/// Full-box version/flags header. Every `st*` box has one.
fn full_box_version(b: &[u8]) -> Result<u8> {
    b.first()
        .copied()
        .ok_or_else(|| Error::Malformed("empty full box".into()))
}

fn parse_stts(data: &[u8]) -> Result<Vec<(u32, u32)>> {
    let count = be_u32(data, 4)? as usize;
    let mut out = Vec::with_capacity(count.min(1 << 20));
    for i in 0..count {
        let at = 8 + i * 8;
        out.push((be_u32(data, at)?, be_u32(data, at + 4)?));
    }
    Ok(out)
}

/// `ctts` composition offsets. Version 0 stores them unsigned, version 1
/// signed — and version 0 files in the wild still use values that only make
/// sense as signed, so both are read as `i32`.
fn parse_ctts(data: &[u8]) -> Result<Vec<(u32, i32)>> {
    let count = be_u32(data, 4)? as usize;
    let mut out = Vec::with_capacity(count.min(1 << 20));
    for i in 0..count {
        let at = 8 + i * 8;
        out.push((be_u32(data, at)?, be_u32(data, at + 4)? as i32));
    }
    Ok(out)
}

fn parse_stsc(data: &[u8]) -> Result<Vec<(u32, u32, u32)>> {
    let count = be_u32(data, 4)? as usize;
    let mut out = Vec::with_capacity(count.min(1 << 20));
    for i in 0..count {
        let at = 8 + i * 12;
        out.push((
            be_u32(data, at)?,
            be_u32(data, at + 4)?,
            be_u32(data, at + 8)?,
        ));
    }
    Ok(out)
}

/// `stsz`: either a single size for every sample, or one per sample.
fn parse_stsz(data: &[u8]) -> Result<Vec<u32>> {
    let uniform = be_u32(data, 4)?;
    let count = be_u32(data, 8)? as usize;
    if uniform != 0 {
        return Ok(vec![uniform; count]);
    }
    let mut out = Vec::with_capacity(count.min(1 << 22));
    for i in 0..count {
        out.push(be_u32(data, 12 + i * 4)?);
    }
    Ok(out)
}

fn parse_chunk_offsets(data: &[u8], is_64: bool) -> Result<Vec<u64>> {
    let count = be_u32(data, 4)? as usize;
    let mut out = Vec::with_capacity(count.min(1 << 22));
    for i in 0..count {
        out.push(if is_64 {
            be_u64(data, 8 + i * 8)?
        } else {
            be_u32(data, 8 + i * 4)? as u64
        });
    }
    Ok(out)
}

fn parse_stss(data: &[u8]) -> Result<Vec<u32>> {
    let count = be_u32(data, 4)? as usize;
    let mut out = Vec::with_capacity(count.min(1 << 20));
    for i in 0..count {
        out.push(be_u32(data, 8 + i * 4)?);
    }
    Ok(out)
}

/// Expand `stsc`'s run-length chunk mapping into per-sample chunk membership,
/// yielding each sample's absolute file offset.
///
/// `stsc` entries say "from chunk N onward, each chunk holds K samples", until
/// the next entry overrides it. The last entry runs to the end of `stco`. Off
/// by one here — treating `first_chunk` as 0-based, or forgetting that the
/// final entry is open-ended — silently reads the wrong bytes.
fn sample_offsets(
    stsc: &[(u32, u32, u32)],
    chunk_offsets: &[u64],
    sizes: &[u32],
) -> Result<Vec<u64>> {
    if stsc.is_empty() {
        return Err(Error::Malformed("stsc is empty".into()));
    }
    let mut offsets = Vec::with_capacity(sizes.len());
    let mut sample = 0usize;

    for (entry_ix, entry) in stsc.iter().enumerate() {
        let (first_chunk, per_chunk, _) = *entry;
        if first_chunk == 0 {
            return Err(Error::Malformed("stsc first_chunk is 1-based; got 0".into()));
        }
        // Runs until the next entry's first_chunk, or to the end of stco.
        let last_chunk = match stsc.get(entry_ix + 1) {
            Some((next_first, _, _)) => (*next_first).saturating_sub(1),
            None => chunk_offsets.len() as u32,
        };

        for chunk in first_chunk..=last_chunk {
            let base = *chunk_offsets
                .get(chunk as usize - 1)
                .ok_or_else(|| Error::Malformed(format!("stsc names chunk {chunk}, stco has {}", chunk_offsets.len())))?;
            let mut running = base;
            for _ in 0..per_chunk {
                if sample >= sizes.len() {
                    // More capacity than samples: legal, the tables just
                    // over-describe. Stop rather than fabricating entries.
                    return Ok(offsets);
                }
                offsets.push(running);
                running += sizes[sample] as u64;
                sample += 1;
            }
        }
    }
    Ok(offsets)
}

/// Expand `stts` run-lengths into a per-sample decode timestamp.
fn sample_dts(stts: &[(u32, u32)], n: usize) -> Vec<u64> {
    let mut out = Vec::with_capacity(n);
    let mut t = 0u64;
    for (count, delta) in stts {
        for _ in 0..*count {
            if out.len() >= n {
                return out;
            }
            out.push(t);
            t += *delta as u64;
        }
    }
    // Tables can under-describe; extend with the last delta rather than
    // truncating the sample list.
    let last_delta = stts.last().map(|(_, d)| *d as u64).unwrap_or(0);
    while out.len() < n {
        out.push(t);
        t += last_delta;
    }
    out
}

fn sample_ctts(ctts: &[(u32, i32)], n: usize) -> Vec<i32> {
    let mut out = Vec::with_capacity(n);
    for (count, off) in ctts {
        for _ in 0..*count {
            if out.len() >= n {
                return out;
            }
            out.push(*off);
        }
    }
    out.resize(n, 0);
    out
}

/// Map a sample-entry fourcc to a codec profile, reading the configuration box
/// inside it where one is needed.
fn profile_from_sample_entry(kind: &FourCC, config: &[u8]) -> Result<VideoProfile> {
    let codec = match &kind.0 {
        b"avc1" | b"avc3" => Codec::H264,
        b"hvc1" | b"hev1" => Codec::Hevc,
        b"av01" => Codec::Av1,
        b"vp09" => Codec::Vp9,
        b"vp08" => Codec::Vp8,
        _ => return Err(Error::Unsupported(format!("sample entry {kind:?}"))),
    };

    // Bit depth is codec-config specific. Only AV1 and VP9 encode it somewhere
    // cheap to reach; for H.264 and HEVC it lives in the SPS, which would mean
    // a bitstream parser. Defaulting to 8 is the safe direction: a 10-bit
    // stream misreported as 8-bit gets offered to hardware that rejects it and
    // falls back to software, whereas the reverse would reject a stream the
    // hardware could have taken.
    let bit_depth = match codec {
        // av1C: marker/version, then seq_profile(3)|seq_level(5), then
        // seq_tier(1)|high_bitdepth(1)|twelve_bit(1)|mono(1)|...
        Codec::Av1 if config.len() >= 3 => {
            let b2 = config[2];
            let high = (b2 >> 6) & 1;
            let twelve = (b2 >> 5) & 1;
            if twelve == 1 {
                12
            } else if high == 1 {
                10
            } else {
                8
            }
        }
        // vpcC is a full box: version+flags(4), profile, level, then
        // bitDepth(4)|chromaSubsampling(3)|videoFullRangeFlag(1)
        Codec::Vp9 if config.len() >= 7 => config[6] >> 4,
        _ => 8,
    };

    Ok(VideoProfile {
        codec,
        profile: None,
        bit_depth: if bit_depth == 0 { 8 } else { bit_depth },
        chroma: Chroma::Yuv420,
        level: None,
    })
}

/// Parse the first video track out of a `moov` box.
pub fn parse_video_track<S: ByteSource + ?Sized>(
    src: &mut S,
    moov: &BoxHeader,
) -> Result<VideoTrack> {
    let mut traks = BoxIter::new(moov.payload_offset, moov.end());
    while let Some(trak) = traks.next(src)? {
        if trak.kind != b"trak" {
            continue;
        }
        // A track is video iff its handler is 'vide'. Checking for a video
        // sample entry instead would also match timed-metadata tracks that
        // reuse the codec fourcc.
        let hdlr = match find_path(src, trak.payload_offset, trak.end(), &[b"mdia", b"hdlr"])? {
            Some(h) => h,
            None => continue,
        };
        let hdlr_data = read_exact_at(src, hdlr.payload_offset, hdlr.payload_len.min(24) as usize)?;
        if hdlr_data.len() < 12 || &hdlr_data[8..12] != b"vide" {
            continue;
        }

        return parse_one_trak(src, &trak);
    }
    Err(Error::Malformed("no video track in moov".into()))
}

fn parse_one_trak<S: ByteSource + ?Sized>(src: &mut S, trak: &BoxHeader) -> Result<VideoTrack> {
    let tkhd = find_path(src, trak.payload_offset, trak.end(), &[b"tkhd"])?
        .ok_or_else(|| Error::Malformed("trak without tkhd".into()))?;
    let tk = read_exact_at(src, tkhd.payload_offset, tkhd.payload_len as usize)?;
    let tk_version = full_box_version(&tk)?;
    let (track_id, dims_at) = if tk_version == 1 {
        (be_u32(&tk, 20)?, 88)
    } else {
        (be_u32(&tk, 12)?, 76)
    };
    // Width/height are 16.16 fixed point.
    let width = be_u32(&tk, dims_at).unwrap_or(0) >> 16;
    let height = be_u32(&tk, dims_at + 4).unwrap_or(0) >> 16;

    let mdhd = find_path(src, trak.payload_offset, trak.end(), &[b"mdia", b"mdhd"])?
        .ok_or_else(|| Error::Malformed("trak without mdhd".into()))?;
    let md = read_exact_at(src, mdhd.payload_offset, mdhd.payload_len as usize)?;
    let md_version = full_box_version(&md)?;
    let (timescale, duration_ticks) = if md_version == 1 {
        (be_u32(&md, 20)?, be_u64(&md, 24)?)
    } else {
        (be_u32(&md, 12)?, be_u32(&md, 16)? as u64)
    };

    let stbl = find_path(
        src,
        trak.payload_offset,
        trak.end(),
        &[b"mdia", b"minf", b"stbl"],
    )?
    .ok_or_else(|| Error::Malformed("trak without stbl".into()))?;

    // --- sample description ------------------------------------------------
    let stsd = find_box(src, stbl.payload_offset, stbl.end(), b"stsd")?
        .ok_or_else(|| Error::Malformed("stbl without stsd".into()))?;
    // full box (4) + entry_count (4), then the sample entries as boxes.
    let mut entries = BoxIter::new(stsd.payload_offset + 8, stsd.end());
    let entry = entries
        .next(src)?
        .ok_or_else(|| Error::Malformed("stsd has no entries".into()))?;

    // A VisualSampleEntry is 78 bytes of fixed fields before its child boxes.
    let config_start = entry.payload_offset + 78;
    let mut config = Vec::new();
    if config_start < entry.end() {
        let mut cfgs = BoxIter::new(config_start, entry.end());
        while let Some(c) = cfgs.next(src)? {
            if matches!(&c.kind.0, b"avcC" | b"hvcC" | b"av1C" | b"vpcC") {
                config = read_exact_at(src, c.payload_offset, c.payload_len as usize)?;
                break;
            }
        }
    }
    let profile = profile_from_sample_entry(&entry.kind, &config)?;

    // --- tables ------------------------------------------------------------
    let read_table = |src: &mut S, kind: &[u8; 4]| -> Result<Option<Vec<u8>>> {
        match find_box(src, stbl.payload_offset, stbl.end(), kind)? {
            Some(h) => Ok(Some(read_exact_at(src, h.payload_offset, h.payload_len as usize)?)),
            None => Ok(None),
        }
    };

    let stts = read_table(src, b"stts")?
        .ok_or_else(|| Error::Malformed("stbl without stts".into()))?;
    let stsz = read_table(src, b"stsz")?
        .ok_or_else(|| Error::Malformed("stbl without stsz".into()))?;
    let stsc = read_table(src, b"stsc")?
        .ok_or_else(|| Error::Malformed("stbl without stsc".into()))?;
    let (chunks_raw, is_64) = match read_table(src, b"stco")? {
        Some(t) => (t, false),
        None => (
            read_table(src, b"co64")?
                .ok_or_else(|| Error::Malformed("stbl without stco or co64".into()))?,
            true,
        ),
    };
    let stss = read_table(src, b"stss")?;
    let ctts = read_table(src, b"ctts")?;

    let sizes = parse_stsz(&stsz)?;
    let offsets = sample_offsets(&parse_stsc(&stsc)?, &parse_chunk_offsets(&chunks_raw, is_64)?, &sizes)?;
    let n = sizes.len().min(offsets.len());
    let dts = sample_dts(&parse_stts(&stts)?, n);
    let cts = match &ctts {
        Some(c) => sample_ctts(&parse_ctts(c)?, n),
        None => vec![0i32; n],
    };

    // No stss means every sample is a sync sample.
    let sync: Option<std::collections::HashSet<u32>> = match &stss {
        Some(t) => Some(parse_stss(t)?.into_iter().collect()),
        None => None,
    };

    let mut samples = Vec::with_capacity(n);
    for i in 0..n {
        let d = dts[i];
        samples.push(Sample {
            offset: offsets[i],
            size: sizes[i],
            dts: d,
            // Composition offset may be negative; saturate rather than wrap,
            // because a wrapped pts sorts to the far future and the frame
            // queue would drop everything after it.
            pts: if cts[i] >= 0 {
                d.saturating_add(cts[i] as u64)
            } else {
                d.saturating_sub((-cts[i]) as u64)
            },
            // stss is 1-based.
            is_sync: sync.as_ref().map(|s| s.contains(&(i as u32 + 1))).unwrap_or(true),
        });
    }

    Ok(VideoTrack {
        track_id,
        timescale: timescale.max(1),
        duration_ticks,
        width,
        height,
        profile,
        codec_config: config,
        samples,
    })
}
