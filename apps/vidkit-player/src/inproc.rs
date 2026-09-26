//! In-process decode (vidkit-libav) as a [`VideoSource`]: no ffmpeg child,
//! no pipe; VAAPI frames reach the GL presenter as dma-bufs.
//!
//! Policy (`VIDKIT_DECODE`): `ffmpeg` never uses it; otherwise it is used
//! wherever it can be -- VAAPI (zero copy, the GPU scales), and software
//! decode that ffmpeg would not have had to shrink. Measured on the 4K60
//! VP9 field file: 22.7 ms of CPU per frame in process vs 27.5 through the
//! ffmpeg child and its pipe (-17..19%, 2 rounds, same decoder). Where it
//! cannot (streams, a frame ffmpeg must scale, a pixel format it does not
//! take, an open that fails), the caller keeps the ffmpeg process.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use vidkit_core::frame::{ColorInfo, Frame};
use vidkit_libav::{Decoder, Hw, Options};

use crate::decode::{self, Accel, PlanePool, Probe, VideoSource};

pub struct LibavSource {
    dec: Decoder,
    pool: Arc<PlanePool>,
    color: ColorInfo,
    hdr_peak: Option<f32>,
    interval: Duration,
    note: Option<String>,
    failed: bool,
    /// VIDKIT_DMABUF_EMULATE=1: software frames re-wrapped as memfd-backed
    /// NV12 "dma-bufs", so the zero-copy and scanout paths can be driven on
    /// a machine with no hardware decoder (llvmpipe and weston's GL
    /// renderer map memfds). A test knob: it adds a copy.
    emulate_dmabuf: Option<EmuRing>,
}

/// The emulated decoder's buffer pool: a ring of memfds, reused like a
/// real decoder's surfaces (so buffer caches see a pool, not a stream of
/// new buffers). Test-only: a slot is rewritten when its turn comes,
/// whether or not the compositor still holds it.
#[derive(Default)]
pub struct EmuRing {
    fds: Vec<std::os::fd::OwnedFd>,
    next: usize,
}

/// Policy, pure: whether to decode in process.
pub fn wanted(mode: Option<&str>, accel: &Accel, scale: Option<(u32, u32)>, gpu: bool) -> Option<Hw> {
    let vaapi = match accel {
        Accel::Vaapi { device, .. } if gpu => Some(Hw::Vaapi { device: device.clone() }),
        _ => None,
    };
    match mode {
        Some("ffmpeg") => None,
        // The GPU scales a VAAPI frame as it draws; software frames that
        // ffmpeg would have shrunk (swscale) stay with ffmpeg.
        // Hardware the in-process path cannot take (VAAPI without the GL
        // presenter, V4L2 M2M, Vulkan) stays with ffmpeg: decoding in
        // software here would throw the hardware away.
        _ => vaapi.or_else(|| (!accel.is_hardware() && scale.is_none()).then_some(Hw::Software)),
    }
}

/// Open in process when the policy says so; `None` (with a note on
/// stderr) means "use the ffmpeg process".
pub fn open(path: &Path, p: &Probe, from: Duration, accel: &Accel) -> Option<Box<dyn VideoSource>> {
    let gpu = decode::gpu_presenter();
    let hw = wanted(std::env::var("VIDKIT_DECODE").ok().as_deref(), accel, decode::scale_plan(p), gpu)?;
    if p.width.max(p.height) > decode::texture_limit() {
        return None;
    }
    let threads = decode::decode_cpus().map(|c| c.len() as u32).unwrap_or(0);
    // AV1 grain for the GPU, as the ffmpeg path does with its feed -- here
    // the decoder hands the parameters over itself.
    let export_grain = gpu && hw == Hw::Software && p.codec == "av1" && !p.high_bit_depth() && std::env::var("VIDKIT_GRAIN_CPU").as_deref() != Ok("1");
    let opts = Options { hw: hw.clone(), from, threads, export_grain };
    // libavcodec's worker threads are created in open and inherit the
    // opening thread's affinity: pin this thread around it, as the ffmpeg
    // child was pinned, and put it back.
    let saved = decode::decode_cpus().map(|c| pin_this_thread(&c));
    let r = Decoder::open(&path.to_string_lossy(), &opts);
    if let Some(Some(old)) = saved {
        restore_this_thread(&old);
    }
    match r {
        Ok(dec) => Some(Box::new(LibavSource {
            dec,
            pool: PlanePool::new(),
            color: p.color(),
            hdr_peak: (p.color().transfer == vidkit_core::codec::Transfer::Pq).then(|| decode::probe_hdr_peak(&path.to_string_lossy())).flatten(),
            interval: Duration::from_secs_f64(1.0 / p.fps.max(1.0)),
            note: None,
            failed: false,
            emulate_dmabuf: (std::env::var("VIDKIT_DMABUF_EMULATE").as_deref() == Ok("1")).then(EmuRing::default),
        })),
        Err(e) => {
            eprintln!("vidkit: in-process decode unavailable ({e}); using ffmpeg");
            None
        }
    }
}

