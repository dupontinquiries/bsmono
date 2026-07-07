//! Proves the graphics harness can bind a sampled atlas and that an
//! `AtlasLayout` tile UV picks the correct tile on the GPU. Synthetic 2×2-tile
//! atlas with four known colors; sampling each tile's centre UV must return that
//! tile's colour. Texture-agnostic — holds regardless of the real art set.

use std::process::Command;
use unendlich_engine::texture_atlas::AtlasLayout;
use vk_harness::graphics::{GraphicsHarness, VertexAttr};

fn glslang() -> bool {
    Command::new("glslangValidator").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn compile(src: &str, stage: &str) -> Option<Vec<u32>> {
    let dir = std::env::temp_dir();
    let inp = dir.join(format!("vk_atlas.{stage}"));
    let outp = dir.join(format!("vk_atlas_{stage}.spv"));
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

// Fullscreen triangle; samples the atlas at a UV supplied by push constant.
const VERT: &str = r#"
#version 450
layout(push_constant) uniform P { vec2 uv; } pc;
layout(location = 0) out vec2 vuv;
void main() {
    vec2 p = vec2(float((gl_VertexIndex << 1) & 2), float(gl_VertexIndex & 2));
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
    vuv = pc.uv;
}
"#;

const FRAG: &str = r#"
#version 450
layout(set = 0, binding = 1) uniform sampler2D atlas;
layout(location = 0) in vec2 vuv;
layout(location = 0) out vec4 o;
void main() { o = texture(atlas, vuv); }
"#;

#[test]
fn atlas_layout_uv_samples_the_right_tile_on_gpu() {
    let gh = match GraphicsHarness::new() { Ok(h) => h, Err(e) => { eprintln!("skip: {e}"); return; } };
    if !glslang() { eprintln!("skip: no glslangValidator"); return; }
    let vs = match compile(VERT, "vert") { Some(s) => s, None => { eprintln!("skip"); return; } };
    let fs = match compile(FRAG, "frag") { Some(s) => s, None => { eprintln!("skip"); return; } };

    // 2x2 px atlas = four 1px tiles: TL red, TR green, BL blue, BR yellow.
    // Rows top-to-bottom (v=0 is the top row).
    let atlas: [u8; 16] = [
        255, 0, 0, 255, 0, 255, 0, 255, // row 0: red,   green
        0, 0, 255, 255, 255, 255, 0, 255, // row 1: blue, yellow
    ];
    let layout = AtlasLayout::new(1, 2, 2);
    let want = [[255u8, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 0]];

    let dim = 16u32;
    for tile in 0..4u32 {
        let [u0, v0, u1, v1] = layout.tile_uv(tile).unwrap();
        let uv = [(u0 + u1) * 0.5, (v0 + v1) * 0.5];
        let mut push = Vec::new();
        push.extend_from_slice(&uv[0].to_le_bytes());
        push.extend_from_slice(&uv[1].to_le_bytes());
        let out = gh
            .render_offscreen_tex(
                dim, [0.0, 0.0, 0.0, 1.0], &vs, &fs,
                None, 3, &push, None, None, (&atlas, 2, 2),
            )
            .expect("render textured");
        let c = pixel(&out, dim, dim / 2, dim / 2);
        let w = want[tile as usize];
        for ch in 0..3 {
            assert!(
                (c[ch] as i32 - w[ch] as i32).abs() <= 2,
                "tile {tile}: sampled {c:?} expected {w:?}"
            );
        }
    }
}

// Chunk-style fragment: sample the per-face base tile, optionally composite an
// overlay (snow/sand) by its alpha, then multiply by the block tint.
const FRAG_CHUNK: &str = r#"
#version 450
layout(set = 0, binding = 1) uniform sampler2D atlas;
layout(push_constant) uniform P {
    vec2 base_uv;
    vec2 overlay_uv;
    vec4 tint;      // rgb = tint, a = has_overlay flag
} pc;
layout(location = 0) out vec4 o;
void main() {
    vec3 col = texture(atlas, pc.base_uv).rgb;
    if (pc.tint.a > 0.5) {
        vec4 ov = texture(atlas, pc.overlay_uv);
        col = mix(col, ov.rgb, ov.a);
    }
    col *= pc.tint.rgb;
    o = vec4(col, 1.0);
}
"#;

const VERT_FS_TRI: &str = r#"
#version 450
void main() {
    vec2 p = vec2(float((gl_VertexIndex << 1) & 2), float(gl_VertexIndex & 2));
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
"#;

#[test]
fn chunk_fragment_samples_tints_and_overlays() {
    let gh = match GraphicsHarness::new() { Ok(h) => h, Err(e) => { eprintln!("skip: {e}"); return; } };
    if !glslang() { eprintln!("skip"); return; }
    let vs = match compile(VERT_FS_TRI, "vert") { Some(s) => s, None => { eprintln!("skip"); return; } };
    let fs = match compile(FRAG_CHUNK, "frag") { Some(s) => s, None => { eprintln!("skip"); return; } };

    // 2x2 atlas: TL stone grey (opaque), TR tan (opaque), BL snow overlay (white,
    // 50% alpha), BR green.
    let atlas: [u8; 16] = [
        128, 128, 128, 255, 200, 160, 120, 255,
        255, 255, 255, 128, 0, 200, 0, 255,
    ];
    let layout = AtlasLayout::new(1, 2, 2);
    let center = |t: u32| { let [u0, v0, u1, v1] = layout.tile_uv(t).unwrap(); [(u0 + u1) * 0.5, (v0 + v1) * 0.5] };
    let stone = center(0);
    let tan = center(1);
    let snow = center(2);

    let dim = 16u32;
    let run = |base: [f32; 2], overlay: [f32; 2], tint: [f32; 4]| -> [u8; 4] {
        let mut push = Vec::new();
        for f in base.iter().chain(overlay.iter()).chain(tint.iter()) {
            push.extend_from_slice(&f.to_le_bytes());
        }
        let out = gh
            .render_offscreen_tex(dim, [0.0, 0.0, 0.0, 1.0], &vs, &fs, None, 3, &push, None, None, (&atlas, 2, 2))
            .expect("render chunk frag");
        pixel(&out, dim, dim / 2, dim / 2)
    };
    let close = |c: [u8; 4], want: [u8; 3]| {
        for ch in 0..3 {
            assert!((c[ch] as i32 - want[ch] as i32).abs() <= 3, "sampled {c:?} expected {want:?}");
        }
    };

    // A: plain stone, no tint.
    close(run(stone, stone, [1.0, 1.0, 1.0, 0.0]), [128, 128, 128]);
    // B: stone tinted (e.g. a recoloured stone-family block).
    close(run(stone, stone, [1.0, 0.5, 0.5, 0.0]), [128, 64, 64]);
    // C: stone with a 50%-alpha snow overlay composited on top.
    close(run(stone, snow, [1.0, 1.0, 1.0, 1.0]), [192, 192, 192]);
    // D: a different per-face tile (the "top" vs "side" case) samples cleanly.
    close(run(tan, tan, [1.0, 1.0, 1.0, 0.0]), [200, 160, 120]);
}

// Per-face UV from world position + normal: the fract() tiling a voxel face needs.
const VERT_FACE: &str = r#"
#version 450
layout(location = 0) in vec3 inpos;
layout(location = 0) out vec3 vworld;
void main() {
    vworld = inpos;
    gl_Position = vec4(inpos.x * 2.0 - 1.0, -(inpos.y * 2.0 - 1.0), 0.0, 1.0);
}
"#;

const FRAG_FACE: &str = r#"
#version 450
layout(set = 0, binding = 1) uniform sampler2D atlas;
layout(push_constant) uniform P { vec4 nrm; vec4 tile; } pc;
layout(location = 0) in vec3 vworld;
layout(location = 0) out vec4 o;
void main() {
    vec3 n = pc.nrm.xyz;
    vec2 p;
    if (abs(n.y) > 0.5) p = vworld.xz;
    else if (abs(n.x) > 0.5) p = vworld.zy;
    else p = vworld.xy;
    vec2 intile = fract(p);
    vec2 uv = mix(pc.tile.xy, pc.tile.zw, intile);
    o = texture(atlas, uv);
}
"#;

#[test]
fn voxel_face_uv_tiles_a_tile_across_the_face() {
    let gh = match GraphicsHarness::new() { Ok(h) => h, Err(e) => { eprintln!("skip: {e}"); return; } };
    if !glslang() { eprintln!("skip"); return; }
    let vs = match compile(VERT_FACE, "vert") { Some(s) => s, None => { eprintln!("skip"); return; } };
    let fs = match compile(FRAG_FACE, "frag") { Some(s) => s, None => { eprintln!("skip"); return; } };

    // 2x2 atlas = one tile of four distinct texels.
    let atlas: [u8; 16] = [
        255, 0, 0, 255, 0, 255, 0, 255,
        0, 0, 255, 255, 255, 255, 0, 255,
    ];
    // A unit quad in the XY plane (a +Z face), world coords 0..1.
    let quad: [[f32; 3]; 6] = [
        [0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0],
        [0.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0],
    ];
    let mut vbytes = Vec::new();
    for v in &quad { for f in v { vbytes.extend_from_slice(&f.to_le_bytes()); } }
    let attrs = [VertexAttr { location: 0, format: ash::vk::Format::R32G32B32_SFLOAT, offset: 0 }];

    // normal +Z; tile = whole atlas (0,0,1,1).
    let mut push = Vec::new();
    for f in [0.0f32, 0.0, 1.0, 0.0, /*tile*/ 0.0, 0.0, 1.0, 1.0] { push.extend_from_slice(&f.to_le_bytes()); }

    let dim = 32u32;
    let out = gh
        .render_offscreen_tex(dim, [0.05, 0.05, 0.05, 1.0], &vs, &fs, Some((&vbytes, 12, &attrs)), 6, &push, None, None, (&atlas, 2, 2))
        .expect("render face");

    // The four screen quadrants should show the four distinct texels — proving the
    // tile maps across the face from world position (not a flat colour).
    let q = [
        pixel(&out, dim, dim / 4, dim / 4),
        pixel(&out, dim, 3 * dim / 4, dim / 4),
        pixel(&out, dim, dim / 4, 3 * dim / 4),
        pixel(&out, dim, 3 * dim / 4, 3 * dim / 4),
    ];
    let key = |c: [u8; 4]| (c[0] >= 128, c[1] >= 128, c[2] >= 128);
    let mut seen = std::collections::HashSet::new();
    for c in q { seen.insert(key(c)); }
    assert_eq!(seen.len(), 4, "face should show all four texels, got {q:?}");
}
