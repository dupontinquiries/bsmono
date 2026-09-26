//! Batched decode for whole-bitstream decoders: GOP by GOP, stitched.
//!
//! vacc's Vulkan Video path has only `decode_all` over a complete bitstream.
//! A player cannot hand it a whole film, but it does not have to: a closed
//! GOP -- a random-access point and everything up to the next -- IS a
//! complete bitstream. So packets are collected until the next
//! random-access point ([`crate::rap`]), the batch is decoded on its own,
//! and batches are stitched in order. YouTube's DASH segments start at
//! keyframes by design (players switch quality between them), and
//! libsvtav1's keyframes are shown ones (measured: rap tests), so real
//! streams cut cleanly. Open-GOP points (AV1 forward keyframes, H.264
//! recovery points) are not cuts; they join the batch before them.
//!
//! The costs, stated: output lags input by one GOP (at start and after a
//! seek, a whole GOP must be demuxed before the first frame); every frame of
//! a GOP is held at once; and each batch opens a fresh decode session. In
//! exchange the GPU decodes in bursts and idles between them.

use std::time::Duration;

use vidkit_core::frame::{ColorInfo, CpuPlanes, Frame, FrameData, PlaneLayout};

use crate::rap;

/// A frame out of a batch decode, before it is given a pts.
pub struct Decoded {
    pub poc: i32,
    pub width: u32,
    pub height: u32,
    pub layout: PlaneLayout,
    pub planes: Vec<Vec<u8>>,
    pub strides: Vec<u32>,
}

/// Decodes one self-contained bitstream (Annex-B for H.264, IVF for
/// VP9/AV1) to frames in DISPLAY order.
pub trait SegmentDecoder {
    fn decode_segment(&mut self, bitstream: Vec<u8>) -> Result<Vec<Decoded>, String>;
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BatchCodec {
    H264,
    Vp9,
    Av1,
}

impl BatchCodec {
    fn is_rap(self, packet: &[u8]) -> bool {
        match self {
            BatchCodec::H264 => rap::h264_is_rap(packet),
            BatchCodec::Vp9 => rap::vp9_is_rap(packet),
            BatchCodec::Av1 => rap::av1_is_rap(packet),
        }
    }
    fn fourcc(self) -> [u8; 4] {
        match self {
            BatchCodec::H264 => *b"H264",
            BatchCodec::Vp9 => *b"VP90",
            BatchCodec::Av1 => *b"AV01",
        }
    }
}

pub struct GopBatcher<D> {
    dec: D,
    codec: BatchCodec,
    size: (u16, u16),
    packets: Vec<Vec<u8>>,
    pts: Vec<Duration>,
    /// Longest batch accepted. Every frame of a GOP is held at once: 300
    /// 1080p frames is ~0.9 GB of I420. Longer -> an error, and the caller
    /// falls back to a streaming decoder.
    pub max_frames: usize,
    pub color: ColorInfo,
    /// Batches decoded, for the profile log.
    pub batches: u64,
}

impl<D: SegmentDecoder> GopBatcher<D> {
    pub fn new(dec: D, codec: BatchCodec, width: u16, height: u16) -> Self {
        Self { dec, codec, size: (width, height), packets: Vec::new(), pts: Vec::new(), max_frames: 300, color: ColorInfo::default(), batches: 0 }
    }

    /// Push one packet (an H.264 access unit, a VP9 frame or superframe, an
    /// AV1 temporal unit) with its pts. Returns the previous batch's frames
    /// when this packet starts a new one.
    pub fn push(&mut self, packet: &[u8], pts: Duration) -> Result<Vec<Frame>, String> {
        let mut out = Vec::new();
        if self.codec.is_rap(packet) && !self.packets.is_empty() {
            out = self.decode_batch()?;
        }
        if self.packets.is_empty() && !self.codec.is_rap(packet) {
            // Joined mid-GOP (a seek not landing on a random-access point):
            // nothing before the next one can be decoded on its own.
            return Ok(out);
        }
        self.packets.push(packet.to_vec());
        self.pts.push(pts);
        if self.packets.len() > self.max_frames {
            return Err(format!("GOP longer than {} frames: too long to batch; decode this title with a streaming decoder", self.max_frames));
        }
        Ok(out)
    }

