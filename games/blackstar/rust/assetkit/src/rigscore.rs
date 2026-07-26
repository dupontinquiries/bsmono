//! Skeletal scoring — how well a model supports human-like motion.
//!
//! The rig work established that limb splitting succeeds on some models and fails
//! badly on others: Zeta Prime got 6,818 triangles in one arm and 100 in the other,
//! and animating that threw limbs a body-width off. That was only discovered by
//! measuring joint gaps through a walk cycle, after the model had shipped.
//!
//! This scores riggability BEFORE any of that, from the geometry alone:
//!
//!   SEPARABILITY  do components partition cleanly into limbs, or does one blob
//!                 span the whole body?
//!   SYMMETRY      are left and right limbs balanced in mass and mirrored in
//!                 position? Asymmetry is what breaks the walk cycle.
//!   PROPORTION    do the limb spans match human anthropometry? A bot with no legs
//!                 to speak of cannot stride however cleanly it splits.
//!   ARTICULATION  is there a clear pivot -- a narrow waist between segments -- or
//!                 is the limb fused to the torso?
//!
//! Proportions are the standard anthropometric fractions of stature (Drillis &
//! Contini): they are what a humanoid walk cycle assumes, so a model far from them
//! will read wrong however well it is rigged.

use crate::harvest::{self, Limb};
use crate::mesh::Mesh;

pub struct Fractions;
impl Fractions {
    pub const HEAD: f32 = 0.130;
    pub const SHOULDER_W: f32 = 0.259;
    pub const ARM_SPAN: f32 = 1.000;
    pub const UPPER_ARM: f32 = 0.186;
    pub const FOREARM: f32 = 0.146;
    pub const THIGH: f32 = 0.245;
    pub const SHIN: f32 = 0.246;
    pub const LEG_TOTAL: f32 = 0.530;   // hip height as a fraction of stature
}

#[derive(Debug, Clone)]
pub struct RigScore {
    pub separability: f32,
    pub symmetry: f32,
    pub proportion: f32,
    pub articulation: f32,
    pub total: f32,
    pub limb_tris: [usize; 6],   // body, head, arm_l, arm_r, leg_l, leg_r
    pub notes: Vec<String>,
}

fn bbox(v: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for p in v { for k in 0..3 { lo[k] = lo[k].min(p[k]); hi[k] = hi[k].max(p[k]); } }
    (lo, hi)
}

/// closeness of `got` to `want`, 1.0 at exact, falling off with relative error
fn prox(got: f32, want: f32) -> f32 {
    if want <= 1e-6 { return 0.0; }
    let e = (got - want).abs() / want;
    (1.0 - e).clamp(0.0, 1.0)
}

