//! "Include" the packed vertex into the real render path: the chunk `.vert`
//! decodes `PackedVertex` (position from chunk-local fixed-point + a push-constant
//! origin and MVP; normal from the octahedral code) and the `.frag` shades it.
//! Proven on lavapipe by asserting pixels — so the decode that the live pipeline
//! will use is verified here, not guessed. (The Rust-side vertex-attribute wiring
//! and the mesher emit swap stay render-gated / behind a flag for the M2.)

use std::process::Command;
use vk_harness::graphics::{GraphicsHarness, VertexAttr};
use ash::vk;
use unendlich_engine::vertex_pack::{pack_vertex, PackedVertex};

const CHUNK_VERT: &str = "#version 450\n\
layout(location=0) in uvec4 p0;\n\
layout(location=1) in uvec2 p1;\n\
layout(push_constant) uniform PC { mat4 mvp; vec4 origin; } pc;\n\
layout(location=0) out vec3 v_normal;\n\
layout(location=1) out flat uint v_block;\n\
layout(location=2) out float v_ao;\n\
vec3 oct_decode(uint packed){\n\
  float qx=float(packed & 0xFFu);\n\
  float qy=float((packed>>8)&0xFFu);\n\
  float ox=(qx/255.0)*2.0-1.0;\n\
  float oy=(qy/255.0)*2.0-1.0;\n\
  vec3 n=vec3(ox,oy,1.0-abs(ox)-abs(oy));\n\
  if(n.z<0.0){\n\
    float tx=(1.0-abs(oy))*(ox>=0.0?1.0:-1.0);\n\
    float ty=(1.0-abs(ox))*(oy>=0.0?1.0:-1.0);\n\
    n.x=tx; n.y=ty;\n\
  }\n\
  return normalize(n);\n\
}\n\
void main(){\n\
  vec3 local=vec3(p0.xyz)/1024.0;\n\
  vec3 world=local+pc.origin.xyz;\n\
  gl_Position=pc.mvp*vec4(world,1.0);\n\
  v_normal=oct_decode(p0.w);\n\
  v_block=p1.x;\n\
  v_ao=float(p1.y)/255.0;\n\
}";

const CHUNK_FRAG: &str = "#version 450\n\
layout(location=0) in vec3 v_normal;\n\
layout(location=1) in flat uint v_block;\n\
layout(location=2) in float v_ao;\n\
layout(location=0) out vec4 o;\n\
void main(){\n\
  vec3 L=normalize(vec3(0.0,0.0,1.0));\n\
  float ndl=max(dot(normalize(v_normal),L),0.0);\n\
  float light=(0.2+0.8*ndl)*v_ao;\n\
  o=vec4(vec3(light),1.0);\n\
}";

fn glslang() -> bool {
    Command::new("glslangValidator").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn compile(src: &str, stage: &str) -> Option<Vec<u32>> {
    let dir = std::env::temp_dir();
    let inp = dir.join(format!("vk_chunk.{stage}"));
    let outp = dir.join(format!("vk_chunk_{stage}.spv"));
    std::fs::write(&inp, src).ok()?;
    let st = Command::new("glslangValidator").args(["-V", inp.to_str()?, "-o", outp.to_str()?]).output().ok()?;
    if !st.status.success() {
        eprintln!("glsl compile failed: {}", String::from_utf8_lossy(&st.stdout));
        return None;
    }
    let bytes = std::fs::read(&outp).ok()?;
    Some(bytes.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
}

fn pixel(px: &[u8], dim: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * dim + x) * 4) as usize;
    [px[i], px[i + 1], px[i + 2], px[i + 3]]
}

fn vbytes(verts: &[PackedVertex]) -> Vec<u8> {
    let mut b = Vec::with_capacity(verts.len() * 10);
    for v in verts {
        b.extend_from_slice(&v.pos[0].to_le_bytes());
        b.extend_from_slice(&v.pos[1].to_le_bytes());
        b.extend_from_slice(&v.pos[2].to_le_bytes());
        b.extend_from_slice(&v.normal_oct.to_le_bytes());
        b.push(v.block_id);
        b.push(v.ao);
    }
    b
}

fn push_bytes(mvp: [f32; 16], origin: [f32; 4]) -> Vec<u8> {
    let mut b = Vec::with_capacity(80);
    for f in mvp.iter().chain(origin.iter()) {
        b.extend_from_slice(&f.to_le_bytes());
    }
    b
}

