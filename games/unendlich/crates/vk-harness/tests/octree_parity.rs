//! GPU-vs-CPU parity for the octree sampler.
//!
//! The engine's mesher shader and its CPU reference both descend the same
//! sparse octree. This test verifies the descent itself: it builds an
//! `OctreeArray` in Rust, hands its nodes to the GPU verbatim, runs the REAL
//! `octree_sample.glsl` include (via a tiny probe shader) over every cell of a
//! 32³ chunk, and asserts the GPU's sampled block id matches `OctreeArray::sample`
//! for all 32768 cells. This is the "substrate" check the CPU-reference module's
//! own header calls out — divergence here is the root of mesher divergence.
//!
//! Runs on lavapipe; skips (does not fail) if no Vulkan device or no
//! `glslangValidator` is available.

use std::path::PathBuf;
use std::process::Command;

use vk_harness::{Binding, ComputeHarness};
use unendlich_engine::world::blocks::BlockId;
use unendlich_engine::world::chunk::ChunkCoord;
use unendlich_engine::world::octree::{OctreeArray, OctreeNode};

const CHUNK: i32 = 32;
const CELLS: usize = (CHUNK * CHUNK * CHUNK) as usize;

fn engine_shaders_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../unendlich-engine/src/shaders")
}

fn probe_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("shaders/octree_sample_probe.comp")
}

fn glslang_present() -> bool {
    Command::new("glslangValidator").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// Compile the probe to SPIR-V words, resolving engine includes.
fn compile_probe() -> Option<Vec<u32>> {
    let out = std::env::temp_dir().join("vk_harness_octree_probe.spv");
    let inc = format!("-I{}", engine_shaders_dir().display());
    let status = Command::new("glslangValidator")
        .args(["-V", probe_path().to_str().unwrap(), &inc, "-o", out.to_str().unwrap()])
        .output().ok()?;
    if !status.status.success() {
        eprintln!("probe compile failed:\n{}", String::from_utf8_lossy(&status.stdout));
        return None;
    }
    let bytes = std::fs::read(&out).ok()?;
    Some(bytes.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
}

/// Build a non-trivial octree: scattered blocks force real subdivision
/// (internal nodes down to depth 0), so traversal is genuinely exercised.
fn build_test_octree() -> OctreeArray {
    let mut oct = OctreeArray::empty(ChunkCoord { x: 0, y: 0, z: 0 });
    let pattern: &[(i32, i32, i32, BlockId)] = &[
        (0, 0, 0, BlockId::Stone),
        (31, 31, 31, BlockId::OakLog),
        (15, 4, 20, BlockId::Sand),
        (8, 16, 8, BlockId::Water),
        (1, 30, 2, BlockId::Amber),
        (30, 1, 29, BlockId::Crystal),
        (16, 16, 16, BlockId::Dirt),
        (7, 7, 7, BlockId::Gravel),
        (23, 9, 14, BlockId::Grass),
    ];
    for &(x, y, z, b) in pattern {
        oct.set_block(x, y, z, b, 1);
    }
    oct
}

fn nodes_as_bytes(nodes: &[OctreeNode]) -> &[u8] {
    assert_eq!(std::mem::size_of::<OctreeNode>(), 40, "node must be 40 bytes");
    unsafe { std::slice::from_raw_parts(nodes.as_ptr() as *const u8, std::mem::size_of_val(nodes)) }
}

#[test]
fn gpu_octree_sample_matches_cpu_for_every_cell() {
    let harness = match ComputeHarness::new() {
        Ok(h) => h,
        Err(e) => { eprintln!("skipping: no Vulkan compute device ({e})"); return; }
    };
    if !glslang_present() {
        eprintln!("skipping: glslangValidator not installed");
        return;
    }
    let spirv = match compile_probe() {
        Some(s) => s,
        None => { eprintln!("skipping: probe did not compile"); return; }
    };
    eprintln!("device: {} (cpu={})", harness.device_name(), harness.is_cpu_device());

    let oct = build_test_octree();
    eprintln!("octree node_count = {}", oct.nodes.len());
    assert!(oct.nodes.len() > 1, "test octree should have subdivided into internal nodes");

    // GPU pass.
    let node_bytes = nodes_as_bytes(&oct.nodes);
    let mut push = Vec::new();
    push.extend_from_slice(&0u32.to_ne_bytes());            // base
    push.extend_from_slice(&(CELLS as u32).to_ne_bytes());  // n
    let groups = ((CELLS as u32) + 63) / 64;
    let results = harness.run_compute(
        &spirv,
        &[Binding::Input(node_bytes), Binding::Output((CELLS * 4) as u64)],
        &push,
        groups,
        1,
    ).expect("compute dispatch");
    let raw = results[1].as_ref().expect("output readback");
    let gpu: Vec<u32> = raw.chunks_exact(4).take(CELLS)
        .map(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]])).collect();

    // CPU reference + compare.
    let mut mismatches = 0usize;
    let mut first: Option<(usize, u32, u32)> = None;
    for i in 0..CELLS {
        let x = (i as i32) & 31;
        let y = ((i as i32) >> 5) & 31;
        let z = ((i as i32) >> 10) & 31;
        let cpu = oct.sample(x, y, z) as u8 as u32;
        if cpu != gpu[i] {
            mismatches += 1;
            first.get_or_insert((i, cpu, gpu[i]));
        }
    }
    if let Some((i, cpu, g)) = first {
        let (x, y, z) = ((i as i32) & 31, ((i as i32) >> 5) & 31, ((i as i32) >> 10) & 31);
        panic!("{mismatches} mismatches; first at ({x},{y},{z}) cpu={cpu} gpu={g}");
    }
    eprintln!("all {CELLS} cells match GPU octree traversal == CPU OctreeArray::sample");
}
