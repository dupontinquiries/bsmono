//! Demux only (no decode): packets for a hardware decoder that takes a
//! bitstream (V4L2 stateful), in the form it takes them (see [`bitstream`]).

use std::ffi::{CStr, CString};
use std::ptr;
use std::time::Duration;

use ffmpeg_sys_next as ff;

use crate::bitstream::{self, Codec};

pub struct Demuxer {
    fmt: *mut ff::AVFormatContext,
    pkt: *mut ff::AVPacket,
    stream: i32,
    time_base: f64,
    start: i64,
    pub codec: Codec,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    annexb: Option<(usize, Vec<Vec<u8>>)>,
    /// VP9 frames split from the last superframe, not yet handed out.
    queued: std::collections::VecDeque<(Vec<u8>, Duration)>,
}

unsafe impl Send for Demuxer {}

/// One access unit for the decoder.
pub struct Unit {
    pub data: Vec<u8>,
    pub pts: Duration,
    pub key: bool,
}

impl Demuxer {
    /// Open `input` (path or URL, with libavformat options) at `from`
    /// (the key frame at or before it: the decoder's output before `from`
    /// is the caller's to drop).
    pub fn open(input: &str, options: &[(String, String)], from: Duration) -> Result<Self, String> {
        let c = CString::new(input).map_err(|_| "input has a NUL")?;
        // SAFETY: libavformat setup; Drop frees.
        unsafe {
            let mut d = Demuxer { fmt: ptr::null_mut(), pkt: ptr::null_mut(), stream: -1, time_base: 0.0, start: 0, codec: Codec::H264, width: 0, height: 0, fps: 0.0, annexb: None, queued: Default::default() };
            let mut dict: *mut ff::AVDictionary = ptr::null_mut();
            for (k, v) in options {
                let (k, v) = (CString::new(k.as_str()).map_err(|_| "NUL")?, CString::new(v.as_str()).map_err(|_| "NUL")?);
                ff::av_dict_set(&mut dict, k.as_ptr(), v.as_ptr(), 0);
            }
            let r = ff::avformat_open_input(&mut d.fmt, c.as_ptr(), ptr::null(), &mut dict);
            ff::av_dict_free(&mut dict);
            if r < 0 {
                return Err(format!("{}: open failed ({r})", input.split('?').next().unwrap_or(input)));
            }
            if ff::avformat_find_stream_info(d.fmt, ptr::null_mut()) < 0 {
                return Err("stream info".into());
            }
            d.stream = ff::av_find_best_stream(d.fmt, ff::AVMediaType::AVMEDIA_TYPE_VIDEO, -1, -1, ptr::null_mut(), 0);
            if d.stream < 0 {
                return Err("no video stream".into());
            }
            let st = *(*d.fmt).streams.add(d.stream as usize);
            let par = (*st).codecpar;
            d.codec = match (*par).codec_id {
                ff::AVCodecID::AV_CODEC_ID_H264 => Codec::H264,
                ff::AVCodecID::AV_CODEC_ID_HEVC => Codec::Hevc,
                ff::AVCodecID::AV_CODEC_ID_VP9 => Codec::Vp9,
                other => return Err(format!("{:?} is not demuxed for a hardware decoder", CStr::from_ptr(ff::avcodec_get_name(other)))),
            };
            let extra = if (*par).extradata.is_null() { &[][..] } else { std::slice::from_raw_parts((*par).extradata, (*par).extradata_size as usize) };
            d.annexb = bitstream::param_sets(d.codec, extra)?;
            d.width = (*par).width as u32;
            d.height = (*par).height as u32;
            let tb = (*st).time_base;
            d.time_base = tb.num as f64 / tb.den.max(1) as f64;
            d.start = if (*st).start_time == ff::AV_NOPTS_VALUE { 0 } else { (*st).start_time };
            let fr = (*st).avg_frame_rate;
            d.fps = if fr.num > 0 && fr.den > 0 { fr.num as f64 / fr.den as f64 } else { 24.0 };
            if !from.is_zero() {
                let ts = d.start + (from.as_secs_f64() / d.time_base) as i64;
                if ff::avformat_seek_file(d.fmt, d.stream, i64::MIN, ts, ts, 0) < 0 {
                    return Err("seek".into());
                }
            }
            d.pkt = ff::av_packet_alloc();
            Ok(d)
        }
    }

    /// The next access unit; `None` at the end.
    pub fn next(&mut self) -> Result<Option<Unit>, String> {
        if let Some((data, pts)) = self.queued.pop_front() {
            return Ok(Some(Unit { data, pts, key: false }));
        }
        // SAFETY: packets owned here, unref'd after use.
        unsafe {
            loop {
                if ff::av_read_frame(self.fmt, self.pkt) < 0 {
                    return Ok(None);
                }
                if (*self.pkt).stream_index != self.stream {
                    ff::av_packet_unref(self.pkt);
                    continue;
                }
                let p = &*self.pkt;
                let ts = if p.pts != ff::AV_NOPTS_VALUE { p.pts } else { p.dts };
                let pts = Duration::from_secs_f64(((ts - self.start) as f64 * self.time_base).max(0.0));
                let key = p.flags & ff::AV_PKT_FLAG_KEY as i32 != 0;
                let bytes = std::slice::from_raw_parts(p.data, p.size as usize);
                let unit = match (self.codec, &self.annexb) {
                    (Codec::Vp9, _) => {
                        let mut frames = bitstream::vp9_frames(bytes).into_iter().map(|f| f.to_vec());
                        let first = frames.next().unwrap_or_default();
                        // Same timestamp for the parts: the hidden frame shows
                        // nothing, the shown one carries it.
                        self.queued.extend(frames.map(|f| (f, pts)));
                        Unit { data: first, pts, key }
                    }
                    (_, Some((len, sets))) => {
                        let mut out = Vec::with_capacity(bytes.len() + 64);
                        bitstream::to_annexb(bytes, *len, key.then_some(sets.as_slice()), &mut out)?;
                        Unit { data: out, pts, key }
                    }
                    (_, None) => Unit { data: bytes.to_vec(), pts, key },
                };
                ff::av_packet_unref(self.pkt);
                return Ok(Some(unit));
            }
        }
    }
}

impl Drop for Demuxer {
    fn drop(&mut self) {
        // SAFETY: tolerate null.
        unsafe {
            ff::av_packet_free(&mut self.pkt);
            ff::avformat_close_input(&mut self.fmt);
        }
    }
}
