//! gpu.rs — slice 11: the GPU bridge. The monorepo carries a real Vulkan
//! engine (games/unendlich — `render-core` builds headless, ash dlopen,
//! lavapipe-tested via crates/vk-harness), and this module is paperbound's
//! on-ramp to it:
//!
//!   • `SceneBuffers::from_mesh` flattens our papercraft Mesh into the
//!     interleaved vertex format GPU pipelines eat — position (3×f32),
//!     per-FACE normal (3×f32, the hard origami facet — never smoothed),
//!     colour (3×f32) — plus a u32 index buffer with shared-vertex dedup.
//!     Byte-deterministic, so a frame's buffers can be golden-tested.
//!   • `vulkan_probe()` answers "can this machine run the engine path" the
//!     way vk-harness does — by looking for a loadable libvulkan — without
//!     linking ash, so the software rasterizer remains the universal
//!     fallback and CI never needs a GPU.
//!
//! Toolchain note, recorded where it bites: unendlich's workspace lockfile
//! is version 4 (needs a newer cargo than this sandbox's 1.75), so the
//! actual `unendlich-engine --features render-core` dependency lands on a
//! GPU host with a current toolchain. The buffer layout here matches what
//! that engine's mesh path uploads (pos/nrm/col interleave + u32 indices),
//! so wiring it is a consume, not a rewrite.

use crate::render::Mesh;

/// One interleaved vertex: pos(3) + nrm(3) + col(3), 36 bytes.
pub const VERTEX_STRIDE: usize = 9 * 4;

#[derive(Debug, Default)]
pub struct SceneBuffers {
    pub vertices: Vec<f32>, // interleaved pos/nrm/col
    pub indices: Vec<u32>,
}

impl SceneBuffers {
    /// Flatten a papercraft mesh. Normals are PER FACE (each triangle's
    /// vertices share its plane normal) — the crease is the style, exactly
    /// like clawbound's flat-shaded soup and unendlich's voxel facets.
    pub fn from_mesh(mesh: &Mesh) -> Self {
        let mut out = SceneBuffers::default();
        let mut lookup: std::collections::HashMap<[u32; 9], u32> =
            std::collections::HashMap::new();
        for t in &mesh.tris {
            // face normal from the world-space winding (x, elev, depth)
            let a = t.p[0];
            let b = t.p[1];
            let c = t.p[2];
            let u = (b.x - a.x, b.e - a.e, b.y - a.y);
            let v = (c.x - a.x, c.e - a.e, c.y - a.y);
            let mut n = (
                u.1 * v.2 - u.2 * v.1,
                u.2 * v.0 - u.0 * v.2,
                u.0 * v.1 - u.1 * v.0,
            );
            let len = (n.0 * n.0 + n.1 * n.1 + n.2 * n.2).sqrt();
            if len > 1e-12 {
                n = (n.0 / len, n.1 / len, n.2 / len);
            } else {
                n = (0.0, 1.0, 0.0);
            }
            for p in &t.p {
                let vert: [f32; 9] = [
                    p.x as f32,
                    p.e as f32,
                    p.y as f32,
                    n.0 as f32,
                    n.1 as f32,
                    n.2 as f32,
                    t.col[0],
                    t.col[1],
                    t.col[2],
                ];
                let key = vert.map(f32::to_bits);
                let idx = *lookup.entry(key).or_insert_with(|| {
                    let i = (out.vertices.len() / 9) as u32;
                    out.vertices.extend_from_slice(&vert);
                    i
                });
                out.indices.push(idx);
            }
        }
        out
    }

    pub fn vertex_count(&self) -> usize {
        self.vertices.len() / 9
    }

    /// The raw bytes a `vkCmdBindVertexBuffers` upload takes.
    pub fn vertex_bytes(&self) -> Vec<u8> {
        self.vertices.iter().flat_map(|f| f.to_le_bytes()).collect()
    }

    pub fn index_bytes(&self) -> Vec<u8> {
        self.indices.iter().flat_map(|i| i.to_le_bytes()).collect()
    }
}

/// Can this machine take the engine path? The vk-harness answer: is there a
/// loadable Vulkan ICD. No linkage — a filesystem probe over the loader's
/// usual homes — so the decision is cheap and the fallback is always sound.
pub fn vulkan_probe() -> Option<std::path::PathBuf> {
    let names = ["libvulkan.so.1", "libvulkan.so"];
    let roots = [
        "/usr/lib/x86_64-linux-gnu",
        "/usr/lib64",
        "/usr/lib",
        "/usr/local/lib",
    ];
    for root in roots {
        for name in names {
            let p = std::path::Path::new(root).join(name);
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::load_factbook;
    use crate::render::{build_scene, rgb, Plateau};
    use crate::skirmish::SkirmishSim;
    use std::path::Path;

    #[test]
    fn a_live_battle_exports_gpu_ready_buffers() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let book = load_factbook(&dir).unwrap();
        let mut sim = SkirmishSim::new(&book, "swarm", "tide");
        for _ in 0..(40 * 30) {
            sim.step(1.0 / 30.0);
        }
        let plats: Vec<Plateau> = sim.world.plateaus.iter()
            .map(|&(x, y, r)| Plateau { x, y, r }).collect();
        let mesh = build_scene(&sim.world, &plats, [rgb(0xD8B45A), rgb(0x8C7CC8)]);
        let buf = SceneBuffers::from_mesh(&mesh);
        // structure: 3 indices per tri, dedup actually deduped something
        assert_eq!(buf.indices.len(), mesh.tris.len() * 3);
        assert!(buf.vertex_count() < buf.indices.len(), "shared vertices dedup");
        assert!(buf.vertex_count() > 200, "a battle is not an empty page");
        // every normal is unit length — facet shading depends on it
        for v in buf.vertices.chunks_exact(9) {
            let n = (v[3] * v[3] + v[4] * v[4] + v[5] * v[5]).sqrt();
            assert!((n - 1.0).abs() < 1e-4, "normal length {n}");
        }
        // byte-deterministic: the same sim state uploads the same buffers
        let again = SceneBuffers::from_mesh(&mesh);
        assert_eq!(buf.vertex_bytes(), again.vertex_bytes());
        assert_eq!(buf.index_bytes(), again.index_bytes());
        assert_eq!(buf.vertex_bytes().len(), buf.vertex_count() * VERTEX_STRIDE);
    }

    #[test]
    fn the_probe_answers_without_linking_vulkan() {
        // either answer is fine — the point is it MUST answer, cheaply, and
        // a None machine takes the software rasterizer with no drama
        let _ = vulkan_probe();
    }
}

/// The real engine link, feature-gated: `--features gpu-engine` (cargo >=
/// 1.96 — unendlich's lockfile is v4). Proves at compile+run time that
/// paperbound-core and the monorepo's Vulkan engine live in one build.
#[cfg(feature = "gpu-engine")]
pub mod engine {
    /// The engine crate is linked; report what the bridge hands it.
    pub fn linkage_report(buffers: &super::SceneBuffers) -> String {
        // referencing the engine crate root proves the cross-workspace link
        let engine = std::any::type_name::<unendlich_engine::render::world_renderer::WorldRenderer>();
        format!(
            "paperbound-core → {} · {} verts ({} bytes) · {} indices",
            engine,
            buffers.vertex_count(),
            buffers.vertex_bytes().len(),
            buffers.indices.len()
        )
    }
}
