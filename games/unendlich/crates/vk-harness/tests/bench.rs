//! Relative-performance bench on lavapipe. Records GPU-side kernel time
//! (timestamp queries) for the real octree sampler and a baseline kernel, in
//! the same min/p50/p90/p99/max shape as the engine's hardware GPU-timer
//! summaries.
//!
//! IMPORTANT: lavapipe is a software (CPU) device. These numbers are a RELATIVE
//! proxy only — useful for catching algorithmic regressions (same kernel before
//! vs after a change, same machine) and NOT comparable to the real M2/Honeykrisp
//! GPU. The test asserts only structural sanity, never an absolute threshold, so
//! it never flakes on perf noise. Set VK_HARNESS_BENCH_LOG=/path to append a
//! JSONL record per run for before/after diffing.
//!
//! Run: VK_ICD_FILENAMES=.../lvp_icd.json cargo test -p vk-harness --test bench -- --nocapture

use std::path::PathBuf;
use std::process::Command;

use vk_harness::{Binding, ComputeHarness, TimingStats, DOUBLE_SPV};
use unendlich_engine::world::blocks::BlockId;
use unendlich_engine::world::chunk::ChunkCoord;
use unendlich_engine::world::octree::{OctreeArray, OctreeNode};

const CELLS: usize = 32 * 32 * 32;
const ITERS: u32 = 64;

fn engine_shaders_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../unendlich-engine/src/shaders")
}
fn glslang_present() -> bool {
    Command::new("glslangValidator").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}
fn compile_probe() -> Option<Vec<u32>> {
    let out = std::env::temp_dir().join("vk_harness_octree_probe_bench.spv");
    let inc = format!("-I{}", engine_shaders_dir().display());
    let probe = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("shaders/octree_sample_probe.comp");
    let st = Command::new("glslangValidator")
        .args(["-V", probe.to_str().unwrap(), &inc, "-o", out.to_str().unwrap()]).output().ok()?;
    if !st.status.success() { return None; }
    let bytes = std::fs::read(&out).ok()?;
    Some(bytes.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
}
fn build_test_octree() -> OctreeArray {
    let mut oct = OctreeArray::empty(ChunkCoord { x: 0, y: 0, z: 0 });
    for &(x, y, z, b) in &[
        (0, 0, 0, BlockId::Stone), (31, 31, 31, BlockId::OakLog), (15, 4, 20, BlockId::Sand),
        (8, 16, 8, BlockId::Water), (1, 30, 2, BlockId::Amber), (30, 1, 29, BlockId::Crystal),
        (16, 16, 16, BlockId::Dirt), (7, 7, 7, BlockId::Gravel), (23, 9, 14, BlockId::Grass),
    ] { oct.set_block(x, y, z, b, 1); }
    oct
}
fn nodes_as_bytes(nodes: &[OctreeNode]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(nodes.as_ptr() as *const u8, std::mem::size_of_val(nodes)) }
}

fn record(stats: &TimingStats, label: &str) {
    eprintln!("{}", stats.summary_us(label));
    if let Ok(path) = std::env::var("VK_HARNESS_BENCH_LOG") {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(f,
                "{{\"kernel\":\"{label}\",\"iters\":{},\"min_ns\":{},\"p50_ns\":{},\"p90_ns\":{},\"p99_ns\":{},\"max_ns\":{}}}",
                stats.iters, stats.min_ns, stats.p50_ns, stats.p90_ns, stats.p99_ns, stats.max_ns);
        }
    }
}

fn sane(stats: &TimingStats) {
    assert_eq!(stats.iters, ITERS as usize);
    assert!(stats.min_ns <= stats.p50_ns && stats.p50_ns <= stats.p99_ns && stats.p99_ns <= stats.max_ns,
        "stats must be monotonic across percentiles");
}

