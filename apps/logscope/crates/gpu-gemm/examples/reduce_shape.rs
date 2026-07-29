fn main() {
    // Rust preserves IEEE order, so `iter().sum()` on floats cannot vectorize — LLVM may not
    // reassociate. C++ with -ffast-math may, and llama.cpp is built that way. Manual multi-accumulator
    // is how you grant the permission explicitly, choosing the order yourself.
    let n = 4096usize;
    let a: Vec<f32> = (0..n).map(|i| (i % 17) as f32 * 0.1 - 0.8).collect();
    let b: Vec<f32> = (0..n).map(|i| (i % 23) as f32 * 0.1 - 1.1).collect();
    let reps = 20000;

    let t = std::time::Instant::now();
    for _ in 0..reps {
        let s: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
        std::hint::black_box(s);
    }
    let scalar = t.elapsed().as_secs_f64() / reps as f64;

    let t = std::time::Instant::now();
    for _ in 0..reps {
        let mut acc = [0f32; 8];
        for (ca, cb) in a.chunks_exact(8).zip(b.chunks_exact(8)) {
            for l in 0..8 { acc[l] += ca[l] * cb[l]; }
        }
        std::hint::black_box(acc.iter().sum::<f32>());
    }
    let lanes = t.elapsed().as_secs_f64() / reps as f64;

    println!("dot of {n} f32, 8-lane vs scalar reduction:");
    println!("  iter().sum()        : {:8.2} us", scalar * 1e6);
    println!("  8 accumulators      : {:8.2} us  ({:+.0}%)", lanes * 1e6, (scalar / lanes - 1.0) * 100.0);

    // The real functions, before/after shape.
    let hd = 128usize;
    let q: Vec<f32> = (0..hd).map(|i| (i % 13) as f32 * 0.2 - 1.0).collect();
    let kc: Vec<f32> = (0..hd * 512).map(|i| (i % 19) as f32 * 0.1 - 0.9).collect();
    let vc: Vec<f32> = (0..hd * 512).map(|i| (i % 7) as f32 * 0.3 - 1.0).collect();
    let mut out = vec![0f32; hd];
    let reps2 = 2000;
    let t = std::time::Instant::now();
    for _ in 0..reps2 {
        gpu_kernels::cpuattn::attend(&q, &kc, &vc, 512, hd, 0.088, &mut out);
        std::hint::black_box(&out);
    }
    let per = t.elapsed().as_secs_f64() / reps2 as f64;
    println!();
    println!("attend over 512 positions, head_dim 128: {:7.2} us/call", per * 1e6);
    println!("  at 32 heads x 32 layers that is {:6.1} ms/token", per * 32.0 * 32.0 * 1e3);
}
