fn main() {
    use logscope::cpuplacement::*;
    let active = (10.5 * (1u64 << 30) as f32) as u64;   // Scout: dense + 1 routed expert/layer
    let ceiling = bandwidth_ceiling_toks(active, 200.0);
    println!("Scout ceiling at 200 GiB/s: {ceiling:.1} tok/s\n");
    for (name, cpu, gpu, contended, fits) in [
        ("Scout today (measured 0.45)", 0.2, 0.45, false, true),
        ("Scout with a healthy GPU",    2.0, 18.0, false, true),
        ("3B liaison, GPU free",        25.0, 40.0, false, true),
        ("3B liaison, GPU busy",        25.0, 40.0, true,  true),
        ("70B on Asahi (24 GiB cap)",   1.0,  0.0, false, false),
    ] {
        let v = decide(&Measurement { cpu_toks: cpu, gpu_toks: gpu, gpu_ceiling_toks: ceiling,
                                      gpu_contended: contended, fits_gpu: fits });
        println!("{name:30} -> {}", v.explain());
    }
}
