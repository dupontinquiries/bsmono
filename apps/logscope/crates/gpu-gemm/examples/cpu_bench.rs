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
}
