//! bench-window — price the output-projection row window on a REAL-WIDTH model. The diffusion loop
//! reads one block's rows; before the window, the projection ran over the whole canvas.
use gpu_kernels::*;
use std::time::Instant;
fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "/tmp/tiny-llada2-wide.gguf".into());
    let t: usize = std::env::args().nth(2).and_then(|a| a.parse().ok()).unwrap_or(512);
    let blk: usize = std::env::args().nth(3).and_then(|a| a.parse().ok()).unwrap_or(32);
    let bytes = std::fs::read(&path).expect("read");
    let gpu = Gpu::new();
    let m = gpu.load_gguf_opt(&bytes, false).expect("load");
    let toks: Vec<u32> = (0..t as u32).map(|i| 1 + (i * 7) % 200).collect();
    let _ = gpu.denoise_stats_rows(&m, &toks, t - blk, blk);   // warm
    let a = Instant::now(); let _ = gpu.denoise_stats(&m, &toks); let full = a.elapsed();
    let a = Instant::now(); let _ = gpu.denoise_stats_rows(&m, &toks, t - blk, blk); let win = a.elapsed();
    println!("t={t} d={} vocab={} block={blk}: whole-canvas {:.3}s | windowed {:.3}s | {:.1}x",
        m.d, m.vocab, full.as_secs_f64(), win.as_secs_f64(), full.as_secs_f64() / win.as_secs_f64().max(1e-9));
}
