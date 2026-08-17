//! glTF export: a bot at any fold time becomes a standard scene.gltf +
//! scene.bin pair (the Sketchfab-style layout skydribble's loader reads).
//! Positions and indices only — skydribble accumulates its own face normals,
//! and ours would just be bytes it ignores. One node + mesh + material per
//! part, base color from the part tint, names preserved for tooling.

use crate::rig::Bot;
use anyhow::Result;
use std::io::Write;
use std::path::Path;

pub fn export(bot: &Bot, t: f64, dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut bin: Vec<u8> = Vec::new();
    let mut views = Vec::new();
    let mut accessors = Vec::new();
    let mut meshes = Vec::new();
    let mut materials = Vec::new();
    let mut nodes = Vec::new();

    for (i, part) in bot.parts.iter().enumerate() {
        let pts = bot.points_at(i, t);
        let (mut mn, mut mx) = ([f32::MAX; 3], [f32::MIN; 3]);
        let pos_off = bin.len();
        for p in &pts {
            for k in 0..3 {
                let v = p[k] as f32;
                mn[k] = mn[k].min(v);
                mx[k] = mx[k].max(v);
                bin.extend_from_slice(&v.to_le_bytes());
            }
        }
        let idx_off = bin.len();
        for &ix in &part.idx {
            bin.extend_from_slice(&ix.to_le_bytes());
        }
        let vb = views.len();
        views.push(serde_json::json!({
            "buffer": 0, "byteOffset": pos_off,
            "byteLength": pts.len() * 12, "target": 34962
        }));
        views.push(serde_json::json!({
            "buffer": 0, "byteOffset": idx_off,
            "byteLength": part.idx.len() * 4, "target": 34963
        }));
        let acc_pos = accessors.len();
        accessors.push(serde_json::json!({
            "bufferView": vb, "componentType": 5126, "count": pts.len(),
            "type": "VEC3", "min": mn, "max": mx
        }));
        let acc_idx = accessors.len();
        accessors.push(serde_json::json!({
            "bufferView": vb + 1, "componentType": 5125,
            "count": part.idx.len(), "type": "SCALAR"
        }));
        materials.push(serde_json::json!({
            "name": part.name,
            "pbrMetallicRoughness": {
                "baseColorFactor": [part.tint[0], part.tint[1], part.tint[2], 1.0],
                "metallicFactor": 0.4, "roughnessFactor": 0.6
            }
        }));
        meshes.push(serde_json::json!({
            "primitives": [{
                "attributes": { "POSITION": acc_pos },
                "indices": acc_idx, "material": i
            }]
        }));
        nodes.push(serde_json::json!({ "mesh": i, "name": part.name }));
    }

    let doc = serde_json::json!({
        "asset": { "version": "2.0", "generator": "foldview" },
        "buffers": [{ "uri": "scene.bin", "byteLength": bin.len() }],
        "bufferViews": views,
        "accessors": accessors,
        "materials": materials,
        "meshes": meshes,
        "nodes": nodes,
        "scenes": [{ "nodes": (0..bot.parts.len()).collect::<Vec<_>>() }],
        "scene": 0
    });

    std::fs::File::create(dir.join("scene.bin"))?.write_all(&bin)?;
    std::fs::File::create(dir.join("scene.gltf"))?
        .write_all(serde_json::to_string(&doc)?.as_bytes())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// POSE STRIPS, for the consumer that already exists.
//
// `blackstar-rs::xform::Morph::build` refuses a strip whose poses disagree
// prim-for-prim and vertex-for-vertex, and falls back to the flipbook the
// morph was written to replace. The 48 chassis shipped in
// `games/blackstar-rs/assets/bots/` are merged per pose, so their prim ORDER
// varies between poses and 47 of 48 are refused.
//
// The fix is to decide the merge ONCE per chassis, from data that does not
// depend on the fold time: a part's name class and its (pose-independent)
// tint. Bucket membership and vertex order are then identical in every pose
// by construction, so the strip is morph-compatible for free.

/// Deterministic, pose-independent merge: part indices grouped by
/// (name class, tint quantized to 1/16), buckets ordered by that key.
fn buckets(bot: &Bot) -> Vec<(String, Vec<usize>)> {
    let mut by_key: std::collections::BTreeMap<String, Vec<usize>> = Default::default();
    for (i, p) in bot.parts.iter().enumerate() {
        let q = |c: f32| (c * 16.0).round() as i32;
        let key = format!("{}|{:02}.{:02}.{:02}", p.name, q(p.tint[0]), q(p.tint[1]), q(p.tint[2]));
        by_key.entry(key).or_default().push(i);
    }
    by_key.into_iter().collect()
}

/// One merged pose: `scene.gltf` + `scene.bin` + a truthful `orient.json`.
fn export_merged(bot: &Bot, t: f64, dir: &Path, groups: &[(String, Vec<usize>)]) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut bin: Vec<u8> = Vec::new();
    let (mut views, mut accessors, mut meshes, mut materials, mut nodes) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());

    for (gi, (_key, members)) in groups.iter().enumerate() {
        let (mut mn, mut mx) = ([f32::MAX; 3], [f32::MIN; 3]);
        let pos_off = bin.len();
        let mut nverts = 0usize;
        let mut idx: Vec<u32> = Vec::new();
        let mut tint = [0f64; 3];
        for &pi in members {
            let pts = bot.points_at(pi, t);
            for p in &pts {
                // foldview canonicalizes nose to +Z; the 424 sidecars already in
                // the tree — and the engine — use nose at -Z. A half turn about Y
                // (x,z -> -x,-z) lands on their convention and, unlike negating Z
                // alone, does not mirror the body.
                let p = [-p[0], p[1], -p[2]];
                for k in 0..3 {
                    let v = p[k] as f32;
                    mn[k] = mn[k].min(v);
                    mx[k] = mx[k].max(v);
                    bin.extend_from_slice(&v.to_le_bytes());
                }
            }
            for &ix in &bot.parts[pi].idx {
                idx.push(ix + nverts as u32);
            }
            nverts += pts.len();
            for k in 0..3 { tint[k] += bot.parts[pi].tint[k] as f64; }
        }
        for k in 0..3 { tint[k] /= members.len() as f64; }
        let idx_off = bin.len();
        for &ix in &idx {
            bin.extend_from_slice(&ix.to_le_bytes());
        }
        let vb = views.len();
        views.push(serde_json::json!({
            "buffer": 0, "byteOffset": pos_off, "byteLength": nverts * 12, "target": 34962
        }));
        views.push(serde_json::json!({
            "buffer": 0, "byteOffset": idx_off, "byteLength": idx.len() * 4, "target": 34963
        }));
        let acc_pos = accessors.len();
        accessors.push(serde_json::json!({
            "bufferView": vb, "componentType": 5126, "count": nverts,
            "type": "VEC3", "min": mn, "max": mx
        }));
        let acc_idx = accessors.len();
        accessors.push(serde_json::json!({
            "bufferView": vb + 1, "componentType": 5125, "count": idx.len(), "type": "SCALAR"
        }));
        materials.push(serde_json::json!({
            "name": format!("mat{gi}"),
            "pbrMetallicRoughness": {
                "baseColorFactor": [tint[0], tint[1], tint[2], 1.0],
                "metallicFactor": 0.4, "roughnessFactor": 0.6
            }
        }));
        meshes.push(serde_json::json!({
            "primitives": [{ "attributes": { "POSITION": acc_pos },
                             "indices": acc_idx, "material": gi }]
        }));
        nodes.push(serde_json::json!({ "mesh": gi, "name": format!("mat{gi}") }));
    }

    let doc = serde_json::json!({
        "asset": { "version": "2.0", "generator": "foldview" },
        "buffers": [{ "uri": "scene.bin", "byteLength": bin.len() }],
        "bufferViews": views, "accessors": accessors,
        "materials": materials, "meshes": meshes, "nodes": nodes,
        "scenes": [{ "nodes": (0..groups.len()).collect::<Vec<_>>() }],
        "scene": 0
    });
    std::fs::File::create(dir.join("scene.bin"))?.write_all(&bin)?;
    std::fs::File::create(dir.join("scene.gltf"))?
        .write_all(serde_json::to_string(&doc)?.as_bytes())?;
    // Measured, not assumed: the shipped strips sit nose at -Z (their mass
    // distribution along Z is the mirror of foldview's canonical +Z output),
    // so their declared forward is truthful and this export matches it after
    // the half turn above. A declared orientation is authoritative in the
    // loader, so this file has to describe the geometry as written.
    std::fs::File::create(dir.join("orient.json"))?.write_all(
        b"{\"up\":[0.0,1.0,0.0],\"forward\":[0.0,0.0,-1.0],\"source\":\"foldview\"}",
    )?;
    Ok(())
}

