fn main() {
    use logscope::diagreport::Report;
    let mut r = Report::new();
    for l in [
        "wgpu-engine: adapter = Apple M2 Ultra (G14D B1) [Vulkan]",
        "wgpu-engine: non-default levers set — LOGSCOPE_MOE_STATS=1",
        "wgpu-engine: loaded in 53.4s — d=5120 layers=48 heads=40/8 head_dim=128 kv ff=16384 vocab=202048",
        &format!("stop=[{}]", (200000..201135).map(|i| i.to_string()).collect::<Vec<_>>().join(", ")),
        "wgpu-engine: [scout] profile — 4256 submits (1064/tok), 193 polls | setup 7ms vs sync 384ms of 4212ms",
        "wgpu-engine: [scout] submit causes — no-frame 3527 (83%), passes 613 (14%)",
        "gate-moe-stream: live streaming decode is BIT-EXACT to resident",
        "worker: load failed: architecture 'glm-dsa' isn't supported",
    ] { r.absorb(l); }
    print!("{}", r.render());
}
