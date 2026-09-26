//! A V4L2 stateful decoder (Qualcomm Venus on the Fairphone 5) in process,
//! as a [`VideoSource`]: libavformat demuxes, vidkit-libav turns packets
//! into what V4L2 takes (Annex B, one VP9 frame per buffer), vidkit-v4l2's
//! state machine drives the device, and the CAPTURE buffers come out as
//! dma-bufs for the GL presenter or the video plane. No copy anywhere --
//! where ffmpeg's `*_v4l2m2m` decoders copied every frame out of the
//! driver's buffers and down a pipe.
//!
//! Buffer return: each frame's dma-buf carries an owner that, when the
//! frame is dropped (presenter replaced it, plane released it, queue
//! discarded it), sends its recycle token back here; the next call hands
//! it to the decoder. Tokens from before a reconfiguration are ignored by
//! the state machine's generation check.
//!
//! Unverified on hardware. Known to check on the FP5: Venus pads luma to 32
//! lines; if the capture format reports the padded height the picture has
//! rows of garbage at the bottom until crop (G_SELECTION) is honoured.

use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use vidkit_core::frame::{BufferOwner, ColorInfo, Frame, FrameData, RecycleToken};
use vidkit_libav::bitstream::Codec as BsCodec;
use vidkit_libav::demux::{Demuxer, Unit};
use vidkit_v4l2::{Output, StatefulDecoder, V4l2Api};

use crate::decode::{self, PlanePool, VideoSource};

/// Sleep until the device has something to say.
pub trait Wait {
    fn wait(&self, timeout_ms: i32);
}

impl Wait for vidkit_v4l2::device::Device {
    fn wait(&self, timeout_ms: i32) {
        vidkit_v4l2::device::Device::wait(self, timeout_ms)
    }
}

/// Sends a token back when the frame holding it goes.
struct Returner {
    tx: mpsc::Sender<RecycleToken>,
    token: Option<RecycleToken>,
}

impl Drop for Returner {
    fn drop(&mut self) {
        if let Some(t) = self.token.take() {
            let _ = self.tx.send(t);
        }
    }
}

pub struct V4l2Source<A: V4l2Api + Wait + Send> {
    demux: Demuxer,
    dec: StatefulDecoder<A>,
    tx: mpsc::Sender<RecycleToken>,
    rx: mpsc::Receiver<RecycleToken>,
    pending: Option<Unit>,
    eof: bool,
    /// The demuxer starts at the key frame before the seek point; frames
    /// before it are decoded (they must be) and dropped.
    skip_before: Duration,
    color: ColorInfo,
    interval: Duration,
    width: u32,
    height: u32,
    name: String,
    note: Option<String>,
    failed: bool,
    pub frames: u64,
}

pub fn core_codec(c: BsCodec) -> vidkit_core::codec::Codec {
    match c {
        BsCodec::H264 => vidkit_core::codec::Codec::H264,
        BsCodec::Hevc => vidkit_core::codec::Codec::Hevc,
        BsCodec::Vp9 => vidkit_core::codec::Codec::Vp9,
    }
}

impl<A: V4l2Api + Wait + Send> V4l2Source<A> {
    pub fn new(demux: Demuxer, api: A, from: Duration, color: ColorInfo, name: &str) -> Self {
        let (tx, rx) = mpsc::channel();
        let (w, h) = (demux.width, demux.height);
        let dec = StatefulDecoder::new(api, core_codec(demux.codec), w, h);
        let interval = Duration::from_secs_f64(1.0 / demux.fps.max(1.0));
        Self { demux, dec, tx, rx, pending: None, eof: false, skip_before: from, color, interval, width: w, height: h, name: name.to_string(), note: None, failed: false, frames: 0 }
    }

    pub fn decoder(&self) -> &StatefulDecoder<A> {
        &self.dec
    }

    fn step(&mut self) -> Result<Option<Frame>, String> {
        loop {
            while let Ok(t) = self.rx.try_recv() {
                self.dec.recycle(t).map_err(|e| format!("recycle: {e:?}"))?;
            }
            match self.dec.pump().map_err(|e| format!("decode: {e:?}"))? {
                Output::Frame(mut f) => {
                    let token = f.recycle.take();
                    match &mut f.data {
                        FrameData::DmaBuf(img) => img.owner = Some(BufferOwner(Box::new(Returner { tx: self.tx.clone(), token }))),
                        // A CPU-mapped buffer (not what Venus gives): give it
                        // straight back once copied out... by dropping here
                        // after use -- the same owner route.
                        _ => drop(Returner { tx: self.tx.clone(), token }),
                    }
                    if f.pts < self.skip_before {
                        continue; // dropped: its token goes back
                    }
                    f.color = self.color;
                    self.frames += 1;
                    return Ok(Some(f));
                }
                Output::FormatChanged { width, height } => {
                    self.width = width;
                    self.height = height;
                }
                Output::EndOfStream => return Ok(None),
                Output::Idle => {
                    if self.pending.is_none() && !self.eof {
                        match self.demux.next()? {
                            Some(u) => self.pending = Some(u),
                            None => {
                                self.eof = true;
                                self.dec.drain().map_err(|e| format!("drain: {e:?}"))?;
                                continue;
                            }
                        }
                    }
                    if let Some(u) = self.pending.take() {
                        if self.dec.send_packet(&u.data, Some(u.pts)).map_err(|e| format!("send: {e:?}"))? {
                            continue;
                        }
                        self.pending = Some(u);
                    }
                    // Nothing to do until the device (or a returned buffer)
                    // moves: sleep on the fd, briefly -- a token coming back
                    // does not wake it.
                    self.dec.device().wait(5);
                }
            }
        }
    }
}

