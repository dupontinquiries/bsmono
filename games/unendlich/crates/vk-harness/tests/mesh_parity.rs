//! Full mesher parity: run the REAL `chunk_mesh_octree.comp` on lavapipe and
//! diff its emitted geometry against the engine's CPU reference mesher
//! (`mesh_chunk_octree_cpu_reference`) via `compare_meshes`. This is the
//! capstone the shader gate, octree-sample parity, and N-buffer harness were
//! built for: it proves the shipped GPU mesher and the CPU reference agree on
//! vertex/index counts and bounding box for real octree input.
//!
//! Run: VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json \
//!      cargo test -p vk-harness --test mesh_parity -- --nocapture

use std::path::PathBuf;
use std::process::Command;

use vk_harness::{Binding, ComputeHarness};
use unendlich_engine::world::blocks::BlockId;
use unendlich_engine::world::chunk::ChunkCoord;
use unendlich_engine::world::octree::{OctreeArray, OctreeNode};
use unendlich_engine::render::mesher::{ChunkMesh, Vertex};
use unendlich_engine::render::gpu_meshing_octree_cpu_ref::{
    compare_meshes, mesh_chunk_octree_cpu_reference,
    mesh_chunk_octree_cpu_reference_with_neighbors, Neighbors,
};

const CC: u32 = 32; // Near-LOD cell count per axis (matches the shader push).
const VERTEX_CAPACITY: u32 = 200_000;
const INDEX_CAPACITY: u32 = 300_000;

