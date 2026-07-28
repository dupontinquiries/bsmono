//! cad-forge — the `.cadf` document: CADscope history as a **forge-pattern** op script, per
//! games/EDITORS.md's reference design (soldier `buildscript.rs`): *names not IDs, ops as a
//! sequence not a snapshot, per-op error tolerance (one typo never voids the script), and an
//! export that round-trips* — so a human, the GUI, an LLM shown the schema, or a collaborator's
//! replica can all read → propose → hand back a full document.
//!
//! v0 vocabulary is deliberately small (M1 grows it: sketches, patterns, spine): add a primitive,
//! transform a named part, combine two named parts with a boolean/blend, recolor, remove. `apply`
//! folds ops into a `cad_kernel::Model`, collecting warnings instead of failing: an op that
//! references a missing name, or carries an out-of-range parameter, is skipped *and reported*.

use cad_kernel::{Model, Part, Sdf};
use glam::{Quat, Vec3};
use serde::{Deserialize, Serialize};

/// Primitive vocabulary, serde-shaped for humans and LLMs (the sanitization boundary in
/// cad-liaison speaks exactly this). All lengths in mm.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ShapeSpec {
    Sphere { r: f32 },
    Cuboid { half: [f32; 3] },
    Cylinder { r: f32, hh: f32 },
    Capsule { r: f32, hh: f32 },
    Torus { major: f32, minor: f32 },
}

impl ShapeSpec {
    fn to_sdf(&self) -> Sdf {
        match *self {
            ShapeSpec::Sphere { r } => cad_kernel::sdf::sphere(r),
            ShapeSpec::Cuboid { half } => cad_kernel::sdf::cuboid(Vec3::from(half)),
            ShapeSpec::Cylinder { r, hh } => cad_kernel::sdf::cylinder(r, hh),
            ShapeSpec::Capsule { r, hh } => cad_kernel::sdf::capsule(r, hh),
            ShapeSpec::Torus { major, minor } => cad_kernel::sdf::torus(major, minor),
        }
    }
    /// The clamp half of the sanitization boundary: dimensions must be finite, positive, and sane.
    fn valid(&self) -> bool {
        let ok = |v: f32| v.is_finite() && v > 0.0 && v < 1.0e5;
        match *self {
            ShapeSpec::Sphere { r } => ok(r),
            ShapeSpec::Cuboid { half } => half.iter().all(|&v| ok(v)),
            ShapeSpec::Cylinder { r, hh } | ShapeSpec::Capsule { r, hh } => ok(r) && ok(hh),
            ShapeSpec::Torus { major, minor } => ok(major) && ok(minor) && minor < major,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// Create a named part from a primitive at a position, with an optional axis-angle rotation.
    Add {
        name: String,
        shape: ShapeSpec,
        #[serde(default)]
        at: [f32; 3],
        /// axis (normalized at apply) + angle in degrees; identity when omitted.
        #[serde(default)]
        rot_axis_deg: Option<([f32; 3], f32)>,
        #[serde(default = "default_color")]
        color: [f32; 4],
    },
    /// Move an existing part by a delta.
    Nudge { name: String, by: [f32; 3] },
    /// Recolor an existing part.
    Color { name: String, color: [f32; 4] },
    /// Combine: `into` becomes `into ⊕ other`; `other` is consumed. `k > 0` selects Blend.
    Combine {
        into: String,
        other: String,
        #[serde(default)]
        mode: BoolMode,
        #[serde(default)]
        k: f32,
    },
    /// Hollow a part: keep a wall of thickness `t` around its surface.
    Shell { name: String, t: f32 },
    /// Repeat a part `count` times along `step`, unioned into one solid. The canonical CAD
    /// linear pattern; `count` includes the original.
    Pattern { name: String, count: u32, step: [f32; 3] },
    /// Remove a part by name.
    Remove { name: String },
}

fn default_color() -> [f32; 4] { [0.78, 0.80, 0.84, 1.0] }

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BoolMode {
    #[default]
    Union,
    Intersect,
    Subtract,
}

/// The document. `name` titles the model; ops replay in order.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Doc {
    pub name: String,
    pub ops: Vec<Op>,
}

/// One skipped op, with the reason — surfaced in the history panel, never a crash.
#[derive(Clone, Debug, PartialEq)]
pub struct OpWarning {
    pub index: usize,
    pub reason: String,
}

impl Doc {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("Doc is always serializable")
    }
    pub fn from_json(s: &str) -> Result<Doc, String> {
        serde_json::from_str(s).map_err(|e| e.to_string())
    }

