//! Engine-acceptance check: run an asset through skydribble's OWN loader
//! (model.rs verbatim via #[path] — parse, orient, normalize) and report what
//! the engine sees. The render::Gpu shim satisfies load()'s signature; only
//! load_raw is exercised, so no GPU and no wgpu compile are needed. Exit 0
//! means the engine accepts the asset.
#[path = "../src/meshgen.rs"]
pub mod meshgen;
#[path = "../src/model.rs"]
pub mod model;
pub mod render {
    pub struct Gpu;
    impl Gpu {
        pub fn add_texture(&mut self, _w: u32, _h: u32, _px: &image::RgbaImage) -> usize { 0 }
        pub fn add_mesh(&mut self, _m: &crate::meshgen::MeshData) -> usize { 0 }
    }
}

use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from(std::env::args().nth(1).expect("usage: botcheck <asset dir>"));
    let (raws, half) = model::load_raw(&dir, 4.2);
    let prims = raws.len();
    let tris: usize = raws.iter().map(|r| r.idx.len() / 3).sum();
    let verts: usize = raws.iter().map(|r| r.verts.len()).sum();
    println!(
        "skydribble accepts {:?}: {prims} prims, {verts} verts, {tris} tris, half-extents [{:.2} {:.2} {:.2}] m",
        dir.file_name().unwrap_or_default(), half.x, half.y, half.z
    );
}
