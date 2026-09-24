//! Writing a bot out as a fold strip blackstar-rs can load.
//!
//! The engine discovers a chassis as `assets/bots/<name>/` containing
//! `chassis.json` and two or more pose directories, each with a `scene.gltf`.
//! It filters on `p.is_dir() && p.join("scene.gltf").is_file()`, sorts the
//! pose names, and treats the first as the vehicle and the last as the robot.
//!
//! So the strip runs VEHICLE -> ROBOT and the names have to sort that way.
//! Five to seven frames: `main.rs` notes that every pose is a real model and
//! gets its own SDF bake, so poses cost startup time, and SDF_BUDGET is 126
//! for the whole roster.

use crate::bot::Bot;
use crate::rig::{solve, Joint, Pose};
use std::io::Write;
use std::path::Path;

/// A posed frame's geometry, ready to write.
pub struct Frame {
    pub pos: Vec<[f32; 3]>,
    pub idx: Vec<u32>,
}

/// Place every plate for one pose. The skeleton's own volumes are cheap
/// boxes: the engine bakes an SDF from this, and an SDF cares about occupied
/// space rather than about how prettily it is tessellated.
pub fn frame(bot: &Bot, pose: &Pose) -> Frame {
    let js = solve(&bot.bones, pose);
    let mut pos = Vec::new();
    let mut idx = Vec::new();

    for (i, b) in bot.bones.iter().enumerate() {
        if b.len <= 0.0 { continue; }
        limb(&js[i], b.radius, &mut pos, &mut idx);
    }
    for p in &bot.plates {
        let j = &js[p.bone];
        plate(j, p.pos, p.normal, p.size, &mut pos, &mut idx);
    }
    Frame { pos, idx }
}

fn push_box(c: [f32; 3], ax: [[f32; 3]; 3], half: [f32; 3],
            pos: &mut Vec<[f32; 3]>, idx: &mut Vec<u32>) {
    let base = pos.len() as u32;
    for s in 0..8 {
        let sx = if s & 1 == 0 { -half[0] } else { half[0] };
        let sy = if s & 2 == 0 { -half[1] } else { half[1] };
        let sz = if s & 4 == 0 { -half[2] } else { half[2] };
        pos.push([c[0] + ax[0][0]*sx + ax[1][0]*sy + ax[2][0]*sz,
                  c[1] + ax[0][1]*sx + ax[1][1]*sy + ax[2][1]*sz,
                  c[2] + ax[0][2]*sx + ax[1][2]*sy + ax[2][2]*sz]);
    }
    // Winding chosen so the signed volume comes out POSITIVE -- normals
    // outward. The first version was inside-out on every box: with backface
    // culling the bot renders hollow, and an SDF bake of an inverted hull
    // fills the world instead of the model. A closed mesh with the wrong
    // winding passes every structural check there is, which is why this needs
    // its own test.
    const F: [[u32; 4]; 6] = [[0,2,3,1],[4,5,7,6],[0,1,5,4],
                              [2,6,7,3],[0,4,6,2],[1,3,7,5]];
    for f in F {
        idx.extend_from_slice(&[base+f[0], base+f[1], base+f[2],
                                base+f[0], base+f[2], base+f[3]]);
    }
}

fn frame_from(dir: [f32; 3]) -> [[f32; 3]; 3] {
    let l = (dir[0]*dir[0] + dir[1]*dir[1] + dir[2]*dir[2]).sqrt().max(1e-6);
    let n = [dir[0]/l, dir[1]/l, dir[2]/l];
    let k = (0..3).min_by(|&a, &b| n[a].abs().partial_cmp(&n[b].abs()).unwrap()).unwrap();
    let mut t = [0.0f32; 3];
    t[k] = 1.0;
    let d = t[0]*n[0] + t[1]*n[1] + t[2]*n[2];
    let mut a = [t[0]-n[0]*d, t[1]-n[1]*d, t[2]-n[2]*d];
    let al = (a[0]*a[0]+a[1]*a[1]+a[2]*a[2]).sqrt().max(1e-6);
    for v in a.iter_mut() { *v /= al; }
    let b = [n[1]*a[2]-n[2]*a[1], n[2]*a[0]-n[0]*a[2], n[0]*a[1]-n[1]*a[0]];
    [a, b, n]
}

fn limb(j: &Joint, r: f32, pos: &mut Vec<[f32; 3]>, idx: &mut Vec<u32>) {
    let d = [j.tail[0]-j.head[0], j.tail[1]-j.head[1], j.tail[2]-j.head[2]];
    let len = (d[0]*d[0]+d[1]*d[1]+d[2]*d[2]).sqrt();
    if len < 1e-4 { return; }
    let ax = frame_from(d);
    let c = [(j.head[0]+j.tail[0])*0.5, (j.head[1]+j.tail[1])*0.5,
             (j.head[2]+j.tail[2])*0.5];
    push_box(c, ax, [r, r, len * 0.5], pos, idx);
}

