//! iqm.rs — reading facts out of Inter-Quake Model files. `games/clawbound/assets/` alone holds
//! over two thousand of these, which is the corpus §17 is built to make addressable.
//!
//! We read the header, the mesh table and the per-frame bounds, and we resolve names out of the
//! text blob — sub-mesh names are what make "arms like `velgorn`" a thing you can point at, and
//! material names are the bridge to the texture corpus.
//!
//! These are files written by other people's exporters, so every read is bounds-checked and
//! anything malformed returns `None`. A corpus scan must never panic on a bad file.

use std::fs;
use std::path::Path;

use glam::Vec3;

use crate::ModelFacts;

const MAGIC: &[u8; 16] = b"INTERQUAKEMODEL\0";
/// Header is the magic plus 27 u32 fields.
const HEADER_U32S: usize = 27;

fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    let s = b.get(off..off + 4)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn f32_at(b: &[u8], off: usize) -> Option<f32> {
    Some(f32::from_bits(u32_at(b, off)?))
}

/// Null-terminated string starting at `base + off` in the text blob.
fn str_at(b: &[u8], base: usize, off: u32) -> Option<String> {
    let start = base.checked_add(off as usize)?;
    let rest = b.get(start..)?;
    let end = rest.iter().position(|&c| c == 0).unwrap_or(rest.len());
    Some(String::from_utf8_lossy(&rest[..end]).into_owned())
}

/// Parse a model file. Only IQM is understood in v0; other model extensions index by name with
/// empty facts rather than lying about what we know.
pub fn facts(path: &Path) -> Option<ModelFacts> {
    let is_iqm = path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("iqm"));
    if is_iqm != Some(true) {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    parse(&bytes)
}

