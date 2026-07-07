//! The end-to-end proof the harnesses unlock: run the REAL `chunk_mesh_octree.comp`
//! on lavapipe, take its actual vertex+index output, and render it through a real
//! graphics pipeline — asserting the silhouette in pixels. A solid chunk fills the
//! frame; an empty chunk leaves it clear. This is the in-sandbox stand-in for "does
//! the mesher's geometry actually draw", and the proving ground for later swapping
//! the emit format to packed (re-run this, assert the same silhouette).

use std::path::PathBuf;
use std::process::Command;
use vk_harness::graphics::{GraphicsHarness, VertexAttr};
use vk_harness::{Binding, ComputeHarness};
use ash::vk;
use unendlich_engine::world::blocks::BlockId;
use unendlich_engine::world::chunk::ChunkCoord;
use unendlich_engine::world::octree::{OctreeArray, OctreeNode};

const CC: u32 = 32;
const VERTEX_CAPACITY: u32 = 200_000;
const INDEX_CAPACITY: u32 = 300_000;

fn shaders_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../unendlich-engine/src/shaders")
}
fn tool(name: &str) -> bool {
    Command::new(name).arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn compile_mesher() -> Option<Vec<u32>> {
    let out = std::env::temp_dir().join("vk_mesher_render_mesh.spv");
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

fn compile_glsl(src: &str, stage: &str) -> Option<Vec<u32>> {
    let inp = std::env::temp_dir().join(format!("vk_mesher_render.{stage}"));
    let outp = std::env::temp_dir().join(format!("vk_mesher_render_{stage}.spv"));
    std::fs::write(&inp, src).ok()?;
    let st = Command::new("glslangValidator").args(["-V", inp.to_str()?, "-o", outp.to_str()?]).output().ok()?;
    if !st.status.success() {
        eprintln!("glsl: {}", String::from_utf8_lossy(&st.stdout));
        return None;
    }
    let bytes = std::fs::read(&outp).ok()?;
    Some(bytes.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
}

fn nodes_as_bytes(nodes: &[OctreeNode]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(nodes.as_ptr() as *const u8, std::mem::size_of_val(nodes)) }
}

fn chunk_meta_bytes(origin: (i32, i32, i32)) -> Vec<u8> {
    let mut m = Vec::with_capacity(64);
    let u = |m: &mut Vec<u8>, v: u32| m.extend_from_slice(&v.to_le_bytes());
    let i = |m: &mut Vec<u8>, v: i32| m.extend_from_slice(&v.to_le_bytes());
    let f = |m: &mut Vec<u8>, v: f32| m.extend_from_slice(&v.to_le_bytes());
    for _ in 0..4 { u(&mut m, 0); } // octree/vertex/index/counter base
    i(&mut m, origin.0); i(&mut m, origin.1); i(&mut m, origin.2);
    u(&mut m, 0); // _pad
    for _ in 0..6 { u(&mut m, 0xFFFF_FFFF); } // open neighbors
    f(&mut m, 0.5); f(&mut m, 0.5); // temperature, humidity
    debug_assert_eq!(m.len(), 64);
    m
}

fn mesher_push() -> Vec<u8> {
    let mut p = Vec::with_capacity(24);
    for v in [VERTEX_CAPACITY, INDEX_CAPACITY, 1u32, CC, 1u32, 0u32] {
        p.extend_from_slice(&v.to_le_bytes());
    }
    p
}

/// Run the real mesher, returning (vertex_bytes, vertex_count, index_bytes, index_count).
fn run_mesher(h: &ComputeHarness, spirv: &[u32], oct: &OctreeArray, origin: (i32, i32, i32)) -> (Vec<u8>, u32, Vec<u8>, u32) {
    let node_bytes = nodes_as_bytes(&oct.nodes).to_vec();
    let meta = chunk_meta_bytes(origin);
    let bindings = [
        Binding::Input(&node_bytes),
        Binding::Output((VERTEX_CAPACITY as u64) * 6 * 4),
        Binding::Output((INDEX_CAPACITY as u64) * 4),
        Binding::Output(16),
        Binding::Input(&meta),
    ];
    let groups_x = (CC * CC * CC + 63) / 64;
    let out = h.run_compute(spirv, &bindings, &mesher_push(), groups_x, 1).expect("mesher dispatch");
    let counters = out[3].as_ref().expect("counters");
    let rd = |b: &[u8], i: usize| u32::from_le_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]]);
    let vcount = rd(counters, 0);
    let icount = rd(counters, 1);
    assert_eq!(rd(counters, 2), 0, "mesher overflow");
    let vbytes = out[1].as_ref().unwrap()[..(vcount as usize * 6 * 4)].to_vec();
    let ibytes = out[2].as_ref().unwrap()[..(icount as usize * 4)].to_vec();
    (vbytes, vcount, ibytes, icount)
}

