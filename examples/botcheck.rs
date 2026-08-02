//! Engine-acceptance check: run an asset through skydribble's OWN loader
//! (model.rs verbatim via #[path] — parse, find frame, split wheels, orient,
//! normalize) and report what the engine sees. The shim satisfies the three
//! siblings model.rs names; only load_raw is exercised, so no GPU and no wgpu
//! compile are needed. Exit 0 means the engine accepts the asset.
include!("shim.rs");

use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from(std::env::args().nth(1).expect("usage: botcheck <asset dir>"));
    let (raws, half) = model::load_raw(&dir, 4.2);
    let prims = raws.len();
    let tris: usize = raws.iter().map(|r| r.idx.len() / 3).sum();
    let verts: usize = raws.iter().map(|r| r.verts.len()).sum();
    let wheels = raws.iter().filter(|r| r.tag != model::Tag::Body).count();
    println!(
        "skydribble accepts {:?}: {prims} prims ({wheels} tagged non-body), \
         {verts} verts, {tris} tris, half-extents [{:.2} {:.2} {:.2}] m",
        dir.file_name().unwrap_or_default(), half.x, half.y, half.z
    );
}
