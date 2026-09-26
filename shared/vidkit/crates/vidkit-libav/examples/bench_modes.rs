//! CPU cost of decoding under power levers: decoder threads, and skipping
//! non-reference frames. In process, the player's own decoder.
//!
//!   cargo run --release --example bench_modes -- FILE [SECONDS]
use std::time::Instant;

fn cpu() -> f64 {
    let mut r: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut r) };
    let t = |v: libc::timeval| v.tv_sec as f64 + v.tv_usec as f64 / 1e6;
    t(r.ru_utime) + t(r.ru_stime)
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let path = &a[1];
    let secs: f64 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(5.0);
    let threads: Vec<u32> = std::env::var("THREADS").ok().map(|t| t.split(',').filter_map(|x| x.parse().ok()).collect()).unwrap_or(vec![1, 2]);
    for &th in &threads {
        for skip in [false, true] {
            let (c0, t0) = (cpu(), Instant::now());
            let mut d = vidkit_libav::Decoder::open(path, &vidkit_libav::Options { hw: vidkit_libav::Hw::Software, from: Default::default(), threads: th, export_grain: false, input_options: vec![], skip_nonref: skip, codec_options: std::env::var("CODEC_OPTS").ok().map(|o| o.split(',').filter_map(|kv| kv.split_once('=').map(|(k, v)| (k.to_string(), v.to_string()))).collect()).unwrap_or_default() }).unwrap();
            let mut pool: Vec<Vec<u8>> = Vec::new();
            let (mut n, mut last) = (0u32, 0.0f64);
            while let Some(f) = d.next_frame(&mut |len| pool.pop().map(|mut v| { v.clear(); v.reserve(len); v }).unwrap_or_else(|| Vec::with_capacity(len))).unwrap() {
                last = f.pts.as_secs_f64();
                if let vidkit_core::frame::FrameData::Cpu(p) = f.data {
                    pool.extend(p.data);
                }
                n += 1;
                if last >= secs {
                    break;
                }
            }
            let (c, w) = (cpu() - c0, t0.elapsed().as_secs_f64());
            println!("{} threads={th} skip_nonref={skip:5}: {n:4} frames out over {last:.2} s of video  CPU {c:6.2} s ({:.2} cores at real time)  wall {w:5.2} s  ({:.1}x real time)", d.codec, c / last.max(1e-3), last / w);
        }
    }
}