    /// End of stream: the last batch.
    pub fn flush(&mut self) -> Result<Vec<Frame>, String> {
        if self.packets.is_empty() { Ok(Vec::new()) } else { self.decode_batch() }
    }

    /// A seek: drop what is collected; the next random-access point starts
    /// the next batch.
    pub fn reset(&mut self) {
        self.packets.clear();
        self.pts.clear();
    }

    fn decode_batch(&mut self) -> Result<Vec<Frame>, String> {
        let packets = std::mem::take(&mut self.packets);
        let mut pts = std::mem::take(&mut self.pts);
        let bitstream = match self.codec {
            BatchCodec::H264 => packets.concat(),
            _ => {
                let refs: Vec<&[u8]> = packets.iter().map(Vec::as_slice).collect();
                rap::ivf_wrap(self.codec.fourcc(), self.size.0, self.size.1, &refs)
            }
        };
        let frames = self.dec.decode_segment(bitstream)?;
        self.batches += 1;
        // One shown frame per packet. A mismatch (a hidden frame counted as
        // shown, a dropped frame) would shift every pts after it: refuse.
        if frames.len() != pts.len() {
            return Err(format!("batch of {} packets decoded to {} frames", pts.len(), frames.len()));
        }
        pts.sort();
        Ok(frames
            .into_iter()
            .zip(pts)
            .map(|(d, pts)| Frame {
                data: FrameData::Cpu(CpuPlanes { data: d.planes, strides: d.strides }),
                width: d.width,
                height: d.height,
                layout: d.layout,
                color: self.color,
                pts,
                duration: None,
                recycle: None,
                grain: None,
            })
            .collect())
    }
}

/// vacc's CPU H.264, one fresh decoder per batch -- the same shape as a
/// whole-bitstream `decode_all`, and the only backend that runs without a
/// GPU: the batcher's test oracle.
pub struct CpuH264Segments;

impl SegmentDecoder for CpuH264Segments {
    fn decode_segment(&mut self, bitstream: Vec<u8>) -> Result<Vec<Decoded>, String> {
        use vacc_core::decoder::Decoder as _;
        let mut d = vacc_software_decode::SwH264Decoder::new(bitstream).map_err(|e| e.to_string())?;
        let mut raw = Vec::new();
        while let Some(f) = d.decode().map_err(|e| e.to_string())? {
            raw.push(f);
        }
        raw.extend(d.flush().map_err(|e| e.to_string())?);
        // A closed GOP: POC is monotonic in display order within it.
        raw.sort_by_key(|f| f.poc);
        raw.iter()
            .filter_map(|f| f.pixel_data.as_ref().map(|p| (f, p)))
            .map(|(f, p)| {
                let v = p.v.as_ref().ok_or("I420 without V")?;
                let mut planes = Vec::new();
                let mut strides = Vec::new();
                for pl in [&p.y, &p.u, v] {
                    planes.push(crate::copy_plane(&p.buffer, pl, 1)?);
                    strides.push(pl.width as u32);
                }
                Ok(Decoded { poc: f.poc, width: f.width, height: f.height, layout: PlaneLayout::I420, planes, strides })
            })
            .collect()
    }
}

/// vacc's Vulkan Video decoder, one `decode_all` per batch. Frames come back
/// read back to system memory (planar), cropped here to the display size.
/// BUILT, NOT RUN: no Vulkan device where this was written.
#[cfg(feature = "vulkan")]
pub struct VulkanSegments;

#[cfg(feature = "vulkan")]
impl SegmentDecoder for VulkanSegments {
    fn decode_segment(&mut self, bitstream: Vec<u8>) -> Result<Vec<Decoded>, String> {
        let mut d = vacc_vulkan_decode::VulkanDecoder::new(bitstream).map_err(|e| e.to_string())?;
        let frames = d.decode_all(usize::MAX).map_err(|e| e.to_string())?;
        let frames = vacc_vulkan_decode::VulkanDecoder::reorder_to_presentation(frames);
        frames
            .into_iter()
            .map(|f| {
                let px = &f.pixels;
                let bps = px.sample_size.max(1) as usize;
                let crop = |plane: &[u8], stride: usize, x0: usize, y0: usize, w: usize, h: usize| -> Result<Vec<u8>, String> {
                    let mut o = Vec::with_capacity(w * h * bps);
                    for y in y0..y0 + h {
                        let s = (y * stride + x0) * bps;
                        o.extend_from_slice(plane.get(s..s + w * bps).ok_or("vulkan plane shorter than its crop")?);
                    }
                    Ok(o)
                };
                let (cw, dw, dh) = (f.coded_width as usize, f.display_width as usize, f.display_height as usize);
                let (cl, ct) = (f.crop_left as usize, f.crop_top as usize);
                let ccw = px.chroma_width as usize;
                let (dcw, dch) = (dw.div_ceil(2), dh.div_ceil(2));
                let planes = vec![
                    crop(&px.y_plane, cw, cl, ct, dw, dh)?,
                    crop(&px.u_plane, ccw, cl / 2, ct / 2, dcw, dch)?,
                    crop(&px.v_plane, ccw, cl / 2, ct / 2, dcw, dch)?,
                ];
                let layout = if bps == 2 { PlaneLayout::I420_10le } else { PlaneLayout::I420 };
                Ok(Decoded { poc: f.poc, width: dw as u32, height: dh as u32, layout, strides: vec![(dw * bps) as u32, (dcw * bps) as u32, (dcw * bps) as u32], planes })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    /// Several closed GOPs, batched and stitched, against ffmpeg decoding
    /// the whole stream: every frame byte-identical, pts in display order.
    #[test]
    fn batched_gops_stitch_to_exactly_the_whole_stream() {
        if Command::new("ffmpeg").arg("-version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("vk-batch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (h264, yuv) = (dir.join("g.h264"), dir.join("g.yuv"));
        // 4 s at 30 fps, IDR every 30 frames, 3 B-frames: four GOPs.
        assert!(Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=30:duration=4",
            "-c:v", "libx264", "-profile:v", "high", "-bf", "3", "-g", "30", "-keyint_min", "30", "-sc_threshold", "0",
            "-x264-params", "aud=1:open-gop=0", "-pix_fmt", "yuv420p"]).arg(&h264).status().unwrap().success());
        assert!(Command::new("ffmpeg").args(["-v", "error", "-y", "-i"]).arg(&h264).args(["-f", "rawvideo", "-pix_fmt", "yuv420p"]).arg(&yuv).status().unwrap().success());
        let data = std::fs::read(&h264).unwrap();
        let reference = std::fs::read(&yuv).unwrap();
        let mut b = GopBatcher::new(CpuH264Segments, BatchCodec::H264, 320, 180);
        let mut frames = Vec::new();
        for (i, au) in crate::split_at_aud(&data).iter().enumerate() {
            frames.extend(b.push(au, Duration::from_millis(i as u64 * 1000 / 30)).unwrap());
        }
        frames.extend(b.flush().unwrap());
        assert_eq!(b.batches, 4, "one batch per GOP");
        let fs = 320 * 180 * 3 / 2;
        assert_eq!(frames.len(), reference.len() / fs);
        for (i, f) in frames.iter().enumerate() {
            let FrameData::Cpu(p) = &f.data else { panic!() };
            assert!(p.data.concat() == reference[i * fs..(i + 1) * fs], "frame {i} differs");
        }
        assert!(frames.windows(2).all(|w| w[0].pts < w[1].pts));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Joining mid-GOP (a seek between keyframes) drops packets until the
    /// next random-access point instead of decoding garbage; a GOP past the
    /// cap is an error the caller can act on.
    #[test]
    fn mid_gop_joins_wait_and_long_gops_are_refused() {
        struct Never;
        impl SegmentDecoder for Never {
            fn decode_segment(&mut self, _: Vec<u8>) -> Result<Vec<Decoded>, String> {
                Ok(Vec::new())
            }
        }
        let idr: &[u8] = &[0, 0, 0, 1, 0x65, 0x88];
        let p: &[u8] = &[0, 0, 0, 1, 0x41, 0x9a];
        let mut b = GopBatcher::new(Never, BatchCodec::H264, 16, 16);
        assert!(b.push(p, Duration::ZERO).unwrap().is_empty());
        assert!(b.packets.is_empty(), "a P frame before any IDR is dropped");
        b.push(idr, Duration::ZERO).unwrap();
        b.max_frames = 3;
        b.push(p, Duration::ZERO).unwrap();
        b.push(p, Duration::ZERO).unwrap();
        assert!(b.push(p, Duration::ZERO).is_err(), "GOP past the cap");
    }
}
