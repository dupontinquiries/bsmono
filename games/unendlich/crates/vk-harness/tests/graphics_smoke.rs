//! Proof that the render pipeline is verifiable headlessly: render to an
//! offscreen image on lavapipe and assert on the pixels. The second test is the
//! real unblock — it feeds the packed vertex layout from `vertex_pack` through a
//! `.vert` that decodes it, proving the GPU decode matches the CPU codec.

use std::process::Command;
use vk_harness::graphics::{GraphicsHarness, VertexAttr};
use ash::vk;

fn glslang() -> bool {
    Command::new("glslangValidator").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// Compile a GLSL source string of the given stage (`vert`/`frag`) to SPIR-V.
fn compile(src: &str, stage: &str) -> Option<Vec<u32>> {
    let dir = std::env::temp_dir();
    let inp = dir.join(format!("vk_gfx_test.{stage}"));
    let outp = dir.join(format!("vk_gfx_test_{stage}.spv"));
    std::fs::write(&inp, src).ok()?;
    let st = Command::new("glslangValidator")
        .args(["-V", inp.to_str()?, "-o", outp.to_str()?])
        .output()
        .ok()?;
    if !st.status.success() {
        eprintln!("glsl compile failed: {}", String::from_utf8_lossy(&st.stdout));
        return None;
    }
    let bytes = std::fs::read(&outp).ok()?;
    Some(bytes.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
}

fn center_pixel(px: &[u8], dim: u32) -> [u8; 4] {
    let i = (((dim / 2) * dim + dim / 2) * 4) as usize;
    [px[i], px[i + 1], px[i + 2], px[i + 3]]
}

#[test]
fn offscreen_renders_a_solid_triangle() {
    let h = match GraphicsHarness::new() {
        Ok(h) => h,
        Err(e) => { eprintln!("skip: no graphics device ({e})"); return; }
    };
    if !glslang() { eprintln!("skip: no glslangValidator"); return; }
    eprintln!("graphics device: {}", h.device_name());

    // Fullscreen triangle from gl_VertexIndex; fragment outputs solid red.
    let vs = compile(
        "#version 450\nvoid main(){ vec2 p=vec2(float((gl_VertexIndex<<1)&2), float(gl_VertexIndex&2)); gl_Position=vec4(p*2.0-1.0,0.0,1.0); }",
        "vert",
    ).expect("vert");
    let fs = compile(
        "#version 450\nlayout(location=0) out vec4 o; void main(){ o=vec4(1.0,0.0,0.0,1.0); }",
        "frag",
    ).expect("frag");

    let dim = 16;
    let px = h.render_offscreen(dim, [0.0, 0.0, 0.0, 1.0], &vs, &fs, None, 3, &[], None, None).expect("render");
    let c = center_pixel(&px, dim);
    assert!(c[0] > 200 && c[1] < 40 && c[2] < 40 && c[3] > 200, "center should be red, got {c:?}");
}

#[test]
fn packed_vertex_layout_decodes_on_gpu() {
    let h = match GraphicsHarness::new() {
        Ok(h) => h,
        Err(e) => { eprintln!("skip: no graphics device ({e})"); return; }
    };
    if !glslang() { eprintln!("skip: no glslangValidator"); return; }

    // .vert reads the EXACT packed layout: loc0 = uvec4 (pos.xyz fixed-point +
    // normal_oct) at offset 0; loc1 = uvec2 (block_id, ao) at offset 8. It
    // decodes position the same way the CPU codec does (÷ POS_SCALE) and maps a
    // 0..4 local range to clip space (a fullscreen triangle), forwarding block_id.
    let vs = compile(
        "#version 450\n\
         layout(location=0) in uvec4 p0;\n\
         layout(location=1) in uvec2 p1;\n\
         layout(location=0) out flat uint vblock;\n\
         void main(){ vec3 local=vec3(p0.xyz)/1024.0; gl_Position=vec4(local.xy-1.0,0.0,1.0); vblock=p1.x; }",
        "vert",
    ).expect("vert");
    // Fragment colors by block_id: 7 → green, anything else → red. So a green
    // center proves the GPU read block_id=7 out of the packed bytes correctly.
    let fs = compile(
        "#version 450\n\
         layout(location=0) in flat uint vblock;\n\
         layout(location=0) out vec4 o;\n\
         void main(){ o = (vblock==7u) ? vec4(0.0,1.0,0.0,1.0) : vec4(1.0,0.0,0.0,1.0); }",
        "frag",
    ).expect("frag");

    // Build three packed vertices with the CPU codec — a screen-covering triangle
    // in local [0,4] space, block_id = 7.
    let verts = [
        vk_harness_pack([0.0, 0.0, 0.0], 7),
        vk_harness_pack([4.0, 0.0, 0.0], 7),
        vk_harness_pack([0.0, 4.0, 0.0], 7),
    ];
    let mut bytes = Vec::new();
    for v in &verts {
        bytes.extend_from_slice(&v.pos[0].to_le_bytes());
        bytes.extend_from_slice(&v.pos[1].to_le_bytes());
        bytes.extend_from_slice(&v.pos[2].to_le_bytes());
        bytes.extend_from_slice(&v.normal_oct.to_le_bytes());
        bytes.push(v.block_id);
        bytes.push(v.ao);
    }
    let attrs = [
        VertexAttr { location: 0, format: vk::Format::R16G16B16A16_UINT, offset: 0 },
        VertexAttr { location: 1, format: vk::Format::R8G8_UINT, offset: 8 },
    ];
    let dim = 16;
    let px = h
        .render_offscreen(dim, [0.0, 0.0, 0.0, 1.0], &vs, &fs, Some((&bytes, 10, &attrs)), 3, &[], None, None)
        .expect("render");
    let c = center_pixel(&px, dim);
    assert!(c[1] > 200 && c[0] < 40, "GPU should have decoded block_id=7 → green, got {c:?}");
}

// Local mirror so the test depends only on the public codec.
fn vk_harness_pack(local: [f32; 3], block_id: u8) -> unendlich_engine::vertex_pack::PackedVertex {
    unendlich_engine::vertex_pack::pack_vertex(local, [0.0, 0.0, 1.0], block_id, 255)
}
