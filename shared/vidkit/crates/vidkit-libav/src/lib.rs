//! In-process decode through libavcodec.
//!
//! Two reasons to leave the `ffmpeg` child process:
//! - **The pipe.** Every decoded byte crossed it twice (ffmpeg writes, the
//!   player reads) -- 720 MB/s of copying for 4K60 NV12, on the CPU, while
//!   the decode itself may have been on the GPU.
//! - **Zero copy.** A child process cannot hand over a VA surface. In
//!   process, a VAAPI frame is mapped to DRM-PRIME (`av_hwframe_map`) and
//!   becomes a [`DmaBufImage`] the GL presenter imports without a copy --
//!   decode and display never touch the CPU.
//!
//! Layers: [`Decoder`] is the whole engine (demux, decode, seek, frame
//! conversion); [`frame_out`] is the pure part of turning an `AVFrame`'s
//! planes into ours. Struct layouts come from bindgen against the system
//! headers (ffmpeg-sys-next), never hand-written: they change per major.

use std::ffi::{CStr, CString};
use std::os::fd::{FromRawFd, OwnedFd};
use std::ptr;
use std::time::Duration;

use ffmpeg_sys_next as ff;
use vidkit_core::frame::{BufferOwner, DmaBufImage, DmaBufPlane, Frame, FrameData, PlaneLayout};

pub mod frame_out;
pub mod grain;

/// Where decoding runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hw {
    Software,
    /// VAAPI on a DRM render node; frames come out as dma-bufs.
    Vaapi { device: String },
}

#[derive(Clone, Debug)]
pub struct Options {
    pub hw: Hw,
    /// Start here (accurate: frames before it are decoded and dropped).
    pub from: Duration,
    /// Decoder threads; 0 = libavcodec's choice.
    pub threads: u32,
    /// AV1: have the decoder export film grain instead of synthesising it,
    /// and attach it to 8-bit frames as `Frame::grain` (the GPU adds it).
    pub export_grain: bool,
}

fn err(code: i32) -> String {
    let mut buf = [0i8; 128];
    // SAFETY: av_strerror writes a NUL-terminated string into buf.
    unsafe { ff::av_strerror(code, buf.as_mut_ptr(), buf.len()) };
    unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned()
}

/// An `AVFrame` kept alive by a [`DmaBufImage`]: for a mapped VAAPI frame
/// this holds the source surface, so libavcodec's pool does not decode
/// into it while it is on screen.
struct HeldFrame(*mut ff::AVFrame);
// av_frame_free on any thread is safe: the buffers are refcounted and the
// hw frame pool locks.
unsafe impl Send for HeldFrame {}
unsafe impl Sync for HeldFrame {}
impl Drop for HeldFrame {
    fn drop(&mut self) {
        // SAFETY: we own this reference.
        unsafe { ff::av_frame_free(&mut self.0) };
    }
}

pub struct Decoder {
    fmt: *mut ff::AVFormatContext,
    dec: *mut ff::AVCodecContext,
    hw_device: *mut ff::AVBufferRef,
    pkt: *mut ff::AVPacket,
    frame: *mut ff::AVFrame,
    stream: i32,
    time_base: f64,
    start: i64,
    /// Frames before this pts (stream ticks) are dropped: the accurate seek.
    skip_before: i64,
    sent_eof: bool,
    done: bool,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    /// The decoder's name (`libdav1d`, `vp9`, `av1`...).
    pub codec: String,
    /// Frames are coming out of the hardware (checked on the first frame:
    /// libavcodec falls back to software by itself when the hardware
    /// refuses a stream).
    pub hw_frames: bool,
    pub hw_requested: bool,
    /// Frames that came out as dma-bufs.
    pub zero_copy: u64,
    /// Frames that carried exported film grain.
    pub grain_frames: u64,
    counter: i64,
}

// The contexts are used from the one decoder thread at a time.
unsafe impl Send for Decoder {}