impl<A: V4l2Api + Wait + Send> VideoSource for V4l2Source<A> {
    fn next_frame(&mut self) -> Option<Frame> {
        if self.failed {
            return None;
        }
        match self.step() {
            Ok(f) => f,
            Err(e) => {
                self.failed = true;
                self.note = Some(e);
                None
            }
        }
    }
    fn frame_interval(&self) -> Duration {
        self.interval
    }
    fn hdr_peak(&self) -> Option<f32> {
        None
    }
    fn set_pool(&mut self, _pool: Arc<PlanePool>) {}
    fn label(&self) -> String {
        format!("V4L2 {} zero-copy {}x{}", self.name, self.width, self.height)
    }
    fn frame_bytes(&self) -> usize {
        decode::frame_bytes(self.width, self.height)
    }
    fn take_note(&mut self) -> Option<String> {
        self.note.take()
    }
}

/// Open the machine's stateful decoder for this input, when policy says
/// so: a detected Venus (or `VIDKIT_DECODE=v4l2`), the GL presenter up,
/// H.264/HEVC/VP9, a decoder node. `None`: use another path.
pub fn open(input: &str, headers: &[(String, String)], p: &decode::Probe, from: Duration) -> Option<Box<dyn VideoSource>> {
    let mode = std::env::var("VIDKIT_DECODE").ok();
    let venus = matches!(decode::machine().kind, decode::MachineKind::QualcommVenus);
    if !(mode.as_deref() == Some("v4l2") || (venus && mode.as_deref() != Some("ffmpeg"))) || !decode::gpu_presenter() {
        return None;
    }
    if !matches!(p.codec.as_str(), "h264" | "hevc" | "vp9") || p.high_bit_depth() {
        return None;
    }
    let (node, name) = vidkit_v4l2::sysprobe::decoder_nodes(std::path::Path::new("/")).into_iter().next()?;
    let network = input.starts_with("http://") || input.starts_with("https://");
    let opts = if network { crate::inproc::stream_options(headers) } else { Vec::new() };
    let demux = match Demuxer::open(input, &opts, from) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("vidkit: V4L2 path: {e}");
            return None;
        }
    };
    match vidkit_v4l2::device::Device::open(&node) {
        Ok(dev) => Some(Box::new(V4l2Source::new(demux, dev, from, p.color(), &name))),
        Err(e) => {
            eprintln!("vidkit: V4L2 decoder {node}: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashSet, VecDeque};
    use std::os::fd::{FromRawFd, OwnedFd};
    use vidkit_v4l2::api::pixfmt;
    use vidkit_v4l2::{ApiError, CaptureFormat, DequeuedCapture, Event, Queue};

    /// A closed-loop stand-in for Venus (no reordering: the clip has no
    /// B-frames; a real decoder outputs in display order): every queued packet becomes a
    /// decoded frame (same timestamp) in a free CAPTURE buffer, as a memfd
    /// dma-buf; SOURCE_CHANGE after the first packet; LAST after STOP.
    /// Capture back-pressure is real: a frame the player holds keeps its
    /// buffer until its token comes back.
    struct Sim {
        w: u32,
        h: u32,
        capture: u32,
        free: VecDeque<u32>,
        queued: HashSet<u32>,
        waiting: VecDeque<Duration>,
        out: VecDeque<DequeuedCapture>,
        events: VecDeque<Event>,
        started: bool,
        streaming: bool,
        stopping: bool,
        fds: Vec<OwnedFd>,
        packets: usize,
        waits: std::cell::Cell<u32>,
    }

    impl Sim {
        fn new(w: u32, h: u32) -> Self {
            Self { w, h, capture: 0, free: VecDeque::new(), queued: HashSet::new(), waiting: VecDeque::new(), out: VecDeque::new(), events: VecDeque::new(), started: false, streaming: false, stopping: false, fds: Vec::new(), packets: 0, waits: std::cell::Cell::new(0) }
        }
        fn run(&mut self) {
            while let (Some(&pts), true) = (self.waiting.front(), self.streaming) {
                let Some(i) = self.queued.iter().copied().min() else { return };
                self.queued.remove(&i);
                self.waiting.pop_front();
                let fd = self.fds[i as usize].try_clone().unwrap();
                self.out.push_back(DequeuedCapture {
                    index: i,
                    data: FrameData::DmaBuf(vidkit_core::frame::DmaBufImage { planes: vec![vidkit_core::frame::DmaBufPlane { fd, offset: 0, stride: self.w }], modifier: 0, fourcc: pixfmt::NV12, owner: None }),
                    pts,
                    last: false,
                    error: false,
                });
            }
            if self.stopping && self.waiting.is_empty() {
                if let Some(i) = self.queued.iter().copied().min() {
                    self.queued.remove(&i);
                    self.stopping = false;
                    self.out.push_back(DequeuedCapture { index: i, data: FrameData::Cpu(vidkit_core::frame::CpuPlanes { data: vec![], strides: vec![] }), pts: Duration::ZERO, last: true, error: false });
                }
            }
        }
    }

    impl Wait for Sim {
        /// Starved for good (buffers never come back) would spin forever:
        /// fail instead.
        fn wait(&self, _ms: i32) {
            let n = self.waits.get() + 1;
            self.waits.set(n);
            assert!(n < 10_000, "decoder starved: capture buffers are not coming back");
        }
    }

    impl V4l2Api for Sim {
        fn set_output_format(&mut self, _: u32, _: u32, _: u32) -> Result<(), ApiError> { Ok(()) }
        fn request_buffers(&mut self, q: Queue, n: u32) -> Result<u32, ApiError> {
            if q == Queue::Capture {
                self.capture = n;
                self.queued.clear();
                while (self.fds.len() as u32) < n {
                    self.fds.push(unsafe { OwnedFd::from_raw_fd(libc::memfd_create(c"sim".as_ptr(), 0)) });
                }
            }
            Ok(n)
        }
        fn stream_on(&mut self, q: Queue) -> Result<(), ApiError> {
            if q == Queue::Capture { self.streaming = true; }
            Ok(())
        }
        fn stream_off(&mut self, q: Queue) -> Result<(), ApiError> {
            if q == Queue::Capture { self.streaming = false; }
            Ok(())
        }
        fn capture_format(&mut self) -> Result<CaptureFormat, ApiError> { Ok(CaptureFormat { width: self.w, height: self.h, fourcc: pixfmt::NV12, modifier: 0 }) }
        fn min_capture_buffers(&mut self) -> Result<u32, ApiError> { Ok(4) }
        fn queue_output(&mut self, _data: &[u8], pts: Option<Duration>) -> Result<bool, ApiError> {
            if self.waiting.len() >= 4 {
                return Ok(false);
            }
            self.packets += 1;
            self.waiting.push_back(pts.unwrap_or_default());
            if !self.started {
                self.started = true;
                self.events.push_back(Event::SourceChange);
            }
            Ok(true)
        }
        fn dequeue_output(&mut self) -> Result<bool, ApiError> { Ok(false) }
        fn queue_capture(&mut self, i: u32) -> Result<(), ApiError> {
            assert!(self.queued.insert(i), "buffer {i} queued twice");
            Ok(())
        }
        fn dequeue_capture(&mut self) -> Result<Option<DequeuedCapture>, ApiError> {
            self.run();
            Ok(self.out.pop_front())
        }
        fn dequeue_event(&mut self) -> Result<Option<Event>, ApiError> { Ok(self.events.pop_front()) }
        fn decoder_cmd_stop(&mut self) -> Result<(), ApiError> { self.stopping = true; Ok(()) }
        fn decoder_cmd_start(&mut self) -> Result<(), ApiError> { Ok(()) }
    }

    fn clip() -> Option<std::path::PathBuf> {
        let dir = std::env::temp_dir().join(format!("vk-v4l2src-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok()?;
        let f = dir.join("a.mp4");
        std::process::Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=160x96:rate=25:duration=2", "-c:v", "libx264", "-g", "25", "-bf", "0", "-pix_fmt", "yuv420p"]).arg(&f).status().ok()?.success().then_some(f)
    }

    /// Every packet's picture comes out in order, the seek drops what is
    /// before it, buffers the player HOLDS stall the decoder only until
    /// they are dropped, and the stream ends with the drain.
    #[test]
    fn frames_flow_buffers_return_and_the_stream_ends() {
        let Some(f) = clip() else { return };
        let from = Duration::from_millis(1200);
        let demux = Demuxer::open(f.to_str().unwrap(), &[], from).unwrap();
        let mut src = V4l2Source::new(demux, Sim::new(160, 96), from, ColorInfo::default(), "sim");
        let mut held = VecDeque::new();
        let mut pts = Vec::new();
        while let Some(fr) = src.next_frame() {
            assert!(fr.is_zero_copy());
            pts.push(fr.pts);
            // Hold three frames at a time, like the queue + presenter.
            held.push_back(fr);
            if held.len() > 3 {
                held.pop_front();
            }
        }
        assert!(src.take_note().is_none());
        assert_eq!(pts.len(), 20, "frames from 1.2 s to the end at 25 fps: {pts:?}");
        assert!((pts[0].as_secs_f64() - 1.2).abs() < 0.001, "{:?}", pts[0]);
        assert!(pts.windows(2).all(|w| w[1] > w[0]), "in order");
        // The key frame is at 1.0 s: 5 frames before the seek were decoded
        // and returned, so the device saw 25 packets through 8 buffers.
        assert_eq!(src.decoder().device().packets, 25);
    }
}
