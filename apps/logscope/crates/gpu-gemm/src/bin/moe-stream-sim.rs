//! moe-stream-sim — what expert streaming costs for a given shape, budget and routing skew.
//!
//! No GPU, no model file. It drives the engine's REAL policy (`expert_cache::LayerSlots`, the LFRU
//! ring + pin store the decode loop uses) and the REAL planner (`memplan::plan_moe_streaming`)
//! with synthetic routing, so the numbers are the code's behaviour and not a description of it.
//!
//! What is synthetic, and therefore what every number here is conditional on: the ROUTING. Real
//! routers are skewed, how skewed is per model and per workload, and nobody has measured
//! Ornith's. Experts are drawn Zipf(s) per layer with a per-layer permutation (layer 3's hot
//! experts are not layer 4's), without replacement within a token, plus an optional "stickiness"
//! — the chance a token reuses an expert the previous token used in that layer, which is what
//! the ring actually feeds on. Sweep `--zipf` and `--sticky`; if the conclusion changes across
//! the plausible range, it is not a conclusion.
//!
//! Defaults are Ornith-1.5-397B from its config.json (60 layers, 512 experts, top-10, expert ff
//! 1024, d 4096) at Q4_K gate/up + Q6_K down, against a 48 GiB GPU buffer budget — the order of
//! what Honeykrisp has been observed to hold, NOT the 192 GiB of RAM behind it.
//!
//!   cargo run --release --bin moe-stream-sim -- --budget-gib 48 --zipf 1.0 --sticky 0.3
//!   … --sweep        one table over ring share x zipf, instead of a single run

use gpu_kernels::expert_cache::LayerSlots;
use gpu_kernels::memplan::plan_moe_streaming;

struct Args {
    layers: usize, experts: u32, top_k: u32, d: u64, ff: u64,
    gu_bpw: f64, dn_bpw: f64, dense_gib: f64, budget_gib: f64,
    zipf: f64, sticky: f64, tokens: usize, warm: usize, ring_share: Option<f64>,
    fill_gib_s: f64, sweep: bool, seed: u64,
}

fn parse() -> Args {
    let mut a = Args { layers: 60, experts: 512, top_k: 10, d: 4096, ff: 1024,
        gu_bpw: 4.5, dn_bpw: 6.5625, dense_gib: 14.0, budget_gib: 48.0,
        zipf: 1.0, sticky: 0.3, tokens: 600, warm: 200, ring_share: None,
        fill_gib_s: 8.0, sweep: false, seed: 0x9E3779B97F4A7C15 };
    let v: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < v.len() {
        let flag = v[i].clone();
        if flag != "--sweep" { i += 1; }
        let arg = v.get(i).cloned();
        let num = |name: &str| -> f64 {
            arg.as_deref().and_then(|s| s.parse().ok()).unwrap_or_else(|| { eprintln!("{name} needs a number"); std::process::exit(2) })
        };
        match flag.as_str() {
            "--layers" => a.layers = num("--layers") as usize,
            "--experts" => a.experts = num("--experts") as u32,
            "--top-k" => a.top_k = num("--top-k") as u32,
            "--d" => a.d = num("--d") as u64,
            "--ff" => a.ff = num("--ff") as u64,
            "--gu-bpw" => a.gu_bpw = num("--gu-bpw"),
            "--dn-bpw" => a.dn_bpw = num("--dn-bpw"),
            "--dense-gib" => a.dense_gib = num("--dense-gib"),
            "--budget-gib" => a.budget_gib = num("--budget-gib"),
            "--zipf" => a.zipf = num("--zipf"),
            "--sticky" => a.sticky = num("--sticky"),
            "--tokens" => a.tokens = num("--tokens") as usize,
            "--warm" => a.warm = num("--warm") as usize,
            "--ring-share" => a.ring_share = Some(num("--ring-share")),
            "--fill-gib-s" => a.fill_gib_s = num("--fill-gib-s"),
            "--sweep" => a.sweep = true,
            other => { eprintln!("unknown flag {other}"); std::process::exit(2) }
        }
        i += 1;
    }
    a
}

fn rng(s: &mut u64) -> f64 {
    *s ^= *s << 13; *s ^= *s >> 7; *s ^= *s << 17;
    (*s >> 11) as f64 / (1u64 << 53) as f64
}

/// Per-layer router: Zipf over a layer-specific permutation, top_k distinct, with stickiness.
struct Router { cdf: Vec<f64>, perm: Vec<Vec<u32>>, prev: Vec<Vec<u32>> }
impl Router {
    fn new(a: &Args, s: &mut u64) -> Self {
        let w: Vec<f64> = (1..=a.experts).map(|r| 1.0 / (r as f64).powf(a.zipf)).collect();
        let tot: f64 = w.iter().sum();
        let mut acc = 0.0;
        let cdf = w.iter().map(|x| { acc += x / tot; acc }).collect();
        let perm = (0..a.layers).map(|_| {
            let mut p: Vec<u32> = (0..a.experts).collect();
            for i in (1..p.len()).rev() { let j = (rng(s) * (i + 1) as f64) as usize; p.swap(i, j.min(i)); }
            p
        }).collect();
        Router { cdf, perm, prev: vec![Vec::new(); a.layers] }
    }
    fn route(&mut self, layer: usize, k: u32, sticky: f64, s: &mut u64) -> Vec<u32> {
        let mut out: Vec<u32> = Vec::with_capacity(k as usize);
        let prev = std::mem::take(&mut self.prev[layer]);
        for &e in &prev { if out.len() < k as usize && rng(s) < sticky { out.push(e); } }
        while out.len() < k as usize {
            let u = rng(s);
            let rank = self.cdf.partition_point(|&c| c < u).min(self.cdf.len() - 1);
            let e = self.perm[layer][rank];
            if !out.contains(&e) { out.push(e); }
        }
        self.prev[layer] = out.clone();
        out
    }
}

