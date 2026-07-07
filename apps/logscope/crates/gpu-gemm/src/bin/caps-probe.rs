//! caps-probe — measure what THIS GPU can actually do, so the first testing session answers the open
//! questions in minutes. No feature flags needed (uses the base engine + the oom primitive).
//!
//! Build:  cargo build -p gpu-gemm --bin caps-probe --release
//! Run:    ./caps-probe [path/to/model.gguf]
//!
//! Reports, in order:
//!   1. Adapter + backend + the limits that actually gate big models (max buffer size, storage binding).
//!   2. The EMPIRICAL allocation ceiling: allocate 1 GiB STORAGE buffers (touching each to force the alloc)
//!      inside an OutOfMemory scope until the GPU refuses — the real answer to "how much can I allocate
//!      before Failed-to-allocate-BO-VMA", which the 48 GiB default only guesses at. Prints the LOGSCOPE_
//!      MAX_GPU_GB you should set.
//!   3. With a .gguf arg: the file size and the zero-copy import decision for it at the measured budget.
//!
//! Safe to run: every allocation is wrapped so an OOM is caught, not crashed; buffers free on exit.

use gpu_kernels::memplan::{gib, recommend_gpu_budget};

fn main() {
    let arg = std::env::args().nth(1);

    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
    })).expect("no GPU adapter");
    let info = adapter.get_info();
    let limits = adapter.limits();
    println!("── adapter ──");
    println!("  {} [{:?}]  driver: {}", info.name, info.backend, info.driver_info);
    println!("  max_buffer_size            = {:.2} GiB", gib(limits.max_buffer_size));
    println!("  max_storage_buffer_binding = {:.2} GiB", gib(limits.max_storage_buffer_binding_size as u64));
    println!("  max_bind_groups            = {}", limits.max_bind_groups);

    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("caps-probe"),
        required_features: wgpu::Features::empty(),
        required_limits: limits.clone(),
        experimental_features: wgpu::ExperimentalFeatures::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    })).expect("no device");

    // ── 2. empirical allocation ceiling ──
    println!("\n── allocation ceiling (allocating 1 GiB storage buffers until the GPU refuses) ──");
    let step = 1u64 << 30; // 1 GiB per buffer (well under the 2 GiB per-buffer limit)
    let hard_cap = 512u64 << 30; // never try past 512 GiB even if the GPU keeps saying yes
    let mut held: Vec<wgpu::Buffer> = Vec::new();
    let mut total = 0u64;
    let probe_word = vec![0u8; 256]; // small touch to force the lazy allocation to materialize
    loop {
        if total + step > hard_cap { println!("  stopped at the {} GiB self-imposed cap (GPU still accepting)", gib(hard_cap)); break; }
        let attempt = gpu_kernels::oom::with_oom_scope(&device, || {
            let buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("probe"), size: step,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
            });
            queue.write_buffer(&buf, 0, &probe_word);       // touch it
            queue.write_buffer(&buf, step - 256, &probe_word); // and the far end
            queue.submit([]);
            device.poll(wgpu::PollType::wait_indefinitely()).ok(); // flush inside the scope so errors are caught
            buf
        });
        match attempt {
            Ok(buf) => { held.push(buf); total += step; if total % (8 << 30) == 0 { println!("  … {} GiB ok", gib(total)); } }
            Err(_) => { println!("  GPU refused past {} GiB — that's the ceiling.", gib(total)); break; }
        }
    }
    println!("  ceiling ≈ {:.0} GiB  ({} buffers of 1 GiB held)", gib(total), held.len());
    let budget = recommend_gpu_budget(total);
    println!("  → recommended: LOGSCOPE_MAX_GPU_GB={:.0}  (85% of the ceiling, leaving KV+staging headroom)", gib(budget));
    if total < (40u64 << 30) {
        println!("  NOTE: under ~40 GiB means even a 70B-Q4 (~39 GiB) is borderline — zero-copy import or the");
        println!("        llama.cpp backend is the path for the 70B, not resident upload.");
    }
    drop(held); // free everything

    // ── 3. per-model zero-copy decision ──
    if let Some(path) = arg {
        println!("\n── model: {path} ──");
        match std::fs::metadata(&path) {
            Ok(md) => {
                let file_bytes = md.len();
                println!("  file size = {:.2} GiB", gib(file_bytes));
                let ram = available_ram();
                let decision = gpu_kernels::zerocopy::should_import(file_bytes, budget, ram);
                println!("  RAM available ≈ {:.0} GiB", gib(ram));
                println!("  zero-copy decision at the measured budget: {decision:?}");
                match decision {
                    gpu_kernels::zerocopy::ImportDecision::Upload => println!("    → fits the GPU budget; the normal upload path handles it."),
                    gpu_kernels::zerocopy::ImportDecision::Import => println!("    → too big to upload but fits RAM: the zero-copy import path is the win (run zerocopy-probe)."),
                    gpu_kernels::zerocopy::ImportDecision::TooBig => println!("    → bigger than RAM: layer streaming from disk is the only option."),
                }
            }
            Err(e) => println!("  can't stat {path}: {e}"),
        }
    } else {
        println!("\n(pass a .gguf path to see the zero-copy decision for a specific model)");
    }
    println!("\ndone.");
}

/// Best-effort free RAM (Linux /proc/meminfo MemAvailable); 0 if unknown.
fn available_ram() -> u64 {
    std::fs::read_to_string("/proc/meminfo").ok().and_then(|s| {
        s.lines().find(|l| l.starts_with("MemAvailable:"))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|kb| kb.parse::<u64>().ok())
            .map(|kb| kb * 1024)
    }).unwrap_or(0)
}
