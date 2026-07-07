//! End-to-end proof of the deferred chunk texture path with the REAL data:
//! the real block_atlas pixels + the real block_texture_table (as a UBO). A
//! fragment indexes the table by block_id, picks the face tile, and samples the
//! atlas * tint. The expected colour is computed independently from
//! block_texture(), so this also catches drift between the T_* tile constants
//! and the actual packed atlas order.

use std::path::PathBuf;
use std::process::Command;
use unendlich_engine::texture_atlas::{block_texture, block_texture_table_bytes, BLOCK_ATLAS};
use unendlich_engine::world::blocks::BlockId;
use vk_harness::graphics::GraphicsHarness;

fn glslang() -> bool {
    Command::new("glslangValidator").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn compile(src: &str, stage: &str) -> Option<Vec<u32>> {
    let dir = std::env::temp_dir();
    let inp = dir.join(format!("vk_ctex.{stage}"));
    let outp = dir.join(format!("vk_ctex_{stage}.spv"));
    std::fs::write(&inp, src).ok()?;
    let st = Command::new("glslangValidator").args(["-V", inp.to_str()?, "-o", outp.to_str()?]).output().ok()?;
    if !st.status.success() {
        eprintln!("glsl: {}", String::from_utf8_lossy(&st.stdout));
        return None;
    }
    let b = std::fs::read(&outp).ok()?;
    Some(b.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
}

fn pixel(px: &[u8], dim: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * dim + x) * 4) as usize;
    [px[i], px[i + 1], px[i + 2], px[i + 3]]
}

fn atlas_raw() -> Option<Vec<u8>> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/textures/block_atlas.raw");
    std::fs::read(p).ok()
}

// Centre texel of a tile in the raw atlas (NEAREST sampling at local 0.5 hits this).
fn tile_center_texel(raw: &[u8], cols: u32, rows: u32, tile_px: u32, tile: u32) -> [u8; 3] {
    let (c, r) = (tile % cols, tile / cols);
    let (px, py) = (c * tile_px + tile_px / 2, r * tile_px + tile_px / 2);
    let width = cols * tile_px;
    let i = ((py * width + px) * 4) as usize;
    [raw[i], raw[i + 1], raw[i + 2]]
}

const VERT: &str = r#"
#version 450
void main() {
    vec2 p = vec2(float((gl_VertexIndex << 1) & 2), float(gl_VertexIndex & 2));
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
"#;

// Deferred chunk fragment: look the block up in the table, pick the face tile,
// sample the atlas, multiply by tint. (Real shader also lights it; this is the
// texture core.)
const FRAG: &str = r#"
#version 450
struct BlockTex { uvec4 tiles; vec4 tint; };
layout(set = 0, binding = 0) uniform Table { BlockTex blocks[81]; } tbl;
layout(set = 0, binding = 1) uniform sampler2D atlas;
layout(push_constant) uniform P { uvec4 meta; vec4 local; } pc; // meta = (block_id, cols, rows, face)
layout(location = 0) out vec4 o;
void main() {
    BlockTex bt = tbl.blocks[pc.meta.x];
    uint face = pc.meta.w;
    uint tile = face == 0u ? bt.tiles.x : (face == 1u ? bt.tiles.y : bt.tiles.z);
    uint cols = pc.meta.y, rows = pc.meta.z;
    vec2 cell = vec2(float(tile % cols), float(tile / cols));
    vec2 uv = (cell + pc.local.xy) / vec2(float(cols), float(rows));
    o = vec4(texture(atlas, uv).rgb * bt.tint.rgb, 1.0);
}
"#;

#[test]
fn real_atlas_and_table_texture_real_blocks() {
    let gh = match GraphicsHarness::new() { Ok(h) => h, Err(e) => { eprintln!("skip: {e}"); return; } };
    if !glslang() { eprintln!("skip"); return; }
    let raw = match atlas_raw() { Some(r) => r, None => { eprintln!("skip: no block_atlas.raw"); return; } };
    let vs = match compile(VERT, "vert") { Some(s) => s, None => { eprintln!("skip"); return; } };
    let fs = match compile(FRAG, "frag") { Some(s) => s, None => { eprintln!("skip"); return; } };

    let (cols, rows, tpx) = (BLOCK_ATLAS.cols, BLOCK_ATLAS.rows, BLOCK_ATLAS.tile_px);
    let (w, h) = (cols * tpx, rows * tpx);
    assert_eq!(raw.len(), (w * h * 4) as usize, "atlas raw size");
    let table = block_texture_table_bytes();

    let dim = 16u32;
    // (block, face): 0=top, 1=bottom, 2=side.
    let cases = [
        (BlockId::Stone, 2u32),
        (BlockId::Sand, 2),
        (BlockId::OakLog, 0), // end-cap
        (BlockId::OakLog, 2), // bark
        (BlockId::Andesite, 2), // tinted, shares stone tile
    ];
    for (block, face) in cases {
        let bt = block_texture(block).expect("authored");
        let tile = match face { 0 => bt.faces.top, 1 => bt.faces.bottom, _ => bt.faces.side };
        let texel = tile_center_texel(&raw, cols, rows, tpx, tile);
        let expect = [
            (texel[0] as f32 * bt.tint[0]) as i32,
            (texel[1] as f32 * bt.tint[1]) as i32,
            (texel[2] as f32 * bt.tint[2]) as i32,
        ];

        let mut push = Vec::new();
        for v in [block as u32, cols, rows, face] { push.extend_from_slice(&v.to_le_bytes()); }
        for v in [0.5f32, 0.5, 0.0, 0.0] { push.extend_from_slice(&v.to_le_bytes()); }

        let out = gh
            .render_offscreen_tex(dim, [0.0, 0.0, 0.0, 1.0], &vs, &fs, None, 3, &push, None, Some(&table), (&raw, w, h))
            .expect("render");
        let c = pixel(&out, dim, dim / 2, dim / 2);
        for ch in 0..3 {
            assert!(
                (c[ch] as i32 - expect[ch]).abs() <= 4,
                "{block:?} face {face}: tile {tile} got {c:?} expected ~{expect:?}"
            );
        }
    }
}
