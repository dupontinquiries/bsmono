//! The `MediaSource` over a parsed MP4.

use crate::annexb::{parse_av1c, parse_config, to_annex_b, with_av1_config, Av1Config, NalConfig};
use crate::boxes::*;
use crate::moov::{parse_video_track, VideoTrack};
use std::time::Duration;
use vidkit_core::codec::VideoProfile;
use vidkit_core::traits::{self as core_traits, MediaSource, Packet};

pub struct Mp4Demuxer<S: ByteSource> {
    src: S,
    track: VideoTrack,
    nal_config: Option<NalConfig>,
    /// AV1's sequence header, which lives in `av1C` and often nowhere else.
    av1_config: Option<Av1Config>,
    cursor: usize,
    /// Samples whose Annex-B conversion should carry the parameter sets. Set
    /// after every seek and at start, cleared once a keyframe has carried them.
    needs_parameter_sets: bool,
}

impl<S: ByteSource> Mp4Demuxer<S> {
    pub fn open(mut src: S) -> Result<Self> {
        let limit = src.size().unwrap_or(u64::MAX);

        // `moov` may sit before or after `mdat`. Faststart files put it first;
        // anything written by a recorder that did not know its final duration
        // puts it last, which over HTTP means a second range request at the
        // end of the file. Scanning top-level boxes handles both.
        let moov = find_box(&mut src, 0, limit, b"moov")?.ok_or_else(|| {
            Error::Malformed("no moov box; a fragmented-only file needs the moof reader".into())
        })?;

        let track = parse_video_track(&mut src, &moov)?;
        let nal_config = parse_config(track.profile.codec, &track.codec_config)?;
        // A malformed av1C is not fatal: the samples may carry their own
        // sequence header, and refusing to open the file would turn a
        // recoverable oddity into an unplayable one.
        let av1_config = if track.profile.codec == vidkit_core::codec::Codec::Av1 {
            parse_av1c(&track.codec_config).ok()
        } else {
            None
        };

        Ok(Self {
            src,
            track,
            nal_config,
            av1_config,
            cursor: 0,
            needs_parameter_sets: true,
        })
    }

    /// The underlying byte source.
    ///
    /// Exists so a network source can rebind an expired URL without rebuilding
    /// the demuxer — the sample tables and the read position are still valid,
    /// only the socket's address changed.
    pub fn source_mut(&mut self) -> &mut S {
        &mut self.src
    }

    pub fn track(&self) -> &VideoTrack {
        &self.track
    }

    pub fn sample_count(&self) -> usize {
        self.track.samples.len()
    }

    pub fn position(&self) -> usize {
        self.cursor
    }
}

fn io(e: Error) -> core_traits::Error {
    core_traits::Error::Backend(e.to_string())
}

impl<S: ByteSource + Send> MediaSource for Mp4Demuxer<S> {
    fn video_profile(&self) -> Option<VideoProfile> {
        Some(self.track.profile)
    }

    fn dimensions(&self) -> (u32, u32) {
        (self.track.width, self.track.height)
    }

    fn nominal_fps(&self) -> u32 {
        self.track.nominal_fps()
    }

    fn duration(&self) -> Option<Duration> {
        Some(self.track.ticks_to_duration(self.track.duration_ticks))
    }

    fn seekable(&self) -> bool {
        true
    }

    fn next_packet(&mut self) -> core_traits::Result<Option<Packet>> {
        let s = match self.track.samples.get(self.cursor) {
            Some(s) => *s,
            None => return Ok(None),
        };
        self.cursor += 1;

        let raw = read_exact_at(&mut self.src, s.offset, s.size as usize).map_err(io)?;

        let data = match &self.nal_config {
            Some(cfg) => {
                let with_ps = s.is_sync && self.needs_parameter_sets;
                let out = to_annex_b(&raw, cfg, with_ps).map_err(io)?;
                if with_ps {
                    self.needs_parameter_sets = false;
                }
                out
            }
            None => match &self.av1_config {
                Some(cfg) => {
                    let with_seq = s.is_sync && self.needs_parameter_sets;
                    let out = with_av1_config(&raw, cfg, with_seq);
                    if with_seq {
                        self.needs_parameter_sets = false;
                    }
                    out
                }
                None => raw,
            },
        };

        Ok(Some(Packet {
            data,
            pts: Some(self.track.ticks_to_duration(s.pts)),
            dts: Some(self.track.ticks_to_duration(s.dts)),
            is_keyframe: s.is_sync,
        }))
    }

    fn seek(&mut self, to: Duration) -> core_traits::Result<()> {
        let ticks = (to.as_secs_f64() * self.track.timescale as f64) as u64;
        self.cursor = self.track.sync_sample_at_or_before(ticks);
        // The decoder will be flushed, losing its parameter sets, so the next
        // keyframe has to carry them again.
        self.needs_parameter_sets = true;
        Ok(())
    }
}
