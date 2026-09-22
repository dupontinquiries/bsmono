//! What the frame pipeline actually costs on real footage.
//!
//!     cargo run --release -p studio-video --example pipe_bench -- CLIP [SECONDS]
//!
//! Prints frames per second for: grey decode (measurement), decode+encode
//! passthrough, the motion search, and a stabilize render. Real media, real
//! ffmpeg — the synthetic tests say nothing about throughput.
use std::path::PathBuf;
use studio_video::framepipe::{Pipe, Size};
use studio_video::stabilize::{measure, solve, Budget, Flavor};

fn main() {
    let mut a = std::env::args().skip(1);
    let input: PathBuf = a.next().expect("usage: pipe_bench CLIP [SECONDS]").into();
    let secs: f64 = a.next().map(|s| s.parse().unwrap()).unwrap_or(10.0);
    let (w, h) = studio_video::probe(&input).map(|p| (p.width, p.height)).unwrap_or((1920, 1080));
    let size = Size::analysis(w, h);
    let fps = 24.0;
    let pipe = Pipe { input: input.clone(), range: Some((0.0, secs)), size, fps };
    println!("{}: {w}x{h} source, analysis {}x{}, {secs}s at {fps} fps",
             input.display(), size.w, size.h);

    let t = std::time::Instant::now();
    let grey = pipe.gray().expect("decode");
    let n = grey.len();
    println!("grey decode        {n:4} frames  {:6.2}s  {:7.1} fps", t.elapsed().as_secs_f64(), n as f64 / t.elapsed().as_secs_f64());

    let t = std::time::Instant::now();
    let path = measure(&grey, (size.w / 20).max(8));
    println!("motion search      {n:4} frames  {:6.2}s  {:7.1} fps", t.elapsed().as_secs_f64(), n as f64 / t.elapsed().as_secs_f64());

    let out = std::env::temp_dir().join("pipe_bench_pass.mp4");
    let t = std::time::Instant::now();
    let k = pipe.map(&out, 20, |_, f| f).expect("passthrough");
    println!("decode+encode      {k:4} frames  {:6.2}s  {:7.1} fps", t.elapsed().as_secs_f64(), k as f64 / t.elapsed().as_secs_f64());

    let sol = solve(&path, &Flavor::Smooth { sigma: 12.0 }, Budget { max_zoom: 1.1 }, size.w, size.h);
    let t = std::time::Instant::now();
    let k = pipe.map(&out, 20, |i, f| {
        let c = sol.corrections[i.min(sol.corrections.len() - 1)];
        f.warp(c.dx, c.dy, sol.zoom)
    }).expect("stabilize render");
    println!("stabilize render   {k:4} frames  {:6.2}s  {:7.1} fps  (zoom {:.3}, clamped {:.1}px)",
             t.elapsed().as_secs_f64(), k as f64 / t.elapsed().as_secs_f64(), sol.zoom, sol.clamped_px);
    let t = std::time::Instant::now();
    let k = pipe.map_window(&out, 20, 3, |i, w| {
        let here = i.min(3).min(w.len() - 1);
        studio_video::cleanplate::clean(w, here, &[], &Default::default()).frame
    }).expect("clean plate");
    println!("clean plate r=3    {k:4} frames  {:6.2}s  {:7.1} fps", t.elapsed().as_secs_f64(), k as f64 / t.elapsed().as_secs_f64());
    let _ = std::fs::remove_file(&out);
}
