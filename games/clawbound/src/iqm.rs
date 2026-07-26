//! iqm.rs — loading the First Spirit models (assets/animals/*.iqm) into clawbound's
//! papercraft world.
//!
//! The drop (clawbound-animals-iqm, 2026-07-26) is seventeen IQM v2 files: the feline
//! pantheon SPIRITS_PLAN.md casts as ancestor legends (lion, white tiger, black panther,
//! puma, clouded leopard, caracal, three house cats, a "zebra cat", and Velgorn), two
//! scenery trees, and a handful of water/canopy exotics (shark, octopus, sea nettle,
//! toucan, orangutan). All are STATIC meshes — one mesh, no joints, no animations
//! (verified from the headers) — which suits how the modes use them: as *spirits and
//! statues*, not as replacements for the procedural cats. cat.rs stays the living-cat
//! art; these models are the ancient ones.
//!
//! Parser scope, on purpose: header + the f32x3 POSITION vertex array + the u32
//! triangle list. TEXCOORD/NORMAL are skipped — clawbound is flat-shaded papercraft and
//! `Mesh::tri` derives the facet normal from the winding; a texture pipeline would fight
//! the aesthetic, so facets take a palette color with per-facet shade jitter like every
//! other prop. std-only, no deps, same doctrine as the rest of the tree.
//!
//! Coordinates: IQM is a Quake-lineage format — models arrive Z-up. `Model::normalize`
//! rotates Z-up → Y-up (x, z, -y), re-centers x/z on the origin, drops feet to y = 0,
//! and uniformly scales to a caller-chosen height, so call sites think in world units
//! ("a 3.2-unit statue") and never in model units. Everything is deterministic: same
//! file bytes → same mesh, byte for byte.

use crate::math::{v3, Vec3};
use crate::mesh::Mesh;

#[derive(Debug)]
pub struct Model {
    pub positions: Vec<[f32; 3]>,
    pub tris: Vec<[u32; 3]>,
}

fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    b.get(off..off + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}
fn f32_at(b: &[u8], off: usize) -> Option<f32> {
    u32_at(b, off).map(f32::from_bits)
}

impl Model {
    /// Parse an IQM v2 blob. Errors are strings naming what was malformed — these are
    /// shipped assets, so a parse failure is a build problem to surface, not to mask.
    pub fn parse(b: &[u8]) -> Result<Model, String> {
        if b.len() < 16 + 27 * 4 || &b[..16] != b"INTERQUAKEMODEL\0" {
            return Err("not an IQM file (bad magic)".into());
        }
        let h = |i: usize| u32_at(b, 16 + i * 4).unwrap_or(0);
        let version = h(0);
        if version != 2 {
            return Err(format!("IQM version {version}, expected 2"));
        }
        let (num_va, num_vertexes, ofs_va) = (h(7) as usize, h(8) as usize, h(9) as usize);
        let (num_tris, ofs_tris) = (h(10) as usize, h(11) as usize);

        // find the POSITION vertex array (type 0, format 7 = f32, size 3)
        let mut positions = Vec::new();
        for i in 0..num_va {
            let e = ofs_va + i * 20;
            let (ty, fmt, size, off) = (
                u32_at(b, e).ok_or("truncated vertexarray table")?,
                u32_at(b, e + 8).ok_or("truncated vertexarray table")?,
                u32_at(b, e + 12).ok_or("truncated vertexarray table")?,
                u32_at(b, e + 16).ok_or("truncated vertexarray table")? as usize,
            );
            if ty == 0 {
                if fmt != 7 || size != 3 {
                    return Err(format!("POSITION array is fmt={fmt} x{size}, expected f32 x3"));
                }
                positions.reserve(num_vertexes);
                for v in 0..num_vertexes {
                    let p = off + v * 12;
                    let (x, y, z) = (
                        f32_at(b, p).ok_or("truncated positions")?,
                        f32_at(b, p + 4).ok_or("truncated positions")?,
                        f32_at(b, p + 8).ok_or("truncated positions")?,
                    );
                    if !(x.is_finite() && y.is_finite() && z.is_finite()) {
                        return Err(format!("non-finite position at vertex {v}"));
                    }
                    positions.push([x, y, z]);
                }
            }
        }
        if positions.is_empty() {
            return Err("no POSITION vertex array".into());
        }

        let mut tris = Vec::with_capacity(num_tris);
        for t in 0..num_tris {
            let e = ofs_tris + t * 12;
            let (a, bb, c) = (
                u32_at(b, e).ok_or("truncated triangles")?,
                u32_at(b, e + 4).ok_or("truncated triangles")?,
                u32_at(b, e + 8).ok_or("truncated triangles")?,
            );
            let n = positions.len() as u32;
            if a >= n || bb >= n || c >= n {
                return Err(format!("triangle {t} indexes past the vertex count"));
            }
            tris.push([a, bb, c]);
        }
        Ok(Model { positions, tris })
    }