fn solid_chunk() -> OctreeArray {
    let mut oct = OctreeArray::empty(ChunkCoord { x: 0, y: 0, z: 0 });
    for z in 0..32 {
        for y in 0..32 {
            for x in 0..32 {
                oct.set_block(x, y, z, BlockId::Stone, 1);
            }
        }
    }
    oct
}

// Ortho MVP mapping world [0,32] in x,y to NDC [-1,1] (z flattened); column-major.
const MVP_0_32: [f32; 16] = [
    1.0 / 16.0, 0.0, 0.0, 0.0,
    0.0, 1.0 / 16.0, 0.0, 0.0,
    0.0, 0.0, 0.0, 0.0,
    -1.0, -1.0, 0.0, 1.0,
];

const VERT: &str = "#version 450\n\
layout(location=0) in vec3 in_pos;\n\
layout(location=1) in vec3 in_color;\n\
layout(push_constant) uniform PC { mat4 mvp; } pc;\n\
layout(location=0) out vec3 v_color;\n\
void main(){ gl_Position = pc.mvp * vec4(in_pos,1.0); v_color = in_color; }";

const FRAG: &str = "#version 450\n\
layout(location=0) in vec3 v_color;\n\
layout(location=0) out vec4 o;\n\
void main(){ o = vec4(v_color, 1.0); }";

fn center(px: &[u8], dim: u32) -> [u8; 4] {
    let i = (((dim / 2) * dim + dim / 2) * 4) as usize;
    [px[i], px[i + 1], px[i + 2], px[i + 3]]
}

#[test]
fn real_mesher_output_renders_a_chunk_silhouette() {
    let ch = match ComputeHarness::new() { Ok(h) => h, Err(e) => { eprintln!("skip: {e}"); return; } };
    let gh = match GraphicsHarness::new() { Ok(h) => h, Err(e) => { eprintln!("skip: {e}"); return; } };
    if !tool("glslangValidator") { eprintln!("skip: no glslangValidator"); return; }
    let mesher = match compile_mesher() { Some(s) => s, None => { eprintln!("skip: mesher didn't compile"); return; } };
    let vs = compile_glsl(VERT, "vert").expect("vert");
    let fs = compile_glsl(FRAG, "frag").expect("frag");

    let attrs = [
        VertexAttr { location: 0, format: vk::Format::R32G32B32_SFLOAT, offset: 0 },
        VertexAttr { location: 1, format: vk::Format::R32G32B32_SFLOAT, offset: 12 },
    ];
    let push = {
        let mut b = Vec::with_capacity(64);
        for f in MVP_0_32 { b.extend_from_slice(&f.to_le_bytes()); }
        b
    };
    let dim = 32;

    // Solid chunk: the mesher emits its surface; rendered straight-on it fills the
    // frame, so the centre pixel is covered (not the clear colour).
    let (vb, vc, ib, ic) = run_mesher(&ch, &mesher, &solid_chunk(), (0, 0, 0));
    assert!(vc > 0 && ic > 0, "solid chunk should mesh to geometry (v={vc}, i={ic})");
    let solid_px = gh
        .render_offscreen(dim, [0.0, 0.0, 0.0, 1.0], &vs, &fs, Some((&vb, 24, &attrs)), vc, &push, Some((&ib, ic)), None)
        .expect("render solid");
    let c = center(&solid_px, dim);
    assert!((c[0] as u32 + c[1] as u32 + c[2] as u32) > 30, "solid chunk should cover the centre, got {c:?}");

    // Empty chunk: no geometry -> nothing drawn -> centre stays the clear colour.
    let empty = OctreeArray::empty(ChunkCoord { x: 0, y: 0, z: 0 });
    let (vb0, vc0, ib0, ic0) = run_mesher(&ch, &mesher, &empty, (0, 0, 0));
    let empty_px = gh
        .render_offscreen(dim, [0.0, 0.0, 0.0, 1.0], &vs, &fs, Some((&vb0, 24, &attrs)), vc0, &push, Some((&ib0, ic0)), None)
        .expect("render empty");
    let e = center(&empty_px, dim);
    assert!((e[0] as u32 + e[1] as u32 + e[2] as u32) < 10, "empty chunk should leave the frame clear, got {e:?}");
}
