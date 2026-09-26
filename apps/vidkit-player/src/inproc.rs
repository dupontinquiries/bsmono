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
}
