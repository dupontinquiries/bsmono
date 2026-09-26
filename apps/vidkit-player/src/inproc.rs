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
    /// Thread learning: decoder threads in use, wall time spent inside
    /// next_frame (the decoder being the bottleneck), video time covered.
    threads: u32,
    busy: Duration,
    span: (Option<Duration>, Duration),
    key: String,
}

/// Decoder threads that keep up with room to spare, from a run at
/// `used` threads that was busy `busy` of `video` seconds. Pure.
///
/// Frame threads cost CPU of their own -- synchronisation, a frame in
/// flight each, colder caches. Measured here (2 cores): 2 threads over 1
/// cost +13% CPU on 1080p60 AV1 and +8% on 4K60 VP9 for the same frames.
/// libavcodec's default is one per core, 12 on an M2 Max, whatever the
/// stream needs. Target: the decoder busy 60% of real time. Under 5 s of
/// video says too little; keep what was used.
pub fn recommend_threads(used: u32, busy: f64, video: f64, cores: u32) -> u32 {
    if video < 5.0 || used == 0 {
        return used;
    }
    let u = busy / video;
    ((used as f64 * u / 0.6 - 1e-9).ceil() as u32).clamp(1, cores.max(1))
}

fn threads_cache() -> Option<std::path::PathBuf> {
    std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")))
        .map(|d| d.join("vidkit/decode-threads"))
}

/// The learned thread count for a kind of stream (codec, size, rate).
pub fn learned_threads(key: &str) -> Option<u32> {
    let text = std::fs::read_to_string(threads_cache()?).ok()?;
    text.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix('=')?.trim().parse().ok())
}

fn learn_threads(key: &str, n: u32) {
    let Some(path) = threads_cache() else { return };
    let mut lines: Vec<String> = std::fs::read_to_string(&path).unwrap_or_default().lines().filter(|l| !l.starts_with(&format!("{key}="))).map(str::to_string).collect();
    lines.push(format!("{key}={n}"));
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(&path, lines.join("\n") + "\n");
}

/// The kind of stream, for learning: codec, height to the nearest 360
/// lines, frame rate to the nearest 10. Pure.
pub fn stream_key(codec: &str, height: u32, fps: f64) -> String {
    format!("{codec}:{}p:{}fps", (height as f64 / 360.0).round() as u32 * 360, (fps / 10.0).round() as u32 * 10)
}

impl Drop for LibavSource {
    fn drop(&mut self) {
        let (Some(first), last) = self.span else { return };
        let video = last.saturating_sub(first).as_secs_f64();
        let cores = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1);
        let n = recommend_threads(self.threads, self.busy.as_secs_f64(), video, cores);
        if n != self.threads && video >= 5.0 {
            eprintln!("vidkit: {}: {} decoder threads were busy {:.0}% of real time; next time {n}", self.key, self.threads, self.busy.as_secs_f64() / video * 100.0);
            learn_threads(&self.key, n);
        }
    }
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

/// Open a local file in process when the policy says so; `None` (with a
/// note on stderr) means "use the ffmpeg process".
pub fn open(path: &Path, p: &Probe, from: Duration, accel: &Accel) -> Option<Box<dyn VideoSource>> {
    open_input(&path.to_string_lossy(), &[], p, from, accel)
}

/// libavformat options for an HTTP(S) stream: the headers it is signed
/// against, and reconnection (a CDN closing an idle socket mid-film is
/// routine) -- the same as the ffmpeg child is given. Pure.
pub fn stream_options(headers: &[(String, String)]) -> Vec<(String, String)> {
    let mut o = Vec::new();
    if !headers.is_empty() {
        o.push(("headers".to_string(), headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect()));
    }
    for (k, v) in [("reconnect", "1"), ("reconnect_streamed", "1"), ("reconnect_delay_max", "5")] {
        o.push((k.to_string(), v.to_string()));
    }
    o
}

