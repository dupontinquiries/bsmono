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

/// One station of a grown spine, in the document's own vocabulary rather than the kernel's — the
/// forge owns what is serialized, so the kernel stays free of serde (and of any obligation to keep
/// its internal types stable across versions).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SpineNode {
    pub at: [f32; 3],
    #[serde(default = "up_default")]
    pub up: [f32; 3],
    pub r: f32,
    /// The grower's 1-5: circle, square, triangle, star, hex.
    #[serde(default = "section_default")]
    pub section: u32,
}

fn up_default() -> [f32; 3] { [0.0, 1.0, 0.0] }
fn section_default() -> u32 { 1 }

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
    /// A grown solid: the stations a Grow-mode session recorded, swept and blended into one part.
    Spine {
        name: String,
        nodes: Vec<SpineNode>,
        #[serde(default)]
        blend: f32,
        #[serde(default)]
        mirror: bool,
        #[serde(default = "default_color")]
        color: [f32; 4],
    },
    /// Scale a part uniformly about its own centre (not the origin, which would move it).
    Scale { name: String, factor: f32 },
    /// Resize a part until its measured volume matches `volume` — the op behind "make it the same
    /// volume as <model>" (plan §17.2). The target is a number the *caller* resolved from the
    /// reference corpus; this op's job is to hit it and to check that it did.
    ///
    /// The check is the point. Volume is measured by meshing the part, the scale factor is the
    /// cube root of the ratio, and the result is re-measured: if it is not within `tol` the op
    /// warns. A claim about geometry is verified on every rebuild, never trusted.
    MatchVolume {
        name: String,
        volume: f32,
        /// Fractional tolerance; 0.02 (2%) when omitted, which is comfortably above the mesher's
        /// own error at the default resolution.
        #[serde(default)]
        tol: f32,
        /// Mesh resolution used for measurement. Higher is slower and more accurate.
        #[serde(default)]
        resolution: u32,
    },
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

