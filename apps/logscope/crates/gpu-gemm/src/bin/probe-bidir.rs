//! probe-bidir — direct observation of denoise_stats zeroness by t. No gates, no envs.
use gpu_kernels::*;
fn main() {
    let path = std::env::args().nth(1).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let gpu = Gpu::new();
    let model = gpu.load_gguf_opt(&bytes, false).unwrap();
    for t in [8usize, 16, 24, 32, 40, 48, 64] {
        let toks: Vec<u32> = (0..t as u32).map(|i| 1 + (i * 7) % 240).collect();
        let s = gpu.denoise_stats(&model, &toks);
        let uniform = (s[1] - 1.0 / 256.0).abs() < 1e-9 && s[2] == 0.0;
        println!("t={t:>2} p={:.6} m={:.6} {}", s[1], s[2], if uniform { "UNIFORM-ZERO" } else { "ok" });
    }
}
