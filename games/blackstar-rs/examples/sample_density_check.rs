// Does foldaudit's fixed 24-sample sweep manufacture "pops" out of smooth
// motion sampled too coarsely, or are the pops real discontinuities?
//
// Test: for one prim of one chassis, compute the worst per-vertex
// DISPLACEMENT between adjacent samples at several sample densities, and
// also the worst per-vertex VELOCITY (displacement / dt). If the motion is
// smooth, velocity converges to a stable number as sample count rises
// (displacement per step just shrinks proportionally). If there is a real
// discontinuity, velocity blows up as dt shrinks around that point instead
// of converging — a true pop concentrates in an ever-smaller time window
// rather than spreading out.
include!("shim.rs");

#[path = "../src/xform.rs"]
pub mod xform;
#[path = "../src/align.rs"]
pub mod align;

use glam::Vec3;
use std::path::{Path, PathBuf};

fn pose_dirs(chassis: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(chassis)
        .into_iter().flatten().flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir() && p.join("scene.gltf").is_file()
                && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('f'))
        })
        .collect();
    v.sort();
    v
}

fn build(bots_dir: &Path, name: &str) -> Option<xform::Morph> {
    let cdir = bots_dir.join(name);
    let dirs = pose_dirs(&cdir);
    if dirs.len() < 2 {
        return None;
    }
    let raw_span = |p: &PathBuf| -> f32 {
        let (_, _, _, s, _) = model::load_raw_at(p, 1.0, false);
        if s > 1e-9 { 1.0 / s } else { 1.0 }
    };
    let span0 = raw_span(&dirs[0]);
    let mut strip: Vec<Vec<model::RawPrim>> = Vec::new();
    for d in &dirs {
        let target = 1.32 * raw_span(d) / span0.max(1e-6);
        let (raws, _) = model::load_raw(d, target);
        strip.push(raws);
    }
    let u = align::unify(strip)?;
    xform::Morph::build(u.poses, u.idx)
}

fn worst_velocity(m: &mut xform::Morph, samples: usize) -> (f32, f32) {
    // returns (worst raw per-sample displacement, worst implied velocity)
    let prims = m.prims();
    let mut prev: Vec<Option<Vec<Vec3>>> = vec![None; prims];
    let mut worst_disp = 0.0f32;
    let mut worst_vel = 0.0f32;
    let dt = 1.0 / (samples - 1) as f32;
    for s in 0..samples {
        let t = s as f32 / (samples - 1) as f32;
        for p in 0..prims {
            let mesh = m.sample(p, t);
            let pts: Vec<Vec3> = mesh.verts.iter().map(|v| Vec3::from_array(v.pos)).collect();
            if let Some(before) = &prev[p] {
                if before.len() == pts.len() {
                    let jump = before.iter().zip(pts.iter())
                        .map(|(a, b)| (*b - *a).length())
                        .fold(0.0f32, f32::max);
                    worst_disp = worst_disp.max(jump);
                    worst_vel = worst_vel.max(jump / dt);
                }
            }
            prev[p] = Some(pts);
        }
    }
    (worst_disp, worst_vel)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let bots_dir = Path::new(&args[1]);
    let name = &args[2];
    let Some(mut m) = build(bots_dir, name) else {
        eprintln!("could not build a Morph for {name}");
        return;
    };
    println!("{name}: worst per-vertex displacement and implied velocity at rising sample density");
    for samples in [24usize, 48, 96, 192, 384] {
        let (disp, vel) = worst_velocity(&mut m, samples);
        println!("  {samples:4} samples (dt={:.4}): worst step {disp:.4}  implied velocity {vel:.2}", 1.0 / (samples - 1) as f32);
    }
}
