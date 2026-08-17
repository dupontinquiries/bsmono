//! diffuse-dump — print the ids diffuse_generate produces, for comparison against
//! tools/oracle/diffuse_ref.py. Prompt matches the reference's default: 1 + (i*13)%200, 12 tokens.
use gpu_kernels::*;
fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "/tmp/tiny-llada2.gguf".into());
    let n_fill: usize = std::env::args().nth(2).and_then(|a| a.parse().ok()).unwrap_or(16);
    let steps: usize = std::env::args().nth(3).and_then(|a| a.parse().ok()).unwrap_or(8);
    let block: usize = std::env::args().nth(4).and_then(|a| a.parse().ok()).unwrap_or(8);
    let bytes = std::fs::read(&path).expect("read");
    let gpu = Gpu::new();
    let m = gpu.load_gguf_opt(&bytes, false).expect("load f32");
    let prompt: Vec<u32> = (0..12u32).map(|i| 1 + (i * 13) % 200).collect();
    let out = gpu.diffuse_generate(&m, &prompt, n_fill, steps, block, None).expect("generate");
    println!("{}", out.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(" "));
}
