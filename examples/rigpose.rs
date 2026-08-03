//! Evaluate a rigged glTF at a time and write the posed result as a plain
//! scene.gltf/scene.bin pair — the format skydribble's own loader reads.
//!
//! This is the rig proving itself. Nothing here is baked at export time by
//! another tool: the hierarchy is walked, the clip is sampled, the skin is
//! applied, and what comes out is a pose that never existed as a file.
//!
//!   rigpose <model dir> <out dir> [clip] [t 0..1]
//!   rigpose <model dir> --list
include!("shim.rs");

#[path = "../src/rig.rs"]
pub mod rig;

use glam::Vec3;
use std::io::Write;
use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("usage: rigpose <model dir> <out dir> [clip] [t]"));
    let a1 = args.next().unwrap_or_else(|| "--list".into());

    let Some(r) = rig::Rig::load(&dir) else {
        eprintln!("rigpose: {dir:?} has no readable glTF");
        std::process::exit(1);
    };

    if a1 == "--joints" {
        // Where the named skeleton actually is at this time. Feet below
        // pelvis below head means the pose stands up; anything else and the
        // model is lying down, and no amount of camera work will hide it.
        let clip: usize = args.next().unwrap_or_else(|| "0".into()).parse().unwrap_or(0);
        let frac: f32 = args.next().unwrap_or_else(|| "1".into()).parse().unwrap_or(1.0);
        let dur = r.clips.get(clip).map(|c| c.duration).unwrap_or(0.0);
        let g = r.pose(clip, frac.clamp(0.0, 1.0) * dur);
        for &j in &r.joint_nodes {
            let n = &r.names[j];
            if n.is_empty() { continue; }
            let p = g[j].to_scale_rotation_translation().2;
            println!("{:34} x {:9.2}  y {:9.2}  z {:9.2}", n, p.x, p.y, p.z);
        }
        return;
    }

    if a1 == "--list" {
        println!("{:?}: {} prims, {} clips", dir.file_name().unwrap_or_default(), r.prims.len(), r.clips.len());
        for (i, c) in r.clips.iter().enumerate() {
            println!("  [{i}] {:24} {:.2}s", c.name, c.duration);
        }
        return;
    }

    let out = PathBuf::from(a1);
    let clip: usize = args.next().unwrap_or_else(|| "0".into()).parse().unwrap_or(0);
    let frac: f32 = args.next().unwrap_or_else(|| "0".into()).parse().unwrap_or(0.0);
    let dur = r.clips.get(clip).map(|c| c.duration).unwrap_or(0.0);
    let t = frac.clamp(0.0, 1.0) * dur;

    let g = r.pose(clip, t);
    let posed = r.skin(&g);
    let (mn, mx) = rig::pose_bounds(&posed);

    // merge by base colour: one node per part is right for a rig and wrong
    // for a game loader, which turns every node into its own draw call
    let mut order: Vec<[i32; 4]> = Vec::new();
    let mut groups: Vec<([f32; 4], Vec<[f32; 3]>, Vec<u32>)> = Vec::new();
    for (pos, idx, col) in &posed {
        let key = [
            (col[0] * 512.0).round() as i32,
            (col[1] * 512.0).round() as i32,
            (col[2] * 512.0).round() as i32,
            (col[3] * 512.0).round() as i32,
        ];
        let gi = match order.iter().position(|k| *k == key) {
            Some(gi) => gi,
            None => {
                order.push(key);
                groups.push((*col, Vec::new(), Vec::new()));
                groups.len() - 1
            }
        };
        let base = groups[gi].1.len() as u32;
        groups[gi].1.extend(pos.iter().copied());
        groups[gi].2.extend(idx.iter().map(|i| i + base));
    }

    std::fs::create_dir_all(&out).expect("mkdir");
    let mut bin: Vec<u8> = Vec::new();
    let (mut views, mut accs, mut meshes, mut mats, mut nodes) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (i, (col, pos, idx)) in groups.iter().enumerate() {
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        let po = bin.len();
        for p in pos {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
                bin.extend_from_slice(&p[k].to_le_bytes());
            }
        }
        let io = bin.len();
        for &x in idx {
            bin.extend_from_slice(&x.to_le_bytes());
        }
        let vb = views.len();
        views.push(serde_json_view(0, po, pos.len() * 12, 34962));
        views.push(serde_json_view(0, io, idx.len() * 4, 34963));
        accs.push(format!(
            r#"{{"bufferView":{vb},"componentType":5126,"count":{},"type":"VEC3","min":[{},{},{}],"max":[{},{},{}]}}"#,
            pos.len(), lo[0], lo[1], lo[2], hi[0], hi[1], hi[2]
        ));
        accs.push(format!(
            r#"{{"bufferView":{},"componentType":5125,"count":{},"type":"SCALAR"}}"#,
            vb + 1,
            idx.len()
        ));
        mats.push(format!(
            r#"{{"name":"m{i}","pbrMetallicRoughness":{{"baseColorFactor":[{},{},{},{}],"metallicFactor":0.4,"roughnessFactor":0.6}}}}"#,
            col[0], col[1], col[2], col[3]
        ));
        meshes.push(format!(
            r#"{{"primitives":[{{"attributes":{{"POSITION":{}}},"indices":{},"material":{i}}}]}}"#,
            i * 2,
            i * 2 + 1
        ));
        nodes.push(format!(r#"{{"mesh":{i},"name":"m{i}"}}"#));
    }
    let scene: Vec<String> = (0..groups.len()).map(|i| i.to_string()).collect();
    let doc = format!(
        r#"{{"asset":{{"version":"2.0","generator":"rigpose"}},"buffers":[{{"uri":"scene.bin","byteLength":{}}}],"bufferViews":[{}],"accessors":[{}],"materials":[{}],"meshes":[{}],"nodes":[{}],"scenes":[{{"nodes":[{}]}}],"scene":0}}"#,
        bin.len(),
        views.join(","),
        accs.join(","),
        mats.join(","),
        meshes.join(","),
        nodes.join(","),
        scene.join(",")
    );
    // Write down which way is up, so the loader never has to guess. The
    // frame is taken from the ROBOT pose (t = end of the clip) rather than
    // this one: a truck half-way through unfolding has no meaningful "up",
    // but the thing it becomes does, and the whole strip shares a frame.
    let dur_end = r.clips.get(clip).map(|c| c.duration).unwrap_or(0.0);
    if let Some((up, fwd)) = rig::skeleton_frame(&r, clip, dur_end) {
        let j = format!(
            "{{\"up\":[{},{},{}],\"forward\":[{},{},{}],\"source\":\"skeleton\"}}",
            up.x, up.y, up.z, fwd.x, fwd.y, fwd.z
        );
        std::fs::write(out.join("orient.json"), j).ok();
        println!("   orient.json: up [{:.2} {:.2} {:.2}] forward [{:.2} {:.2} {:.2}] (from skeleton)",
            up.x, up.y, up.z, fwd.x, fwd.y, fwd.z);
    }
    std::fs::File::create(out.join("scene.bin")).unwrap().write_all(&bin).unwrap();
    std::fs::File::create(out.join("scene.gltf")).unwrap().write_all(doc.as_bytes()).unwrap();

    let ext = mx - mn;
    println!(
        "rigpose: clip {clip} {:?} t={t:.2}s/{dur:.2}s -> {} prims, {} verts, extents [{:.2} {:.2} {:.2}] -> {}",
        r.clips.get(clip).map(|c| c.name.as_str()).unwrap_or("?"),
        groups.len(),
        groups.iter().map(|g| g.1.len()).sum::<usize>(),
        ext.x, ext.y, ext.z,
        out.display()
    );
}

fn serde_json_view(buf: usize, off: usize, len: usize, target: u32) -> String {
    format!(r#"{{"buffer":{buf},"byteOffset":{off},"byteLength":{len},"target":{target}}}"#)
}
