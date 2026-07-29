fn main() {
    // Is the gap a LANGUAGE difference or a LOOP SHAPE difference? Three versions of the same dot,
    // same arithmetic, differing only in how the compiler is allowed to see it.
    let nblk = 160usize;                                  // 5120 weights
    let raw: Vec<u8> = (0..nblk * 34).map(|i| (i % 251) as u8).collect();
    let act: Vec<f32> = (0..nblk * 32).map(|i| ((i % 17) as f32) - 8.0).collect();
    let aq = gpu_kernels::cpumatvec::quantize_act(&act);
    let reps = 20000;

    // A: what we ship — indexed with a modulo accumulator.
    let t = std::time::Instant::now();
    for _ in 0..reps { std::hint::black_box(gpu_kernels::cpumatvec::dot_q8_0_iq(&raw, &aq)); }
    let a = t.elapsed().as_secs_f64() / reps as f64;

    // B: slice once per block so the length is proven, and accumulate over chunks_exact so the
    // compiler sees a fixed-width reduction instead of a modulo index.
    let t = std::time::Instant::now();
    for _ in 0..reps {
        let mut total = 0f32;
        for (b, (da, qa)) in aq.iter().enumerate() {
            let blk = &raw[b * 34..b * 34 + 34];
            let dw = f32::from_bits(((blk[1] as u32) << 24) | 0x3f00_0000);   // stand-in, same cost
            let mut sum = 0i32;
            for (w, q) in blk[2..34].iter().zip(qa.iter()) {
                sum += (*w as i8 as i32) * (*q as i32);
            }
            total += dw * da * sum as f32;
        }
        std::hint::black_box(total);
    }
    let b = t.elapsed().as_secs_f64() / reps as f64;

    // C: as B, but with the quants as a fixed-size array so the trip count is a constant.
    let t = std::time::Instant::now();
    for _ in 0..reps {
        let mut total = 0f32;
        for (bi, (da, qa)) in aq.iter().enumerate() {
            let blk = &raw[bi * 34..bi * 34 + 34];
            let w: &[u8; 32] = blk[2..34].try_into().unwrap();
            let dw = f32::from_bits(((blk[1] as u32) << 24) | 0x3f00_0000);
            let mut sum = 0i32;
            for i in 0..32 { sum += (w[i] as i8 as i32) * (qa[i] as i32); }
            total += dw * da * sum as f32;
        }
        std::hint::black_box(total);
    }
    let c = t.elapsed().as_secs_f64() / reps as f64;

    println!("dot over {nblk} blocks ({} weights):", nblk * 32);
    println!("  A shipped (modulo acc, indexed)  : {:8.2} us", a * 1e6);
    println!("  B sliced + zip                   : {:8.2} us  ({:+.0}%)", b * 1e6, (a / b - 1.0) * 100.0);
    println!("  C sliced + fixed-size array      : {:8.2} us  ({:+.0}%)", c * 1e6, (a / c - 1.0) * 100.0);
}
