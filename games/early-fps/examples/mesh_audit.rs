//! mesh_audit.rs — a real diagnostic harness for mesh-construction defects, not just texture coverage.
//! It instantiates every primitive generator plus full levels across several themes/seeds, then scans each
//! `Mesh3` for the defect classes that actually cause visible artifacts in a painter's-algorithm software
//! renderer once a texture isn't there to disguise them:
//!
//!   - degenerate triangles (near-zero area) — waste, and can produce garbage normals
//!   - NaN / zero-length normals — undefined lighting, often shows as black or white flecks
//!   - duplicate/overlapping coplanar triangles in the SAME mesh — the classic painter's-algorithm
//!     flicker: two triangles at (nearly) the same position/depth race for draw order every frame
//!   - excess per-vertex colour noise on untextured (no atlas material) meshes — a flat surface that's
//!     supposed to read as one clean tint but has much more variance than the intended dither amount
//!     reads as visible speckle/static once there's no texture pattern to bury it in
//!
//! Run: cargo run --example mesh_audit --release

use early_fps::level::{Level, PRESETS};
use early_fps::math::Vec3;
use early_fps::mesh::*;
use std::collections::HashMap;

struct Report {
    name: String,
    tris: usize,
    degenerate: usize,
    bad_normals: usize,
    overlapping: usize,
    color_stddev: f32,
    textured: bool,
}

fn analyze(name: &str, m: &Mesh3) -> Report {
    let tris = m.tris.len();
    let mut degenerate = 0usize;
    let mut bad_normals = 0usize;
    // bucket triangle (centroid, normal) so overlap checks stay near-linear instead of O(n^2)
    let mut buckets: HashMap<(i64, i64, i64, i32, i32, i32), Vec<usize>> = HashMap::new();
    let mut areas = Vec::with_capacity(tris);
    for (ti, tri) in m.tris.iter().enumerate() {
        let a = m.verts[tri[0] as usize];
        let b = m.verts[tri[1] as usize];
        let c = m.verts[tri[2] as usize];
        let cross = (b - a).cross(c - a);
        let area = cross.len() * 0.5;
        areas.push(area);
        if area < 1e-5 {
            degenerate += 1;
        }
        let n = if area > 1e-8 { cross.normalized() } else { Vec3::new(0.0, 0.0, 0.0) };
        if !n.x.is_finite() || !n.y.is_finite() || !n.z.is_finite() || (n.len() - 1.0).abs() > 0.05 && area > 1e-5 {
            bad_normals += 1;
        }
        let centroid = (a + b + c) * (1.0 / 3.0);
        // round position to ~2mm and normal to ~0.05 so near-identical/coplanar tris land in one bucket
        let key = (
            (centroid.x * 500.0).round() as i64,
            (centroid.y * 500.0).round() as i64,
            (centroid.z * 500.0).round() as i64,
            (n.x * 20.0).round() as i32,
            (n.y * 20.0).round() as i32,
            (n.z * 20.0).round() as i32,
        );
        buckets.entry(key).or_default().push(ti);
    }
    let overlapping = buckets.values().filter(|v| v.len() > 1).map(|v| v.len() - 1).sum();

    let textured = m.material.is_some();
    let mut color_stddev = 0.0f32;
    if !textured {
        let cols: Vec<f32> = match &m.vcol {
            Some(vc) if !vc.is_empty() => vc.iter().map(|c| 0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32).collect(),
            _ => vec![0.299 * m.color.r() as f32 + 0.587 * m.color.g() as f32 + 0.114 * m.color.b() as f32],
        };
        if cols.len() > 1 {
            let mean = cols.iter().sum::<f32>() / cols.len() as f32;
            let var = cols.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / cols.len() as f32;
            color_stddev = var.sqrt();
        }
    }

    Report { name: name.to_string(), tris, degenerate, bad_normals, overlapping, color_stddev, textured }
}

fn print_report(r: &Report) {
    let flag = if r.degenerate > 0 || r.bad_normals > 0 || r.overlapping > 0 || (!r.textured && r.color_stddev > 22.0) { "⚠" } else { " " };
    println!(
        "{flag} {:<38} tris={:<7} degen={:<5} badN={:<5} overlap={:<5} untex_stddev={:.1}{}",
        r.name,
        r.tris,
        r.degenerate,
        r.bad_normals,
        r.overlapping,
        r.color_stddev,
        if r.textured { " (textured)" } else { "" }
    );
}

