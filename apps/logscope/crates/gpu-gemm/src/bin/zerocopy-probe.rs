//! zerocopy-probe — the on-hardware experiment for the UMA zero-copy import path.
//!
//! Build:   cargo build -p gpu-gemm --bin zerocopy-probe --features zerocopy --release
//! Run:     ./zerocopy-probe [path/to/model.gguf]
//!
//! Step 1 (no file): create a wgpu device with VK_EXT_external_memory_host enabled and PROBE — prints the
//! minImportedHostPointerAlignment. If it's 0, Honeykrisp doesn't support host-pointer import and the whole
//! zero-copy approach is off the table (→ pivot to layer streaming); nothing below will work.
//!
//! Step 2 (with a file): mmap it, import the whole thing in place as a GPU buffer (ZERO copy, zero upload),
//! then copy a chunk device→device to measure the GPU's read bandwidth over the imported host pages, and map
//! a small readback to prove the bytes the GPU sees match the file. This is the "does it work + how fast on
//! the 70B's actual weights file" answer.
//!
//! DRAFT UNTIL RUN: this compiles against wgpu-29 hal + ash-0.38 but has never touched a real Vulkan device.

fn main() {
    let path = std::env::args().nth(1);

    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
    })).expect("no GPU adapter");
    let info = adapter.get_info();
    println!("adapter: {} [{:?}]", info.name, info.backend);
    if info.backend != wgpu::Backend::Vulkan {
        eprintln!("not a Vulkan backend — import needs Vulkan (Honeykrisp). Aborting.");
        return;
    }

    // Check the extension is ADVERTISED before requesting it — on Honeykrisp/Mesa it isn't, and requesting
    // a missing extension panics wgpu-hal. This is the clean answer to "is zero-copy viable here?".
    if !unsafe { gpu_kernels::zerocopy::imp::import_available(&adapter) } {
        println!("import support: VK_EXT_external_memory_host is NOT advertised on this device.");
        println!("→ zero-copy import is unavailable on Honeykrisp/Mesa. Pivot to q4-resident + layer streaming.");
        return;
    }
    println!("VK_EXT_external_memory_host is advertised — attempting an import-capable device…");

    // Create the device WITH the import extension enabled (the whole point — request_device won't do it).
    let desc = wgpu::DeviceDescriptor {
        label: Some("zerocopy-probe"),
        required_features: wgpu::Features::empty(),
        required_limits: adapter.limits(),
        experimental_features: wgpu::ExperimentalFeatures::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    };
    let (device, queue) = match unsafe { gpu_kernels::zerocopy::imp::create_device_with_import(&adapter, &desc) } {
        Ok(dq) => dq,
        Err(e) => { eprintln!("could not create a device with VK_EXT_external_memory_host: {e}\n→ the extension may be unsupported on this driver; zero-copy import isn't available."); return; }
    };

    let support = match unsafe { gpu_kernels::zerocopy::imp::probe(&device) } {
        Some(s) => s,
        None => { eprintln!("probe failed (not a Vulkan device?)"); return; }
    };
    println!("import support: available={} minImportedHostPointerAlignment={}", support.available, support.min_align);
    if !support.available {
        println!("→ Honeykrisp does NOT support external-memory-host import. Pivot to layer streaming.");
        return;
    }
    println!("→ import IS supported. Good — the zero-copy path is viable on this box.");

    let Some(path) = path else {
        println!("(pass a .gguf path to run the import + bandwidth test)");
        return;
    };

    // mmap the file and import it whole, in place.
    let mmap = match gpu_kernels::mmap_ro(&path) {
        Ok(m) => m, Err(e) => { eprintln!("mmap {path}: {e}"); return; }
    };
    let file_len = mmap.len() as u64;
    let ptr = mmap.as_ptr();
    let min_align = support.min_align.max(1);
    let import_size = gpu_kernels::zerocopy::aligned_import_size(file_len, min_align);
    if !gpu_kernels::zerocopy::import_pointer_ok(ptr as usize, import_size, min_align) {
        eprintln!("mmap base {:p} not aligned to minImportedHostPointerAlignment {} — can't import (unexpected for a page-aligned mmap)", ptr, min_align);
        return;
    }
    // one span covering the whole file (import in place; no per-tensor split needed for the bandwidth test)
    let test_bytes = file_len.min(1 << 30); // read up to 1 GiB for the bandwidth measurement
    let spans = vec![(0u64, import_size, "gguf".to_string())];
    let imported = match unsafe { gpu_kernels::zerocopy::imp::import_region(&device, ptr, import_size, &spans) } {
        Ok(i) => i, Err(e) => { eprintln!("import failed: {e}"); return; }
    };
    println!("imported {:.2} GiB in place (zero copy, zero upload)", import_size as f64 / (1u64 << 30) as f64);
    let src = &imported.buffers[0];

    // device→device copy of `test_bytes` to measure the GPU's read bandwidth over the imported host pages.
    let dst = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("bw-dst"), size: test_bytes,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false,
    });
    // warm + timed
    for (label, timed) in [("warmup", false), ("timed", true)] {
        let t0 = std::time::Instant::now();
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) });
        enc.copy_buffer_to_buffer(src, 0, &dst, 0, test_bytes);
        queue.submit([enc.finish()]);
        device.poll(wgpu::PollType::wait_indefinitely()).ok();
        if timed {
            let secs = t0.elapsed().as_secs_f64();
            let gib = test_bytes as f64 / (1u64 << 30) as f64;
            println!("read {:.2} GiB from the imported buffer in {:.3}s → {:.1} GiB/s", gib, secs, gib / secs);
        }
    }

    // correctness: map the first 64 bytes of the copy and compare to the file (GGUF magic etc.).
    let check = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("check"), size: 64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    enc.copy_buffer_to_buffer(&dst, 0, &check, 0, 64);
    queue.submit([enc.finish()]);
    check.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).ok();
    let got = check.slice(..).get_mapped_range();
    let ok = got[..8] == mmap[..8]; // GGUF magic + version
    println!("readback matches file first-8-bytes: {ok} (GGUF magic {:02x?})", &got[..4]);
    drop(got);
    check.unmap();

    println!("done. If bandwidth is healthy and readback matches, the 70B's weights can be imported instead of uploaded.");
}