/// Scale about the part's own centre. Scaling about the origin would silently translate anything
/// not modelled there, which is never what "make it bigger" means.
fn scale_about_centre(sdf: &cad_kernel::Sdf, factor: f32) -> cad_kernel::Sdf {
    let (lo, hi) = sdf.aabb();
    let c = (lo + hi) * 0.5;
    cad_kernel::sdf::translate(c, cad_kernel::sdf::scale(factor, cad_kernel::sdf::translate(-c, sdf.clone())))
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
                Op::Spine { name, nodes, blend, mirror, color } => {
                    if model.parts.iter().any(|p| &p.name == name) {
                        warn(&mut warns, i, format!("spine: name '{name}' already exists"));
                        continue;
                    }
                    let mut spine = cad_kernel::Spine::new(blend.max(0.0));
                    for n in nodes {
                        spine.nodes.push(cad_kernel::Node {
                            pos: Vec3::from(n.at),
                            up: Vec3::from(n.up),
                            radius: n.r.max(1e-3),
                            section: cad_kernel::Section::from_digit(n.section).unwrap_or_default(),
                        });
                    }
                    match spine.to_sdf() {
                        Some(sdf) => {
                            let sdf = if *mirror {
                                cad_kernel::sdf::blend(
                                    sdf.clone(),
                                    cad_kernel::spine::mirror_x(&sdf),
                                    blend.max(0.01),
                                )
                            } else {
                                sdf
                            };
                            model.parts.push(Part { id: next_id, name: name.clone(), sdf, color: *color });
                            next_id += 1;
                        }
                        None => warn(&mut warns, i, format!("spine '{name}': needs at least two stations")),
                    }
                }
                Op::Scale { name, factor } => match model.parts.iter_mut().find(|p| &p.name == name) {
                    Some(p) if factor.is_finite() && *factor > 1e-4 => {
                        p.sdf = scale_about_centre(&p.sdf, *factor)
                    }
                    Some(_) => warn(&mut warns, i, format!("scale '{name}': factor must be > 0")),
                    None => warn(&mut warns, i, format!("scale: no part named '{name}'")),
                },
                Op::MatchVolume { name, volume, tol, resolution } => {
                    let tol = if *tol > 0.0 { *tol } else { 0.02 };
                    let res = if *resolution >= 8 { *resolution } else { 40 };
                    let target = *volume;
                    match model.parts.iter_mut().find(|p| &p.name == name) {
                        Some(p) if target.is_finite() && target > 0.0 => {
                            let measure = |sdf: &cad_kernel::Sdf| {
                                cad_kernel::dual_contour(sdf, cad_kernel::MeshOpts { resolution: res })
                                    .signed_volume()
                                    .abs()
                            };
                            let current = measure(&p.sdf);
                            if current <= 1e-9 {
                                warn(&mut warns, i, format!("match_volume '{name}': part has no measurable volume"));
                            } else {
                                let factor = (target / current).cbrt();
                                p.sdf = scale_about_centre(&p.sdf, factor);
                                let got = measure(&p.sdf);
                                let err = (got - target).abs() / target;
                                if err > tol {
                                    warn(
                                        &mut warns,
                                        i,
                                        format!(
                                            "match_volume '{name}': wanted {target:.3}, measured {got:.3} ({:.1}% off, tolerance {:.1}%)",
                                            err * 100.0,
                                            tol * 100.0
                                        ),
                                    );
                                }
                            }
                        }
                        Some(_) => warn(&mut warns, i, format!("match_volume '{name}': target volume must be > 0")),
                        None => warn(&mut warns, i, format!("match_volume: no part named '{name}'")),
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
    fn a_spine_op_round_trips_and_builds_a_solid() {
        let nodes: Vec<SpineNode> = (0..8)
            .map(|i| SpineNode {
                at: [0.0, 0.5, i as f32 * 0.3],
                up: [0.0, 1.0, 0.0],
                r: 0.3,
                section: 1,
            })
            .collect();
        let d = Doc {
            name: "grown".into(),
            ops: vec![Op::Spine {
                name: "tendril".into(),
                nodes,
                blend: 0.1,
                mirror: false,
                color: default_color(),
            }],
        };
        assert_eq!(Doc::from_json(&d.to_json()).unwrap(), d, "spine ops must round-trip");
        let (m, warns) = d.apply();
        assert!(warns.is_empty(), "{warns:?}");
        assert_eq!(m.parts.len(), 1);
        // On the centreline it is solid; a long way off the axis it is not.
        assert!(m.parts[0].sdf.eval(glam::Vec3::new(0.0, 0.5, 1.0)) < 0.0);
        assert!(m.parts[0].sdf.eval(glam::Vec3::new(3.0, 0.5, 1.0)) > 0.0);
    }

    #[test]
    fn a_one_station_spine_is_reported_not_fatal() {
        let d = Doc {
            name: "stub".into(),
            ops: vec![
                Op::Spine { name: "x".into(), nodes: vec![SpineNode { at: [0.0; 3], up: [0.0, 1.0, 0.0], r: 0.2, section: 1 }], blend: 0.0, mirror: false, color: default_color() },
                Op::Add { name: "ok".into(), shape: ShapeSpec::Sphere { r: 1.0 }, at: [0.0; 3], rot_axis_deg: None, color: default_color() },
            ],
        };
        let (m, warns) = d.apply();
        assert_eq!(warns.len(), 1);
        assert_eq!(m.parts.len(), 1, "the later op still applies");
    }

    #[test]
    fn match_volume_hits_its_target_and_stays_put() {
        // A sphere of radius 2 (volume 33.5) asked to become volume 8.
        let d = Doc {
            name: "resize".into(),
            ops: vec![
                Op::Add { name: "blob".into(), shape: ShapeSpec::Sphere { r: 2.0 }, at: [5.0, 0.0, 0.0], rot_axis_deg: None, color: default_color() },
                Op::MatchVolume { name: "blob".into(), volume: 8.0, tol: 0.0, resolution: 40 },
            ],
        };
        let (m, warns) = d.apply();
        assert!(warns.is_empty(), "post-condition should hold: {warns:?}");
        let (lo, hi) = m.parts[0].sdf.aabb();
        let c = (lo + hi) * 0.5;
        // Scaling must not have dragged the part toward the origin.
        assert!((c.x - 5.0).abs() < 0.05, "part moved to {c:?}");
        // And the radius should now be the one that gives volume 8: (3*8/4pi)^(1/3) = 1.241
        let r = (hi.x - lo.x) * 0.5;
        assert!((r - 1.241).abs() < 0.05, "radius {r}");
    }

    #[test]
    fn match_volume_reports_a_target_it_cannot_verify() {
        // Tolerance far tighter than the mesher's own error at a coarse resolution: the op must
        // say so rather than quietly claim success.
        let d = Doc {
            name: "picky".into(),
            ops: vec![
                Op::Add { name: "blob".into(), shape: ShapeSpec::Sphere { r: 2.0 }, at: [0.0; 3], rot_axis_deg: None, color: default_color() },
                Op::MatchVolume { name: "blob".into(), volume: 8.0, tol: 1e-6, resolution: 12 },
            ],
        };
        let (_, warns) = d.apply();
        assert_eq!(warns.len(), 1);
        assert!(warns[0].reason.contains("measured"), "{}", warns[0].reason);
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