// Column-major MVP mapping world.xy in [0,4] to NDC [-1,1], z=0.
const MVP_0_4: [f32; 16] = [
    0.5, 0.0, 0.0, 0.0,
    0.0, 0.5, 0.0, 0.0,
    0.0, 0.0, 0.0, 0.0,
    -1.0, -1.0, 0.0, 1.0,
];

fn quad(normal: [f32; 3], max_x: f32, max_y: f32) -> [PackedVertex; 6] {
    let (mx, my) = (max_x, max_y);
    [
        pack_vertex([0.0, 0.0, 0.0], normal, 1, 255),
        pack_vertex([mx, 0.0, 0.0], normal, 1, 255),
        pack_vertex([0.0, my, 0.0], normal, 1, 255),
        pack_vertex([mx, 0.0, 0.0], normal, 1, 255),
        pack_vertex([mx, my, 0.0], normal, 1, 255),
        pack_vertex([0.0, my, 0.0], normal, 1, 255),
    ]
}

const ATTRS: [VertexAttr; 2] = [
    VertexAttr { location: 0, format: vk::Format::R16G16B16A16_UINT, offset: 0 },
    VertexAttr { location: 1, format: vk::Format::R8G8_UINT, offset: 8 },
];

#[test]
fn chunk_shaders_light_faces_by_decoded_normal() {
    let h = match GraphicsHarness::new() { Ok(h) => h, Err(e) => { eprintln!("skip: {e}"); return; } };
    if !glslang() { eprintln!("skip: no glslangValidator"); return; }
    let vs = compile(CHUNK_VERT, "vert").expect("vert");
    let fs = compile(CHUNK_FRAG, "frag").expect("frag");
    let dim = 16;
    let push = push_bytes(MVP_0_4, [0.0, 0.0, 0.0, 0.0]);

    // +Z face fully faces the light L=(0,0,1) -> bright.
    let front = vbytes(&quad([0.0, 0.0, 1.0], 4.0, 4.0));
    let pf = h.render_offscreen(dim, [0.0, 0.0, 0.0, 1.0], &vs, &fs, Some((&front, 10, &ATTRS)), 6, &push, None, None).expect("render");
    let cf = pixel(&pf, dim, dim / 2, dim / 2);
    assert!(cf[0] > 230, "+Z face should be near-fully lit, got {cf:?}");

    // +Y face is perpendicular to L -> only ambient.
    let side = vbytes(&quad([0.0, 1.0, 0.0], 4.0, 4.0));
    let ps = h.render_offscreen(dim, [0.0, 0.0, 0.0, 1.0], &vs, &fs, Some((&side, 10, &ATTRS)), 6, &push, None, None).expect("render");
    let cs = pixel(&ps, dim, dim / 2, dim / 2);
    assert!(cs[0] > 20 && cs[0] < 90, "+Y face should be ~ambient only, got {cs:?}");
    assert!(cf[0] > cs[0] + 100, "lit face must be far brighter than perpendicular face ({} vs {})", cf[0], cs[0]);
}

#[test]
fn chunk_shaders_place_vertices_via_origin_and_mvp() {
    let h = match GraphicsHarness::new() { Ok(h) => h, Err(e) => { eprintln!("skip: {e}"); return; } };
    if !glslang() { eprintln!("skip: no glslangValidator"); return; }
    let vs = compile(CHUNK_VERT, "vert").expect("vert");
    let fs = compile(CHUNK_FRAG, "frag").expect("frag");
    let dim = 16;

    // A quad of local width 2, shifted by origin x=2 -> world x in [2,4] ->
    // NDC x in [0,1] = the right half of the screen. Proves the origin
    // translation + MVP land geometry where expected.
    let push = push_bytes(MVP_0_4, [2.0, 0.0, 0.0, 0.0]);
    let verts = vbytes(&quad([0.0, 0.0, 1.0], 2.0, 4.0));
    let px = h.render_offscreen(dim, [0.0, 0.0, 0.0, 1.0], &vs, &fs, Some((&verts, 10, &ATTRS)), 6, &push, None, None).expect("render");

    let right = pixel(&px, dim, dim * 3 / 4, dim / 2); // NDC x ~ +0.5, covered
    let left = pixel(&px, dim, dim / 4, dim / 2); // NDC x ~ -0.5, not covered
    assert!(right[0] > 200, "right half should be covered+lit, got {right:?}");
    assert!(left[0] < 30, "left half should be clear, got {left:?}");
}
