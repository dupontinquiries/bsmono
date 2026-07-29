fn main() {
    use logscope::hybridsplit::*;
    const GIB: f64 = (1u64 << 30) as f64;
    // Scout: 48 layers, ~220 MiB of weight touched per layer per token (dense + one routed expert).
    // Bandwidths measured on the M2 Ultra: GPU 258.1 GiB/s (gate-perf), CPU 113.2 (24 threads).
    let mk = |budget_gib: f64, gpu: f32| Plan {
        n_layers: 48, bytes_per_layer: 220 << 20,
        gpu_budget_bytes: (budget_gib * GIB) as u64,
        gpu_gib_s: gpu, cpu_gib_s: 113.2,
        boundary_bytes: 5120 * 4, boundary_gib_s: 50.0,
    };
    println!("Scout, 48 layers x 220 MiB/token, CPU 113.2 GiB/s\n");
    for (label, budget, gpu) in [
        ("engine's old 24 GiB cap, healthy GPU", 24.0, 258.1f32),
        ("measured 63 GiB ceiling, healthy GPU", 63.0, 258.1),
        ("63 GiB, GPU as actually delivered",    63.0, 5.0),
        ("2.5 GiB (a tight budget)",              2.5, 258.1),
    ] {
        let p = mk(budget, gpu);
        let s = choose(&p);
        println!("{label}:\n  {}\n  -> {:.1} tok/s\n", explain(&p, &s), 1000.0 / s.ms_per_token);
    }
}
