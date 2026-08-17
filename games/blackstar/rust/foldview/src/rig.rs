//! Assembly + joint web. Mirrors the identity map in content/bots.json
//! (`transform_map_reference`): every part has two lives. t=0 robot, t=1 vehicle.
//!
//! All motion here is rigid — rotation, translation, nothing else — which is
//! what lets model_verify prove each frame lossless (TRANSFORM_OVERHAUL §1, §5).

use crate::iqm;
use anyhow::{Context, Result};
use model_verify::motion::{self, Joint, Profile, Slot};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct Chassis {
    pub fold_spin: f64, // degrees — the big wheel-arm spin; per-chassis character
    pub xform_dur: f64, // seconds
    #[serde(default = "one")] pub arm: f64,
    #[serde(default = "one")] pub bulk: f64,
    #[serde(default = "one")] pub leg: f64,
    #[serde(default = "one")] pub head: f64,
    #[serde(default = "one")] pub shoulder: f64,
}
fn one() -> f64 { 1.0 }

#[derive(Deserialize)]
struct ChassisFile { chassis: BTreeMap<String, serde_json::Value> }

pub fn load_chassis(content_dir: &Path) -> Result<BTreeMap<String, Chassis>> {
    let raw: ChassisFile = serde_json::from_str(
        &std::fs::read_to_string(content_dir.join("chassis.json")).context("chassis.json")?,
    )?;
    let mut out = BTreeMap::new();
    for (k, v) in raw.chassis {
        if let Ok(c) = serde_json::from_value::<Chassis>(v) {
            out.insert(k, c); // skips _doc and other non-chassis keys
        }
    }
    Ok(out)
}

/// One placed part instance: mesh + rest transform + joint index + tint.
pub struct PartInst {
    pub name: &'static str,
    pub rest: Vec<[f64; 3]>, // model-space points at t=0 (robot stance)
    pub nrm: Vec<[f64; 3]>,
    pub idx: Vec<u32>,
    pub joint: usize,
    pub tint: [f32; 3],
}

pub struct Bot {
    pub parts: Vec<PartInst>,
    pub joints: Vec<Joint>,
    pub slots: Vec<Slot>,
    pub profile: Profile,
    pub duration: f64,
    pub label: String,
}

fn place(mesh: &iqm::Mesh, scale: [f64; 3], rot_y: f64, at: [f64; 3]) -> (Vec<[f64; 3]>, Vec<[f64; 3]>) {
    let c = iqm::centroid(&mesh.pos);
    let (s, co) = (rot_y.sin(), rot_y.cos());
    let mut pos = Vec::with_capacity(mesh.pos.len());
    let mut nrm = Vec::with_capacity(mesh.nrm.len());
    for (p, n) in mesh.pos.iter().zip(&mesh.nrm) {
        let q = [(p[0] - c[0]) * scale[0], (p[1] - c[1]) * scale[1], (p[2] - c[2]) * scale[2]];
        pos.push([co * q[0] + s * q[2] + at[0], q[1] + at[1], -s * q[0] + co * q[2] + at[2]]);
        nrm.push([co * n[0] + s * n[2], n[1], -s * n[0] + co * n[2]]);
    }
    (pos, nrm)
}

fn part_mesh(data: &Path, stem: &str, variant: &str) -> Result<iqm::Mesh> {
    let with_variant = data.join(format!("{stem}_{variant}.iqm"));
    let base = data.join(format!("{stem}.iqm"));
    let path = if with_variant.exists() { with_variant } else { base };
    let m = iqm::load(&path)?;
    // p-parts are emitted as a single mesh; if split, merge for the viewer
    let mut merged = iqm::Mesh { name: stem.into(), pos: vec![], nrm: vec![], idx: vec![] };
    for mm in m.meshes {
        let b = merged.pos.len() as u32;
        merged.pos.extend(mm.pos);
        merged.nrm.extend(mm.nrm);
        merged.idx.extend(mm.idx.iter().map(|&i| i + b));
    }
    Ok(merged)
}

fn tint_for(variant: &str, part: &str) -> [f32; 3] {
    let base = match variant {
        "watch" => [0.86, 0.80, 0.62],   // bone + old gold — the 12+1
        "corsair" => [0.42, 0.13, 0.13], // oxblood
        _ => [0.30, 0.33, 0.38],         // gunmetal
    };
    let k = match part {
        "phead" => 1.15,
        "phand" | "wheel" => 0.55,
        "ppauldron" => 0.9,
        _ => 1.0,
    };
    [base[0] * k, base[1] * k, base[2] * k]
}

