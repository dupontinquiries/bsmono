//! bench-dequant — CPU dequantization throughput per GGUF type, single thread.
//!
//! The load path turns every non-native tensor into f32 here before requantizing it (blockquant),
//! so on a big model this is minutes of wall time. One thread on purpose: `dequantize_par` splits by
//! rows, so per-core throughput is the number that scales. Build OPTIMIZED or the figures mean
//! nothing:
//!   cargo build --profile ailite --config profile.ailite.opt-level=2 --bin bench-dequant
use gpu_kernels::gguf::dequantize;
use std::time::Instant;
fn main() {
    let n: usize = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(16 << 20); // weights
    let mut s = 0x9E3779B97F4A7C15u64;
    let mut byte = || { s ^= s << 13; s ^= s >> 7; s ^= s << 17; (s >> 24) as u8 };
    // (ggml type, name, block weights, block bytes, byte offsets of the f16 scale fields to normalise)
    let types: [(u32, &str, usize, usize, &[usize]); 8] = [
        (1, "F16", 1, 2, &[0]), (8, "Q8_0", 32, 34, &[0]), (2, "Q4_0", 32, 18, &[0]),
        (10, "Q2_K", 256, 84, &[80, 82]), (11, "Q3_K", 256, 110, &[108]),
        (12, "Q4_K", 256, 144, &[0, 2]), (13, "Q5_K", 256, 176, &[0, 2]), (14, "Q6_K", 256, 210, &[208]),
    ];
    println!("{:<6} {:>12} {:>14}", "type", "M weights/s", "GB/s of f32 out");
    for (ty, name, bw, bb, scales) in types {
        let blocks = n / bw;
        let mut raw: Vec<u8> = (0..blocks * bb).map(|_| byte()).collect();
        // every f16 scale a NORMAL half, varied — constant scales flatter branchy decoders
        for (b, blk) in raw.chunks_mut(bb).enumerate() { for &o in scales { let h = 0x2800u16 + ((b * 37 + o) % 0x0800) as u16; blk[o] = h as u8; blk[o + 1] = (h >> 8) as u8; } }
        let run = || dequantize(ty, &raw, blocks * bw);
        let Ok(w) = run() else { println!("{name:<6} (not supported by dequantize)"); continue };
        let mut sink = 0f64; let reps = 3;
        let t = Instant::now();
        for _ in 0..reps { let v = run().unwrap(); sink += v[v.len() / 2] as f64; }
        let secs = t.elapsed().as_secs_f64() / reps as f64;
        println!("{name:<6} {:>12.0} {:>14.2}   (check {:.3e}, {} finite of {})", (blocks * bw) as f64 / 1e6 / secs, (blocks * bw * 4) as f64 / 1e9 / secs, sink, w.iter().filter(|x| x.is_finite()).count(), w.len());
    }
}