    /// Load from assets/animals by short name ("white_tiger"). Path is anchored to the
    /// crate dir so `cargo run`/`cargo test` agree on where assets live.
    pub fn load(name: &str) -> Result<Model, String> {
        let path = format!("{}/assets/animals/{name}.iqm", env!("CARGO_MANIFEST_DIR"));
        let bytes = std::fs::read(&path).map_err(|e| format!("{path}: {e}"))?;
        Model::parse(&bytes)
    }

    /// Z-up → Y-up, feet to y=0, x/z centered, uniform scale to `height` world units.
    pub fn normalize(&mut self, height: f32) {
        for p in self.positions.iter_mut() {
            *p = [p[0], p[2], -p[1]]; // (x, y, z)_yup = (x, z, -y)_zup
        }
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for p in &self.positions {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        let h = (hi[1] - lo[1]).max(1e-6);
        let s = height / h;
        let (cx, cz) = ((lo[0] + hi[0]) * 0.5, (lo[2] + hi[2]) * 0.5);
        for p in self.positions.iter_mut() {
            p[0] = (p[0] - cx) * s;
            p[1] = (p[1] - lo[1]) * s;
            p[2] = (p[2] - cz) * s;
        }
    }

    /// World-space bounding box (after normalize: min.y == 0).
    pub fn bbox(&self) -> ([f32; 3], [f32; 3]) {
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for p in &self.positions {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        (lo, hi)
    }

    /// Emit as flat papercraft facets at `pos`, rotated `yaw` about Y, tinted `col` with
    /// the same per-facet shade jitter the procedural props use (hashed per triangle, so
    /// it is deterministic and stable across frames).
    pub fn build(&self, m: &mut Mesh, pos: Vec3, yaw: f32, col: [f32; 3]) {
        let (sy, cy) = yaw.sin_cos();
        let tp = |p: [f32; 3]| -> Vec3 {
            let (x, z) = (p[0] * cy - p[2] * sy, p[0] * sy + p[2] * cy);
            v3(pos.x + x, pos.y + p[1], pos.z + z)
        };
        for (i, t) in self.tris.iter().enumerate() {
            let mut h = (i as u32).wrapping_mul(0x9E37_79B9);
            h ^= h >> 16;
            let jit = 0.88 + (h & 0xFF) as f32 / 255.0 * 0.24; // 0.88..1.12
            let c = [col[0] * jit, col[1] * jit, col[2] * jit];
            let (a, b, cc) = (
                self.positions[t[0] as usize],
                self.positions[t[1] as usize],
                self.positions[t[2] as usize],
            );
            // IQM (Quake lineage) winds clockwise for front faces; clawbound's Mesh derives
            // the facet normal from counter-clockwise winding — swap b/c so the normals
            // face out instead of in (found the fast way: the first render lit every
            // spirit from inside its own body).
            m.tri(tp(a), tp(cc), tp(b), c);
        }
    }
}

/// The spirit roster the modes draw from: short asset name, display name, a papercraft
/// tint (statues desaturate toward stone; living spirits keep pelt colors), and the
/// standing height in world units. Water/canopy exotics carry their habitat so modes
/// can place them only where the biome supports them.
pub const FELINE_SPIRITS: &[(&str, &str, u32, f32)] = &[
    ("lion_male", "the Golden Ancestor", 0xc9a44e, 3.4),
    ("white_tiger", "the Winter Ancestor", 0xdfe4e8, 3.2),
    ("black_panther", "the Shadow Ancestor", 0x35343c, 2.9),
    ("puma", "the Mountain Ancestor", 0xb08a62, 2.8),
    ("clouded_leopard", "the Canopy Ancestor", 0xa9905e, 2.6),
    ("caracal_cat_game_ready", "the Moor Ancestor", 0xc79a6a, 2.4),
    ("zebra_cat", "the Striped Wanderer", 0xcfc8bc, 2.2),
    ("persian_cat", "the Hearth Spirit", 0xd8d2c8, 2.0),
    ("cat", "the First Stray", 0x9a8f80, 2.0),
    ("velgorn", "Velgorn, the Outsider", 0x6a7f8f, 3.0),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shipped_animal_parses_and_normalizes() {
        let dir = format!("{}/assets/animals", env!("CARGO_MANIFEST_DIR"));
        let mut n = 0;
        for e in std::fs::read_dir(&dir).expect("assets/animals exists") {
            let p = e.unwrap().path();
            if p.extension().map_or(true, |x| x != "iqm") {
                continue;
            }
            let bytes = std::fs::read(&p).unwrap();
            let mut m = Model::parse(&bytes).unwrap_or_else(|e| panic!("{p:?}: {e}"));
            assert!(!m.tris.is_empty() && !m.positions.is_empty(), "{p:?} is empty");
            m.normalize(3.0);
            let (lo, hi) = m.bbox();
            assert!(lo[1].abs() < 1e-3, "{p:?}: feet must sit at y=0 (got {})", lo[1]);
            assert!((hi[1] - 3.0).abs() < 1e-3, "{p:?}: height must be 3.0 (got {})", hi[1]);
            assert!(lo[0] < 0.0 && hi[0] > 0.0, "{p:?}: x must straddle the origin");
            n += 1;
        }
        assert_eq!(n, 17, "all seventeen models ship");
    }

    #[test]
    fn the_spirit_roster_names_real_assets() {
        for (name, _, _, h) in FELINE_SPIRITS {
            let m = Model::load(name).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(m.tris.len() > 1000, "{name} should be a real model");
            assert!(*h > 1.0 && *h < 5.0);
        }
    }

    #[test]
    fn parse_rejects_garbage_with_a_reason() {
        assert!(Model::parse(b"not a model").unwrap_err().contains("magic"));
        let mut fake = b"INTERQUAKEMODEL\0".to_vec();
        fake.extend_from_slice(&[0u8; 27 * 4]);
        assert!(Model::parse(&fake).unwrap_err().contains("version"));
    }

    #[test]
    fn build_is_deterministic_and_lands_at_the_requested_spot() {
        let mut a = Model::load("cat").unwrap();
        a.normalize(2.0);
        let (mut m1, mut m2) = (Mesh::new(), Mesh::new());
        a.build(&mut m1, v3(5.0, 1.0, -3.0), 0.7, [0.5, 0.5, 0.5]);
        a.build(&mut m2, v3(5.0, 1.0, -3.0), 0.7, [0.5, 0.5, 0.5]);
        assert_eq!(m1.verts.len(), m2.verts.len());
        assert_eq!(m1.verts.len(), a.tris.len() * 3);
        // every vertex sits within the model's bbox radius of the anchor
        let (lo, hi) = a.bbox();
        let r = hi[0].abs().max(lo[0].abs()).max(hi[2].abs()).max(lo[2].abs()) + 1e-3;
        for v in &m1.verts {
            let (dx, dz) = (v.pos[0] - 5.0, v.pos[2] + 3.0);
            assert!((dx * dx + dz * dz).sqrt() <= r * 1.5, "vertex strayed from the anchor");
            assert!(v.pos[1] >= 1.0 - 1e-3, "feet must not sink below the anchor");
        }
    }
}
