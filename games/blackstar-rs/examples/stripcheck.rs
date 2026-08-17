//! Does the game get a fold or a flipbook? Runs `xform::Morph::build` — the
//! real one, compiled from `src/xform.rs` — over a chassis directory's
//! `f00..` strip and says which.
//!
//! This exists because the answer is invisible from the outside. `Morph`
//! refuses a strip whose poses disagree prim for prim, `main.rs` stores the
//! `Option` and steps the poses when it is `None`, and nothing prints. A
//! chassis can ship five poses, load without error, and still be a flipbook.
//!
//!   stripcheck <chassis dir>            one chassis
//!   stripcheck <bots dir> --all         every chassis under it
//!
//! Measured 2026-08-15 over `assets/bots`: 1 of 48 accepted. The cause is
//! ordering, not content — the poses hold the same multiset of prim sizes in
//! a different order, because the colour merge runs per pose instead of once
//! per chassis.
include!("shim.rs");

#[path = "../src/xform.rs"]
pub mod xform;

use meshgen::Vertex;
use std::path::{Path, PathBuf};

/// One pose as `Morph` wants it: prims of vertices, plus their indices.
fn pose(dir: &Path) -> Option<(Vec<Vec<Vertex>>, Vec<Vec<u32>>)> {
    let doc = gltf::Gltf::open(dir.join("scene.gltf")).ok()?;
    let bin = std::fs::read(dir.join("scene.bin")).ok()?;
    let (mut prims, mut idxs) = (Vec::new(), Vec::new());
    for mesh in doc.meshes() {
        for prim in mesh.primitives() {
            let r = prim.reader(|_| Some(&bin));
            let verts: Vec<Vertex> = r
                .read_positions()?
                .map(|p| Vertex { pos: p, nrm: [0.0, 1.0, 0.0], uv: [0.0, 0.0] })
                .collect();
            let idx: Vec<u32> = match r.read_indices() {
                Some(i) => i.into_u32().collect(),
                None => (0..verts.len() as u32).collect(),
            };
            prims.push(verts);
            idxs.push(idx);
        }
    }
    Some((prims, idxs))
}

/// Where the strip first disagrees, which is the only useful half of a "no".
fn first_mismatch(poses: &[Vec<Vec<Vertex>>]) -> Option<(usize, usize)> {
    let shape: Vec<usize> = poses[0].iter().map(|p| p.len()).collect();
    for (k, p) in poses.iter().enumerate().skip(1) {
        if p.len() != shape.len() {
            return Some((k, usize::MAX));
        }
        for (i, (a, b)) in p.iter().zip(&shape).enumerate() {
            if a.len() != *b {
                return Some((k, i));
            }
        }
    }
    None
}

fn check(root: &Path) -> bool {
    let mut dirs: Vec<PathBuf> = match std::fs::read_dir(root) {
        Ok(rd) => rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.is_dir()
                    && p.file_name().map(|n| n.to_string_lossy().starts_with('f')).unwrap_or(false)
                    && p.join("scene.gltf").is_file()
            })
            .collect(),
        Err(_) => return false,
    };
    dirs.sort();
    let name = root.file_name().unwrap_or_default().to_string_lossy().to_string();
    if dirs.len() < 2 {
        println!("{name}: {} pose(s) — no strip to morph", dirs.len());
        return false;
    }
    let mut poses = Vec::new();
    let mut idx = Vec::new();
    for d in &dirs {
        let Some((p, i)) = pose(d) else {
            println!("{name}: {} unreadable", d.display());
            return false;
        };
        if idx.is_empty() {
            idx = i;
        }
        poses.push(p);
    }
    let n = poses.len();
    let where_ = first_mismatch(&poses);
    match xform::Morph::build(poses, idx) {
        Some(mut m) => {
            let v = m.sample(0, 0.5).verts.len();
            println!("{name}: ACCEPTED — {n} poses, {} prims, prim0 blends {v} verts", m.prims());
            true
        }
        None => {
            let detail = match where_ {
                Some((k, i)) if i == usize::MAX => format!("pose {k} has a different prim count"),
                Some((k, i)) => format!("pose {k} prim {i} is a different size"),
                None => "rejected on pose count".into(),
            };
            println!("{name}: REFUSED — {detail}; the game steps the poses instead");
            false
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().expect("usage: stripcheck <chassis dir> [--all]"));
    let all = args.next().as_deref() == Some("--all");
    if !all {
        std::process::exit(if check(&root) { 0 } else { 1 });
    }
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .expect("read bots dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    let (mut ok, mut bad) = (0, 0);
    for d in &dirs {
        if check(d) { ok += 1 } else { bad += 1 }
    }
    println!("\n{ok} accepted, {bad} refused, of {} chassis", dirs.len());
    std::process::exit(if bad == 0 { 0 } else { 1 });
}
