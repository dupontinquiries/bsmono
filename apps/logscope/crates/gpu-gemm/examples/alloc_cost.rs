fn main() {
    use gpu_kernels::cpumatvec::*;
    // matvec returns a fresh Vec every call. A decode does ~336 matvecs per token, so if allocation is
    // material a reusable scratch buffer is worth having — and if it is not, adding one is the arena
    // mistake from _227/_231 repeated on a different processor.
    let (rows, k) = (5120usize, 5120usize);
    let raw: Vec<u8> = (0..rows * Quant::Q8_0.row_bytes(k)).map(|i| (i % 251) as u8).collect();
    let act: Vec<f32> = (0..k).map(|i| ((i % 17) as f32) - 8.0).collect();
    let prep = ActPrep::new(&act);
    let pool = gpu_kernels::cpupool::Pool::new(1);
    let n = 20;

    let t0 = std::time::Instant::now();
    for _ in 0..n { std::hint::black_box(matvec(&pool, Quant::Q8_0, &raw, rows, k, &prep)); }
    let with_alloc = t0.elapsed().as_secs_f64() / n as f64;

    // Isolate the allocation alone: same size, no work.
    let t1 = std::time::Instant::now();
    for _ in 0..n { std::hint::black_box(vec![0f32; rows]); }
    let alloc_only = t1.elapsed().as_secs_f64() / n as f64;

    println!("matvec {rows} rows:");
    println!("  full call        : {:8.3} ms", with_alloc * 1e3);
    println!("  the Vec alone    : {:8.3} ms  ({:.2}% of the call)",
             alloc_only * 1e3, alloc_only / with_alloc * 100.0);
    println!("  at 336 calls/token: {:6.2} ms per token spent allocating", alloc_only * 336.0 * 1e3);
}
