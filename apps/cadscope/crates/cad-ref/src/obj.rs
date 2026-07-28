//! obj.rs — reading real geometry, not just headers.
//!
//! The corpus scan turned up a decisive fact: of 2,150 IQM models in the tree, exactly **one**
//! carries bounds, because IQM stores them per animation frame and nearly every export is static.
//! So the cheap header proxy for "how big is this" is unavailable precisely where a designer wants
//! it, and `volume of <model>` has to come from vertex data.
//!
//! That is what this module does, for the 1,005 OBJ meshes in the tree. It reads vertices and
//! faces, triangulates polygons by fan, and measures the enclosed volume with the same
//! divergence-theorem sum `cad_kernel::TriMesh::signed_volume` uses for meshed parts — deliberately
//! the same code, so a reference volume and a modelled volume are commensurable rather than merely
//! both being called "volume".

use std::fs;
use std::path::Path;

use cad_kernel::TriMesh;
use glam::Vec3;

use crate::ModelFacts;

/// Files past this size are indexed by name but not measured. A corpus scan should not be able to
/// spend a minute on one pathological export.
pub const MAX_BYTES: u64 = 16 * 1024 * 1024;

pub fn facts(path: &Path) -> Option<ModelFacts> {
    let is_obj = path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("obj"));
    if is_obj != Some(true) {
        return None;
    }
    if fs::metadata(path).map(|m| m.len()).unwrap_or(u64::MAX) > MAX_BYTES {
        return None;
    }
    let text = fs::read_to_string(path).ok()?;
    Some(parse(&text))
}

/// Parse OBJ text into measured facts.
pub fn parse(text: &str) -> ModelFacts {
    let mut pos: Vec<Vec3> = Vec::new();
    let mut mesh = TriMesh::default();
    let mut f = ModelFacts::default();

    for line in text.lines() {
        let line = line.trim();
        let mut it = line.split_whitespace();
        match it.next() {
            Some("v") => {
                let c: Vec<f32> = it.take(3).filter_map(|t| t.parse().ok()).collect();
                if c.len() == 3 {
                    pos.push(Vec3::new(c[0], c[1], c[2]));
                }
            }
            Some("f") => {
                // Each vertex reference is v, v/vt, v//vn or v/vt/vn, 1-based, negatives counting
                // back from the end.
                let idx: Vec<usize> = it
                    .filter_map(|tok| {
                        let first = tok.split('/').next()?;
                        let i: i64 = first.parse().ok()?;
                        let n = pos.len() as i64;
                        let r = if i > 0 { i - 1 } else if i < 0 { n + i } else { return None };
                        (r >= 0 && r < n).then_some(r as usize)
                    })
                    .collect();
                // Fan-triangulate; a quad becomes two triangles, an n-gon n-2.
                for k in 1..idx.len().saturating_sub(1) {
                    let tri = [idx[0], idx[k], idx[k + 1]];
                    let base = mesh.positions.len() as u32;
                    for &v in &tri {
                        mesh.positions.push(pos[v].to_array());
                    }
                    mesh.indices.extend_from_slice(&[base, base + 1, base + 2]);
                }
            }
            Some("g") | Some("o") => {
                if let Some(name) = it.next() {
                    let name = name.to_string();
                    if !name.is_empty() && !f.mesh_names.contains(&name) {
                        f.mesh_names.push(name);
                    }
                }
            }
            Some("usemtl") => {
                if let Some(m) = it.next() {
                    let m = m.to_string();
                    if !m.is_empty() && !f.materials.contains(&m) {
                        f.materials.push(m);
                    }
                }
            }
            _ => {}
        }
    }

    f.vertices = pos.len() as u32;
    f.triangles = mesh.tri_count() as u32;
    f.meshes = f.mesh_names.len().max(1) as u32;
    if !pos.is_empty() {
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for p in &pos {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
        if lo.is_finite() && hi.is_finite() {
            f.bounds = Some((lo, hi));
        }
    }
    if !mesh.is_empty() {
        // Absolute value because exporters disagree about winding, and an inside-out model is
        // still a model of a given size. Area is winding-independent already.
        f.volume = Some(mesh.signed_volume().abs());
        f.area = Some(surface_area(&mesh));
    }
    f
}

fn surface_area(m: &TriMesh) -> f32 {
    let mut a = 0.0;
    for t in m.indices.chunks_exact(3) {
        let p = |i: u32| Vec3::from(m.positions[i as usize]);
        a += (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0])).length() * 0.5;
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit cube at the origin, written the way an exporter would.
    const CUBE: &str = "\
o cube
usemtl steel
v 0 0 0
v 1 0 0
v 1 1 0
v 0 1 0
v 0 0 1
v 1 0 1
v 1 1 1
v 0 1 1
f 1 4 3 2
f 5 6 7 8
f 1 2 6 5
f 2 3 7 6
f 3 4 8 7
f 4 1 5 8
";

    #[test]
    fn measures_a_cube() {
        let f = parse(CUBE);
        assert_eq!(f.vertices, 8);
        assert_eq!(f.triangles, 12, "six quads fan into twelve triangles");
        assert_eq!(f.bounds, Some((Vec3::ZERO, Vec3::ONE)));
        assert!((f.volume.unwrap() - 1.0).abs() < 1e-5, "volume {:?}", f.volume);
        assert!((f.area.unwrap() - 6.0).abs() < 1e-5, "area {:?}", f.area);
        assert_eq!(f.materials, vec!["steel".to_string()]);
        assert_eq!(f.mesh_names, vec!["cube".to_string()]);
    }

    #[test]
    fn volume_is_scale_cubed() {
        // The property `volume of` depends on: doubling every coordinate must give 8x the volume.
        let doubled: String = CUBE
            .lines()
            .map(|l| {
                if let Some(rest) = l.strip_prefix("v ") {
                    let v: Vec<String> = rest
                        .split_whitespace()
                        .map(|t| (t.parse::<f32>().unwrap() * 2.0).to_string())
                        .collect();
                    format!("v {}\n", v.join(" "))
                } else {
                    format!("{l}\n")
                }
            })
            .collect();
        let a = parse(CUBE).volume.unwrap();
        let b = parse(&doubled).volume.unwrap();
        assert!((b / a - 8.0).abs() < 1e-3, "{b} / {a}");
    }

    #[test]
    fn handles_face_formats_and_negative_indices() {
        let tri = "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1/1/1 2/2/2 3/3/3\nf -3//1 -2//2 -1//3\n";
        let f = parse(tri);
        assert_eq!(f.vertices, 3);
        assert_eq!(f.triangles, 2, "both index styles parsed");
    }

    #[test]
    fn junk_and_dangling_indices_are_survived() {
        let bad = "v 0 0\nv nan nan nan\nf 1 2 3\nf 99 100 101\nf\nvt 0 0\n# comment\n";
        let f = parse(bad); // must not panic
        assert!(f.triangles <= 1);
    }
}
