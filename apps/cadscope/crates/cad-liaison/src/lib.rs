//! cad-liaison — the sanitization boundary. Everything a language model is permitted to say to
//! CADscope passes through this crate, and the rule is `apps/logscope/DESIGN_generative_scenes.md`'s,
//! verbatim: **the model proposes; sanitize disposes.**
//!
//! What crosses the boundary is a constrained JSON `Proposal`: a list of ops drawn from a fixed
//! vocabulary, and reference queries naming things by name. What may never cross:
//!
//!   * geometry — no vertices, no meshes, no distance functions. A model that could hand over
//!     geometry could hand over anything, and nothing downstream could tell whether it was
//!     measured or imagined.
//!   * code, shader source, expressions to evaluate.
//!   * paths and URLs. A reference is a *name*, resolved against the corpus by `cad-ref`; a path
//!     would let a proposal reach a file the corpus never indexed and never checked.
//!   * facts. A model may not state a volume, an extent or a material — it may only *ask*, and
//!     `cad-ref` answers from measurement. This is the rule that makes "same volume as puma"
//!     trustworthy: the number never travels through the model.
//!
//! Every numeric field is clamped rather than rejected where a clamp is meaningful, because a
//! proposal that is 90% sensible should produce 90% of a part and a note about the rest — the same
//! per-op tolerance `cad-forge` documents. Unknown ops are dropped with a reason. A garbage
//! proposal degrades to an empty script and a list of complaints, never a panic and never a
//! surprise.

use cad_forge::{BoolMode, Doc, Op, ShapeSpec};
use serde::{Deserialize, Serialize};

/// Hard limits. These exist so that a proposal cannot be a denial-of-service (ten thousand ops, a
/// pattern of a million copies) and cannot express a part at a scale that is certainly a mistake.
pub mod limits {
    /// Model units are millimetres; a metre of bracket is already generous.
    pub const MAX_DIM_MM: f32 = 10_000.0;
    pub const MIN_DIM_MM: f32 = 0.01;
    pub const MAX_COORD_MM: f32 = 100_000.0;
    pub const MAX_OPS: usize = 256;
    pub const MAX_PATTERN: u32 = 512;
    pub const MAX_BLEND_MM: f32 = 500.0;
    pub const MAX_NAME: usize = 64;
}

/// A primitive, in the vocabulary a model is shown. Deliberately the same five shapes `cad-forge`
/// knows — the boundary offers no way to describe a shape the kernel cannot already build.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum ProposedShape {
    Sphere { r: f32 },
    Cuboid { half: [f32; 3] },
    Cylinder { r: f32, hh: f32 },
    Capsule { r: f32, hh: f32 },
    Torus { major: f32, minor: f32 },
}

/// One proposed step. Additive and subtractive modelling, plus the reference verbs — nothing that
/// reads a file, runs anything, or asserts a measurement.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ProposedOp {
    Add {
        name: String,
        #[serde(flatten)]
        shape: ProposedShape,
        #[serde(default)]
        at: [f32; 3],
    },
    Move {
        name: String,
        by: [f32; 3],
    },
    Combine {
        into: String,
        other: String,
        #[serde(default)]
        mode: String,
        #[serde(default)]
        blend: f32,
    },
    Shell {
        name: String,
        thickness: f32,
    },
    Pattern {
        name: String,
        count: u32,
        step: [f32; 3],
    },
    Remove {
        name: String,
    },
    /// "make <name> the same volume as <reference>" — the model names both and asserts neither.
    MatchVolumeOf {
        name: String,
        reference: String,
    },
}

/// What a model is allowed to send. Note what is absent: no free-text command, no expression, no
/// file reference, and no field in which a number could arrive claiming to be a measurement.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Proposal {
    /// Prose is permitted here and used only for display. It is never parsed.
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub ops: Vec<ProposedOp>,
}

/// Why something in a proposal did not survive. Every rejection names itself, because a silent
/// drop is indistinguishable from a bug.
#[derive(Clone, Debug, PartialEq)]
pub struct Complaint {
    pub index: usize,
    pub reason: String,
}

/// The result of sanitizing: ops that may be applied, reference lookups that must be performed
/// first, and everything that was refused or adjusted.
#[derive(Clone, Debug, Default)]
pub struct Sanitized {
    pub ops: Vec<Op>,
    /// `(part name, reference name)` pairs the caller must resolve through `cad_ref` before the
    /// corresponding `MatchVolume` op can be built. The liaison deliberately cannot resolve these
    /// itself — measurement belongs to the corpus.
    pub pending_volume: Vec<(String, String)>,
    pub complaints: Vec<Complaint>,
}

impl Sanitized {
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty() && self.pending_volume.is_empty()
    }
}

