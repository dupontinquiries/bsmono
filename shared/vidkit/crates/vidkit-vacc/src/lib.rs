//! In-process video decode through vacc (vendored: third_party/vacc).
//!
//! The seam the player needs: push access units as the demuxer produces
//! them, get vidkit [`Frame`]s back in display order, flush at the end,
//! reset on a seek. In process -- unlike the ffmpeg pipe -- which is the
//! prerequisite for handing decoded GPU surfaces to the presenter without
//! copies, the step after this one.
//!
//! Backends:
//! - [`Backend::Cpu`]: vacc's pure-Rust H.264 (edge264 port) and H.265
//!   (hevc.js port). Streams. The test oracle here: nothing else in a
//!   GPU-less sandbox can exercise the path.
//! - [`Backend::Vulkan`]: vacc's ash-based Vulkan Video. Builds, but
//!   vacc's Vulkan decoder only decodes a WHOLE bitstream (`decode_all`), so
//!   [`StreamDecoder::open`] refuses it, with the reason, until the
//!   streaming patch exists.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::time::Duration;

use vacc_core::decoder::Decoder as _;
use vidkit_core::frame::{ColorInfo, CpuPlanes, Frame, FrameData, PlaneLayout};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Backend {
    Cpu,
    Vulkan,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Codec {
    H264,
    H265,
}

/// Display-order reordering by picture order count. Pure.
///
/// vacc's streaming `decode()` returns frames in DECODE order (measured: a
/// B-pyramid stream came out 0, 4, 2, 1, 3, 8, 6, 5, 7...; each frame pixel-
/// exact, the order not); its whole-file paths sort by POC afterwards. For
/// streaming, frames are held sorted and the smallest released once more
/// than `depth` are held. A POC below the last one released means the POC
/// reset at an IDR -- a new GOP -- so everything held is released first.
pub struct Reorder<T> {
    held: Vec<(i32, T)>,
    depth: usize,
    last_out: Option<i32>,
}

impl<T> Reorder<T> {
    pub fn new(depth: usize) -> Self {
        Self { held: Vec::new(), depth, last_out: None }
    }

    pub fn set_depth(&mut self, depth: usize) {
        self.depth = depth;
    }

    /// Add a decoded frame; returns what may now be shown, in order.
    pub fn push(&mut self, poc: i32, item: T) -> Vec<T> {
        let mut out = Vec::new();
        if self.last_out.is_some_and(|l| poc < l) || self.held.iter().any(|(p, _)| *p > poc && poc == 0) {
            out.extend(self.flush());
        }
        let at = self.held.partition_point(|(p, _)| *p < poc);
        self.held.insert(at, (poc, item));
        while self.held.len() > self.depth {
            let (p, it) = self.held.remove(0);
            self.last_out = Some(p);
            out.push(it);
        }
        out
    }

    pub fn flush(&mut self) -> Vec<T> {
        if let Some((p, _)) = self.held.last() {
            self.last_out = Some(*p);
        }
        let out = self.held.drain(..).map(|(_, t)| t).collect();
        self.last_out = None;
        out
    }
}

enum Inner {
    H264(vacc_software_decode::SwH264Decoder),
    H265(vacc_software_decode::SoftwareH265Decoder),
}

pub struct StreamDecoder {
    inner: Inner,
    /// Submitted pts, smallest first: frames leave in DISPLAY order, so the
    /// n-th frame out takes the n-th smallest pts in.
    pts: BinaryHeap<Reverse<Duration>>,
    reorder: Reorder<vacc_core::frame::DecodedFrame>,
    pub color: ColorInfo,
}

impl StreamDecoder {
    /// Open with the first access unit, which must carry the parameter
    /// sets (SPS/PPS, and VPS for H.265) in Annex-B form.
    pub fn open(backend: Backend, codec: Codec, first: &[u8], pts: Duration) -> Result<Self, String> {
        if backend == Backend::Vulkan {
            return Err("vacc's Vulkan Video backend decodes whole bitstreams only (decode_all); \
                        streaming playback needs the incremental-submit patch -- see third_party/vacc/VENDORED.md"
                .into());
        }
        let inner = match codec {
            Codec::H264 => Inner::H264(vacc_software_decode::SwH264Decoder::new(first.to_vec()).map_err(|e| format!("vacc h264: {e}"))?),
            Codec::H265 => Inner::H265(vacc_software_decode::SoftwareH265Decoder::new(first.to_vec()).map_err(|e| format!("vacc h265: {e}"))?),
        };
        let mut pts_q = BinaryHeap::new();
        pts_q.push(Reverse(pts));
        // Depth: the DPB size the stream declared (an upper bound on the
        // reorder it needs), known once the parameter sets are parsed.
        let depth = match &inner {
            Inner::H264(d) => d.info().dpb_slots,
            Inner::H265(d) => d.info().dpb_slots,
        }
        .clamp(1, 16) as usize;
        Ok(Self { inner, pts: pts_q, reorder: Reorder::new(depth), color: ColorInfo::default() })
    }

    /// Push one access unit; returns every frame now ready, in display order.
    pub fn push(&mut self, au: &[u8], pts: Duration) -> Result<Vec<Frame>, String> {
        match &mut self.inner {
            Inner::H264(d) => d.submit(au).map_err(|e| e.to_string())?,
            Inner::H265(d) => d.submit(au).map_err(|e| e.to_string())?,
        }
        self.pts.push(Reverse(pts));
        self.drain()
    }

    /// Frames ready now, without new input.
    pub fn drain(&mut self) -> Result<Vec<Frame>, String> {
        let mut out = Vec::new();
        loop {
            let f = match &mut self.inner {
                Inner::H264(d) => d.decode().map_err(|e| e.to_string())?,
                Inner::H265(d) => d.decode().map_err(|e| e.to_string())?,
            };
            match f {
                Some(f) => {
                    for f in self.reorder.push(f.poc, f) {
                        if let Some(fr) = self.convert(&f)? {
                            out.push(fr);
                        }
                    }
                }
                None => return Ok(out),
            }
        }
    }

    /// End of stream: everything still held for reordering.
    pub fn flush(&mut self) -> Result<Vec<Frame>, String> {
        let mut out = self.drain()?;
        let rest = match &mut self.inner {
            Inner::H264(d) => d.flush().map_err(|e| e.to_string())?,
            Inner::H265(d) => d.flush().map_err(|e| e.to_string())?,
        };
        let mut held = Vec::new();
        for f in rest {
            held.extend(self.reorder.push(f.poc, f));
        }
        held.extend(self.reorder.flush());
        for f in &held {
            if let Some(fr) = self.convert(f)? {
                out.push(fr);
            }
        }
        Ok(out)
    }

    /// A seek: drop references and pending input.
    pub fn reset(&mut self) -> Result<(), String> {
        self.pts.clear();
        let _ = self.reorder.flush();
        match &mut self.inner {
            Inner::H264(d) => d.reset().map_err(|e| e.to_string()),
            Inner::H265(d) => d.reset().map_err(|e| e.to_string()),
        }
    }

    fn convert(&mut self, f: &vacc_core::frame::DecodedFrame) -> Result<Option<Frame>, String> {
        let Some(px) = f.pixel_data.as_ref() else { return Ok(None) };
        let pts = self.pts.pop().map(|Reverse(p)| p).unwrap_or_default();
        let (layout, bps) = match px.format.as_str() {
            "I420" => (PlaneLayout::I420, 1usize),
            "I420-10" => (PlaneLayout::I420_10le, 2),
            other => return Err(format!("vacc frame format {other} is not handled")),
        };
        let mut planes = Vec::with_capacity(3);
        let mut strides = Vec::with_capacity(3);
        let v = px.v.as_ref().ok_or("vacc I420 frame without a V plane")?;
        for p in [&px.y, &px.u, v] {
            planes.push(copy_plane(&px.buffer, p, bps)?);
            strides.push((p.width * bps) as u32);
        }
        Ok(Some(Frame {
            data: FrameData::Cpu(CpuPlanes { data: planes, strides }),
            width: f.width,
            height: f.height,
            layout,
            color: self.color,
            pts,
            duration: None,
            recycle: None,
        }))
    }
}

/// One plane out of vacc's owned buffer, tightly packed. The plane's pointer
/// is turned into an OFFSET into `buffer` and bounds-checked, rather than
/// dereferenced: a pointer outside the buffer is an error, not UB.
fn copy_plane(buffer: &[u8], p: &vacc_core::frame::PixelPlane, bps: usize) -> Result<Vec<u8>, String> {
    let base = buffer.as_ptr() as usize;
    let at = (p.data as usize).checked_sub(base).ok_or("vacc plane pointer before its buffer")?;
    let row = p.width * bps;
    let mut out = Vec::with_capacity(row * p.height);
    for y in 0..p.height {
        let s = at + y * p.pitch;
        out.extend_from_slice(buffer.get(s..s + row).ok_or("vacc plane runs past its buffer")?);
    }
    Ok(out)
}

/// An MP4 (AVCC/HVCC) sample -- NAL units with big-endian length prefixes of
/// `len_size` bytes -- as Annex-B (start codes), which vacc parses. Pure.
pub fn avcc_to_annexb(sample: &[u8], len_size: usize) -> Result<Vec<u8>, String> {
    if !(1..=4).contains(&len_size) {
        return Err(format!("NAL length size {len_size}"));
    }
    let mut out = Vec::with_capacity(sample.len() + 16);
    let mut i = 0;
    while i < sample.len() {
        let hdr = sample.get(i..i + len_size).ok_or("truncated NAL length")?;
        let n = hdr.iter().fold(0usize, |a, b| (a << 8) | *b as usize);
        i += len_size;
        let nal = sample.get(i..i + n).ok_or("NAL runs past the sample")?;
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(nal);
        i += n;
    }
    Ok(out)
}

/// Split an Annex-B stream into access units at access-unit delimiters (NAL
/// type 9, emitted by `x264 aud=1`). Pure; for tests and raw .h264 files.
pub fn split_at_aud(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 4 < data.len() {
        let sc = if data[i..].starts_with(&[0, 0, 0, 1]) { 4 } else if data[i..].starts_with(&[0, 0, 1]) { 3 } else { 0 };
        if sc > 0 {
            if data[i + sc] & 0x1f == 9 {
                starts.push(i);
            }
            i += sc;
        } else {
            i += 1;
        }
    }
    starts.push(data.len());
    starts.windows(2).map(|w| &data[w[0]..w[1]]).filter(|s| !s.is_empty()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    /// The decode order actually observed from vacc's CPU H.264 decoder on a
    /// 3-B-frame pyramid, POCs as display indices; then a second GOP whose
    /// POC restarts at 0.
    #[test]
    fn decode_order_becomes_display_order() {
        let mut r = Reorder::new(3);
        let mut out = Vec::new();
        for poc in [0, 4, 2, 1, 3, 8, 6, 5, 7, 12, 10, 9, 11] {
            out.extend(r.push(poc, poc));
        }
        out.extend(r.flush());
        assert_eq!(out, (0..=12).collect::<Vec<_>>());
        // Across an IDR: the POC resets.
        let mut r = Reorder::new(3);
        let mut out = Vec::new();
        for poc in [0, 4, 2, 1, 3, 0, 2, 1] {
            out.extend(r.push(poc, poc));
        }
        out.extend(r.flush());
        assert_eq!(out, vec![0, 1, 2, 3, 4, 0, 1, 2]);
    }

    #[test]
    fn avcc_samples_become_annexb() {
        let sample = [0, 0, 0, 2, 0x65, 0x88, 0, 0, 0, 1, 0x06];
        assert_eq!(avcc_to_annexb(&sample, 4).unwrap(), vec![0, 0, 0, 1, 0x65, 0x88, 0, 0, 0, 1, 0x06]);
        assert!(avcc_to_annexb(&[0, 0, 0, 9, 1], 4).is_err(), "a length past the end is an error");
        assert!(avcc_to_annexb(&sample, 5).is_err());
    }

    #[test]
    fn vulkan_is_refused_for_streaming_with_the_reason() {
        let e = StreamDecoder::open(Backend::Vulkan, Codec::H264, &[], Duration::ZERO).err().unwrap();
        assert!(e.contains("whole bitstreams only"), "{e}");
    }

    fn have_ffmpeg() -> bool {
        Command::new("ffmpeg").arg("-version").stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
    }

    /// The whole streaming path, one access unit at a time, against ffmpeg:
    /// H.264 High with 3 B-frames (reordering, references), every frame
    /// byte-identical, and pts coming out in display order.
    #[test]
    fn streaming_h264_matches_ffmpeg_byte_for_byte() {
        if !have_ffmpeg() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("vk-vacc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (h264, yuv) = (dir.join("c.h264"), dir.join("c.yuv"));
        assert!(Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=30:duration=2",
            "-c:v", "libx264", "-profile:v", "high", "-bf", "3", "-g", "30", "-x264-params", "aud=1", "-pix_fmt", "yuv420p"]).arg(&h264).status().unwrap().success());
        assert!(Command::new("ffmpeg").args(["-v", "error", "-y", "-i"]).arg(&h264).args(["-f", "rawvideo", "-pix_fmt", "yuv420p"]).arg(&yuv).status().unwrap().success());
        let data = std::fs::read(&h264).unwrap();
        let reference = std::fs::read(&yuv).unwrap();
        let aus = split_at_aud(&data);
        assert!(aus.len() >= 60, "{} access units", aus.len());
        // Decode order pts: the encoder's display order is not decode order,
        // but with a fixed frame rate the set of pts is the same, and the
        // decoder's display-order output must take them smallest first.
        let tick = |i: usize| Duration::from_millis(i as u64 * 1000 / 30);
        let mut d = StreamDecoder::open(Backend::Cpu, Codec::H264, aus[0], tick(0)).unwrap();
        let mut frames = d.drain().unwrap();
        for (i, au) in aus.iter().enumerate().skip(1) {
            frames.extend(d.push(au, tick(i)).unwrap());
        }
        frames.extend(d.flush().unwrap());
        let fs = 320 * 180 * 3 / 2;
        assert_eq!(frames.len(), reference.len() / fs, "frame count");
        let which: Vec<Option<usize>> = frames
            .iter()
            .map(|f| {
                let FrameData::Cpu(p) = &f.data else { panic!() };
                let got: Vec<u8> = p.data.concat();
                (0..reference.len() / fs).find(|&j| got == reference[j * fs..(j + 1) * fs])
            })
            .collect();
        for (i, w) in which.iter().enumerate() {
            assert_eq!(*w, Some(i), "frame {i}");
        }
        assert!(frames.windows(2).all(|w| w[0].pts < w[1].pts), "pts in display order");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
