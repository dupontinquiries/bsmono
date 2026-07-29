fn main() {
    // What does thread::scope cost per call? matvec_q8_0 spawns fresh threads every invocation, and a
    // decode does ~7 matvecs x 48 layers = 336 of them per token.
    let n = 200;
    for threads in [2usize, 4, 8] {
        let t0 = std::time::Instant::now();
        for _ in 0..n {
            std::thread::scope(|sc| {
                for _ in 0..threads { sc.spawn(|| std::hint::black_box(1u32)); }
            });
        }
        let per = t0.elapsed().as_secs_f64() / n as f64;
        println!("  scope with {threads:2} threads: {:7.1} us/call → {:6.1} ms per token at 336 calls",
                 per * 1e6, per * 336.0 * 1e3);
    }

    println!("\npersistent pool (llama.cpp's approach):");
    for threads in [2usize, 4, 8] {
        let p = gpu_kernels::cpupool::Pool::new(threads);
        for _ in 0..50 { p.run(|_, _| { std::hint::black_box(1u32); }); }   // warm
        let t0 = std::time::Instant::now();
        for _ in 0..n { p.run(|_, _| { std::hint::black_box(1u32); }); }
        let per = t0.elapsed().as_secs_f64() / n as f64;
        println!("  pool  with {threads:2} threads: {:7.1} us/call → {:6.1} ms per token at 336 calls",
                 per * 1e6, per * 336.0 * 1e3);
    }
}