unsafe extern "C" fn pick_vaapi(ctx: *mut ff::AVCodecContext, fmts: *const ff::AVPixelFormat) -> ff::AVPixelFormat {
    let mut p = fmts;
    while *p != ff::AVPixelFormat::AV_PIX_FMT_NONE {
        if *p == ff::AVPixelFormat::AV_PIX_FMT_VAAPI {
            return *p;
        }
        p = p.add(1);
    }
    // No hardware format for this stream (profile, size): libavcodec's own
    // first choice, which is software.
    let _ = ctx;
    *fmts
}

impl Decoder {
    pub fn open(path: &str, opts: &Options) -> Result<Self, String> {
        static QUIET: std::sync::Once = std::sync::Once::new();
        // libavcodec logs to stderr at INFO ("libdav1d 1.4.1" per open);
        // errors still come back through our Results.
        QUIET.call_once(|| unsafe { ff::av_log_set_level(ff::AV_LOG_ERROR) });
        let c_path = CString::new(path).map_err(|_| "path has a NUL")?;
        // SAFETY: standard libavformat/libavcodec setup; every pointer is
        // checked, and Drop frees whatever was allocated.
        unsafe {
            let mut d = Decoder {
                fmt: ptr::null_mut(),
                dec: ptr::null_mut(),
                hw_device: ptr::null_mut(),
                pkt: ptr::null_mut(),
                frame: ptr::null_mut(),
                stream: -1,
                time_base: 0.0,
                start: 0,
                skip_before: i64::MIN,
                sent_eof: false,
                done: false,
                width: 0,
                height: 0,
                fps: 0.0,
                codec: String::new(),
                hw_frames: false,
                hw_requested: opts.hw != Hw::Software,
                zero_copy: 0,
                grain_frames: 0,
                counter: 0,
            };
            let r = ff::avformat_open_input(&mut d.fmt, c_path.as_ptr(), ptr::null(), ptr::null_mut());
            if r < 0 {
                return Err(format!("{path}: {}", err(r)));
            }
            let r = ff::avformat_find_stream_info(d.fmt, ptr::null_mut());
            if r < 0 {
                return Err(format!("stream info: {}", err(r)));
            }
            let mut codec: *const ff::AVCodec = ptr::null();
            d.stream = ff::av_find_best_stream(d.fmt, ff::AVMediaType::AVMEDIA_TYPE_VIDEO, -1, -1, &mut codec, 0);
            if d.stream < 0 || codec.is_null() {
                return Err(format!("no decodable video stream: {}", err(d.stream)));
            }
            let st = *(*d.fmt).streams.add(d.stream as usize);
            let par = (*st).codecpar;
            // libdav1d is AV1's default decoder and never uses the hardware;
            // for VAAPI the native decoder must be named (as on the CLI).
            if d.hw_requested && (*par).codec_id == ff::AVCodecID::AV_CODEC_ID_AV1 {
                let native = ff::avcodec_find_decoder_by_name(c"av1".as_ptr());
                if !native.is_null() {
                    codec = native;
                }
            }
            d.codec = CStr::from_ptr((*codec).name).to_string_lossy().into_owned();
            d.dec = ff::avcodec_alloc_context3(codec);
            if d.dec.is_null() {
                return Err("avcodec_alloc_context3".into());
            }
            let r = ff::avcodec_parameters_to_context(d.dec, par);
            if r < 0 {
                return Err(format!("codec parameters: {}", err(r)));
            }
            (*d.dec).pkt_timebase = (*st).time_base;
            if let Hw::Vaapi { device } = &opts.hw {
                let dev = CString::new(device.as_str()).map_err(|_| "device has a NUL")?;
                let r = ff::av_hwdevice_ctx_create(&mut d.hw_device, ff::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI, dev.as_ptr(), ptr::null_mut(), 0);
                if r < 0 {
                    return Err(format!("VAAPI device {device}: {}", err(r)));
                }
                (*d.dec).hw_device_ctx = ff::av_buffer_ref(d.hw_device);
                (*d.dec).get_format = Some(pick_vaapi);
                // Frames held on screen and queued in the player must not
                // starve the decoder's surface pool.
                (*d.dec).extra_hw_frames = 8;
            } else {
                (*d.dec).thread_count = opts.threads as i32;
            }
            if opts.export_grain {
                (*d.dec).export_side_data |= ff::AV_CODEC_EXPORT_DATA_FILM_GRAIN;
            }
            let r = ff::avcodec_open2(d.dec, codec, ptr::null_mut());
            if r < 0 {
                return Err(format!("open {}: {}", d.codec, err(r)));
            }
            d.pkt = ff::av_packet_alloc();
            d.frame = ff::av_frame_alloc();
            if d.pkt.is_null() || d.frame.is_null() {
                return Err("out of memory".into());
            }
            let tb = (*st).time_base;
            d.time_base = tb.num as f64 / tb.den.max(1) as f64;
            d.start = if (*st).start_time == ff::AV_NOPTS_VALUE { 0 } else { (*st).start_time };
            d.width = (*par).width as u32;
            d.height = (*par).height as u32;
            let fr = (*st).avg_frame_rate;
            let fr = if fr.num > 0 && fr.den > 0 { fr } else { (*st).r_frame_rate };
            d.fps = if fr.num > 0 && fr.den > 0 { fr.num as f64 / fr.den as f64 } else { 24.0 };
            if !opts.from.is_zero() {
                let ts = d.start + (opts.from.as_secs_f64() / d.time_base) as i64;
                let r = ff::avformat_seek_file(d.fmt, d.stream, i64::MIN, ts, ts, 0);
                if r < 0 {
                    return Err(format!("seek: {}", err(r)));
                }
                // Half a tick of slack: a frame AT the target is kept.
                d.skip_before = ts;
            }
            Ok(d)
        }
    }