fn clean_name(raw: &str) -> Option<String> {
    let n: String = raw
        .trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-' || *c == '.')
        .take(limits::MAX_NAME)
        .collect();
    // A name that was entirely punctuation, or that looks like a path, is not a name. Rejecting
    // path-ish strings here is what stops a reference from reaching outside the corpus.
    (!n.is_empty() && !raw.contains('/') && !raw.contains('\\') && !raw.contains("..")).then_some(n)
}

fn clamp_dim(v: f32) -> Option<f32> {
    v.is_finite().then(|| v.abs().clamp(limits::MIN_DIM_MM, limits::MAX_DIM_MM))
}

fn clamp_coord(v: [f32; 3]) -> Option<[f32; 3]> {
    v.iter()
        .all(|x| x.is_finite())
        .then(|| v.map(|x| x.clamp(-limits::MAX_COORD_MM, limits::MAX_COORD_MM)))
}

fn to_shape(p: &ProposedShape) -> Option<ShapeSpec> {
    Some(match *p {
        ProposedShape::Sphere { r } => ShapeSpec::Sphere { r: clamp_dim(r)? },
        ProposedShape::Cuboid { half } => ShapeSpec::Cuboid {
            half: [clamp_dim(half[0])?, clamp_dim(half[1])?, clamp_dim(half[2])?],
        },
        ProposedShape::Cylinder { r, hh } => {
            ShapeSpec::Cylinder { r: clamp_dim(r)?, hh: clamp_dim(hh)? }
        }
        ProposedShape::Capsule { r, hh } => {
            ShapeSpec::Capsule { r: clamp_dim(r)?, hh: clamp_dim(hh)? }
        }
        ProposedShape::Torus { major, minor } => {
            let major = clamp_dim(major)?;
            // A torus whose tube is thicker than its ring is not a torus; pull the minor radius
            // under the major rather than refusing the whole op.
            let minor = clamp_dim(minor)?.min(major * 0.95);
            ShapeSpec::Torus { major, minor }
        }
    })
}

/// Parse and sanitize a proposal's JSON. Malformed JSON is a single complaint, not an error the
/// caller has to handle differently from a bad op — from a person's point of view both are "the
/// model said something I could not use".
pub fn sanitize_json(json: &str) -> Sanitized {
    match serde_json::from_str::<Proposal>(json) {
        Ok(p) => sanitize(&p),
        Err(e) => Sanitized {
            complaints: vec![Complaint { index: 0, reason: format!("not a valid proposal: {e}") }],
            ..Default::default()
        },
    }
}

/// Sanitize a parsed proposal.
pub fn sanitize(p: &Proposal) -> Sanitized {
    let mut out = Sanitized::default();
    for (i, op) in p.ops.iter().enumerate() {
        if out.ops.len() + out.pending_volume.len() >= limits::MAX_OPS {
            out.complaints.push(Complaint {
                index: i,
                reason: format!("proposal exceeds {} ops; the rest were dropped", limits::MAX_OPS),
            });
            break;
        }
        let mut complain = |reason: String| out.complaints.push(Complaint { index: i, reason });
        match op {
            ProposedOp::Add { name, shape, at } => {
                let (Some(name), Some(shape), Some(at)) =
                    (clean_name(name), to_shape(shape), clamp_coord(*at))
                else {
                    complain("add: unusable name, dimensions, or position".into());
                    continue;
                };
                out.ops.push(Op::Add {
                    name,
                    shape,
                    at,
                    rot_axis_deg: None,
                    color: [0.72, 0.75, 0.80, 1.0],
                });
            }
            ProposedOp::Move { name, by } => match (clean_name(name), clamp_coord(*by)) {
                (Some(name), Some(by)) => out.ops.push(Op::Nudge { name, by }),
                _ => complain("move: unusable name or offset".into()),
            },
            ProposedOp::Combine { into, other, mode, blend } => {
                let (Some(into), Some(other)) = (clean_name(into), clean_name(other)) else {
                    complain("combine: unusable names".into());
                    continue;
                };
                if into == other {
                    complain(format!("combine: '{into}' with itself"));
                    continue;
                }
                let m = match mode.trim().to_ascii_lowercase().as_str() {
                    "" | "union" | "add" | "join" | "fuse" => BoolMode::Union,
                    "subtract" | "cut" | "remove" | "difference" => BoolMode::Subtract,
                    "intersect" | "common" => BoolMode::Intersect,
                    other_mode => {
                        // An unknown mode defaults to union *and says so*, rather than silently
                        // cutting a part away because a word was misspelled.
                        complain(format!("combine: unknown mode '{other_mode}', treated as union"));
                        BoolMode::Union
                    }
                };
                let k = if blend.is_finite() && *blend > 0.0 {
                    blend.min(limits::MAX_BLEND_MM)
                } else {
                    0.0
                };
                out.ops.push(Op::Combine { into, other, mode: m, k });
            }
            ProposedOp::Shell { name, thickness } => {
                match (clean_name(name), clamp_dim(*thickness)) {
                    (Some(name), Some(t)) => out.ops.push(Op::Shell { name, t }),
                    _ => complain("shell: unusable name or thickness".into()),
                }
            }
            ProposedOp::Pattern { name, count, step } => {
                let (Some(name), Some(step)) = (clean_name(name), clamp_coord(*step)) else {
                    complain("pattern: unusable name or step".into());
                    continue;
                };
                if *count < 2 {
                    complain("pattern: a count below two repeats nothing".into());
                    continue;
                }
                let clamped = (*count).min(limits::MAX_PATTERN);
                if clamped != *count {
                    complain(format!("pattern: count clamped to {clamped}"));
                }
                out.ops.push(Op::Pattern { name, count: clamped, step });
            }
            ProposedOp::Remove { name } => match clean_name(name) {
                Some(name) => out.ops.push(Op::Remove { name }),
                None => complain("remove: unusable name".into()),
            },
            ProposedOp::MatchVolumeOf { name, reference } => {
                match (clean_name(name), clean_name(reference)) {
                    (Some(name), Some(reference)) => out.pending_volume.push((name, reference)),
                    _ => complain("match volume: unusable part or reference name".into()),
                }
            }
        }
    }
    out
}