/// Build the bot: humanoid rest stance from chassis proportions, and the joint
/// web that folds it into the vehicle (chest→hull wedge, head→cab, pauldrons→
/// fenders, arms→front wheels, thighs→rear wheels, shins→exhausts).
pub fn build(data: &Path, content: &Path, variant: &str, chassis_name: &str, profile: Profile) -> Result<Bot> {
    let all = load_chassis(content)?;
    let ch = all
        .get(chassis_name)
        .with_context(|| format!("chassis '{chassis_name}' (have: {:?})", all.keys().collect::<Vec<_>>()))?
        .clone();

    let chest = part_mesh(data, "pchest", variant)?;
    let head = part_mesh(data, "phead", variant)?;
    let pauld = part_mesh(data, "ppauldron", variant)?;
    let hand = part_mesh(data, "phand", variant)?;
    let limb = part_mesh(data, "plimb", variant)?;

    let (clo, chi) = iqm::bbox(&chest.pos);
    let ch_h = (chi[1] - clo[1]).max(1e-6) * ch.bulk; // everything scales off chest height
    let shoulder_x = (chi[0] - clo[0]).max(1e-6) * 0.62 * ch.bulk * ch.shoulder;
    let hip_x = shoulder_x * 0.55;
    let arm_l = ch_h * 0.95 * ch.arm;
    let leg_l = ch_h * 1.05 * ch.leg;
    let spin = ch.fold_spin.to_radians();

    let mut parts: Vec<PartInst> = Vec::new();
    let mut joints: Vec<Joint> = Vec::new();
    let mut add = |name: &'static str, mesh: &iqm::Mesh, scale: [f64; 3], rot_y: f64, at: [f64; 3],
                   parent: Option<usize>, pivot: [f64; 3], axis: [f64; 3], angle: f64, slide: f64,
                   variant: &str, tintkey: &str| -> usize {
        let (rest, nrm) = place(mesh, scale, rot_y, at);
        let j = joints.len();
        joints.push(Joint { part: j, parent, pivot, axis, angle, slide, reclip: None });
        parts.push(PartInst { name, rest, nrm, idx: mesh.idx.clone(), joint: j, tint: tint_for(variant, tintkey) });
        j
    };

    // chest — anchored; pitches into the hull wedge
    let chest_j = add("chest", &chest, [ch.bulk; 3], 0.0, [0.0, leg_l + ch_h * 0.5, 0.0],
        None, [0.0, leg_l, 0.0], [1.0, 0.0, 0.0], 0.32, 0.0, variant, "pchest");

    // head — folds back into the cab
    add("head", &head, [ch.head; 3], 0.0, [0.0, leg_l + ch_h * 1.18, 0.0],
        Some(chest_j), [0.0, leg_l + ch_h * 0.98, 0.0], [1.0, 0.0, 0.0], -1.75, -ch_h * 0.18, variant, "phead");

    for side in [-1.0f64, 1.0] {
        let sx = side * shoulder_x;
        // pauldron — swings out and down: fender
        let pj = add(if side < 0.0 { "pauldron_l" } else { "pauldron_r" }, &pauld,
            [ch.shoulder; 3], 0.0, [sx, leg_l + ch_h * 0.88, 0.0],
            Some(chest_j), [sx * 0.8, leg_l + ch_h * 0.92, 0.0], [0.0, 0.0, side], 1.35, side * ch_h * 0.10,
            variant, "ppauldron");
        // upper arm — the signature spin down to front-wheel position
        let aj = add(if side < 0.0 { "arm_l" } else { "arm_r" }, &limb,
            [0.8 * ch.arm, arm_l / ch_h, 0.8 * ch.arm], 0.0, [sx, leg_l + ch_h * 0.45, 0.0],
            Some(pj), [sx, leg_l + ch_h * 0.85, 0.0], [1.0, 0.0, 0.0], spin, 0.0, variant, "plimb");
        // hand — tucks flush: hull nub
        add(if side < 0.0 { "hand_l" } else { "hand_r" }, &hand, [ch.arm; 3], 0.0,
            [sx, leg_l - ch_h * 0.05, 0.0],
            Some(aj), [sx, leg_l + ch_h * 0.02, 0.0], [0.0, 1.0, 0.0], side * 1.57, -ch_h * 0.12, variant, "phand");
        // thigh — sweeps back: rear wheel
        let tj = add(if side < 0.0 { "thigh_l" } else { "thigh_r" }, &limb,
            [0.9 * ch.leg, (leg_l * 0.55) / ch_h, 0.9 * ch.leg], 0.0, [side * hip_x, leg_l * 0.72, 0.0],
            Some(chest_j), [side * hip_x, leg_l * 0.98, 0.0], [1.0, 0.0, 0.0], -spin * 0.8, 0.0, variant, "plimb");
        // shin — folds flat along the thigh: exhaust
        add(if side < 0.0 { "shin_l" } else { "shin_r" }, &limb,
            [0.75 * ch.leg, (leg_l * 0.5) / ch_h, 0.75 * ch.leg], 0.0, [side * hip_x, leg_l * 0.25, 0.02],
            Some(tj), [side * hip_x, leg_l * 0.5, 0.0], [1.0, 0.0, 0.0], 2.6, side * ch_h * 0.06, variant, "plimb");
    }

    let slots = motion::schedule(&joints, 0.55);
    Ok(Bot {
        parts,
        joints,
        slots,
        profile,
        duration: ch.xform_dur,
        label: format!("{variant}/{chassis_name}  spin {:.0}\u{b0}  {:.2}s", ch.fold_spin, ch.xform_dur),
    })
}