    /// The next frame (in presentation order), or `None` at the end. `alloc`
    /// supplies plane buffers (the player's pool).
    pub fn next_frame(&mut self, alloc: &mut dyn FnMut(usize) -> Vec<u8>) -> Result<Option<Frame>, String> {
        // SAFETY: the send/receive loop of the libavcodec API, on contexts
        // this struct owns.
        unsafe {
            loop {
                if self.done {
                    return Ok(None);
                }
                let r = ff::avcodec_receive_frame(self.dec, self.frame);
                if r == 0 {
                    let pts = match (*self.frame).best_effort_timestamp {
                        ff::AV_NOPTS_VALUE => {
                            self.counter += 1;
                            self.start + ((self.counter - 1) as f64 / self.fps / self.time_base) as i64
                        }
                        p => p,
                    };
                    if pts < self.skip_before {
                        ff::av_frame_unref(self.frame);
                        continue;
                    }
                    let t = Duration::from_secs_f64(((pts - self.start) as f64 * self.time_base).max(0.0));
                    let out = self.convert(t, alloc);
                    ff::av_frame_unref(self.frame);
                    return out.map(Some);
                }
                if r == ff::AVERROR_EOF {
                    self.done = true;
                    return Ok(None);
                }
                if r != ff::AVERROR(libc::EAGAIN) {
                    return Err(format!("decode: {}", err(r)));
                }
                if self.sent_eof {
                    self.done = true;
                    return Ok(None);
                }
                // Needs input: the next packet of our stream.
                let r = ff::av_read_frame(self.fmt, self.pkt);
                if r < 0 {
                    // End of file (or a read error): drain the decoder.
                    ff::avcodec_send_packet(self.dec, ptr::null());
                    self.sent_eof = true;
                    continue;
                }
                if (*self.pkt).stream_index == self.stream {
                    let r = ff::avcodec_send_packet(self.dec, self.pkt);
                    if r < 0 && r != ff::AVERROR(libc::EAGAIN) {
                        ff::av_packet_unref(self.pkt);
                        return Err(format!("send packet: {}", err(r)));
                    }
                }
                ff::av_packet_unref(self.pkt);
            }
        }
    }

