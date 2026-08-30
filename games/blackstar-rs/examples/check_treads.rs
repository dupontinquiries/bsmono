// Does turret::find's tread/turret detection actually work on real
// tank-category chassis, or is this the same "untuned heuristic" gap
// hover had before it got a category-based override?
//
//   check_treads <bots dir>   -- checks every chassis whose name/category
//                                 suggests "tank" against real geometry
include!("shim.rs");

use glam::Vec3;
use std::path::{Path, PathBuf};

fn pose_dirs(chassis: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(chassis)
        .into_iter().flatten().flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.join("scene.gltf").is_file())
        .collect();
    v.sort();
    v
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let bots_dir = Path::new(&args[1]);
    let mut names: Vec<String> = std::fs::read_dir(bots_dir)
        .into_iter().flatten().flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.to_ascii_lowercase().contains("tank"))
        .collect();
    names.sort();

    for name in &names {
        let dirs = pose_dirs(&bots_dir.join(name));
        for (pi, d) in dirs.iter().enumerate() {
            let (raws, _) = model::load_raw(d, 1.32);
            let clouds: Vec<Vec<Vec3>> = raws.iter().map(|r| r.verts.iter().map(|v| Vec3::from_array(v.pos)).collect()).collect();
            let refs: Vec<&[Vec3]> = clouds.iter().map(|v| v.as_slice()).collect();
            let f = turret::find(&refs);
            println!("  {name} pose {pi} ({}): {} treads, {} turret prims, {} rollers", d.display(), f.treads.len(), f.turret.len(), f.rollers.len());

            if name == "tank" && pi == 0 {
                // DIAGNOSTIC: reproduce turret::find's own measurements per
                // prim, to see which of the four TREAD conditions fails.
                const TREAD_LONG: f32 = 0.55;
                const TREAD_LOW: f32 = 0.10;
                const TREAD_OUT: f32 = 0.25;
                const SHOULDER: f32 = 0.15;
                const CENTRED: f32 = 0.55;
                const COMPACT: f32 = 0.62;
                let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
                for p in &refs { for v in p.iter() { lo = lo.min(*v); hi = hi.max(*v); } }
                let half = (hi - lo) * 0.5;
                let mid = (lo + hi) * 0.5;
                println!("    body half={half:?} mid={mid:?}");
                for (i, p) in refs.iter().enumerate() {
                    if p.len() < 8 { continue; }
                    let (mut plo, mut phi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
                    let mut sum = Vec3::ZERO;
                    for v in p.iter() { plo = plo.min(*v); phi = phi.max(*v); sum += *v; }
                    let c = sum / p.len() as f32;
                    let ph = (phi - plo) * 0.5;
                    let long_v = ph.z / half.z;
                    let low_v = (phi.y - mid.y) / half.y;
                    let out_v = (c.x - mid.x).abs() / half.x;
                    let flat = ph.y < ph.x * 2.2;
                    let above_v = (c.y - mid.y) / half.y;
                    let centred_v = (c.z - mid.z).abs() / half.z;
                    let compact_v = (ph.z / half.z).max(ph.x / half.x);
                    println!(
                        "    P{i} ({} pts): long={long_v:.2}(need>{TREAD_LONG}) low={low_v:.2}(need<{TREAD_LOW}) out={out_v:.2}(need>{TREAD_OUT}) flat={flat}  |  above={above_v:.2}(need>{SHOULDER}) centred={centred_v:.2}(need<{CENTRED}) compact={compact_v:.2}(need<{COMPACT})",
                        p.len()
                    );
                }
            }
        }
    }
}
