//! yield_model.rs — hypotheses per GPU-second, batching against reaping.
//!
//! Two ways to get more hypotheses out of a round: widen the batch, or turn slots over faster.
//! They cost completely different things, and the arithmetic says which to spend on first.
//!
//! Run: `cargo run --example yield_model`

use gpu_kernels::reap::slot_yield;

/// Llama 3.1 8B, int8-resident projections: 8e9 params at 1 byte + a per-32 f32 scale.
const W_GIB: f64 = 8.0e9 * (1.0 + 4.0 / 32.0) / (1024.0 * 1024.0 * 1024.0);
/// int8 KV: 2 (K,V) x 32 layers x kv_dim 1024 = 64 KiB/token.
const KV_KIB_PER_TOKEN: f64 = 64.0;
/// M2 Ultra spec figure. The engine's measured fraction of roofline on a dense 8B is ~9%.
const BW_GIB_S: f64 = 800.0 * 1e9 / (1024.0 * 1024.0 * 1024.0);
const ACHIEVED: f64 = 0.09;

fn bytes_per_token_gib(m: usize, ctx: usize) -> f64 {
    W_GIB / m as f64 + (ctx as f64 * KV_KIB_PER_TOKEN) / (1024.0 * 1024.0)
}

/// Batch width past which the marginal branch pays full attention bandwidth for 1/m of a weight
/// pass: m* = W / (C * k).
fn m_star(ctx: usize) -> f64 {
    (W_GIB * 1024.0 * 1024.0) / (ctx as f64 * KV_KIB_PER_TOKEN)
}

fn main() {
    let budget: f64 = std::env::var("BUDGET").ok().and_then(|s| s.parse().ok()).unwrap_or(400.0);
    let reaped: f32 = std::env::var("REAPED").ok().and_then(|s| s.parse().ok()).unwrap_or(0.9);
    let at: f32 = std::env::var("AT").ok().and_then(|s| s.parse().ok()).unwrap_or(0.2);

    println!("Llama 3.1 8B int8-resident, int8 KV, {budget:.0}-token hypotheses");
    println!("reaper: {:.0}% of branches killed at {:.0}% of budget -> slot yield {:.2}x\n",
             reaped * 100.0, at * 100.0, slot_yield(reaped, at));
    println!("{:>6} {:>5} {:>8} {:>10} {:>10} {:>10} {:>8}",
             "ctx", "m", "m*", "GiB/tok", "hyp/s", "hyp/s+reap", "KV GiB");
    println!("{}", "-".repeat(64));

    for ctx in [2048usize, 4096, 8192] {
        for m in [1usize, 8, 16, 32] {
            let bpt = bytes_per_token_gib(m, ctx);
            let tok_s = BW_GIB_S * ACHIEVED / bpt;
            let hyp_s = tok_s / budget;
            let kv = m as f64 * ctx as f64 * KV_KIB_PER_TOKEN / (1024.0 * 1024.0);
            println!("{:>6} {:>5} {:>8.1} {:>10.3} {:>10.3} {:>10.3} {:>8.1}",
                     ctx, m, m_star(ctx), bpt, hyp_s, hyp_s * slot_yield(reaped, at) as f64, kv);
        }
        println!();
    }

    println!("the comparison that matters, at ctx=4096:");
    let base = BW_GIB_S * ACHIEVED / bytes_per_token_gib(16, 4096) / budget;
    let wider = BW_GIB_S * ACHIEVED / bytes_per_token_gib(32, 4096) / budget;
    let reap16 = base * slot_yield(reaped, at) as f64;
    println!("  m=16, no reaping        : {base:.3} hyp/s   KV {:.1} GiB", 16.0 * 4096.0 * 64.0 / 1048576.0);
    println!("  m=32, no reaping        : {wider:.3} hyp/s   KV {:.1} GiB   ({:.2}x for 2x the KV)",
             32.0 * 4096.0 * 64.0 / 1048576.0, wider / base);
    println!("  m=16, reaping           : {reap16:.3} hyp/s   KV {:.1} GiB   ({:.2}x for no extra KV)",
             16.0 * 4096.0 * 64.0 / 1048576.0, reap16 / base);
    println!("  m=32, reaping           : {:.3} hyp/s   KV {:.1} GiB   ({:.2}x)",
             wider * slot_yield(reaped, at) as f64, 32.0 * 4096.0 * 64.0 / 1048576.0,
             wider * slot_yield(reaped, at) as f64 / base);
}
