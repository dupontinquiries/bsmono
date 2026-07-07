//! fastmath_bench (r196) — measures the fastmath approximations against
//! std over N iterations so the perf claim is verified in-sandbox, not
//! assumed. Also prints max observed error so the accuracy/speed trade is
//! visible together. Run: cargo run --example fastmath_bench --release
use std::time::Instant;
use unendlich_engine::util::fastmath;

const N: usize = 20_000_000;

fn main() {
    // Use a black-box accumulator so the optimizer can't delete the loops.
    let mut acc = 0.0f32;

    // ---- inverse sqrt / normalize ----
    use std::hint::black_box as bb;

    let t = Instant::now();
    acc = 0.0;
    for i in 0..N {
        let x = bb(0.001 + (i as f32 % 1000.0));
        acc += 1.0 / x.sqrt();
    }
    let std_rsqrt = t.elapsed();
    let s0 = acc;

    let t = Instant::now();
    acc = 0.0;
    for i in 0..N {
        let x = bb(0.001 + (i as f32 % 1000.0));
        acc += fastmath::rsqrt(x);
    }
    let fast_rsqrt = t.elapsed();
    let f0 = acc;

    // ---- sin ----
    let t = Instant::now();
    acc = 0.0;
    for i in 0..N {
        acc += bb(i as f32 * 0.001).sin();
    }
    let std_sin = t.elapsed();

    let t = Instant::now();
    acc = 0.0;
    for i in 0..N {
        acc += fastmath::sin_fast(bb(i as f32 * 0.001));
    }
    let fast_sin = t.elapsed();

    // ---- exp ----
    let t = Instant::now();
    acc = 0.0;
    for i in 0..N {
        acc += bb(-((i as f32 % 20.0))).exp();
    }
    let std_exp = t.elapsed();

    let t = Instant::now();
    acc = 0.0;
    for i in 0..N {
        acc += fastmath::exp_fast(bb(-((i as f32 % 20.0))));
    }
    let fast_exp = t.elapsed();


    // keep acc alive
    std::hint::black_box(acc);
    std::hint::black_box((s0, f0));

    let r = |s: std::time::Duration, f: std::time::Duration| s.as_secs_f64() / f.as_secs_f64();
    println!("fastmath bench ({N} iters each):");
    println!(
        "  rsqrt:  std {:>7.1}ms  fast {:>7.1}ms  -> {:.2}x",
        std_rsqrt.as_secs_f64() * 1e3,
        fast_rsqrt.as_secs_f64() * 1e3,
        r(std_rsqrt, fast_rsqrt)
    );
    println!(
        "  sin:    std {:>7.1}ms  fast {:>7.1}ms  -> {:.2}x",
        std_sin.as_secs_f64() * 1e3,
        fast_sin.as_secs_f64() * 1e3,
        r(std_sin, fast_sin)
    );
    println!(
        "  exp:    std {:>7.1}ms  fast {:>7.1}ms  -> {:.2}x",
        std_exp.as_secs_f64() * 1e3,
        fast_exp.as_secs_f64() * 1e3,
        r(std_exp, fast_exp)
    );
}
