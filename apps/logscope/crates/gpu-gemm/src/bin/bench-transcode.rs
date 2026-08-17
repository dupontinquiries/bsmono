//! bench-transcode — Q4_0 → resident, serial vs parallel vs the f32 round trip it replaces.
//! The point of the comparison: a transcode that does a quarter of the work on ONE core can lose in
//! wall time to a round trip that does more work on sixteen. Doing less work is not the same as
//! taking less time, and only the clock settles it.
use gpu_kernels::*;
use std::time::Instant;
fn main() {
    let n: usize = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(4096);
    let k: usize = std::env::args().nth(2).and_then(|a| a.parse().ok()).unwrap_or(2048);
    let kb = k / 32;
    // Arbitrary but valid Q4_0 blocks.
    let mut raw = Vec::with_capacity(n * kb * 18);
    let mut r = 0x1234_5678u64;
    for _ in 0..(n * kb) {
        raw.extend_from_slice(&0x2C00u16.to_le_bytes());
        for _ in 0..16 { r ^= r << 13; r ^= r >> 7; r ^= r << 17; raw.push((r >> 24) as u8); }
    }
    let gpu = Gpu::new();
    let weights = n * k;
    let mb = (n * kb * 18) as f64 / 1e6;
    println!("n={n} k={k}: {:.1} M weights, {mb:.0} MB of Q4_0 source", weights as f64 / 1e6);

    let time = |label: &str, f: &dyn Fn()| {
        f();                                  // warm
        let t = Instant::now(); f(); let s = t.elapsed().as_secs_f64();
        println!("  {label:<34} {s:6.3}s  ({:.0} M weights/s)", weights as f64 / 1e6 / s);
        s
    };
    std::env::set_var("LOGSCOPE_LOAD_THREADS", "1");
    let serial = time("transcode, 1 thread", &|| { gpu.q4_0_transcode_host_bytes(&raw, n, k); });
    std::env::remove_var("LOGSCOPE_LOAD_THREADS");
    let par = time("transcode, all threads", &|| { gpu.q4_0_transcode_host_bytes(&raw, n, k); });
    // The path it replaced: dequantise to f32, then the parallel resident quantiser.
    let deq: Vec<f32> = {
        let mut v = vec![0f32; n * k];
        for b in 0..(n * kb) {
            let o = b * 18;
            let d = f32::from(half_of(u16::from_le_bytes([raw[o], raw[o + 1]])));
            for j in 0..16 {
                let byte = raw[o + 2 + j];
                v[b * 32 + j] = ((byte & 0xF) as i32 - 8) as f32 * d;
                v[b * 32 + j + 16] = ((byte >> 4) as i32 - 8) as f32 * d;
            }
        }
        v
    };
    let rt = time("f32 round trip (quantise only)", &|| { gpu.quantize_q4_host_bytes(&deq, n, k); });
    println!("  parallel is {:.1}x the serial transcode, and {:.1}x the round trip's quantise step",
             serial / par, rt / par);
    println!("  (the round trip ALSO pays the dequantise above, which the transcode never does)");
}
fn half_of(h: u16) -> f32 {
    let (s, e, m) = ((h >> 15) & 1, (h >> 10) & 0x1F, h & 0x3FF);
    let v = if e == 0 { (m as f32) * 2f32.powi(-24) }
            else { (1.0 + m as f32 / 1024.0) * 2f32.powi(e as i32 - 15) };
    if s == 1 { -v } else { v }
}
