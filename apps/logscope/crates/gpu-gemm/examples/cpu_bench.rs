//! Standalone CPU measurement — creates no Gpu, needs no device.
//!
//! What the numbers mean on a 24-core M2 Ultra:
//!   single-core GiB/s   is the inner loop vectorizing? ~4 GiB/s here on x86 with no NEON; if the M2
//!                       single-core figure is similar or better the loop shape is fine, and if it is
//!                       far lower the aarch64 codegen needs looking at.
//!   scaling to 24       does the contiguous row split work? This container reports
//!                       available_parallelism = 1, so every threading number here is meaningless.
fn main() {
    use gpu_kernels::cpumatvec::*;
    let (rows, k) = (5120usize, 5120usize);
    let row_bytes = (k / 32) * 34;
    let raw: Vec<u8> = (0..rows * row_bytes).map(|i| (i % 251) as u8).collect();
    let act: Vec<f32> = (0..k).map(|i| ((i % 17) as f32) - 8.0).collect();
    let bytes = (rows * row_bytes) as u64;
    println!("matvec {rows}x{k} Q8_0 = {:.1} MiB", bytes as f64 / (1 << 20) as f64);
    let avail = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let prep = ActPrep::new(&act);
    for t in [1usize, 2, 4, avail] {
        let pool = gpu_kernels::cpupool::Pool::new(t);
        let _ = matvec(&pool, Quant::Q8_0, &raw, rows, k, &prep);
        let n = 5;
        let t0 = std::time::Instant::now();
        for _ in 0..n { let _ = matvec(&pool, Quant::Q8_0, &raw, rows, k, &prep); }
        let per = t0.elapsed().as_secs_f64() / n as f64;
        println!("  {t:3} threads: {:7.2} ms  {:7.1} GiB/s", per * 1e3,
                 bytes as f64 / per / (1u64 << 30) as f64);
    }
    println!("  available_parallelism = {avail}");

    // Q4_K at a real Scout shape, which is what a model actually loads.
    let q4_row = Quant::Q4_K.row_bytes(k);
    let q4raw: Vec<u8> = (0..rows * q4_row).map(|i| (i % 251) as u8).collect();
    let pool = gpu_kernels::cpupool::Pool::new(avail);
    let _ = matvec(&pool, Quant::Q4_K, &q4raw, rows, k, &prep);
    let t0 = std::time::Instant::now();
    for _ in 0..5 { let _ = matvec(&pool, Quant::Q4_K, &q4raw, rows, k, &prep); }
    let per = t0.elapsed().as_secs_f64() / 5.0;
    println!("\nQ4_K {rows}x{k} = {:.1} MiB: {:7.2} ms  {:6.1} GiB/s",
             (rows * q4_row) as f64 / (1 << 20) as f64, per * 1e3,
             (rows * q4_row) as f64 / per / (1u64 << 30) as f64);
}