fn plate(j: &Joint, local: [f32; 3], normal: [f32; 3], size: [f32; 3],
         pos: &mut Vec<[f32; 3]>, idx: &mut Vec<u32>) {
    let m = &j.mat;
    let w = |v: [f32; 3]| [
        m[0][0]*v[0] + m[0][1]*v[1] + m[0][2]*v[2] + j.head[0],
        m[1][0]*v[0] + m[1][1]*v[1] + m[1][2]*v[2] + j.head[1],
        m[2][0]*v[0] + m[2][1]*v[1] + m[2][2]*v[2] + j.head[2]];
    let n = [m[0][0]*normal[0] + m[0][1]*normal[1] + m[0][2]*normal[2],
             m[1][0]*normal[0] + m[1][1]*normal[1] + m[1][2]*normal[2],
             m[2][0]*normal[0] + m[2][1]*normal[1] + m[2][2]*normal[2]];
    let ax = frame_from(n);
    push_box(w(local), ax, [size[0]*0.5, size[1]*0.5, size[2]], pos, idx);
}

/// Write one pose directory: scene.gltf, scene.bin, orient.json.
///
/// glTF with an external .bin, which is what the loader reads when there is
/// no `<dir>.glb` or `slim.glb`. `orient.json` states which way the model
/// stands: model.rs searches for an orientation when it is absent, and a
/// generated model knows, so saying so skips the search and removes its
/// failure mode.
pub fn write_pose(dir: &Path, f: &Frame) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut bin: Vec<u8> = Vec::with_capacity(f.pos.len() * 12 + f.idx.len() * 4);
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for p in &f.pos {
        for a in 0..3 { lo[a] = lo[a].min(p[a]); hi[a] = hi[a].max(p[a]); }
        for a in 0..3 { bin.extend_from_slice(&p[a].to_le_bytes()); }
    }
    let idx_off = bin.len();
    for i in &f.idx { bin.extend_from_slice(&i.to_le_bytes()); }

    // POSITION needs min/max per the spec, and a loader that trusts them will
    // cull the mesh wrongly if they are absent or wrong.
    let js = format!(concat!(
        r#"{{"asset":{{"version":"2.0","generator":"boneforge"}},"#,
        r#""scene":0,"scenes":[{{"nodes":[0]}}],"#,
        r#""nodes":[{{"mesh":0}}],"#,
        r#""meshes":[{{"primitives":[{{"attributes":{{"POSITION":0}},"indices":1}}]}}],"#,
        r#""accessors":[{{"bufferView":0,"componentType":5126,"count":{nv},"type":"VEC3","#,
        r#""min":[{l0},{l1},{l2}],"max":[{h0},{h1},{h2}]}},"#,
        r#"{{"bufferView":1,"componentType":5125,"count":{ni},"type":"SCALAR"}}],"#,
        r#""bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":{vb}}},"#,
        r#"{{"buffer":0,"byteOffset":{io},"byteLength":{ib}}}],"#,
        r#""buffers":[{{"byteLength":{tb},"uri":"scene.bin"}}]}}"#),
        nv = f.pos.len(), ni = f.idx.len(),
        l0 = lo[0], l1 = lo[1], l2 = lo[2],
        h0 = hi[0], h1 = hi[1], h2 = hi[2],
        vb = idx_off, io = idx_off, ib = bin.len() - idx_off, tb = bin.len());

    std::fs::File::create(dir.join("scene.gltf"))?.write_all(js.as_bytes())?;
    std::fs::File::create(dir.join("scene.bin"))?.write_all(&bin)?;
    std::fs::File::create(dir.join("orient.json"))?.write_all(
        br#"{"up":[0.0,1.0,0.0],"forward":[-1.0,0.0,0.0],"source":"boneforge"}"#)?;
    Ok(())
}

/// Write a whole chassis: the fold strip plus its manifest.
pub fn write_chassis(root: &Path, name: &str, bot: &Bot, poses: &[Pose],
                     source: &str) -> std::io::Result<usize> {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir)?;
    for (k, p) in poses.iter().enumerate() {
        write_pose(&dir.join(format!("f{k:02}")), &frame(bot, p))?;
    }
    // `scale` and `boss` are the fields the engine already reads per chassis.
    // `scale` is NOT 1.0. The engine normalises every model's longest extent
    // to 1.32, so a scale of 1 makes a bot from a truck and one from a
    // hatchback exactly the same size in game. The wheels say what a file
    // unit is worth, so the bot's real height is known and the scale can undo
    // the normalisation.
    let manifest = format!(
        concat!(r#"{{"scale": {s:.3}, "boss": false, "source": "boneforge:{src}", "#,
                r#""height_m": {m}, "units_per_m": {u}}}"#),
        s = bot.chassis_scale(), src = source,
        m = bot.height_m().map(|v| format!("{v:.2}")).unwrap_or("null".into()),
        u = bot.units_per_m.map(|v| format!("{v:.2}")).unwrap_or("null".into()));
    std::fs::File::create(dir.join("chassis.json"))?.write_all(manifest.as_bytes())?;
    Ok(poses.len())
}