impl Bot {
    /// All points of part `i` at fold time `t` (0 robot .. 1 vehicle). Rigid by
    /// construction — pose_at composes rotations+translations only.
    pub fn points_at(&self, i: usize, t: f64) -> Vec<[f64; 3]> {
        let p = &self.parts[i];
        p.rest
            .iter()
            .map(|&q| motion::pose_at(&self.joints, &self.slots, p.joint, t, self.profile, q))
            .collect()
    }
}

pub fn profile_for(chassis: &str) -> Profile {
    match chassis {
        "angular" => Profile::Anger,
        "raptor" | "strider" | "sentinel" => Profile::Combat,
        _ => Profile::Fluid,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use model_verify::verify_pair;
    use std::path::PathBuf;

    fn dirs() -> (PathBuf, PathBuf) {
        // FV_DATA overrides, because this trunk keeps 21 example models and
        // 1,012 voxel representatives where the part library used to be. The
        // rig tests need the part IQMs themselves, so point them at a corpus
        // checkout when there is one.
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        match std::env::var_os("FV_DATA") {
            Some(d) => (PathBuf::from(d), root.join("content")),
            None => (root.join("engine_src/data"), root.join("content")),
        }
    }

    /// The part library is not in this tree. Skipping is honest; passing
    /// would not be, and failing says nothing about the code.
    fn corpus_or_skip() -> Option<(PathBuf, PathBuf)> {
        let (d, c) = dirs();
        if d.join("ppauldron.iqm").is_file() {
            return Some((d, c));
        }
        eprintln!("SKIP: no part library at {} (set FV_DATA to a corpus checkout)", d.display());
        None
    }

    #[test]
    fn rest_pose_is_identity() {
        let Some((d, c)) = corpus_or_skip() else { return };
        let bot = build(&d, &c, "watch", "standard", Profile::Fluid).unwrap();
        for i in 0..bot.parts.len() {
            let now = bot.points_at(i, 0.0);
            for (a, b) in now.iter().zip(&bot.parts[i].rest) {
                for k in 0..3 {
                    assert!((a[k] - b[k]).abs() < 1e-9, "part {i} moved at t=0");
                }
            }
        }
    }

    #[test]
    fn every_frame_is_a_rigid_motion_of_every_part() {
        // The §1 fix, proven with the repo's own verifier: no scale, no crimp,
        // no stretch, correct winding — at rest, mid-fold, and arrival.
        let Some((d, c)) = corpus_or_skip() else { return };
        for chassis in ["standard", "raptor", "juggernaut"] {
            let bot = build(&d, &c, "watch", chassis, profile_for(chassis)).unwrap();
            for t in [0.0, 0.31, 0.63, 0.9, 1.0] {
                for i in 0..bot.parts.len() {
                    let v = verify_pair(&bot.parts[i].rest, &bot.points_at(i, t))
                        .expect("kabsch should recover a rigid transform");
                    assert!(v.passed(), "part {} not rigid at t={t} ({chassis}): {v:?}", bot.parts[i].name);
                }
            }
        }
    }

    #[test]
    fn children_never_start_before_parents() {
        let Some((d, c)) = corpus_or_skip() else { return };
        let bot = build(&d, &c, "corsair", "bruiser", Profile::Fluid).unwrap();
        for j in &bot.joints {
            if let Some(p) = j.parent {
                assert!(
                    bot.slots[j.part].start + 1e-9 >= bot.slots[p].start,
                    "joint {} starts before its parent", j.part
                );
            }
        }
    }
}
