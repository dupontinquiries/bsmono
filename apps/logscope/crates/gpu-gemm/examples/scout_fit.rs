fn main() {
    use gpu_kernels::memplan::*;
    const GIB: f64 = (1u64 << 30) as f64;
    // Scout 17B-16E: 48 layers, d=5120, ff=8192, vocab 202048.
    let s = ModelShape { d: 5120, ffn: 8192, layers: 48, vocab: 202048,
                         n_heads: 40, n_kv_heads: 8, tied_embed: false };
    // The MoE decision is NOT plan_streaming — that shape has no expert fields, so it costs Scout as
    // if it were dense. Experts are decided separately, which is why this check was missing for so long.
    println!("Scout 17B-16E: does the WHOLE model fit, experts included?");
    let per_expert_all_layers = (48u64 * (3 * 5120 * 8192) as u64) / 2;   // ~q4, gate+up+down
    let dense_only = (8.0 * GIB) as u64;
    for (label, gib_b) in [("old constant", 24.0f64), ("raised in _340", 48.0), ("measured", 63.0)] {
        let budget = (gib_b * GIB) as u64;
        let full = dense_only + 16 * per_expert_all_layers;
        println!("  {label:16} ({gib_b:.0} GiB): full resident needs {:.1} GiB → {}",
            full as f64 / GIB,
            if full <= budget { "RESIDENT, no streaming" } else { "streams experts" });
    }
    println!();
    println!("plan_streaming (dense-only view, for contrast):");
    for (label, gib) in [("old constant", 24.0f64), ("raised in _340", 48.0), ("measured", 63.0)] {
        let budget = (gib * GIB) as u64;
        match plan_streaming(&s, budget, Residency::Q4, 8192, 2, (2.0 * GIB) as u64) {
            None => println!("  {label:16} ({gib:.0} GiB): NO PLAN — refuses"),
            Some(p) => println!("  {label:16} ({gib:.0} GiB): pinned {}/{}, ring {} → {}",
                p.pinned_layers, p.total_layers, p.ring_slots,
                if p.ring_slots == 0 { "FULLY RESIDENT" } else { "streams" }),
        }
    }
}
