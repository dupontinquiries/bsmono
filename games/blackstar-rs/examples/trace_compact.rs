// Does `skeleton::compact` even see these prims as limbs? Traces the
// exact same call model.rs makes (`skeletonize` -> `compact`) against a
// real chassis's robot pose and reports the tuck vector for named prim
// indices, so "still oversized after compaction" and "never classified
// as a limb at all" can be told apart instead of guessed at.
//
//   trace_compact <bots dir> <chassis> <prim> [prim...]
include!("shim.rs");

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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let bots_dir = Path::new(&args[1]);
    let name = &args[2];
    let prims_of_interest: Vec<usize> = args[3..].iter().filter_map(|s| s.parse().ok()).collect();

    let dirs = pose_dirs(&bots_dir.join(name));
    if dirs.len() < 2 {
        eprintln!("{name}: fewer than 2 fold poses");
        return;
    }
    let last = dirs.last().unwrap();
    let raw_span = |p: &PathBuf| -> f32 {
        let (_, _, _, s, _) = model::load_raw_at(p, 1.0, false);
        if s > 1e-9 { 1.0 / s } else { 1.0 }
    };
    let span0 = raw_span(&dirs[0]);
    let target = 1.32 * raw_span(last) / span0.max(1e-6);
    let (raws, _) = model::load_raw(last, target);

    let clouds: Vec<Vec<Vec3>> = raws.iter().map(|r| r.verts.iter().map(|v| Vec3::from_array(v.pos)).collect()).collect();
    let refs: Vec<&[Vec3]> = clouds.iter().map(|v| v.as_slice()).collect();
    let sk = skeleton::skeletonize(&refs);
    let tuck = skeleton::compact(&sk, refs.len(), 0.5);

    println!("{name}: {} prims, {} limbs found by skeletonize", refs.len(), sk.limbs.len());
    for limb in &sk.limbs {
        println!("  limb: reach={:.2} root={:?} tip={:?} prims={:?}", limb.reach, limb.root, limb.tip, limb.prims);
    }
    println!("  core_half={:?} core_centre={:?} core={:?} joined={}", sk.core_half, sk.core_centre, sk.core, sk.joined);
    for &p in &prims_of_interest {
        let t = tuck.get(p).copied().unwrap_or(Vec3::ZERO);
        let in_a_limb = sk.limbs.iter().any(|l| l.prims.contains(&p));
        println!(
            "  P{p}: tuck={:?} (magnitude {:.2}), claimed by a detected limb: {}",
            t, t.length(), in_a_limb
        );
    }
}

#[cfg(test)]
mod scratch_tests {
    use super::*;

    fn blob(c: Vec3, r: Vec3, n: usize) -> Vec<Vec3> {
        (0..n).map(|i| {
            let t = i as f32 * 0.7;
            c + Vec3::new(t.sin() * r.x, (t * 1.7).cos() * r.y, (t * 0.4).sin() * r.z)
        }).collect()
    }

    #[test]
    fn many_small_debris_plus_one_offender() {
        // core: several small "greeble" prims clustered near origin
        // (mimicking real chassis' many small detail prims), plus one
        // genuinely large off-center appendage.
        let mut prims: Vec<Vec<Vec3>> = Vec::new();
        for i in 0..25 {
            let a = i as f32 * 0.9;
            prims.push(blob(Vec3::new(a.sin() * 0.3, a.cos() * 0.3, 0.0), Vec3::splat(0.05), 8));
        }
        // one big offender, far off to one side
        prims.push(blob(Vec3::new(0.0, -2.0, 0.0), Vec3::new(0.3, 1.0, 0.3), 200));
        let refs: Vec<&[Vec3]> = prims.iter().map(|v| v.as_slice()).collect();
        let sk = skeleton::skeletonize(&refs);
        println!("many-small-debris case: {} limbs, core={:?}", sk.limbs.len(), sk.core);
        assert!(sk.limbs.len() >= 1, "the big offender did not read as a limb at all: core={:?}", sk.core);
    }
}