/// Append sanitized ops to a document. Returns the document's own per-op warnings, so a caller
/// sees both what the boundary refused and what the forge could not apply.
pub fn apply_to(doc: &mut Doc, s: &Sanitized) -> Vec<String> {
    doc.ops.extend(s.ops.iter().cloned());
    let (_, warns) = doc.apply();
    warns.into_iter().map(|w| format!("op #{}: {}", w.index, w.reason)).collect()
}

/// The schema a model is shown. Kept next to the parser deliberately: if the vocabulary changes
/// and this does not, the model is being asked for something the boundary will refuse.
pub const SCHEMA: &str = r#"Reply with JSON only: {"note": string, "ops": [...]}.
Ops (each an object with "op"):
  {"op":"add","name":"base","shape":"cuboid","half":[10,2,6],"at":[0,0,0]}
     shape is one of: sphere{r} | cuboid{half:[x,y,z]} | cylinder{r,hh} | capsule{r,hh} | torus{major,minor}
  {"op":"move","name":"base","by":[0,5,0]}
  {"op":"combine","into":"base","other":"boss","mode":"union|subtract|intersect","blend":0.5}
  {"op":"shell","name":"cup","thickness":1.5}
  {"op":"pattern","name":"stud","count":4,"step":[10,0,0]}
  {"op":"remove","name":"scrap"}
  {"op":"match_volume_of","name":"blank","reference":"puma"}