#[test]
fn bench_octree_sample_and_baseline_kernels() {
    let harness = match ComputeHarness::new() {
        Ok(h) => h,
        Err(e) => { eprintln!("skipping bench: no Vulkan device ({e})"); return; }
    };
    eprintln!("== lavapipe relative bench == device: {} (cpu={})", harness.device_name(), harness.is_cpu_device());

    // Baseline kernel: double 4096 u32.
    let data: Vec<u8> = (0..4096u32).flat_map(|v| v.to_ne_bytes()).collect();
    let dbl = harness.bench_compute(DOUBLE_SPV, &[Binding::InOut(&data)], &4096u32.to_ne_bytes(), 64, 1, ITERS)
        .expect("double bench");
    sane(&dbl);
    record(&dbl, "double_u32x4096");

    // Real engine kernel: octree_sample over a 32^3 chunk.
    if !glslang_present() { eprintln!("skipping octree bench: no glslangValidator"); return; }
    let Some(spirv) = compile_probe() else { eprintln!("skipping octree bench: probe didn't compile"); return; };
    let oct = build_test_octree();
    let node_bytes = nodes_as_bytes(&oct.nodes);
    let mut push = Vec::new();
    push.extend_from_slice(&0u32.to_ne_bytes());
    push.extend_from_slice(&(CELLS as u32).to_ne_bytes());
    let groups = ((CELLS as u32) + 63) / 64;
    let oc = harness.bench_compute(
        &spirv, &[Binding::Input(node_bytes), Binding::Output((CELLS * 4) as u64)], &push, groups, 1, ITERS)
        .expect("octree bench");
    sane(&oc);
    record(&oc, "octree_sample_32cubed");
}

// --- Mesher dispatch bench -------------------------------------------------
// Benches the REAL chunk_mesh_octree.comp so a regression in its traversal/emit
// path shows up next to the octree-sample baseline. Capacities are sized so the
// atomic counters (which accumulate across bench_compute's re-submissions) stay
// well under capacity for ITERS iterations — otherwise the emit path would
// early-out on overflow and pollute the timing. The per-cell traversal (the
// dominant, scene-independent cost) is identical every iteration.

const MESHER_VCAP: u32 = 200_000; // 64 iters * ~1536 verts (8^3 cube) = 98k < cap
const MESHER_ICAP: u32 = 300_000;

