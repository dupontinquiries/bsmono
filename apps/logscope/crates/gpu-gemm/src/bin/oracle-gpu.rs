//! oracle-gpu — the gpu-gemm side of the cross-engine forward oracle.
//! Same fixed ids as tools/oracle/oracle-ref.c (1 + (i*7) % (vocab-16)), f32-resident
//! weights, causal teacher-forced, prints "pos argmax margin" per line.
use gpu_kernels::*;
fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "/tmp/tiny-qwen2.gguf".into());
    let k: usize = std::env::args().nth(2).and_then(|a| a.parse().ok()).unwrap_or(48);
    let bytes = std::fs::read(&path).expect("read gguf");
    let gpu = Gpu::new();
    let vocab: usize = std::env::args().nth(3).and_then(|a| a.parse().ok()).unwrap_or(256);
    let model = gpu.load_gguf_opt(&bytes, false).expect("load gguf f32");
    let ids: Vec<u32> = (0..k as u32).map(|i| 1 + (i * 7) % (vocab as u32 - 16)).collect();
    let stats = gpu.causal_argmax_stats(&model, &ids);
    for i in 0..k {
        let s = &stats[i * 4..i * 4 + 4];
        println!("{i} {} {:.6}", s[0] as u32, s[2]);
    }
}