pub fn score(m: &Mesh, min_tris: usize) -> RigScore {
    let mut notes = Vec::new();
    let comps = harvest::components(m, min_tris);
    let (lo, hi) = bbox(&m.v);
    let height = (hi[1] - lo[1]).max(1e-6);
    let ax = harvest::mirror_axis(&m.v);

    // torso half-width, same rule the splitter uses
    let torso_hw = (hi[ax] - lo[ax]) * 0.5 * 0.28;
    let mut tris = [0usize; 6];
    let mut cent: [Vec<[f32; 3]>; 6] = Default::default();
    let mut spans: [f32; 6] = [0.0; 6];
    for c in &comps {
        let cm = Mesh { v: c.v.clone(), f: c.f.clone() };
        let mut ctr = [0.0f32; 3];
        for p in &cm.v { for k in 0..3 { ctr[k] += p[k]; } }
        for k in 0..3 { ctr[k] /= cm.v.len().max(1) as f32; }
        let limb = harvest::classify(ctr, lo, hi, torso_hw, ax);
        let i = match limb {
            Limb::Body => 0, Limb::Head => 1, Limb::ArmL => 2,
            Limb::ArmR => 3, Limb::LegL => 4, Limb::LegR => 5,
        };
        tris[i] += cm.f.len();
        cent[i].push(ctr);
        let (clo, chi) = bbox(&cm.v);
        spans[i] = spans[i].max(chi[1] - clo[1]);
    }
    let total_tris: usize = tris.iter().sum::<usize>().max(1);

    // SEPARABILITY: limbs should hold real mass. A model whose geometry is one
    // fused blob scores near zero however humanoid it looks.
    let limb_frac = (tris[2] + tris[3] + tris[4] + tris[5]) as f32 / total_tris as f32;
    let separability = (limb_frac / 0.45).clamp(0.0, 1.0);
    if limb_frac < 0.10 {
        notes.push(format!("only {:.0}% of geometry is in limbs -- likely fused or not humanoid",
                           limb_frac * 100.0));
    }

    // SYMMETRY: mass balance and mirrored position. This is the one that predicts
    // whether the walk cycle will hold together.
    let bal = |a: usize, b: usize| -> f32 {
        let (x, y) = (tris[a] as f32, tris[b] as f32);
        if x.max(y) <= 0.0 { return 0.0; }
        x.min(y) / x.max(y)
    };
    let arm_bal = bal(2, 3);
    let leg_bal = bal(4, 5);
    let mirror = |i: usize, j: usize| -> f32 {
        if cent[i].is_empty() || cent[j].is_empty() { return 0.0; }
        let mi: f32 = cent[i].iter().map(|c| c[ax]).sum::<f32>() / cent[i].len() as f32;
        let mj: f32 = cent[j].iter().map(|c| c[ax]).sum::<f32>() / cent[j].len() as f32;
        let mid = (lo[ax] + hi[ax]) * 0.5;
        if (mi - mid) * (mj - mid) >= 0.0 { return 0.0; }   // same side
        prox((mi - mid).abs(), (mj - mid).abs())
    };
    // WORST pair, not the average. A weighted mean let Zeta Prime score 0.59
    // symmetry with arms of 44 and 5,874 triangles, because its legs were fine --
    // and that model threw limbs a body-width off when animated. An animation
    // fails at its worst joint; averaging hides exactly the case worth catching.
    let arm_sym = (arm_bal + mirror(2, 3)) * 0.5;
    let leg_sym = (leg_bal + mirror(4, 5)) * 0.5;
    let symmetry = arm_sym.min(leg_sym).clamp(0.0, 1.0);
    if arm_bal < 0.25 && tris[2] + tris[3] > 0 {
        notes.push(format!("arms {} vs {} triangles -- too asymmetric to rig",
                           tris[2], tris[3]));
    }
    if leg_bal < 0.25 && tris[4] + tris[5] > 0 {
        notes.push(format!("legs {} vs {} triangles -- too asymmetric to rig",
                           tris[4], tris[5]));
    }

    // PROPORTION against anthropometric fractions of stature
    let span_ax = (hi[ax] - lo[ax]) / height;
    let head_f = spans[1] / height;
    let leg_f = spans[4].max(spans[5]) / height;
    let arm_f = spans[2].max(spans[3]) / height;
    let proportion = (prox(span_ax, Fractions::ARM_SPAN) * 0.3
                    + prox(head_f, Fractions::HEAD) * 0.2
                    + prox(leg_f, Fractions::LEG_TOTAL) * 0.3
                    + prox(arm_f, Fractions::UPPER_ARM + Fractions::FOREARM) * 0.2)
                    .clamp(0.0, 1.0);
    if leg_f < 0.20 {
        notes.push(format!("legs span {:.0}% of height (human ~53%) -- little to stride with",
                           leg_f * 100.0));
    }

    // ARTICULATION: a rig needs a WAIST between segments. Measure how much the
    // cross-section narrows between the leg band and the torso band -- a fused
    // model has no narrowing and nothing to pivot about.
    let band = |y0: f32, y1: f32| -> f32 {
        let a = lo[1] + height * y0;
        let b = lo[1] + height * y1;
        let sel: Vec<f32> = m.v.iter().filter(|p| p[1] >= a && p[1] <= b).map(|p| p[ax]).collect();
        if sel.len() < 8 { return 0.0; }
        let mn = sel.iter().cloned().fold(f32::MAX, f32::min);
        let mx = sel.iter().cloned().fold(f32::MIN, f32::max);
        (mx - mn) / height
    };
    let hip = band(0.42, 0.52);
    let chest = band(0.60, 0.75);
    let thigh = band(0.25, 0.40);
    let waist_ratio = if chest.max(thigh) > 1e-6 { hip / chest.max(thigh) } else { 1.0 };
    // below 1.0 means the hip is narrower than what it joins -- a real waist
    let articulation = (1.0 - waist_ratio).clamp(0.0, 1.0) * 2.0;
    let articulation = articulation.clamp(0.0, 1.0);

    let total = separability * 0.30 + symmetry * 0.35 + proportion * 0.20 + articulation * 0.15;
    RigScore { separability, symmetry, proportion, articulation, total, limb_tris: tris, notes }
}

pub fn verdict(s: &RigScore) -> &'static str {
    if s.total > 0.60 { "RIGGABLE -- limb split and walk cycle should hold" }
    else if s.total > 0.35 { "MARGINAL -- expect asymmetric limbs; validate before animating" }
    else { "NOT RIGGABLE as a humanoid -- fused, asymmetric or wrong proportions" }
}