All lengths are millimetres. Reference other work by NAME only.
Do not include geometry, code, file paths, or measured values: state no volume or
size you were not given. To use a reference, name it and it will be measured."#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_proposal_becomes_ops() {
        let json = r#"{"note":"a drilled plate","ops":[
            {"op":"add","name":"plate","shape":"cuboid","half":[10,2,6]},
            {"op":"add","name":"bore","shape":"cylinder","r":3,"hh":4},
            {"op":"combine","into":"plate","other":"bore","mode":"subtract"}
        ]}"#;
        let s = sanitize_json(json);
        assert!(s.complaints.is_empty(), "{:?}", s.complaints);
        assert_eq!(s.ops.len(), 3);
        let mut doc = Doc { name: "t".into(), ops: Vec::new() };
        let warns = apply_to(&mut doc, &s);
        assert!(warns.is_empty(), "{warns:?}");
        let (m, _) = doc.apply();
        assert_eq!(m.parts.len(), 1, "the bore was consumed by the subtract");
        assert!(
            m.parts[0].sdf.eval(glam::Vec3::ZERO) > 0.0,
            "the bore left the centre open"
        );
    }

    #[test]
    fn paths_and_traversal_are_not_names() {
        let json = r#"{"ops":[
            {"op":"add","name":"../../etc/passwd","shape":"sphere","r":1},
            {"op":"add","name":"/tmp/evil","shape":"sphere","r":1},
            {"op":"match_volume_of","name":"x","reference":"../secrets/model.obj"}
        ]}"#;
        let s = sanitize_json(json);
        assert!(s.ops.is_empty(), "no path-shaped name may become an op");
        assert!(s.pending_volume.is_empty(), "nor a path-shaped reference");
        assert_eq!(s.complaints.len(), 3);
    }

    #[test]
    fn measurements_cannot_be_asserted_only_requested() {
        // A model trying to state a volume has nowhere to put it: the field does not exist, and
        // serde drops it. What survives is the request, which the corpus will answer.
        let json = r#"{"ops":[{"op":"match_volume_of","name":"blank","reference":"puma","volume_mm3":999999}]}"#;
        let s = sanitize_json(json);
        assert_eq!(s.pending_volume, vec![("blank".to_string(), "puma".to_string())]);
        assert!(s.ops.is_empty(), "a volume match is not an op until it has been measured");
    }

    #[test]
    fn nonsense_numbers_are_clamped_not_obeyed() {
        let json = r#"{"ops":[
            {"op":"add","name":"huge","shape":"sphere","r":1e9},
            {"op":"add","name":"tiny","shape":"sphere","r":0.0000001},
            {"op":"add","name":"ring","shape":"torus","major":5,"minor":50}
        ]}"#;
        let s = sanitize_json(json);
        assert_eq!(s.ops.len(), 3);
        let dims: Vec<f32> = s
            .ops
            .iter()
            .filter_map(|o| match o {
                Op::Add { shape: ShapeSpec::Sphere { r }, .. } => Some(*r),
                _ => None,
            })
            .collect();
        assert_eq!(dims[0], limits::MAX_DIM_MM);
        assert_eq!(dims[1], limits::MIN_DIM_MM);
        // The torus survives with its tube pulled inside its ring, so it is still a torus.
        let Some(Op::Add { shape: ShapeSpec::Torus { major, minor }, .. }) = s.ops.get(2) else {
            panic!("expected a torus")
        };
        assert!(minor < major, "{minor} vs {major}");
    }

    #[test]
    fn not_a_number_is_refused_rather_than_clamped() {
        let json = r#"{"ops":[{"op":"add","name":"nan","shape":"sphere","r":null}]}"#;
        let s = sanitize_json(json);
        assert!(s.ops.is_empty());
        assert_eq!(s.complaints.len(), 1);
    }

    #[test]
    fn an_unknown_mode_defaults_to_union_and_says_so() {
        let json = r#"{"ops":[{"op":"combine","into":"a","other":"b","mode":"subtrct"}]}"#;
        let s = sanitize_json(json);
        assert_eq!(s.ops.len(), 1, "the op survives");
        assert_eq!(s.complaints.len(), 1, "but the misspelling is reported");
        assert!(s.complaints[0].reason.contains("union"), "{}", s.complaints[0].reason);
        // Crucially it did NOT guess 'subtract' — a wrong boolean silently removes material.
        assert!(matches!(s.ops[0], Op::Combine { mode: BoolMode::Union, .. }));
    }

    #[test]
    fn garbage_degrades_to_a_complaint() {
        for junk in ["", "not json", "{}", r#"{"ops":"a string"}"#, r#"{"ops":[{"op":"launch_missiles"}]}"#] {
            let s = sanitize_json(junk);
            assert!(s.ops.is_empty(), "{junk:?} produced ops");
        }
        assert!(!sanitize_json("not json").complaints.is_empty());
    }

    #[test]
    fn a_flood_of_ops_is_truncated() {
        let ops: Vec<String> = (0..limits::MAX_OPS + 50)
            .map(|i| format!(r#"{{"op":"add","name":"p{i}","shape":"sphere","r":1}}"#))
            .collect();
        let s = sanitize_json(&format!(r#"{{"ops":[{}]}}"#, ops.join(",")));
        assert_eq!(s.ops.len(), limits::MAX_OPS);
        assert!(s.complaints.iter().any(|c| c.reason.contains("exceeds")));
    }

    #[test]
    fn one_bad_op_does_not_void_the_others() {
        let json = r#"{"ops":[
            {"op":"add","name":"good","shape":"sphere","r":2},
            {"op":"add","name":"","shape":"sphere","r":2},
            {"op":"shell","name":"good","thickness":0.5}
        ]}"#;
        let s = sanitize_json(json);
        assert_eq!(s.ops.len(), 2);
        assert_eq!(s.complaints.len(), 1);
    }

    #[test]
    fn the_schema_names_every_op_the_parser_accepts() {
        // If the vocabulary grows and SCHEMA does not, a model is being asked for the wrong thing.
        for op in ["add", "move", "combine", "shell", "pattern", "remove", "match_volume_of"] {
            assert!(SCHEMA.contains(op), "SCHEMA omits '{op}'");
        }
        for shape in ["sphere", "cuboid", "cylinder", "capsule", "torus"] {
            assert!(SCHEMA.contains(shape), "SCHEMA omits '{shape}'");
        }
    }
}