/// Parse IQM bytes. Split out so tests can feed a synthetic file without touching the disk.
pub fn parse(b: &[u8]) -> Option<ModelFacts> {
    if b.len() < 16 + HEADER_U32S * 4 || &b[..16] != MAGIC {
        return None;
    }
    let h = |i: usize| u32_at(b, 16 + i * 4);
    // Field order per the IQM spec, after magic:
    //  0 version      1 filesize        2 flags
    //  3 num_text     4 ofs_text        5 num_meshes      6 ofs_meshes
    //  7 num_vertexarrays               8 num_vertexes    9 ofs_vertexarrays
    // 10 num_triangles               11 ofs_triangles    12 ofs_adjacency
    // 13 num_joints   14 ofs_joints   15 num_poses       16 ofs_poses
    // 17 num_anims    18 ofs_anims    19 num_frames      20 num_framechannels
    // 21 ofs_frames   22 ofs_bounds   23 num_comment     24 ofs_comment
    // 25 num_extensions              26 ofs_extensions
    let version = h(0)?;
    if version != 2 {
        return None;
    }
    let (num_text, ofs_text) = (h(3)?, h(4)? as usize);
    let (num_meshes, ofs_meshes) = (h(5)?, h(6)? as usize);
    let num_vertexes = h(8)?;
    let num_triangles = h(10)?;
    let num_frames = h(19)?;
    let ofs_bounds = h(22)? as usize;

    let mut f = ModelFacts {
        triangles: num_triangles,
        vertices: num_vertexes,
        meshes: num_meshes,
        ..Default::default()
    };

    // Mesh table: six u32 per mesh — name, material, first_vertex, num_vertexes,
    // first_triangle, num_triangles. We want the two name offsets.
    let text_ok = num_text > 0 && ofs_text < b.len();
    let cap = num_meshes.min(4096) as usize;
    for i in 0..cap {
        let base = ofs_meshes.checked_add(i * 24)?;
        if base + 24 > b.len() {
            break;
        }
        if text_ok {
            if let Some(n) = u32_at(b, base).and_then(|o| str_at(b, ofs_text, o)) {
                if !n.is_empty() {
                    f.mesh_names.push(n);
                }
            }
            if let Some(m) = u32_at(b, base + 4).and_then(|o| str_at(b, ofs_text, o)) {
                if !m.is_empty() && !f.materials.contains(&m) {
                    f.materials.push(m);
                }
            }
        }
    }

    // Bounds are stored per animation frame: bbmin[3], bbmax[3], xyradius, radius. A static model
    // has no frames and therefore no bounds — which we report as absence, not as a zero box.
    if num_frames > 0 && ofs_bounds > 0 && ofs_bounds + 32 <= b.len() {
        let g = |i: usize| f32_at(b, ofs_bounds + i * 4);
        let lo = Vec3::new(g(0)?, g(1)?, g(2)?);
        let hi = Vec3::new(g(3)?, g(4)?, g(5)?);
        if lo.is_finite() && hi.is_finite() && (hi - lo).min_element() >= 0.0 {
            f.bounds = Some((lo, hi));
        }
    }

    Some(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal but structurally valid IQM in memory: one mesh named "arm_L" with material
    /// "steel", and one frame of bounds.
    fn synthetic() -> Vec<u8> {
        let text = b"\0arm_L\0steel\0"; // offset 0 is the empty string, by convention
        let ofs_text = 16 + HEADER_U32S * 4;
        let ofs_meshes = ofs_text + text.len();
        let ofs_bounds = ofs_meshes + 24;

        let mut b = Vec::new();
        b.extend_from_slice(MAGIC);
        let mut hdr = [0u32; HEADER_U32S];
        hdr[0] = 2; // version
        hdr[3] = text.len() as u32;
        hdr[4] = ofs_text as u32;
        hdr[5] = 1; // num_meshes
        hdr[6] = ofs_meshes as u32;
        hdr[8] = 24; // num_vertexes
        hdr[10] = 12; // num_triangles
        hdr[19] = 1; // num_frames
        hdr[22] = ofs_bounds as u32;
        for v in hdr {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(text);
        // mesh: name=1 ("arm_L"), material=7 ("steel"), then four counts
        for v in [1u32, 7, 0, 24, 0, 12] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        for v in [-1.0f32, -2.0, -3.0, 1.0, 2.0, 3.0, 2.2, 3.7] {
            b.extend_from_slice(&v.to_bits().to_le_bytes());
        }
        assert_eq!(b.len(), ofs_bounds + 32);
        b
    }

    #[test]
    fn reads_counts_names_and_bounds() {
        let f = parse(&synthetic()).expect("valid iqm");
        assert_eq!(f.triangles, 12);
        assert_eq!(f.vertices, 24);
        assert_eq!(f.meshes, 1);
        assert_eq!(f.mesh_names, vec!["arm_L".to_string()]);
        assert_eq!(f.materials, vec!["steel".to_string()]);
        assert_eq!(f.bounds, Some((Vec3::new(-1.0, -2.0, -3.0), Vec3::new(1.0, 2.0, 3.0))));
        assert_eq!(f.bounds_volume(), Some(48.0));
    }

    #[test]
    fn rejects_junk_without_panicking() {
        assert!(parse(b"not an iqm at all").is_none());
        assert!(parse(&[]).is_none());
        // Right magic, wrong version.
        let mut wrong = synthetic();
        wrong[16..20].copy_from_slice(&3u32.to_le_bytes());
        assert!(parse(&wrong).is_none());
    }

    #[test]
    fn survives_truncation_at_every_length() {
        // A corpus scan meets half-written and corrupt files; none of them may panic.
        let full = synthetic();
        for n in 0..full.len() {
            let _ = parse(&full[..n]);
        }
    }

    #[test]
    fn lying_offsets_are_refused_not_followed() {
        let mut b = synthetic();
        // Point the mesh table far past the end of the file.
        b[16 + 6 * 4..16 + 7 * 4].copy_from_slice(&0xFFFF_0000u32.to_le_bytes());
        let f = parse(&b);
        assert!(f.is_none() || f.unwrap().mesh_names.is_empty());
    }
}