fn compile_mesher() -> Option<Vec<u32>> {
    let out = std::env::temp_dir().join("vk_harness_chunk_mesh_octree_bench.spv");
    let inc = format!("-I{}", engine_shaders_dir().display());
    let src = engine_shaders_dir().join("chunk_mesh_octree.comp");
    let st = Command::new("glslangValidator")
        .args(["-V", src.to_str().unwrap(), &inc, "-o", out.to_str().unwrap()]).output().ok()?;
    if !st.status.success() { return None; }
    let bytes = std::fs::read(&out).ok()?;
    Some(bytes.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
}

fn chunk_meta_open() -> Vec<u8> {
    let mut m = Vec::with_capacity(64);
    let u = |m: &mut Vec<u8>, v: u32| m.extend_from_slice(&v.to_le_bytes());
    for _ in 0..4 { u(&mut m, 0); }          // octree_base, vertex_base, index_base, counter_base
    for _ in 0..3 { u(&mut m, 0); }          // chunk_origin xyz
    u(&mut m, 0);                            // _pad
    for _ in 0..6 { u(&mut m, 0xFFFF_FFFF); } // neighbors open
    m.extend_from_slice(&0.5f32.to_le_bytes()); // temperature
    m.extend_from_slice(&0.5f32.to_le_bytes()); // humidity
    m
}

fn mesher_push() -> Vec<u8> {
    let mut p = Vec::with_capacity(24);
    for v in [MESHER_VCAP, MESHER_ICAP, 1u32, 32u32 /*lod_cell_count*/, 1u32 /*stride*/, 0u32] {
        p.extend_from_slice(&v.to_le_bytes());
    }
    p
}

#[test]
fn bench_mesher_dispatch() {
    let harness = match ComputeHarness::new() {
        Ok(h) => h,
        Err(e) => { eprintln!("skipping mesher bench: no Vulkan device ({e})"); return; }
    };
    if !glslang_present() { eprintln!("skipping mesher bench: no glslangValidator"); return; }
    let Some(spirv) = compile_mesher() else { eprintln!("skipping mesher bench: shader didn't compile"); return; };

    // Solid 8^3 stone cube: full 32^3 traversal + a healthy shell emit (~1536 v).
    let mut oct = OctreeArray::empty(ChunkCoord { x: 0, y: 0, z: 0 });
    for x in 0..8 { for y in 0..8 { for z in 0..8 { oct.set_block(x, y, z, BlockId::Stone, 1); } } }
    let node_bytes = nodes_as_bytes(&oct.nodes);
    let meta = chunk_meta_open();
    let bindings = [
        Binding::Input(node_bytes),
        Binding::Output((MESHER_VCAP as u64) * 6 * 4),
        Binding::Output((MESHER_ICAP as u64) * 4),
        Binding::Output(16),
        Binding::Input(&meta),
    ];
    let groups = (32u32 * 32 * 32 + 63) / 64; // 512
    let stats = harness.bench_compute(&spirv, &bindings, &mesher_push(), groups, 1, ITERS)
        .expect("mesher bench");
    sane(&stats);
    record(&stats, "mesher_octree_8cubed");
}

// Same-process A/B for the buried-cell fast path (perf/mesher-buried-skip), on a
// fully-solid 32^3 chunk — the best case, ~82% of cells buried. Same-process
// before/after is the only lavapipe timing BENCH.md considers trustworthy, and
// even then it's a CPU device: treat a delta as a hint, confirm on the M2.
//
// Set OLD_MESHER to a pre-optimization chunk_mesh_octree.comp to compare; without
// it this benches only the current shader. Caps are sized so the accumulating
// atomic counters stay under capacity for AB_ITERS (24576 v/iter).
fn compile_comp_at(src: &std::path::Path) -> Option<Vec<u32>> {
    let out = std::env::temp_dir().join("vk_harness_ab.spv");
    let inc = format!("-I{}", engine_shaders_dir().display());
    let st = Command::new("glslangValidator")
        .args(["-V", src.to_str()?, &inc, "-o", out.to_str()?]).output().ok()?;
    if !st.status.success() {
        eprintln!("compile failed for {}: {}", src.display(), String::from_utf8_lossy(&st.stdout));
        return None;
    }
    let bytes = std::fs::read(&out).ok()?;
    Some(bytes.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
}

#[test]
fn bench_mesher_buried_skip_ab() {
    const AB_ITERS: u32 = 24;
    const AB_VCAP: u32 = 700_000; // 24576 v/iter * 24 = 590k < cap
    const AB_ICAP: u32 = 1_050_000;
    let harness = match ComputeHarness::new() {
        Ok(h) => h,
        Err(e) => { eprintln!("skipping A/B: no Vulkan device ({e})"); return; }
    };
    if !glslang_present() { eprintln!("skipping A/B: no glslangValidator"); return; }
    let Some(new_spv) = compile_mesher() else { eprintln!("skipping A/B: current shader didn't compile"); return; };

    let mut oct = OctreeArray::empty(ChunkCoord { x: 0, y: 0, z: 0 });
    for x in 0..32 { for y in 0..32 { for z in 0..32 { oct.set_block(x, y, z, BlockId::Stone, 1); } } }
    let node_bytes = nodes_as_bytes(&oct.nodes);
    let meta = chunk_meta_open();
    let mut push = Vec::with_capacity(24);
    for v in [AB_VCAP, AB_ICAP, 1u32, 32u32, 1u32, 0u32] { push.extend_from_slice(&v.to_le_bytes()); }
    let groups = (32u32 * 32 * 32 + 63) / 64; // 512

    let bench = |spv: &[u32]| -> TimingStats {
        let bindings = [
            Binding::Input(node_bytes),
            Binding::Output((AB_VCAP as u64) * 6 * 4),
            Binding::Output((AB_ICAP as u64) * 4),
            Binding::Output(16),
            Binding::Input(&meta),
        ];
        harness.bench_compute(spv, &bindings, &push, groups, 1, AB_ITERS).expect("A/B bench")
    };

    if let Ok(old_path) = std::env::var("OLD_MESHER") {
        if let Some(old_spv) = compile_comp_at(std::path::Path::new(&old_path)) {
            // Interleave a warm-up of each, then measure, to share thermal state.
            let _ = bench(&old_spv);
            let _ = bench(&new_spv);
            let old = bench(&old_spv);
            let new = bench(&new_spv);
            record(&old, "solid32_OLD(no-skip)");
            record(&new, "solid32_NEW(buried-skip)");
            let ratio = old.p50_ns as f64 / new.p50_ns.max(1) as f64;
            eprintln!("  A/B p50 speedup (lavapipe, HINT ONLY — confirm on M2): {ratio:.2}x");
            return;
        }
        eprintln!("OLD_MESHER set but didn't compile; benching current only");
    }
    let new = bench(&new_spv);
    record(&new, "solid32_NEW(buried-skip)");
    eprintln!("  (set OLD_MESHER=<pre-opt .comp> for an A/B ratio)");
}