/// Interior classes: parts that are MEANT not to touch their neighbours, so a
/// gap is the design rather than a fault.
fn is_interior(name: &str) -> bool {
    matches!(name, "guts" | "muscle" | "bone" | "link" | "rib" | "gyro" | "reactor"
        | "vertebra" | "cable" | "piston" | "coil" | "keel" | "collar" | "telescope")
}

/// `joints.json`: the fitted rig as the viewer reads it, with the adjacency
/// diagnostics computed rather than stamped.
fn export_joints(bot: &Bot, path: &Path) -> Result<()> {
    let n = bot.joints.len();
    // point cloud per joint, in the robot pose — that is the stance the
    // parenting is meant to be true in.
    let mut cloud: Vec<Vec<[f64; 3]>> = vec![Vec::new(); n];
    let mut names: Vec<Vec<&str>> = vec![Vec::new(); n];
    let mut interior_votes = vec![(0usize, 0usize); n];
    for (i, p) in bot.parts.iter().enumerate() {
        let j = p.joint.min(n - 1);
        let pts = bot.points_at(i, 1.0);
        let step = (pts.len() / 64).max(1);
        cloud[j].extend(pts.iter().step_by(step).copied());
        if names[j].len() < 4 { names[j].push(p.name); }
        if is_interior(p.name) { interior_votes[j].0 += 1 } else { interior_votes[j].1 += 1 }
    }
    let extent = |c: &Vec<[f64; 3]>| -> f64 {
        if c.is_empty() { return 0.0; }
        let (mut mn, mut mx) = ([f64::MAX; 3], [f64::MIN; 3]);
        for p in c { for k in 0..3 { mn[k] = mn[k].min(p[k]); mx[k] = mx[k].max(p[k]); } }
        ((mx[0] - mn[0]).powi(2) + (mx[1] - mn[1]).powi(2) + (mx[2] - mn[2]).powi(2)).sqrt()
    };
    let gap = |a: &Vec<[f64; 3]>, b: &Vec<[f64; 3]>| -> f64 {
        let mut best = f64::MAX;
        for p in a { for q in b {
            let d = (p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2);
            if d < best { best = d; }
        }}
        if best == f64::MAX { f64::MAX } else { best.sqrt() }
    };

    let mut out = String::from("{\n \"joints\": [\n");
    for (i, j) in bot.joints.iter().enumerate() {
        let parent = j.parent.map(|p| p as i64).unwrap_or(-1);
        let parts = bot.parts.iter().filter(|p| p.joint == i).count();
        // gap to the declared parent, in units of the smaller joined part
        let unit = extent(&cloud[i])
            .min(if parent >= 0 { extent(&cloud[parent as usize]) } else { f64::MAX })
            .max(1e-6);
        let gap_now = if parent >= 0 { gap(&cloud[i], &cloud[parent as usize]) / unit } else { 0.0 };
        // the joint it is actually adjacent to
        let mut best = (f64::MAX, i);
        for k in 0..n {
            if k == i || cloud[k].is_empty() || cloud[i].is_empty() { continue; }
            let u = extent(&cloud[i]).min(extent(&cloud[k])).max(1e-6);
            let g = gap(&cloud[i], &cloud[k]) / u;
            if g < best.0 { best = (g, k); }
        }
        let (gap_best, should) = best;
        let interior = interior_votes[i].0 > interior_votes[i].1;
        let misparented = parent >= 0 && gap_now > 1.0 && gap_best < gap_now * 0.5;
        let nm: Vec<String> = names[i].iter().map(|s| format!("\"{s}\"")).collect();
        out.push_str(&format!(
            "  {{\"i\":{i},\"parent\":{parent},\"parts\":{parts},\
             \"pivot\":[{:.4},{:.4},{:.4}],\"axis\":[{:.3},{:.3},{:.3}],\"angle\":{:.3},\
             \"names\":[{}],\"misparented\":{misparented},\"interior\":{interior},\
             \"should_parent\":{},\"gap_now\":{:.3},\"gap_best\":{:.3}}}{}\n",
            j.pivot[0], j.pivot[1], j.pivot[2], j.axis[0], j.axis[1], j.axis[2], j.angle,
            nm.join(", "),
            if misparented { should.to_string() } else { "null".into() },
            if gap_now.is_finite() { gap_now } else { -1.0 },
            if gap_best.is_finite() { gap_best } else { -1.0 },
            if i + 1 == n { "" } else { "," }
        ));
    }
    out.push_str(" ]\n}\n");
    std::fs::write(path, out)?;
    Ok(())
}

/// A whole chassis as `blackstar-rs` expects to find it: `f00..f04`, a
/// `joints.json`, and a `chassis.json` naming the source model.
pub fn export_strip(bot: &Bot, dir: &Path, source: &str, poses: usize) -> Result<usize> {
    std::fs::create_dir_all(dir)?;
    let groups = buckets(bot);
    for k in 0..poses {
        let t = k as f64 / (poses - 1).max(1) as f64;
        export_merged(bot, t, &dir.join(format!("f{k:02}")), &groups)?;
    }
    export_joints(bot, &dir.join("joints.json"))?;
    std::fs::write(
        dir.join("chassis.json"),
        format!("{{\"scale\": 1.0, \"boss\": false, \"source\": \"{source}\"}}\n"),
    )?;
    Ok(groups.len())
}
