fn main() {
    use gpu_kernels::cpumatvec::*;
    let (rows, k) = (5120usize, 5120usize);
    let row_bytes = (k / 32) * 34;
    let raw: Vec<u8> = (0..rows * row_bytes).map(|i| (i % 251) as u8).collect();
    let act: Vec<f32> = (0..k).map(|i| ((i % 17) as f32) - 8.0).collect();
    let bytes = (rows * row_bytes) as u64;
    println!("matvec {rows}x{k} Q8_0 = {:.1} MiB", bytes as f64 / (1 << 20) as f64);
    let avail = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    for t in [1usize, 2, 4, avail] {
        let _ = matvec_q8_0(&raw, rows, k, &act, t);
        let n = 5;
        let t0 = std::time::Instant::now();
        for _ in 0..n { let _ = matvec_q8_0(&raw, rows, k, &act, t); }
        let per = t0.elapsed().as_secs_f64() / n as f64;
        println!("  {t:3} threads: {:7.2} ms  {:7.1} GiB/s", per * 1e3,
                 bytes as f64 / per / (1u64 << 30) as f64);
    }
    println!("  available_parallelism = {avail}; threads_for says {}", threads_for(bytes, avail));


    // llama.cpp's approach: quantize the activation once, then int8 x int8 -> i32 with a single float
    // scale per block. The activation quantization amortizes over every row, which is why it is hoisted
    // out of the loop here exactly as llama.cpp hoists it.
    println!("\ninteger dot (llama.cpp style), 1 thread:");
    let n = 5;
    let t0 = std::time::Instant::now();
    for _ in 0..n {
        let mut out = vec![0f32; rows];
        for r in 0..rows { out[r] = dot_q8_0(&raw[r * row_bytes..(r + 1) * row_bytes], &act); }
        std::hint::black_box(out);
    }
    let f32_ms = t0.elapsed().as_secs_f64() / n as f64;

    let t1 = std::time::Instant::now();
    for _ in 0..n {
        let aq = quantize_act(&act);                 // once per matvec, not per row
        let mut out = vec![0f32; rows];
        for r in 0..rows { out[r] = dot_q8_0_iq(&raw[r * row_bytes..(r + 1) * row_bytes], &aq); }
        std::hint::black_box(out);
    }
    let iq_ms = t1.elapsed().as_secs_f64() / n as f64;

    println!("  float   : {:7.2} ms  {:6.1} GiB/s", f32_ms * 1e3, bytes as f64 / f32_ms / (1u64 << 30) as f64);
    println!("  integer : {:7.2} ms  {:6.1} GiB/s  ({:+.1}%)", iq_ms * 1e3,
             bytes as f64 / iq_ms / (1u64 << 30) as f64, (f32_ms / iq_ms - 1.0) * 100.0);

    // And the error it costs, on this data.
    let aq = quantize_act(&act);
    let (mut worst, mut scale) = (0f32, 0f32);
    for r in 0..rows.min(256) {
        let f = dot_q8_0(&raw[r * row_bytes..(r + 1) * row_bytes], &act);
        let i = dot_q8_0_iq(&raw[r * row_bytes..(r + 1) * row_bytes], &aq);
        worst = worst.max((f - i).abs());
        scale = scale.max(f.abs());
    }
    println!("  worst relative error over 256 rows: {:.3e}", worst / scale.max(1e-6));
}