    /// Replay the ops into a Model. Warnings are per-op and non-fatal, per the forge pattern.
    pub fn apply(&self) -> (Model, Vec<OpWarning>) {
        let mut model = Model::default();
        let mut warns = Vec::new();
        let mut next_id = 1u32;
        let warn = |warns: &mut Vec<OpWarning>, i: usize, r: String| warns.push(OpWarning { index: i, reason: r });

        for (i, op) in self.ops.iter().enumerate() {
            match op {
                Op::Add { name, shape, at, rot_axis_deg, color } => {
                    if model.parts.iter().any(|p| &p.name == name) {
                        warn(&mut warns, i, format!("add: name '{name}' already exists"));
                        continue;
                    }
                    if !shape.valid() {
                        warn(&mut warns, i, format!("add '{name}': shape dimensions out of range"));
                        continue;
                    }
                    let mut sdf = shape.to_sdf();
                    if let Some((axis, deg)) = rot_axis_deg {
                        let axis = Vec3::from(*axis);
                        if axis.length_squared() > 1e-8 && deg.is_finite() {
                            sdf = cad_kernel::sdf::rotate(Quat::from_axis_angle(axis.normalize(), deg.to_radians()), sdf);
                        } else {
                            warn(&mut warns, i, format!("add '{name}': degenerate rotation ignored"));
                        }
                    }
                    let at = Vec3::from(*at);
                    if at.length_squared() > 0.0 {
                        sdf = cad_kernel::sdf::translate(at, sdf);
                    }
                    model.parts.push(Part { id: next_id, name: name.clone(), sdf, color: *color });
                    next_id += 1;
                }
                Op::Nudge { name, by } => match model.parts.iter_mut().find(|p| &p.name == name) {
                    Some(p) => p.sdf = cad_kernel::sdf::translate(Vec3::from(*by), p.sdf.clone()),
                    None => warn(&mut warns, i, format!("nudge: no part named '{name}'")),
                },
                Op::Color { name, color } => match model.parts.iter_mut().find(|p| &p.name == name) {
                    Some(p) => p.color = *color,
                    None => warn(&mut warns, i, format!("color: no part named '{name}'")),
                },
                Op::Combine { into, other, mode, k } => {
                    let oi = model.parts.iter().position(|p| &p.name == other);
                    let ii = model.parts.iter().position(|p| &p.name == into);
                    match (ii, oi) {
                        (Some(ii), Some(oi)) if ii != oi => {
                            let other_part = model.parts.remove(oi);
                            let ii = if oi < ii { ii - 1 } else { ii };
                            let a = model.parts[ii].sdf.clone();
                            let b = other_part.sdf;
                            model.parts[ii].sdf = if *k > 0.0 && *mode == BoolMode::Union {
                                cad_kernel::sdf::blend(a, b, *k)
                            } else {
                                match mode {
                                    BoolMode::Union => cad_kernel::sdf::union(a, b),
                                    BoolMode::Intersect => cad_kernel::sdf::intersect(a, b),
                                    BoolMode::Subtract => cad_kernel::sdf::subtract(a, b),
                                }
                            };
                        }
                        (Some(_), Some(_)) => warn(&mut warns, i, format!("combine: '{into}' with itself")),
                        _ => warn(&mut warns, i, format!("combine: missing '{into}' or '{other}'")),
                    }
                }
                Op::Shell { name, t } => match model.parts.iter_mut().find(|p| &p.name == name) {
                    Some(p) if t.is_finite() && *t > 0.0 => {
                        p.sdf = cad_kernel::sdf::shell(*t, p.sdf.clone())
                    }
                    Some(_) => warn(&mut warns, i, format!("shell '{name}': thickness must be > 0")),
                    None => warn(&mut warns, i, format!("shell: no part named '{name}'")),
                },
                Op::Pattern { name, count, step } => {
                    let step_v = Vec3::from(*step);
                    let ok = (2..=512).contains(count) && step_v.length_squared() > 1e-9;
                    match model.parts.iter_mut().find(|p| &p.name == name) {
                        Some(p) if ok => {
                            let base = p.sdf.clone();
                            let mut acc = base.clone();
                            for n in 1..*count {
                                acc = cad_kernel::sdf::union(
                                    acc,
                                    cad_kernel::sdf::translate(step_v * n as f32, base.clone()),
                                );
                            }
                            p.sdf = acc;
                        }
                        Some(_) => warn(
                            &mut warns,
                            i,
                            format!("pattern '{name}': count must be 2..=512 and step non-zero"),
                        ),
                        None => warn(&mut warns, i, format!("pattern: no part named '{name}'")),
                    }
                }
                Op::Remove { name } => {
                    let before = model.parts.len();
                    model.parts.retain(|p| &p.name != name);
                    if model.parts.len() == before {
                        warn(&mut warns, i, format!("remove: no part named '{name}'"));
                    }
                }
            }
        }
        (model, warns)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo() -> Doc {
        Doc {
            name: "demo".into(),
            ops: vec![
                Op::Add { name: "base".into(), shape: ShapeSpec::Cuboid { half: [4.0, 1.0, 3.0] }, at: [0.0, 1.0, 0.0], rot_axis_deg: None, color: default_color() },
                Op::Add { name: "boss".into(), shape: ShapeSpec::Cylinder { r: 1.0, hh: 2.0 }, at: [0.0, 2.0, 0.0], rot_axis_deg: None, color: default_color() },
                Op::Combine { into: "base".into(), other: "boss".into(), mode: BoolMode::Union, k: 0.4 },
            ],
        }
    }

    #[test]
    fn round_trips_bytes_stable() {
        let d = demo();
        let j = d.to_json();
        let d2 = Doc::from_json(&j).unwrap();
        assert_eq!(d, d2);
        assert_eq!(j, d2.to_json(), "export must round-trip byte-identically");
    }

    #[test]
    fn applies_to_one_blended_part() {
        let (m, warns) = demo().apply();
        assert!(warns.is_empty(), "{warns:?}");
        assert_eq!(m.parts.len(), 1);
        // The blended boss reads as solid a bit above the slab top.
        assert!(m.parts[0].sdf.eval(glam::Vec3::new(0.0, 2.5, 0.0)) < 0.0);
    }

    #[test]
    fn one_typo_never_voids_the_script() {
        let mut d = demo();
        d.ops.insert(1, Op::Nudge { name: "bsae".into(), by: [1.0, 0.0, 0.0] }); // typo'd name
        d.ops.push(Op::Add { name: "bad".into(), shape: ShapeSpec::Sphere { r: -3.0 }, at: [0.0; 3], rot_axis_deg: None, color: default_color() });
        let (m, warns) = d.apply();
        assert_eq!(warns.len(), 2, "{warns:?}");
        assert_eq!(m.parts.len(), 1, "good ops still applied");
    }

    #[test]
    fn shell_hollows_and_pattern_repeats() {
        let d = Doc {
            name: "m1-ops".into(),
            ops: vec![
                Op::Add { name: "cup".into(), shape: ShapeSpec::Sphere { r: 3.0 }, at: [0.0; 3], rot_axis_deg: None, color: default_color() },
                Op::Shell { name: "cup".into(), t: 0.2 },
                Op::Add { name: "stud".into(), shape: ShapeSpec::Cylinder { r: 0.4, hh: 0.5 }, at: [20.0, 0.0, 0.0], rot_axis_deg: None, color: default_color() },
                Op::Pattern { name: "stud".into(), count: 4, step: [2.0, 0.0, 0.0] },
            ],
        };
        let (m, warns) = d.apply();
        assert!(warns.is_empty(), "{warns:?}");
        let cup = m.parts.iter().find(|p| p.name == "cup").unwrap();
        assert!(cup.sdf.eval(glam::Vec3::ZERO) > 0.0, "shelled sphere is hollow at its centre");
        assert!(cup.sdf.eval(glam::Vec3::new(3.0, 0.0, 0.0)) < 0.0, "the wall itself is solid");
        let stud = m.parts.iter().find(|p| p.name == "stud").unwrap();
        for n in 0..4 {
            let x = 20.0 + 2.0 * n as f32;
            assert!(stud.sdf.eval(glam::Vec3::new(x, 0.0, 0.0)) < 0.0, "copy {n} missing");
        }
        assert!(stud.sdf.eval(glam::Vec3::new(29.0, 0.0, 0.0)) > 0.0, "pattern must not run on");
    }

    #[test]
    fn bad_pattern_is_reported_not_fatal() {
        let d = Doc {
            name: "guard".into(),
            ops: vec![
                Op::Add { name: "a".into(), shape: ShapeSpec::Sphere { r: 1.0 }, at: [0.0; 3], rot_axis_deg: None, color: default_color() },
                Op::Pattern { name: "a".into(), count: 1, step: [0.0, 0.0, 0.0] },
            ],
        };
        let (m, warns) = d.apply();
        assert_eq!(warns.len(), 1);
        assert_eq!(m.parts.len(), 1);
    }

    #[test]
    fn subtract_consumes_the_tool_part() {
        let d = Doc {
            name: "drilled".into(),
            ops: vec![
                Op::Add { name: "block".into(), shape: ShapeSpec::Cuboid { half: [2.0, 2.0, 2.0] }, at: [0.0; 3], rot_axis_deg: None, color: default_color() },
                Op::Add { name: "drill".into(), shape: ShapeSpec::Cylinder { r: 0.5, hh: 3.0 }, at: [0.0; 3], rot_axis_deg: None, color: default_color() },
                Op::Combine { into: "block".into(), other: "drill".into(), mode: BoolMode::Subtract, k: 0.0 },
            ],
        };
        let (m, warns) = d.apply();
        assert!(warns.is_empty());
        assert_eq!(m.parts.len(), 1);
        assert!(m.parts[0].sdf.eval(glam::Vec3::ZERO) > 0.0, "hole is open at the origin");
    }
}