fn main() {
    println!("=== Primitive generators (isolated) ===");
    let white = rgb(200, 200, 200);
    let mut prims: Vec<(&str, Mesh3)> = vec![
        ("boxx", boxx(Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 1.0), white, FACE_ALL)),
        ("panel", panel(Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 1.0), 2.0, 2.0, white)),
        ("ramp", ramp(Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 0.0, -1.0), Vec3::new(1.0, 1.0, 1.0), Vec3::new(-1.0, 1.0, 1.0), white)),
        ("uv_sphere", uv_sphere(Vec3::new(0.0, 0.0, 0.0), 1.0, 8, 6, white)),
        ("tube", tube(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 2.0, 0.0), 0.5, 8, white)),
        ("rock", rock(Vec3::new(0.0, 0.0, 0.0), 1.0, 42, white)),
        ("crystal", crystal(Vec3::new(0.0, 0.0, 0.0), 2.0, 0.6, 6, 7, white)),
        ("disc", disc(Vec3::new(0.0, 0.0, 0.0), 1.5, 10, white)),
        ("leaf_cluster", leaf_cluster(Vec3::new(0.0, 0.0, 0.0), 1.2, 5, white)),
        ("bullet_decal", bullet_decal(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.3, 0.7, 0.2), 0.15, 88)),
    ];
    for (name, verts) in cairn(Vec3::new(0.0, 0.0, 0.0), 5, 0.8, 11, white).into_iter().enumerate().map(|(i, m)| (format!("cairn[{i}]"), m)) {
        prims.push((Box::leak(name.into_boxed_str()), verts));
    }
    for (mat, name) in ATLAS_MATERIALS.iter().zip(["Concrete", "Brick", "Corrugated", "StoneBlock", "Cobble", "Gravel", "Grass", "Sand", "Snow", "RustPlate", "Tile", "Planks", "Dirt", "Organic", "Plaster", "Cloth"]) {
        let mut b = boxx(Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 1.0), white, FACE_ALL);
        b.material = Some(*mat);
        prims.push((Box::leak(format!("boxx+{name}").into_boxed_str()), b));
    }
    let mut worst = Vec::new();
    for (name, m) in &prims {
        let r = analyze(name, m);
        print_report(&r);
        worst.push(r);
    }

    println!("\n=== Full-level meshes (procedural themes) ===");
    let mut all_flagged: Vec<(String, Report)> = Vec::new();
    let themes = [
        "Deep Bunker", "Foundry Night", "Downtown", "Redwood Basin", "Frozen Bifrost",
    ];
    for name in themes {
        let Some(p) = PRESETS.iter().find(|p| p.name == name) else { continue };
        let lvl = Level::generate(p.config(9));
        println!("--- {name} ({} meshes) ---", lvl.meshes.len());
        for (i, m) in lvl.meshes.iter().enumerate() {
            if m.tri_count() == 0 {
                continue;
            }
            let label = format!("{name}/mesh[{i}]");
            let r = analyze(&label, m);
            if r.degenerate > 0 || r.bad_normals > 0 || r.overlapping > 3 || (!r.textured && r.color_stddev > 22.0) {
                print_report(&r);
                all_flagged.push((label, r));
            }
        }
    }

    println!("\n=== Authored maps ===");
    for name in ["Desert Town", "Flood Zone", "Reference Town"] {
        let Some(p) = PRESETS.iter().find(|p| p.name == name) else { continue };
        let lvl = Level::generate(p.config(7));
        println!("--- {name} ({} meshes) ---", lvl.meshes.len());
        for (i, m) in lvl.meshes.iter().enumerate() {
            if m.tri_count() == 0 {
                continue;
            }
            let label = format!("{name}/mesh[{i}]");
            let r = analyze(&label, m);
            if r.degenerate > 0 || r.bad_normals > 0 || r.overlapping > 3 || (!r.textured && r.color_stddev > 22.0) {
                print_report(&r);
                all_flagged.push((label, r));
            }
        }
    }

    println!("\n=== Summary ===");
    println!("primitives flagged: {}", worst.iter().filter(|r| r.degenerate > 0 || r.bad_normals > 0 || r.overlapping > 0).count());
    println!("level meshes flagged: {}", all_flagged.len());
    let total_overlap: usize = all_flagged.iter().map(|(_, r)| r.overlapping).sum();
    let total_degen: usize = all_flagged.iter().map(|(_, r)| r.degenerate).sum();
    println!("total overlapping/z-fight-risk tris across flagged meshes: {total_overlap}");
    println!("total degenerate tris across flagged meshes: {total_degen}");
}