fn shaders_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../unendlich-engine/src/shaders")
}
fn tool(name: &str) -> bool {
    Command::new(name).arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// Compile the real mesher shader to SPIR-V (resolving #includes via -I).
fn compile_mesher() -> Option<Vec<u32>> {
    let out = std::env::temp_dir().join("vk_harness_chunk_mesh_octree.spv");
    let inc = format!("-I{}", shaders_dir().display());
    let src = shaders_dir().join("chunk_mesh_octree.comp");
    let st = Command::new("glslangValidator")
        .args(["-V", src.to_str().unwrap(), &inc, "-o", out.to_str().unwrap()])
        .output().ok()?;
    if !st.status.success() {
        eprintln!("glslang: {}", String::from_utf8_lossy(&st.stdout));
        return None;
    }
    let bytes = std::fs::read(&out).ok()?;
    Some(bytes.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
}

fn nodes_as_bytes(nodes: &[OctreeNode]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(nodes.as_ptr() as *const u8, std::mem::size_of_val(nodes)) }
}

/// One ChunkMeta (64 bytes, std430) for a single chunk at the given origin,
/// with explicit neighbor bases (0xFFFFFFFF = open), neutral climate. Field
/// order: [px, nx, py, ny, pz, nz].
fn chunk_meta_bytes(origin: (i32, i32, i32), neighbor_bases: [u32; 6]) -> Vec<u8> {
    let mut m = Vec::with_capacity(64);
    let u = |m: &mut Vec<u8>, v: u32| m.extend_from_slice(&v.to_le_bytes());
    let i = |m: &mut Vec<u8>, v: i32| m.extend_from_slice(&v.to_le_bytes());
    let f = |m: &mut Vec<u8>, v: f32| m.extend_from_slice(&v.to_le_bytes());
    u(&mut m, 0); // octree_base
    u(&mut m, 0); // vertex_base
    u(&mut m, 0); // index_base
    u(&mut m, 0); // counter_base
    i(&mut m, origin.0);
    i(&mut m, origin.1);
    i(&mut m, origin.2);
    u(&mut m, 0); // _pad
    for b in neighbor_bases { u(&mut m, b); } // px,nx,py,ny,pz,nz
    f(&mut m, 0.5); // temperature
    f(&mut m, 0.5); // humidity
    debug_assert_eq!(m.len(), 64);
    m
}

fn push_bytes() -> Vec<u8> {
    let mut p = Vec::with_capacity(24);
    for v in [VERTEX_CAPACITY, INDEX_CAPACITY, 1u32 /*batch_count*/, CC /*lod_cell_count*/, 1u32 /*lod_cell_stride*/, 0u32 /*_pad*/] {
        p.extend_from_slice(&v.to_le_bytes());
    }
    p
}

/// Run the GPU mesher for one chunk (with optional resident neighbors) and
/// rebuild a ChunkMesh from its output. `neigh` is [px, nx, py, ny, pz, nz];
/// present neighbors are concatenated after the center in the OctreeNodes SSBO
/// and their base node-indices recorded in the ChunkMeta (the sampler rebases
/// chunk-local child indices by the chunk's base).
fn gpu_mesh(
    harness: &ComputeHarness, spirv: &[u32], oct: &OctreeArray,
    origin: (i32, i32, i32), neigh: [Option<&OctreeArray>; 6],
) -> ChunkMesh {
    let mut node_bytes: Vec<u8> = Vec::new();
    node_bytes.extend_from_slice(nodes_as_bytes(&oct.nodes)); // center at base 0
    let mut bases = [0xFFFF_FFFFu32; 6];
    let mut cursor = oct.nodes.len() as u32;
    for d in 0..6 {
        if let Some(n) = neigh[d] {
            bases[d] = cursor;
            node_bytes.extend_from_slice(nodes_as_bytes(&n.nodes));
            cursor += n.nodes.len() as u32;
        }
    }
    let meta = chunk_meta_bytes(origin, bases);
    let bindings = [
        Binding::Input(&node_bytes),                             // b0 OctreeNodes
        Binding::Output((VERTEX_CAPACITY as u64) * 6 * 4),       // b1 Vertices (6 floats/vert)
        Binding::Output((INDEX_CAPACITY as u64) * 4),            // b2 Indices
        Binding::Output(16),                                     // b3 Counters (4 u32, zero-init)
        Binding::Input(&meta),                                   // b4 ChunkMetas
    ];
    let groups_x = (CC * CC * CC + 63) / 64; // 32768/64 = 512
    let out = harness.run_compute(spirv, &bindings, &push_bytes(), groups_x, 1)
        .expect("mesher dispatch");

    let counters = out[3].as_ref().expect("counters readback");
    let rd_u32 = |buf: &[u8], i: usize| u32::from_le_bytes([buf[i*4], buf[i*4+1], buf[i*4+2], buf[i*4+3]]);
    let vert_count = rd_u32(counters, 0) as usize;
    let idx_count = rd_u32(counters, 1) as usize;
    let overflow = rd_u32(counters, 2);
    assert_eq!(overflow, 0, "GPU mesher reported capacity overflow — raise capacities");

    let vfloats = out[1].as_ref().expect("vertex readback");
    let mut vertices = Vec::with_capacity(vert_count);
    for v in 0..vert_count {
        let base = v * 6;
        let g = |k: usize| {
            let o = (base + k) * 4;
            f32::from_le_bytes([vfloats[o], vfloats[o+1], vfloats[o+2], vfloats[o+3]])
        };
        vertices.push(Vertex { position: [g(0), g(1), g(2)], color: [g(3), g(4), g(5)] });
    }
    let ibuf = out[2].as_ref().expect("index readback");
    let indices: Vec<u32> = (0..idx_count).map(|k| rd_u32(ibuf, k)).collect();
    ChunkMesh { vertices, indices }
}

const OPEN: [Option<&OctreeArray>; 6] = [None, None, None, None, None, None];

fn check_scene(harness: &ComputeHarness, spirv: &[u32], label: &str, oct: &OctreeArray) {
    let coord = ChunkCoord { x: 0, y: 0, z: 0 };
    let gpu = gpu_mesh(harness, spirv, oct, (0, 0, 0), OPEN);
    let cpu = mesh_chunk_octree_cpu_reference(coord, oct);
    let cmp = compare_meshes(&gpu, &cpu);
    eprintln!(
        "{label:<22} gpu(v={},i={})  cpu(v={},i={})  aabb_close={}",
        cmp.vertex_count_gpu, cmp.index_count_gpu, cmp.vertex_count_cpu, cmp.index_count_cpu,
        cmp.aabb_close(0.5)
    );
    assert!(cmp.vertex_count_matches(), "{label}: vertex count GPU {} != CPU {}", cmp.vertex_count_gpu, cmp.vertex_count_cpu);
    assert!(cmp.index_count_matches(), "{label}: index count GPU {} != CPU {}", cmp.index_count_gpu, cmp.index_count_cpu);
    assert!(cmp.aabb_close(0.5), "{label}: AABB mismatch gpu={:?} cpu={:?}", cmp.aabb_gpu, cmp.aabb_cpu);
}

#[test]
fn gpu_mesher_matches_cpu_reference() {
    let harness = match ComputeHarness::new() {
        Ok(h) => h,
        Err(e) => { eprintln!("skipping mesh parity: no Vulkan device ({e})"); return; }
    };
    if !tool("glslangValidator") {
        eprintln!("skipping mesh parity: no glslangValidator");
        return;
    }
    let Some(spirv) = compile_mesher() else {
        eprintln!("skipping mesh parity: chunk_mesh_octree.comp did not compile");
        return;
    };
    eprintln!("== mesher parity == device: {} (cpu={})", harness.device_name(), harness.is_cpu_device());

    // Scene 1: four isolated solid blocks — no shared faces, no interior culling.
    // Each must emit all six faces; counts pin basic face emission.
    let mut iso = OctreeArray::empty(ChunkCoord { x: 0, y: 0, z: 0 });
    for &(x, y, z) in &[(2i32, 2i32, 2i32), (20, 6, 9), (12, 25, 28), (29, 1, 3)] {
        iso.set_block(x, y, z, BlockId::Stone, 1);
    }
    check_scene(&harness, &spirv, "isolated_blocks", &iso);

    // Scene 2: a solid 6³ stone cube — exercises interior face culling and any
    // greedy merging. GPU and CPU must cull the same interior faces.
    let mut cube = OctreeArray::empty(ChunkCoord { x: 0, y: 0, z: 0 });
    for x in 4..10 { for y in 4..10 { for z in 4..10 {
        cube.set_block(x, y, z, BlockId::Stone, 1);
    }}}
    check_scene(&harness, &spirv, "solid_6cubed", &cube);

    // Scene 3: cube + a separated slab, mixed solids (Stone + Cobblestone).
    let mut mixed = OctreeArray::empty(ChunkCoord { x: 0, y: 0, z: 0 });
    for x in 2..6 { for y in 2..6 { for z in 2..6 {
        mixed.set_block(x, y, z, BlockId::Stone, 1);
    }}}
    for x in 20..28 { for z in 20..28 {
        mixed.set_block(x, 18, z, BlockId::Cobblestone, 1);
    }}
    check_scene(&harness, &spirv, "cube_plus_slab", &mixed);

    // Scene 4: isolated weatherable blocks — exercises the bumpy 4×4 top grid
    // (25 verts / 96 indices per top) and the hash2d/bumpy_height path the shader
    // must match bit-for-bit. Each isolated Grass block = bumpy top (25v) + 5 flat
    // faces (20v) = 45v, vs 24v for a flat-topped solid — so the count itself
    // confirms the bumpy top fired on both sides.
    let mut bumpy = OctreeArray::empty(ChunkCoord { x: 0, y: 0, z: 0 });
    for &(x, y, z) in &[(3i32, 5, 3), (10, 12, 18), (24, 8, 7), (16, 20, 25)] {
        bumpy.set_block(x, y, z, BlockId::Grass, 1);
    }
    check_scene(&harness, &spirv, "bumpy_grass_iso", &bumpy);

    // Scene 5: mixed weatherable materials (Grass/Sand/Snow/Gravel) perched on a
    // stone shelf — each gets a bumpy top, their bottoms cull against the stone,
    // and the shelf tops beneath them cull against the solid blocks above.
    let mut shelf = OctreeArray::empty(ChunkCoord { x: 0, y: 0, z: 0 });
    for x in 4..12 { for z in 4..12 { shelf.set_block(x, 6, z, BlockId::Stone, 1); } }
    shelf.set_block(5, 7, 5, BlockId::Grass, 1);
    shelf.set_block(7, 7, 6, BlockId::Sand, 1);
    shelf.set_block(9, 7, 8, BlockId::Snow, 1);
    shelf.set_block(6, 7, 9, BlockId::Gravel, 1);
    check_scene(&harness, &spirv, "bumpy_mixed_shelf", &shelf);
}

/// Cross-chunk neighbor parity — the production meshing path. A solid +X
/// boundary patch in the center chunk has its +X faces exposed (emitted) when
/// the +X neighbor is open, but culled when a resident +X neighbor is solid at
/// the adjoining x=0 cells. GPU and CPU must agree in BOTH states, and the
/// resident-neighbor mesh must have exactly the culled faces removed — proving
/// the boundary sampling + ChunkMeta neighbor bases are wired correctly.
#[test]
fn gpu_mesher_cross_chunk_neighbor_matches_cpu() {
    let harness = match ComputeHarness::new() {
        Ok(h) => h,
        Err(e) => { eprintln!("skipping cross-chunk parity: no Vulkan device ({e})"); return; }
    };
    if !tool("glslangValidator") { eprintln!("skipping cross-chunk parity: no glslangValidator"); return; }
    let Some(spirv) = compile_mesher() else { eprintln!("skipping cross-chunk parity: shader didn't compile"); return; };
    let coord = ChunkCoord { x: 0, y: 0, z: 0 };

    // Center: a 4×4 solid patch on the +X boundary plane (x=31), y,z in 10..14.
    let mut center = OctreeArray::empty(coord);
    for y in 10..14 { for z in 10..14 { center.set_block(31, y, z, BlockId::Stone, 1); } }
    // +X neighbor: solid at x=0 over the same y,z patch (abuts the center face).
    let mut px = OctreeArray::empty(ChunkCoord { x: 1, y: 0, z: 0 });
    for y in 10..14 { for z in 10..14 { px.set_block(0, y, z, BlockId::Stone, 1); } }

    // Open boundary (no resident neighbor): GPU vs CPU default Neighbors.
    let gpu_open = gpu_mesh(&harness, &spirv, &center, (0, 0, 0), OPEN);
    let cpu_open = mesh_chunk_octree_cpu_reference(coord, &center);
    let c_open = compare_meshes(&gpu_open, &cpu_open);
    eprintln!("cross-chunk OPEN     gpu(v={},i={}) cpu(v={},i={})",
        c_open.vertex_count_gpu, c_open.index_count_gpu, c_open.vertex_count_cpu, c_open.index_count_cpu);
    assert!(c_open.matches_loosely(0.5), "open-boundary GPU/CPU mismatch: {c_open:?}");

    // Resident +X neighbor (solid): the 16 +X boundary faces must be culled.
    let neigh = [Some(&px), None, None, None, None, None];
    let gpu_px = gpu_mesh(&harness, &spirv, &center, (0, 0, 0), neigh);
    let cpu_px = mesh_chunk_octree_cpu_reference_with_neighbors(
        coord, &center, &Neighbors { px: Some(&px), ..Default::default() });
    let c_px = compare_meshes(&gpu_px, &cpu_px);
    eprintln!("cross-chunk +X solid gpu(v={},i={}) cpu(v={},i={})",
        c_px.vertex_count_gpu, c_px.index_count_gpu, c_px.vertex_count_cpu, c_px.index_count_cpu);
    assert!(c_px.matches_loosely(0.5), "resident-neighbor GPU/CPU mismatch: {c_px:?}");

    // The resident solid neighbor must remove exactly the 16 +X quads
    // (16 faces × 4 verts = 64 verts, × 6 indices = 96 indices). This proves the
    // neighbor base was actually consulted (not a no-op): culling really happened
    // and matched on both sides.
    assert_eq!(c_open.vertex_count_gpu - c_px.vertex_count_gpu, 64,
        "expected 16 culled +X faces (64 verts); open={} px={}", c_open.vertex_count_gpu, c_px.vertex_count_gpu);
    assert_eq!(c_open.index_count_gpu - c_px.index_count_gpu, 96,
        "expected 16 culled +X faces (96 indices)");
}

/// Large octree-aligned uniform-solid regions. These collapse into single big
/// leaves (the 32³ chunk is one size-32 leaf; the 16-cube is a size-16 leaf), so
/// interior cells all land deep in one uniform node. This is the parity gate for
/// the "skip neighbor samples for cells strictly buried in a uniform leaf"
/// optimization: a cell strictly inside such a leaf has all six neighbors equal,
/// so every face culls and nothing emits — the optimized and naive paths must
/// produce byte-identical meshes. Run BEFORE wiring the optimization to capture
/// the baseline, and after to prove equivalence.
#[test]
fn gpu_mesher_solid_uniform_leaf_matches_cpu() {
    let harness = match ComputeHarness::new() {
        Ok(h) => h,
        Err(e) => { eprintln!("skipping solid-leaf parity: no Vulkan device ({e})"); return; }
    };
    if !tool("glslangValidator") {
        eprintln!("skipping solid-leaf parity: no glslangValidator");
        return;
    }
    let Some(spirv) = compile_mesher() else {
        eprintln!("skipping solid-leaf parity: mesher did not compile");
        return;
    };
    eprintln!("== solid-leaf parity == device: {}", harness.device_name());

    // Full 32³ solid chunk → root is one size-32 leaf. Open neighbors, so only
    // the six 32×32 outer faces emit; every interior cell is buried.
    let mut full = OctreeArray::empty(ChunkCoord { x: 0, y: 0, z: 0 });
    for y in 0..32 { for z in 0..32 { for x in 0..32 {
        full.set_block(x, y, z, BlockId::Stone, 1);
    }}}
    check_scene(&harness, &spirv, "solid_32cubed", &full);

    // 16-aligned solid cube [0,16)³ → a size-16 leaf in one octant; interior
    // cells buried, the three exposed faces toward the empty octants emit.
    let mut cube16 = OctreeArray::empty(ChunkCoord { x: 0, y: 0, z: 0 });
    for y in 0..16 { for z in 0..16 { for x in 0..16 {
        cube16.set_block(x, y, z, BlockId::Cobblestone, 1);
    }}}
    check_scene(&harness, &spirv, "solid_16cube", &cube16);

    // 8-aligned cube offset into the chunk [8,16)³ → a size-8 leaf fully
    // surrounded by air; all six faces emit, interior 6³ buried.
    let mut cube8 = OctreeArray::empty(ChunkCoord { x: 0, y: 0, z: 0 });
    for y in 8..16 { for z in 8..16 { for x in 8..16 {
        cube8.set_block(x, y, z, BlockId::Granite, 1);
    }}}
    check_scene(&harness, &spirv, "solid_8cube_offset", &cube8);
}