fn pin_this_thread(cpus: &[usize]) -> Option<libc::cpu_set_t> {
    // SAFETY: affinity get/set of the calling thread with local sets.
    unsafe {
        let mut old: libc::cpu_set_t = std::mem::zeroed();
        if libc::sched_getaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &mut old) != 0 {
            return None;
        }
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        for &c in cpus {
            libc::CPU_SET(c, &mut set);
        }
        (libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set) == 0).then_some(old)
    }
}

fn restore_this_thread(old: &libc::cpu_set_t) {
    // SAFETY: as above.
    unsafe { libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), old) };
}

impl VideoSource for LibavSource {
    fn next_frame(&mut self) -> Option<Frame> {
        if self.failed {
            return None;
        }
        let pool = self.pool.clone();
        match self.dec.next_frame(&mut |n| pool.take(n)) {
            Ok(Some(mut f)) => {
                f.color = self.color;
                if let Some(ring) = self.emulate_dmabuf.as_mut() {
                    f = emulate_dmabuf(f, ring);
                }
                if self.dec.hw_requested && !self.dec.hw_frames && self.note.is_none() && self.dec.zero_copy == 0 {
                    self.note = Some("VAAPI declined this stream; decoding in software".into());
                }
                Some(f)
            }
            Ok(None) => None,
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
        self.hdr_peak
    }
    fn set_pool(&mut self, pool: Arc<PlanePool>) {
        self.pool = pool;
    }
    fn label(&self) -> String {
        let how = if self.dec.hw_frames { "VAAPI zero-copy" } else if self.dec.hw_requested { "VAAPI (in process)" } else { "software (in process)" };
        format!("{how} {} {}x{}", self.dec.codec, self.dec.width, self.dec.height)
    }
    fn frame_bytes(&self) -> usize {
        decode::frame_bytes(self.dec.width, self.dec.height)
    }
    fn take_note(&mut self) -> Option<String> {
        self.note.take()
    }
}

/// A CPU I420/NV12 frame as a LINEAR NV12 memfd "dma-buf" (see
/// `emulate_dmabuf` on LibavSource).
pub fn emulate_dmabuf(f: Frame, ring: &mut EmuRing) -> Frame {
    use std::os::fd::{FromRawFd, OwnedFd};
    use vidkit_core::frame::{DmaBufImage, DmaBufPlane, FrameData, PlaneLayout};
    let FrameData::Cpu(p) = &f.data else { return f };
    let (w, h) = (f.width as usize, f.height as usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut buf = Vec::with_capacity(w * h + cw * ch * 2);
    let row = |plane: &[u8], stride: usize, y: usize, n: usize| plane[y * stride..y * stride + n].to_vec();
    for y in 0..h {
        buf.extend(row(&p.data[0], p.strides[0] as usize, y, w));
    }
    match f.layout {
        PlaneLayout::I420 => {
            for y in 0..ch {
                for x in 0..cw {
                    buf.push(p.data[1][y * p.strides[1] as usize + x]);
                    buf.push(p.data[2][y * p.strides[2] as usize + x]);
                }
            }
        }
        PlaneLayout::Nv12 => {
            for y in 0..ch {
                buf.extend(row(&p.data[1], p.strides[1] as usize, y, cw * 2));
            }
        }
        _ => return f,
    }
    const SLOTS: usize = 6;
    if ring.fds.len() < SLOTS {
        // SAFETY: a new fd we own.
        ring.fds.push(unsafe { OwnedFd::from_raw_fd(libc::memfd_create(c"vk-emulated".as_ptr(), 0)) });
    }
    let slot = ring.next % ring.fds.len();
    ring.next += 1;
    let Ok(fd) = ring.fds[slot].try_clone() else { return f };
    use std::io::{Seek, Write};
    let mut file = std::fs::File::from(fd.try_clone().expect("dup"));
    if file.seek(std::io::SeekFrom::Start(0)).and_then(|_| file.write_all(&buf)).is_err() {
        return f;
    }
    Frame {
        data: FrameData::DmaBuf(DmaBufImage {
            planes: vec![DmaBufPlane { fd, offset: 0, stride: w as u32 }],
            modifier: vidkit_core::dmabuf::DRM_FORMAT_MOD_LINEAR,
            fourcc: vidkit_core::dmabuf::DRM_FORMAT_NV12,
            owner: None,
        }),
        layout: PlaneLayout::Nv12,
        grain: None,
        recycle: None,
        ..f
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy() {
        let va = Accel::Vaapi { device: "/dev/dri/renderD128".into(), codec: "vp9".into(), vpp: false };
        let dev = Some(Hw::Vaapi { device: "/dev/dri/renderD128".into() });
        assert_eq!(wanted(None, &va, None, true), dev, "VAAPI goes zero-copy by default");
        assert_eq!(wanted(None, &va, None, false), None, "not without the GL presenter to import -- and not in software either");
        let m2m = Accel::V4l2m2m { decoder: "vp9_v4l2m2m".into() };
        assert_eq!(wanted(None, &m2m, None, true), None, "V4L2 M2M stays with ffmpeg");
        assert_eq!(wanted(None, &Accel::Software, None, true), Some(Hw::Software), "software in process by default");
        assert_eq!(wanted(Some("inproc"), &Accel::Software, None, true), Some(Hw::Software));
        assert_eq!(wanted(Some("inproc"), &Accel::Software, Some((1920, 1080)), true), None, "ffmpeg still does the shrinking");
        assert_eq!(wanted(Some("inproc"), &va, Some((1920, 1080)), true), dev, "the GPU shrinks a VAAPI frame");
        assert_eq!(wanted(Some("ffmpeg"), &va, None, true), None);
    }

    /// Through the VideoSource seam: frames arrive, pts from zero, colour
    /// from the probe, and the label says in process.
    #[test]
    fn frames_through_the_seam() {
        let dir = std::env::temp_dir().join(format!("vk-inproc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.webm");
        let ok = std::process::Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=160x90:rate=25:duration=1", "-c:v", "libvpx-vp9", "-deadline", "realtime", "-pix_fmt", "yuv420p"]).arg(&f).status().map(|s| s.success()).unwrap_or(false);
        if !ok {
            return;
        }
        std::env::set_var("VIDKIT_DECODE", "inproc");
        let p = decode::probe(&f).unwrap();
        let mut v = open(&f, &p, Duration::ZERO, &Accel::Software).expect("in process");
        std::env::remove_var("VIDKIT_DECODE");
        let mut n = 0;
        let mut last = None;
        while let Some(fr) = v.next_frame() {
            assert_eq!(fr.color, p.color());
            assert_eq!((fr.width, fr.height), (160, 90));
            last = Some(fr.pts);
            n += 1;
        }
        assert_eq!(n, 25);
        assert!((last.unwrap().as_secs_f64() - 0.96).abs() < 0.002);
        assert!(v.label().starts_with("software (in process) vp9 160x90"), "{}", v.label());
    }

    /// The emulated buffer reads back (through the presenter's own mmap
    /// path) as the frame it was made from, and the ring reuses its slots.
    #[test]
    fn emulated_dmabufs_carry_the_frame_and_cycle_a_pool() {
        use vidkit_core::frame::{CpuPlanes, FrameData, PlaneLayout};
        let mk = |k: u8| Frame {
            data: FrameData::Cpu(CpuPlanes { data: vec![(0..24u8).map(|i| i + k).collect(), vec![1, 2, 3, 4, 5, 6], vec![7, 8, 9, 10, 11, 12]], strides: vec![6, 3, 3] }),
            width: 6, height: 4, layout: PlaneLayout::I420, color: Default::default(), pts: Duration::ZERO, duration: None, recycle: None, grain: None,
        };
        let mut ring = EmuRing::default();
        let mut inodes = std::collections::BTreeSet::new();
        for k in 0..8u8 {
            let f = emulate_dmabuf(mk(k), &mut ring);
            let FrameData::DmaBuf(img) = &f.data else { panic!("not emulated") };
            let (layout, p) = vidkit_egui::dmabuf_gl::map_linear(img, 6, 4).unwrap();
            assert_eq!(layout, PlaneLayout::Nv12);
            assert_eq!(p.data[0], (0..24u8).map(|i| i + k).collect::<Vec<_>>());
            assert_eq!(p.data[1], vec![1, 7, 2, 8, 3, 9, 4, 10, 5, 11, 6, 12]);
            let mut st: libc::stat = unsafe { std::mem::zeroed() };
            unsafe { libc::fstat(std::os::fd::AsRawFd::as_raw_fd(&img.planes[0].fd), &mut st) };
            inodes.insert(st.st_ino);
        }
        assert_eq!(inodes.len(), 6, "a pool of six buffers, reused");
    }
}