struct Outcome { pin_hits: u64, ring_hits: u64, misses: u64 }
impl Outcome {
    fn total(&self) -> u64 { self.pin_hits + self.ring_hits + self.misses }
    fn miss_rate(&self) -> f64 { self.misses as f64 / self.total().max(1) as f64 }
}

/// Run the real policy. `pinned`/`ring` are PER LAYER. Pins are the true hottest experts of each
/// layer — i.e. a PERFECT pin choice, the best case for pinning; a real run has to learn it
/// (repin), so pin-heavy splits are flattered here, and the comparison should be read that way.
fn simulate(a: &Args, pinned: u32, ring: u32) -> Outcome {
    let mut s = a.seed;
    let mut router = Router::new(a, &mut s);
    let mut layers: Vec<LayerSlots> = (0..a.layers).map(|l| {
        let pins: Vec<u32> = router.perm[l][..pinned as usize].to_vec();
        LayerSlots::new(&pins, ring)
    }).collect();
    let mut o = Outcome { pin_hits: 0, ring_hits: 0, misses: 0 };
    for t in 0..a.warm + a.tokens {
        for l in 0..a.layers {
            let routed = router.route(l, a.top_k, a.sticky, &mut s);
            let plan = layers[l].resolve_token(&routed);
            if t < a.warm { continue; }
            for (_, acc) in &plan.resident {
                if matches!(acc, gpu_kernels::expert_cache::Access::Pinned) { o.pin_hits += 1 } else { o.ring_hits += 1 }
            }
            o.misses += plan.misses.len() as u64;
        }
    }
    o
}

fn main() {
    let a = parse();
    let gib = (1u64 << 30) as f64;
    // one expert in ONE layer, file format: gate + up at gu_bpw, down at dn_bpw
    let per_layer_expert = ((2 * a.d * a.ff) as f64 * a.gu_bpw / 8.0 + (a.d * a.ff) as f64 * a.dn_bpw / 8.0) as u64;
    // the planner's unit is one expert id across ALL layers
    let expert_all_layers = per_layer_expert * a.layers as u64;
    let all_experts_gib = expert_all_layers as f64 * a.experts as f64 / gib;
    println!("shape: {} layers x {} experts, top-{}, d {} ff {} — {:.2} MiB per expert per layer, {:.1} GiB of routed experts, {:.1} GiB dense (given)",
        a.layers, a.experts, a.top_k, a.d, a.ff, per_layer_expert as f64 / (1 << 20) as f64, all_experts_gib, a.dense_gib);
    let budget = (a.budget_gib * gib) as u64;
    let Some(plan) = plan_moe_streaming((a.dense_gib * gib) as u64, expert_all_layers, a.experts, a.top_k, budget) else {
        println!("plan_moe_streaming: None — dense + (top_k+1) experts do not fit {:.0} GiB; nothing to simulate", a.budget_gib);
        return;
    };
    let slots = plan.pinned_experts + plan.stream_slots;
    println!("plan_moe_streaming @ {:.0} GiB: {} resident per layer of {} ({:.0}%) = {} pinned + {} ring   [the planner's own split]",
        a.budget_gib, slots, a.experts, 100.0 * slots as f64 / a.experts as f64, plan.pinned_experts, plan.stream_slots);
    if plan.stream_slots == 0 { println!("everything fits resident; no streaming"); return; }

    let report = |label: &str, pinned: u32, ring: u32, a: &Args| {
        let o = simulate(a, pinned, ring);
        let miss_per_tok = o.misses as f64 / a.tokens as f64;
        let gib_per_tok = miss_per_tok * per_layer_expert as f64 / gib;
        println!("  {label:<28} pins {pinned:>4} ring {ring:>4} | miss {:>5.1}%  ({:>5.1} of {} activations/token) | {:>5.2} GiB filled/token | {:>6.0} ms/token at {:.0} GiB/s fill",
            100.0 * o.miss_rate(), miss_per_tok, a.layers as u32 * a.top_k, gib_per_tok, gib_per_tok / a.fill_gib_s * 1000.0, a.fill_gib_s);
        o.miss_rate()
    };

    if a.sweep {
        for z in [0.6, 0.8, 1.0, 1.2] {
            let mut b = Args { zipf: z, ..parse() };
            b.sweep = false;
            println!("zipf {z} sticky {}:", b.sticky);
            for share in [0.0, 0.25, 0.5, 1.0] {
                let ring = ((slots as f64 * share) as u32).max(plan.stream_slots).min(slots);
                report(&format!("ring share {:.0}%{}", share * 100.0, if share == 0.0 { " (planner)" } else { "" }), slots - ring, ring, &b);
            }
        }
        return;
    }
    println!("routing: zipf {} sticky {} — SYNTHETIC; {} warm + {} measured tokens", a.zipf, a.sticky, a.warm, a.tokens);
    report("planner split", plan.pinned_experts, plan.stream_slots, &a);
    if let Some(share) = a.ring_share {
        let ring = ((slots as f64 * share) as u32).max(plan.stream_slots).min(slots);
        report(&format!("ring share {:.0}%", share * 100.0), slots - ring, ring, &a);
    } else {
        for share in [0.25, 0.5, 1.0] {
            let ring = ((slots as f64 * share) as u32).max(plan.stream_slots).min(slots);
            report(&format!("ring share {:.0}%", share * 100.0), slots - ring, ring, &a);
        }
    }
    println!("pins here are the TRUE hottest experts per layer (a perfect pin choice), so pin-heavy rows are a best case.");
}