    unsafe fn convert(&mut self, pts: Duration, alloc: &mut dyn FnMut(usize) -> Vec<u8>) -> Result<Frame, String> {
        let f = self.frame;
        let (w, h) = ((*f).width as u32, (*f).height as u32);
        self.width = w;
        self.height = h;
        let meta = |data: FrameData, layout: PlaneLayout| Frame {
            data,
            width: w,
            height: h,
            layout,
            color: Default::default(),
            pts,
            duration: Some(Duration::from_secs_f64(1.0 / self.fps.max(1.0))),
            recycle: None,
            grain: None,
        };
        if (*f).format == ff::AVPixelFormat::AV_PIX_FMT_VAAPI as i32 {
            self.hw_frames = true;
            let img = self.map_drm(f)?;
            self.zero_copy += 1;
            let layout = vidkit_core::dmabuf::layout_of(img.fourcc).unwrap_or(PlaneLayout::Nv12);
            return Ok(meta(FrameData::DmaBuf(img), layout));
        }
        let fmt = frame_out::SwFormat::from_av((*f).format).ok_or_else(|| format!("pixel format {} is not handled in process", (*f).format))?;
        let planes: Vec<(&[u8], usize)> = (0..fmt.planes())
            .map(|i| {
                let ls = (*f).linesize[i] as usize;
                let rows = fmt.plane_rows(i, h as usize);
                (std::slice::from_raw_parts((*f).data[i], ls * rows), ls)
            })
            .collect();
        let (layout, cpu) = frame_out::to_cpu(fmt, &planes, w as usize, h as usize, alloc);
        let mut out = meta(FrameData::Cpu(cpu), layout);
        if matches!(layout, PlaneLayout::I420 | PlaneLayout::Nv12) {
            let sd = ff::av_frame_get_side_data(f, ff::AVFrameSideDataType::AV_FRAME_DATA_FILM_GRAIN_PARAMS);
            if !sd.is_null() && (*sd).size as usize >= std::mem::size_of::<grain::AVFilmGrainParamsPrefix>() {
                let g = &*((*sd).data as *const grain::AVFilmGrainParamsPrefix);
                if g.type_ == grain::AV_FILM_GRAIN_PARAMS_AV1 {
                    out.grain = Some(std::sync::Arc::new(grain::from_aom(g.seed, &g.aom)));
                    self.grain_frames += 1;
                }
            }
        }
        Ok(out)
    }

    /// A VAAPI surface as DRM-PRIME: fds dup'd, the mapping (and through it
    /// the surface) held by the image.
    unsafe fn map_drm(&self, src: *mut ff::AVFrame) -> Result<DmaBufImage, String> {
        let mut drm = ff::av_frame_alloc();
        if drm.is_null() {
            return Err("out of memory".into());
        }
        (*drm).format = ff::AVPixelFormat::AV_PIX_FMT_DRM_PRIME as i32;
        let r = ff::av_hwframe_map(drm, src, ff::AV_HWFRAME_MAP_READ as i32);
        if r < 0 {
            ff::av_frame_free(&mut drm);
            return Err(format!("map VAAPI to DRM-PRIME: {}", err(r)));
        }
        let held = HeldFrame(drm);
        let desc = (*drm).data[0] as *const ff::AVDRMFrameDescriptor;
        let desc = &*desc;
        let mut planes = Vec::new();
        let mut layer_formats = Vec::new();
        for l in 0..desc.nb_layers as usize {
            let layer = &desc.layers[l];
            layer_formats.push(layer.format);
            for p in 0..layer.nb_planes as usize {
                let pl = &layer.planes[p];
                let obj = &desc.objects[pl.object_index as usize];
                let fd = libc::fcntl(obj.fd, libc::F_DUPFD_CLOEXEC, 0);
                if fd < 0 {
                    return Err("dup of a dma-buf fd failed".into());
                }
                planes.push(DmaBufPlane { fd: OwnedFd::from_raw_fd(fd), offset: pl.offset as u32, stride: pl.pitch as u32 });
            }
        }
        let modifier = desc.objects[0].format_modifier;
        let fourcc = frame_out::fourcc_of_layers(&layer_formats).ok_or_else(|| format!("DRM layers {layer_formats:08x?} are not NV12/P010"))?;
        Ok(DmaBufImage { planes, modifier, fourcc, owner: Some(BufferOwner(Box::new(held))) })
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: each free takes a pointer-to-pointer and tolerates null.
        unsafe {
            ff::av_frame_free(&mut self.frame);
            ff::av_packet_free(&mut self.pkt);
            ff::avcodec_free_context(&mut self.dec);
            ff::av_buffer_unref(&mut self.hw_device);
            ff::avformat_close_input(&mut self.fmt);
        }
    }
}
