//! CPU cost of getting decoded frames into this process: the ffmpeg child
//! writing raw planes to a pipe (what vidkit-player did) vs libavcodec in
//! process. Same decoder, same thread count choice (libavcodec's).
//!
//!   cargo run --release --example bench_decode -- FILE [FRAMES]
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::Instant;

fn cpu(who: i32) -> f64 {
    let mut r: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(who, &mut r) };
    let t = |v: libc::timeval| v.tv_sec as f64 + v.tv_usec as f64 / 1e6;
    t(r.ru_utime) + t(r.ru_stime)
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let path = a.get(1).expect("FILE");
    let n: usize = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(300);
    for round in 0..2 {
        // In process.
        let (c0, t0) = (cpu(libc::RUSAGE_SELF), Instant::now());
        let mut d = vidkit_libav::Decoder::open(path, &vidkit_libav::Options { hw: vidkit_libav::Hw::Software, from: Default::default(), threads: 0, export_grain: false, input_options: vec![] }).unwrap();
        let mut pool: Vec<Vec<u8>> = Vec::new();
        let mut k = 0;
        while k < n {
            let Some(f) = d.next_frame(&mut |len| pool.pop().map(|mut v| { v.clear(); v.reserve(len); v }).unwrap_or_else(|| Vec::with_capacity(len))).unwrap() else { break };
            if let vidkit_core::frame::FrameData::Cpu(p) = f.data {
                pool.extend(p.data); // recycled like the player's pool
            }
            k += 1;
        }
        let (w, h) = (d.width as usize, d.height as usize);
        drop(d);
        let inproc = (cpu(libc::RUSAGE_SELF) - c0, t0.elapsed().as_secs_f64(), k);

        // Child + pipe.
        let (s0, ch0, t0) = (cpu(libc::RUSAGE_SELF), cpu(libc::RUSAGE_CHILDREN), Instant::now());
        let mut c = Command::new("ffmpeg")
            .args(["-v", "error", "-nostdin", "-i", path, "-map", "0:v:0", "-frames:v", &n.to_string(), "-f", "rawvideo", "-pix_fmt", "yuv420p", "-"])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut out = c.stdout.take().unwrap();
        let mut buf = vec![0u8; w * h + 2 * w.div_ceil(2) * h.div_ceil(2)];
        let mut m = 0;
        while out.read_exact(&mut buf).is_ok() {
            m += 1;
        }
        c.wait().unwrap();
        let pipe = (cpu(libc::RUSAGE_SELF) - s0 + cpu(libc::RUSAGE_CHILDREN) - ch0, t0.elapsed().as_secs_f64(), m);
        println!(
            "round {round}: {w}x{h}  in-process {:.2} s CPU / {} frames ({:.1} ms/frame)   ffmpeg+pipe {:.2} s CPU / {} frames ({:.1} ms/frame)   -> {:+.0}%",
            inproc.0, inproc.2, inproc.0 * 1e3 / inproc.2 as f64, pipe.0, pipe.2, pipe.0 * 1e3 / pipe.2 as f64,
            (inproc.0 / inproc.2 as f64 / (pipe.0 / pipe.2 as f64) - 1.0) * 100.0
        );
    }
}