/// [`open`] for a path or URL (`headers` only for URLs).
pub fn open_input(input: &str, headers: &[(String, String)], p: &Probe, from: Duration, accel: &Accel) -> Option<Box<dyn VideoSource>> {
    let network = input.starts_with("http://") || input.starts_with("https://");
    let gpu = decode::gpu_presenter();
    let hw = wanted(std::env::var("VIDKIT_DECODE").ok().as_deref(), accel, decode::scale_plan(p), gpu)?;
    if p.width.max(p.height) > decode::texture_limit() {
        return None;
    }
    // Pinned: one per pinned core. Otherwise what this kind of stream was
    // learned to need, else one per core (libavcodec's default) for the
    // first run to learn from.
    let key = stream_key(&p.codec, p.height, p.fps);
    let cores = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1);
    let threads = match (decode::decode_cpus(), hw == Hw::Software) {
        (Some(c), _) => c.len() as u32,
        (None, true) => learned_threads(&key).unwrap_or(cores).min(cores),
        (None, false) => 0,
    };
    // AV1 grain for the GPU, as the ffmpeg path does with its feed -- here
    // the decoder hands the parameters over itself.
    let export_grain = gpu && hw == Hw::Software && p.codec == "av1" && !p.high_bit_depth() && std::env::var("VIDKIT_GRAIN_CPU").as_deref() != Ok("1");
    let input_options = if network { stream_options(headers) } else { Vec::new() };
    // Showing at most `cap` fps of a stream with more than half again as
    // many: frames nothing references are frames nobody will see -- don't
    // decode them. Drift-free (no reference changes). Measured, SVT-AV1
    // 1080p60 (mini-GOP 16): half the frames skipped, -41..46% decode CPU.
    // YouTube's libvpx VP9 has no non-reference frames: no effect there.
    let skip_nonref = decode::fps_cap().is_some_and(|c| (c as f64) * 1.5 < p.fps);
    let opts = Options { hw: hw.clone(), from, threads, export_grain, input_options, skip_nonref, codec_options: Vec::new() };
    // libavcodec's worker threads are created in open and inherit the
    // opening thread's affinity: pin this thread around it, as the ffmpeg
    // child was pinned, and put it back.
    let saved = decode::decode_cpus().map(|c| pin_this_thread(&c));
    let r = Decoder::open(input, &opts);
    if let Some(Some(old)) = saved {
        restore_this_thread(&old);
    }
    match r {
        Ok(dec) => Some(Box::new(LibavSource {
            dec,
            pool: PlanePool::new(),
            color: p.color(),
            // ffprobe of a URL would need the headers too; the stream path
            // takes the EETF default, as the ffmpeg child's did not ask.
            hdr_peak: (!network && p.color().transfer == vidkit_core::codec::Transfer::Pq).then(|| decode::probe_hdr_peak(input)).flatten(),
            interval: Duration::from_secs_f64(1.0 / p.fps.max(1.0)),
            note: None,
            failed: false,
            emulate_dmabuf: (std::env::var("VIDKIT_DMABUF_EMULATE").as_deref() == Ok("1")).then(EmuRing::default),
            threads,
            busy: Duration::ZERO,
            span: (None, Duration::ZERO),
            key,
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
        let t0 = std::time::Instant::now();
        let r = self.dec.next_frame(&mut |n| pool.take(n));
        self.busy += t0.elapsed();
        match r {
            Ok(Some(mut f)) => {
                f.color = self.color;
                self.span = (self.span.0.or(Some(f.pts)), f.pts);
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

    /// A signed-URL stand-in: an HTTP server with byte ranges that answers
    /// 403 without the header. In process, the stream opens only with the
    /// headers, seeks by range, and decodes the same pictures as the CLI
    /// does from the file.
    #[test]
    fn a_stream_opens_with_its_headers_and_seeks() {
        use std::io::{BufRead, BufReader, Read, Write};
        let dir = std::env::temp_dir().join(format!("vk-inproc-http-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("s.webm");
        let ok = std::process::Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=25:duration=4", "-c:v", "libvpx-vp9", "-deadline", "realtime", "-cpu-used", "8", "-g", "25", "-pix_fmt", "yuv420p"]).arg(&f).status().map(|s| s.success()).unwrap_or(false);
        if !ok {
            return;
        }
        let body = std::fs::read(&f).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let ranged = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let r2 = ranged.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut c) = conn else { continue };
                let (body, ranged) = (body.clone(), r2.clone());
                std::thread::spawn(move || loop {
                    let mut rd = BufReader::new(c.try_clone().unwrap());
                    let mut lines = Vec::new();
                    loop {
                        let mut l = String::new();
                        if rd.read_line(&mut l).unwrap_or(0) == 0 {
                            return;
                        }
                        if l == "\r\n" {
                            break;
                        }
                        lines.push(l.to_ascii_lowercase());
                    }
                    let signed = lines.iter().any(|l| l.starts_with("x-vidkit: yes"));
                    if !signed {
                        let _ = c.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n");
                        continue;
                    }
                    let start = lines.iter().find_map(|l| l.strip_prefix("range: bytes=")).and_then(|r| r.split('-').next()?.trim().parse::<usize>().ok());
                    let from = start.unwrap_or(0).min(body.len());
                    if from > 0 {
                        ranged.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    }
                    let head = if start.is_some() {
                        format!("HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nAccept-Ranges: bytes\r\n\r\n", body.len() - from, from, body.len() - 1, body.len())
                    } else {
                        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\n\r\n", body.len())
                    };
                    if c.write_all(head.as_bytes()).and_then(|_| c.write_all(&body[from..])).is_err() {
                        return;
                    }
                    let _ = rd.read(&mut [0u8; 0]);
                });
            }
        });
        let url = format!("http://127.0.0.1:{port}/s.webm?sig=secret");
        let p = decode::probe(&f).unwrap();
        let headers = vec![("X-Vidkit".to_string(), "yes".to_string())];
        assert!(open_input(&url, &[], &p, Duration::ZERO, &Accel::Software).is_none(), "403 without the header");
        let mut v = open_input(&url, &headers, &p, Duration::from_millis(1500), &Accel::Software).expect("opens with the header");
        let mut got = Vec::new();
        let mut first = None;
        while let Some(fr) = v.next_frame() {
            first.get_or_insert(fr.pts);
            let vidkit_core::frame::FrameData::Cpu(pl) = fr.data else { panic!() };
            for d in pl.data {
                got.extend(d);
            }
        }
        let want = std::process::Command::new("ffmpeg").args(["-v", "error", "-ss", "1.5", "-i"]).arg(&f).args(["-f", "rawvideo", "-pix_fmt", "yuv420p", "-"]).output().unwrap().stdout;
        let t = first.unwrap().as_secs_f64();
        assert!((1.5..1.54).contains(&t), "first frame at or just after 1.5 s (25 fps: 1.52): {t}");
        assert_eq!(got.len(), want.len(), "frame count after the seek");
        assert!(got == want, "stream pictures differ from the file's");
        assert!(ranged.load(std::sync::atomic::Ordering::SeqCst) > 0, "the seek was a range request, not a read from the start");
    }

    #[test]
    fn threads_follow_how_busy_the_decoder_was() {
        assert_eq!(recommend_threads(12, 1.0, 10.0, 12), 2, "12 threads busy 10%: 2 do it at 60%");
        assert_eq!(recommend_threads(2, 9.5, 10.0, 12), 4, "saturated: more");
        assert_eq!(recommend_threads(1, 0.5, 10.0, 12), 1, "never below one");
        assert_eq!(recommend_threads(4, 1.0, 3.0, 12), 4, "too little video to judge");
        assert_eq!(recommend_threads(2, 9.9, 10.0, 2), 2, "never past the cores");
        assert_eq!(stream_key("vp9", 2160, 59.94), "vp9:2160p:60fps");
        assert_eq!(stream_key("av1", 1080, 30.0), "av1:1080p:30fps");
    }
}
