//! Dragon forge — the canonical procedural dragon model, ported from the
//! `dragon_forge_3d.html` Three.js viewer.
//!
//! A genome becomes a set of geometric *solids* (a CatmullRom spine of body
//! spheres with an arched/reared neck, four two-segment capsule legs with toes,
//! wing bones, and a sculpted head: skull, jaws, teeth, brow/cheek shields,
//! eyes, crest, horns) which are then **voxelized** — stamped into a grid and
//! reduced to the visible surface shell — exactly as the forge does. The result
//! is a list of cube cells (body- or eye-coloured) the renderer draws, so the
//! in-world dragon matches the forge preview.
//!
//! Pure and deterministic: genome in, voxel cells out. No GPU, no time.

use crate::sim::dragon_archetype::{Genome, NUM_ELEMENTS};

type V3 = [f32; 3];

#[inline] fn sub(a: V3, b: V3) -> V3 { [a[0]-b[0], a[1]-b[1], a[2]-b[2]] }
#[inline] fn add(a: V3, b: V3) -> V3 { [a[0]+b[0], a[1]+b[1], a[2]+b[2]] }
#[inline] fn scale(a: V3, s: f32) -> V3 { [a[0]*s, a[1]*s, a[2]*s] }
#[inline] fn adds(a: V3, b: V3, s: f32) -> V3 { [a[0]+b[0]*s, a[1]+b[1]*s, a[2]+b[2]*s] }
#[inline] fn dot(a: V3, b: V3) -> f32 { a[0]*b[0]+a[1]*b[1]+a[2]*b[2] }
#[inline] fn cross(a: V3, b: V3) -> V3 { [a[1]*b[2]-a[2]*b[1], a[2]*b[0]-a[0]*b[2], a[0]*b[1]-a[1]*b[0]] }
#[inline] fn len(a: V3) -> f32 { dot(a, a).sqrt() }
#[inline] fn norm(a: V3) -> V3 { let l = len(a); if l > 1e-9 { scale(a, 1.0/l) } else { [0.0, 0.0, 0.0] } }
#[inline] fn lerp3(a: V3, b: V3, f: f32) -> V3 { [a[0]+(b[0]-a[0])*f, a[1]+(b[1]-a[1])*f, a[2]+(b[2]-a[2])*f] }

/// Rotate `pt` around `piv` about the X axis (Y-Z plane), matching the forge.
fn rot_x(pt: V3, piv: V3, a: f32) -> V3 {
    let (y, z) = (pt[1]-piv[1], pt[2]-piv[2]);
    let (c, s) = (a.cos(), a.sin());
    [pt[0], piv[1] + y*c - z*s, piv[2] + y*s + z*c]
}

/// The forge "P" proportions, each a slider mapped from a `[0,1]` gene.
struct Proportions {
    pose: f32, bulk: f32, neck_len: f32, neck_arch: f32, tail_len: f32, leg_len: f32,
    wing_span: f32, wing_spread: f32, snout: f32, jaw_depth: f32, snout_curve: f32,
    brow: f32, cheek: f32, teeth: f32, crest: f32, horns: f32, horn_sweep: f32,
}
#[inline] fn map(g: f32, lo: f32, hi: f32) -> f32 { lo + g.clamp(0.0, 1.0) * (hi - lo) }
impl Proportions {
    fn from_genome(g: &Genome) -> Proportions {
        // r336-dragon-size: fold the size-class scale into the REST dimensions so the
        // dragon spans small→huge; the relative r332 `clamp_extent` (rest·1.15) then
        // scales with it (bounds *animation* growth, not base size). Shape ratios
        // (pose/arch/curve/spread/face features) stay unscaled to preserve proportions.
        let size = crate::sim::dragon_archetype::size_scale(
            crate::sim::dragon_archetype::classify(g).size_class,
        );
        Proportions {
            pose: map(g.pose_bias, 0.0, 1.25), bulk: map(g.bulk, 0.6, 1.98) * size,
            neck_len: map(g.neck_len, 0.6, 1.98) * size, neck_arch: map(g.neck_arch, 0.0, 1.25),
            tail_len: map(g.tail_len, 0.6, 2.1) * size, leg_len: map(g.leg_len, 0.6, 1.73) * size,
            wing_span: map(g.wing_span, 0.4, 2.03) * size, wing_spread: map(g.wing_spread, 0.0, 1.25),
            snout: map(g.muzzle_len, 0.5, 1.9) * size, jaw_depth: map(g.jaw_depth, 0.4, 2.1) * size,
            snout_curve: map(g.snout_curve, -0.6, 1.0), brow: map(g.brow, 0.0, 1.6),
            cheek: map(g.cheek, 0.0, 1.6), teeth: map(g.teeth, 0.0, 1.6),
            crest: map(g.crest, 0.0, 1.6), horns: map(g.horns, 0.0, 4.0),
            horn_sweep: map(g.horn_sweep, -0.5, 1.2),
        }
    }
}

enum Solid {
    Sphere { c: V3, r: f32 },
    Capsule { a: V3, b: V3, r: f32 },
    OBox { c: V3, u: V3, v: V3, w: V3, half: [f32; 3] },
}
struct Emis { c: V3, r: f32 }

// ---- CatmullRom (tension 0.5), matching THREE.CatmullRomCurve3 ----
fn cr1(x0: f32, x1: f32, x2: f32, x3: f32, t: f32) -> f32 {
    let t0 = 0.5 * (x2 - x0);
    let t1 = 0.5 * (x3 - x1);
    let c0 = x1; let c1 = t0;
    let c2 = -3.0*x1 + 3.0*x2 - 2.0*t0 - t1;
    let c3 = 2.0*x1 - 2.0*x2 + t0 + t1;
    c0 + c1*t + c2*t*t + c3*t*t*t
}
fn cr_point(pts: &[V3], t: f32) -> V3 {
    let l = pts.len();
    let p = (l - 1) as f32 * t.clamp(0.0, 1.0);
    let mut i = p.floor() as isize;
    if i < 0 { i = 0; }
    if i > l as isize - 2 { i = l as isize - 2; }
    let i = i as usize;
    let w = p - i as f32;
    let p1 = pts[i];
    let p2 = pts[i + 1];
    let p0 = if i > 0 { pts[i - 1] } else { sub(scale(p1, 2.0), p2) };
    let p3 = if i + 2 < l { pts[i + 2] } else { sub(scale(p2, 2.0), p1) };
    [
        cr1(p0[0], p1[0], p2[0], p3[0], w),
        cr1(p0[1], p1[1], p2[1], p3[1], w),
        cr1(p0[2], p1[2], p2[2], p3[2], w),
    ]
}
/// N+1 points spaced by arc length (like getSpacedPoints).
fn cr_spaced(pts: &[V3], n: usize) -> Vec<V3> {
    const S: usize = 400;
    let mut samples = Vec::with_capacity(S + 1);
    let mut cum = Vec::with_capacity(S + 1);
    let mut prev = cr_point(pts, 0.0);
    samples.push(prev);
    cum.push(0.0f32);
    let mut total = 0.0;
    for i in 1..=S {
        let p = cr_point(pts, i as f32 / S as f32);
        total += len(sub(p, prev));
        cum.push(total);
        samples.push(p);
        prev = p;
    }
    let mut out = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let target = if n == 0 { 0.0 } else { i as f32 / n as f32 * total };
        let mut j = 0;
        while j < S && cum[j + 1] < target { j += 1; }
        let seg = (cum[j + 1] - cum[j]).max(1e-6);
        let f = ((target - cum[j]) / seg).clamp(0.0, 1.0);
        out.push(lerp3(samples[j], samples[j + 1], f));
    }
    out
}
fn cr_tangent_end(pts: &[V3]) -> V3 {
    norm(sub(cr_point(pts, 1.0), cr_point(pts, 1.0 - 1e-3)))
}

/// Build the forge solids + emissive (eye/glow) markers from the proportions.
fn build_solids(p: &Proportions, glow: bool) -> (Vec<Solid>, Vec<Emis>) {
    let mut solids: Vec<Solid> = Vec::new();
    let mut emis: Vec<Emis> = Vec::new();
    let b = p.bulk;

    // base spine control points (z forward, y up): (point, radius)
    let ctrl: [(V3, f32); 5] = [
        ([0.0, 0.55, -3.3 * p.tail_len], 0.10 * b),
        ([0.0, 0.78, -2.25 * p.tail_len], 0.20 * b),
        ([0.0, 1.05, -1.15], 0.42 * b), // hips (idx2)
        ([0.0, 1.22, 0.0], 0.56 * b),   // torso
        ([0.0, 1.16, 1.0], 0.50 * b),   // shoulders (idx4)
    ];
    let hips = ctrl[2].0;
    let nl = p.neck_len;
    let mut neck: Vec<(V3, f32)> = vec![
        ([0.0, 1.30, 1.55], 0.40 * b),
        ([0.0, 1.52, 1.55 + 0.55 * nl], 0.33 * b),
        ([0.0, 1.74, 1.55 + 1.05 * nl], 0.27 * b),
        ([0.0, 1.92, 1.55 + 1.45 * nl], 0.23 * b),
    ];
    let nlen = neck.len();
    for (i, n) in neck.iter_mut().enumerate() {
        let t = i as f32 / (nlen - 1) as f32;
        let lift = p.neck_arch * (t * std::f32::consts::PI).sin() * 0.9;
        n.0 = [n.0[0], n.0[1] + lift, n.0[2] - lift * 0.5];
    }
    // shoulders + neck = the reared portion, rotated about the hips by pose
    let mut front: Vec<(V3, f32)> = vec![ctrl[4]];
    front.extend(neck.iter().cloned());
    let ang = -p.pose * 1.15;
    for f in front.iter_mut() {
        f.0 = rot_x(f.0, hips, ang);
    }
    let mut all_ctrl: Vec<(V3, f32)> = ctrl[0..4].to_vec();
    all_ctrl.extend(front.iter().cloned());

    let ctrl_pts: Vec<V3> = all_ctrl.iter().map(|c| c.0).collect();
    let nn = 72usize;
    let pts = cr_spaced(&ctrl_pts, nn);
    let mut rad = Vec::with_capacity(nn + 1);
    for i in 0..=nn {
        let u = i as f32 / nn as f32 * (all_ctrl.len() - 1) as f32;
        let lo = (u.floor() as usize).min(all_ctrl.len() - 2);
        let f = u - lo as f32;
        rad.push(all_ctrl[lo].1 * (1.0 - f) + all_ctrl[lo + 1].1 * f);
    }
    for i in 0..=nn {
        solids.push(Solid::Sphere { c: pts[i], r: rad[i] });
    }

    let hip_p = pts[(0.30 * nn as f32).round() as usize];
    let hip_r = rad[(0.30 * nn as f32).round() as usize];
    let sh_p = pts[(0.52 * nn as f32).round() as usize];
    let sh_r = rad[(0.52 * nn as f32).round() as usize];
    let head_p = pts[nn];
    let head_tan = cr_tangent_end(&ctrl_pts);

    // ---- legs: 2 segments + toes ----
    let reared = p.pose;
    let mut leg = |solids: &mut Vec<Solid>, root: V3, side: f32, front_: bool| {
        let root_s = [root[0] + side * (hip_r * 0.85), root[1] - 0.05, root[2]];
        let mut foot_y = 0.0f32;
        let mut foot_z = root[2] + if front_ { 0.35 } else { -0.25 };
        if front_ {
            let fold = ((reared - 0.3) / 0.7).max(0.0);
            foot_y = 0.0 * (1.0 - fold) + (root[1] + 0.4) * fold;
            foot_z = (root[2] + 0.35) * (1.0 - fold) + (root[2] + 0.9) * fold;
        }
        let foot = [root[0] + side * (hip_r * 0.9), foot_y, foot_z];
        // Knee: keep the fore/aft + vertical bend (they give the leg its silhouette
        // coverage across the gait sweep — the pixel-peeper guards this), but pull
        // the lateral splay in (was side*0.18, which jutted the knee out past the
        // hip→foot line and read as bow-legged "strange" legs).
        let knee = [
            (root_s[0] + foot[0]) / 2.0 + side * 0.09,
            (root_s[1] + foot[1]) / 2.0 - 0.15,
            (root_s[2] + foot[2]) / 2.0 + if front_ { 0.28 } else { -0.28 },
        ];
        let (r1, r2) = (0.16 * p.bulk, 0.12 * p.bulk);
        solids.push(Solid::Capsule { a: root_s, b: knee, r: r1 });
        solids.push(Solid::Capsule { a: knee, b: foot, r: r2 });
        for k in -1..=1 {
            let toe = [foot[0] + side * 0.04, 0.04, foot[2] + 0.18 + k as f32 * 0.07];
            if foot_y < 0.2 {
                solids.push(Solid::Capsule { a: foot, b: toe, r: 0.05 * p.bulk });
            }
        }
    };
    leg(&mut solids, hip_p, -1.0, false);
    leg(&mut solids, hip_p, 1.0, false);
    leg(&mut solids, sh_p, -1.0, true);
    leg(&mut solids, sh_p, 1.0, true);

    // ---- wings: bones (membrane is render-only, omitted from voxels) ----
    let (span, spread) = (p.wing_span, p.wing_spread);
    let mut wing = |solids: &mut Vec<Solid>, side: f32| {
        let base = [sh_p[0] + side * sh_r * 0.8, sh_p[1] + 0.25, sh_p[2] - 0.1];
        let elbow = [base[0] + side * (1.1 * span), base[1] + 0.9 * spread + 0.5, base[2] - 0.5 * spread - 0.3];
        let wrist = [elbow[0] + side * (1.5 * span), elbow[1] + 0.3 * spread, elbow[2] - 1.4 * span * spread - 0.2];
        solids.push(Solid::Capsule { a: base, b: elbow, r: 0.10 * p.bulk });
        solids.push(Solid::Capsule { a: elbow, b: wrist, r: 0.075 * p.bulk });
        for f in 0..4 {
            let t = f as f32 / 3.0;
            let ln = (1.3 - 0.5 * t) * span;
            let ft = [
                wrist[0] + side * ln * (0.3 + 0.7 * spread),
                wrist[1] - t * 1.6 - (1.0 - spread) * 0.8,
                wrist[2] - ln * (0.2 + spread * 0.9) + t * 0.9 * span,
            ];
            solids.push(Solid::Capsule { a: wrist, b: ft, r: 0.05 * p.bulk });
        }
    };
    wing(&mut solids, -1.0);
    wing(&mut solids, 1.0);

    // ---- head ----
    let b2 = p.bulk;
    let fwd2 = norm(head_tan);
    let up_h = norm(adds([0.0, 1.0, 0.0], fwd2, -fwd2[1]));
    let right_h = norm(cross(fwd2, up_h));
    let p3 = |o: V3, a: f32, bb: f32, c: f32| -> V3 {
        [
            o[0] + fwd2[0]*a + up_h[0]*bb + right_h[0]*c,
            o[1] + fwd2[1]*a + up_h[1]*bb + right_h[1]*c,
            o[2] + fwd2[2]*a + up_h[2]*bb + right_h[2]*c,
        ]
    };
    let sk = 0.30 * b2;
    let skull = p3(head_p, 0.12, 0.0, 0.0);
    solids.push(Solid::Sphere { c: skull, r: sk });
    solids.push(Solid::OBox { c: p3(skull, -0.16, 0.04, 0.0), u: fwd2, v: up_h, w: right_h, half: [0.12*b2, 0.20*b2, 0.26*b2] });

    let ml = 0.95 * p.snout;
    let jd = 0.16 * p.jaw_depth;
    let jw = 0.15 * b2;
    let crv = p.snout_curve;
    let uj_tip = p3(skull, 0.30 + ml*0.85, -0.02 - 0.22*crv, 0.0);
    let uj_mid = p3(skull, 0.30 + ml*0.30, -0.02, 0.0);
    let uj_f = norm(sub(uj_tip, skull));
    let uj_u = norm(adds([0.0, 1.0, 0.0], uj_f, -uj_f[1]));
    let uj_r = norm(cross(uj_f, uj_u));
    solids.push(Solid::OBox { c: p3(skull, 0.30 + ml*0.28, -0.02, 0.0), u: uj_f, v: uj_u, w: uj_r, half: [ml*0.34, jd*1.05, jw*1.05] });
    solids.push(Solid::OBox { c: scale(add(uj_mid, uj_tip), 0.5), u: uj_f, v: uj_u, w: uj_r, half: [ml*0.30, jd*0.8, jw*0.82] });
    solids.push(Solid::OBox { c: p3(skull, 0.30 + ml*0.42, jd*0.9, 0.0), u: uj_f, v: uj_u, w: uj_r, half: [ml*0.5, jd*0.3, jw*0.28] }); // nose ridge
    for s in [-1.0f32, 1.0] {
        solids.push(Solid::OBox { c: p3(uj_tip, -0.06, jd*0.5, s*jw*0.5), u: uj_f, v: uj_u, w: uj_r, half: [0.05*b2, 0.05*b2, 0.05*b2] });
    }
    // lower jaw
    let hinge = p3(skull, 0.05, -jd*1.3, 0.0);
    let lj_tip = p3(skull, 0.30 + ml*0.78, -jd*1.6 - 0.04, 0.0);
    let lj_f = norm(sub(lj_tip, hinge));
    let lj_u = norm(adds([0.0, 1.0, 0.0], lj_f, -lj_f[1]));
    let lj_r = norm(cross(lj_f, lj_u));
    solids.push(Solid::OBox { c: scale(add(hinge, lj_tip), 0.5), u: lj_f, v: lj_u, w: lj_r, half: [ml*0.42, jd*0.55, jw*0.9] });
    // teeth
    if p.teeth > 0.02 {
        let tn = 6;
        let tsz = 0.05 * b2 * (0.4 + p.teeth);
        for i in 0..tn {
            let f = 0.30 + ml * (0.30 + 0.5 * i as f32 / tn as f32);
            for s in [-1.0f32, 1.0] {
                solids.push(Solid::OBox { c: p3(skull, f, -jd*0.95, s*jw*0.8), u: uj_f, v: uj_u, w: uj_r, half: [tsz*0.5, tsz*1.4, tsz*0.5] });
                solids.push(Solid::OBox { c: p3(skull, f, -jd*1.5, s*jw*0.8), u: lj_f, v: lj_u, w: lj_r, half: [tsz*0.5, tsz*1.2, tsz*0.5] });
            }
        }
    }
    // brow
    if p.brow > 0.02 {
        for s in [-1.0f32, 1.0] {
            solids.push(Solid::OBox { c: p3(skull, 0.18, 0.16 + 0.06*p.brow, s*0.16), u: fwd2, v: up_h, w: right_h, half: [0.16*b2, 0.05*b2 + 0.05*p.brow, 0.13*b2] });
        }
    }
    // cheek
    if p.cheek > 0.02 {
        for s in [-1.0f32, 1.0] {
            solids.push(Solid::OBox { c: p3(skull, 0.12, -jd*0.6, s*(sk*0.7 + 0.05*p.cheek)), u: fwd2, v: up_h, w: right_h, half: [0.16*b2, 0.14*b2, 0.04*b2 + 0.05*p.cheek] });
        }
    }
    // eyes (emissive only)
    for s in [-1.0f32, 1.0] {
        let eye = p3(skull, sk*0.55, sk*0.18, s*sk*0.62);
        emis.push(Emis { c: eye, r: 0.08 * b2 });
    }
    // crest
    if p.crest > 0.02 {
        for i in 0..3 {
            let s = (i - 1) as f32;
            solids.push(Solid::OBox { c: p3(skull, -0.05 - 0.04*i as f32, sk*0.7 + 0.18*p.crest, s*0.10), u: fwd2, v: up_h, w: right_h, half: [0.03*b2, 0.10*b2 + 0.12*p.crest, 0.05*b2] });
        }
    }
    // horns
    let n_h = p.horns.round() as i32;
    let sweep = p.horn_sweep;
    for h in 0..n_h {
        for s in [-1.0f32, 1.0] {
            let base = p3(skull, -0.10 + 0.02*h as f32, sk*0.5 + 0.05*h as f32, s*(0.12 + 0.05*h as f32));
            let mid = p3(base, -0.18*sweep, 0.26 + 0.06*h as f32, s*0.06);
            let tip = p3(mid, -0.30*sweep - 0.06, 0.30 + 0.10*h as f32, s*0.10);
            solids.push(Solid::Capsule { a: base, b: mid, r: 0.055 * b2 });
            solids.push(Solid::Capsule { a: mid, b: tip, r: 0.035 * b2 });
        }
    }
    // glow ridge plates
    if glow {
        let i0 = (0.34 * nn as f32).round() as usize;
        let i1 = (0.62 * nn as f32).round() as usize;
        let mut i = i0;
        while i < i1 {
            let p = pts[i];
            let r = rad[i];
            emis.push(Emis { c: [p[0], p[1] + r + 0.12, p[2]], r: 0.10 });
            i += 3;
        }
    }

    (solids, emis)
}

fn solid_bounds(s: &Solid) -> (V3, V3) {
    match *s {
        Solid::Sphere { c, r } => ([c[0]-r, c[1]-r, c[2]-r], [c[0]+r, c[1]+r, c[2]+r]),
        Solid::Capsule { a, b, r } => (
            [a[0].min(b[0])-r, a[1].min(b[1])-r, a[2].min(b[2])-r],
            [a[0].max(b[0])+r, a[1].max(b[1])+r, a[2].max(b[2])+r],
        ),
        Solid::OBox { c, u, v, w, half } => {
            let ex = u[0].abs()*half[0] + v[0].abs()*half[1] + w[0].abs()*half[2];
            let ey = u[1].abs()*half[0] + v[1].abs()*half[1] + w[1].abs()*half[2];
            let ez = u[2].abs()*half[0] + v[2].abs()*half[1] + w[2].abs()*half[2];
            ([c[0]-ex, c[1]-ey, c[2]-ez], [c[0]+ex, c[1]+ey, c[2]+ez])
        }
    }
}
fn point_in(s: &Solid, x: f32, y: f32, z: f32) -> bool {
    match *s {
        Solid::Sphere { c, r } => {
            let (dx, dy, dz) = (x-c[0], y-c[1], z-c[2]);
            dx*dx + dy*dy + dz*dz <= r*r
        }
        Solid::Capsule { a, b, r } => {
            let u = sub(b, a);
            let wv = [x-a[0], y-a[1], z-a[2]];
            let denom = dot(u, u).max(1e-9);
            let mut t = (wv[0]*u[0] + wv[1]*u[1] + wv[2]*u[2]) / denom;
            t = t.clamp(0.0, 1.0);
            let dx = wv[0]-t*u[0]; let dy = wv[1]-t*u[1]; let dz = wv[2]-t*u[2];
            dx*dx + dy*dy + dz*dz <= r*r
        }
        Solid::OBox { c, u, v, w, half } => {
            let px = x-c[0]; let py = y-c[1]; let pz = z-c[2];
            let du = px*u[0] + py*u[1] + pz*u[2];
            let dv = px*v[0] + py*v[1] + pz*v[2];
            let dw = px*w[0] + py*w[1] + pz*w[2];
            du.abs() <= half[0] && dv.abs() <= half[1] && dw.abs() <= half[2]
        }
    }
}

/// A merged run of surface voxels along the model's Z (the body's long axis),
/// collapsed into one elongated box to cut the rendered primitive count.
/// Centre + half in model space.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct ForgeBox {
    pub center: V3,
    pub half: V3,
    /// True for emissive (eye/glow) boxes, false for body.
    pub glow: bool,
    /// True for interior mouth-cavity boxes — shade these with
    /// [`ForgeModel::mouth_color`] (a dark throat) so an open jaw reads as a maw
    /// rather than showing the background through the gap.
    pub mouth: bool,
    /// Skeleton joint this box is rigged to (index into [`ForgeModel::joints`]);
    /// `skin` animates the box about that joint's pivot. Set in [`build`].
    pub joint: u8,
}

fn voxelize(solids: &[Solid], emis: &[Emis], res: u32) -> (Vec<ForgeBox>, V3, V3) {
    let res = res.max(8) as f32;
    let mut mn = [1e9f32; 3];
    let mut mx = [-1e9f32; 3];
    for s in solids {
        let (lo, hi) = solid_bounds(s);
        for k in 0..3 { mn[k] = mn[k].min(lo[k]); mx[k] = mx[k].max(hi[k]); }
    }
    if solids.is_empty() {
        return (Vec::new(), [0.0; 3], [0.0; 3]);
    }
    let dim = (mx[0]-mn[0]).max(mx[1]-mn[1]).max(mx[2]-mn[2]).max(1.0);
    let cell = dim / res;
    let half = cell * 0.46;
    let gx = (((mx[0]-mn[0]) / cell).ceil() as usize + 1).max(1);
    let gy = (((mx[1]-mn[1]) / cell).ceil() as usize + 1).max(1);
    let gz = (((mx[2]-mn[2]) / cell).ceil() as usize + 1).max(1);
    let mut occ = vec![0u8; gx * gy * gz];
    let at = |ix: usize, iy: usize, iz: usize| ((ix * gy) + iy) * gz + iz;

    let mut stamp = |occ: &mut [u8], lo: V3, hi: V3, v: u8, test: &dyn Fn(f32, f32, f32) -> bool| {
        let ix0 = (((lo[0]-mn[0]) / cell).floor().max(0.0)) as usize;
        let ix1 = ((((hi[0]-mn[0]) / cell).ceil()) as usize).min(gx - 1);
        let iy0 = (((lo[1]-mn[1]) / cell).floor().max(0.0)) as usize;
        let iy1 = ((((hi[1]-mn[1]) / cell).ceil()) as usize).min(gy - 1);
        let iz0 = (((lo[2]-mn[2]) / cell).floor().max(0.0)) as usize;
        let iz1 = ((((hi[2]-mn[2]) / cell).ceil()) as usize).min(gz - 1);
        for ix in ix0..=ix1.min(gx - 1) {
            let x = mn[0] + (ix as f32 + 0.5) * cell;
            for iy in iy0..=iy1.min(gy - 1) {
                let y = mn[1] + (iy as f32 + 0.5) * cell;
                for iz in iz0..=iz1.min(gz - 1) {
                    let z = mn[2] + (iz as f32 + 0.5) * cell;
                    if test(x, y, z) {
                        let k = at(ix, iy, iz);
                        if v > occ[k] { occ[k] = v; }
                    }
                }
            }
        }
    };

    for s in solids {
        let (lo, hi) = solid_bounds(s);
        stamp(&mut occ, lo, hi, 1, &|x, y, z| point_in(s, x, y, z));
    }
    for e in emis {
        let r = e.r + cell * 0.5;
        let lo = [e.c[0]-r, e.c[1]-r, e.c[2]-r];
        let hi = [e.c[0]+r, e.c[1]+r, e.c[2]+r];
        let c = e.c;
        stamp(&mut occ, lo, hi, 2, &move |x, y, z| {
            let (dx, dy, dz) = (x-c[0], y-c[1], z-c[2]);
            dx*dx + dy*dy + dz*dz <= r*r
        });
    }

    // shell extraction + ground clip + greedy merge along Z (the body's long
    // axis) — runs of same-type surface cells collapse into one elongated box.
    let kept = |ix: usize, iy: usize, iz: usize| -> Option<u8> {
        let v = occ[at(ix, iy, iz)];
        if v == 0 { return None; }
        let y = mn[1] + (iy as f32 + 0.5) * cell;
        if y < half { return None; } // clip below ground
        let shell = ix == 0 || iy == 0 || iz == 0 || ix == gx-1 || iy == gy-1 || iz == gz-1
            || occ[at(ix-1, iy, iz)] == 0 || occ[at(ix+1, iy, iz)] == 0
            || occ[at(ix, iy-1, iz)] == 0 || occ[at(ix, iy+1, iz)] == 0
            || occ[at(ix, iy, iz-1)] == 0 || occ[at(ix, iy, iz+1)] == 0;
        if shell { Some(v) } else { None }
    };
    let mut boxes = Vec::new();
    let gap = 0.96; // matches the forge's cube gap

    // --- Eyes (glow) stay at fine resolution: few cells, visually important.
    //     Per-column Z-run on glow shell cells only.
    for ix in 0..gx {
        for iy in 0..gy {
            let mut iz = 0;
            while iz < gz {
                if kept(ix, iy, iz) == Some(2) {
                    let z0 = iz;
                    let mut z1 = iz;
                    while z1 + 1 < gz && kept(ix, iy, z1 + 1) == Some(2) {
                        z1 += 1;
                    }
                    let run = (z1 - z0 + 1) as f32;
                    boxes.push(ForgeBox {
                        center: [
                            mn[0] + (ix as f32 + 0.5) * cell,
                            mn[1] + (iy as f32 + 0.5) * cell,
                            mn[2] + ((z0 + z1) as f32 * 0.5 + 0.5) * cell,
                        ],
                        half: [cell * 0.5 * gap, cell * 0.5 * gap, run * cell * 0.5 * gap],
                        glow: true,
                        mouth: false,
                        joint: 0,
                    });
                    iz = z1 + 1;
                } else {
                    iz += 1;
                }
            }
        }
    }

    // --- Body: coarsen X-Y into 2x2 super-cells, then cull interior super-cells
    //     ("inner body detection"), then Z-run merge. A super-cell is occupied if
    //     any of its (up to) four fine body shell-cells is set.
    let sx = (gx + 1) / 2;
    let sy = (gy + 1) / 2;
    let sat = |scx: usize, scy: usize, iz: usize| ((scx * sy) + scy) * gz + iz;
    let mut socc = vec![false; sx * sy * gz];
    for scx in 0..sx {
        for scy in 0..sy {
            let ix1 = (2 * scx + 1).min(gx - 1);
            let iy1 = (2 * scy + 1).min(gy - 1);
            for iz in 0..gz {
                let mut body = false;
                'blk: for ix in (2 * scx)..=ix1 {
                    for iy in (2 * scy)..=iy1 {
                        if kept(ix, iy, iz) == Some(1) {
                            body = true;
                            break 'blk;
                        }
                    }
                }
                if body {
                    socc[sat(scx, scy, iz)] = true;
                }
            }
        }
    }
    // visible = occupied AND has an empty super-neighbour (or sits on a boundary);
    // fully-enclosed super-cells are interior and dropped.
    let svis = |scx: usize, scy: usize, iz: usize| -> bool {
        if !socc[sat(scx, scy, iz)] {
            return false;
        }
        scx == 0 || scy == 0 || iz == 0 || scx == sx - 1 || scy == sy - 1 || iz == gz - 1
            || !socc[sat(scx - 1, scy, iz)] || !socc[sat(scx + 1, scy, iz)]
            || !socc[sat(scx, scy - 1, iz)] || !socc[sat(scx, scy + 1, iz)]
            || !socc[sat(scx, scy, iz - 1)] || !socc[sat(scx, scy, iz + 1)]
    };
    for scx in 0..sx {
        for scy in 0..sy {
            let ix0 = 2 * scx;
            let ix1 = (2 * scx + 1).min(gx - 1);
            let iy0 = 2 * scy;
            let iy1 = (2 * scy + 1).min(gy - 1);
            let xext = (ix1 - ix0 + 1) as f32; // 1 (odd edge) or 2
            let yext = (iy1 - iy0 + 1) as f32;
            let cx = mn[0] + ((ix0 + ix1) as f32 * 0.5 + 0.5) * cell;
            let cy = mn[1] + ((iy0 + iy1) as f32 * 0.5 + 0.5) * cell;
            let mut iz = 0;
            while iz < gz {
                if svis(scx, scy, iz) {
                    let z0 = iz;
                    let mut z1 = iz;
                    while z1 + 1 < gz && svis(scx, scy, z1 + 1) {
                        z1 += 1;
                    }
                    let run = (z1 - z0 + 1) as f32;
                    boxes.push(ForgeBox {
                        center: [cx, cy, mn[2] + ((z0 + z1) as f32 * 0.5 + 0.5) * cell],
                        half: [xext * cell * 0.5 * gap, yext * cell * 0.5 * gap, run * cell * 0.5 * gap],
                        glow: false,
                        mouth: false,
                        joint: 0,
                    });
                    iz = z1 + 1;
                } else {
                    iz += 1;
                }
            }
        }
    }

    // --- Deduplicate exact-coincident boxes (defensive; quantised key).
    {
        use std::collections::HashSet;
        let q = |v: f32| (v / (cell * 0.25)).round() as i64;
        let mut seen: HashSet<(i64, i64, i64, i64, i64, i64, bool)> = HashSet::new();
        boxes.retain(|b| {
            seen.insert((
                q(b.center[0]), q(b.center[1]), q(b.center[2]),
                q(b.half[0]), q(b.half[1]), q(b.half[2]), b.glow,
            ))
        });
    }
    (boxes, mn, mx)
}

fn hex(c: u32) -> V3 {
    [((c >> 16) & 0xff) as f32 / 255.0, ((c >> 8) & 0xff) as f32 / 255.0, (c & 0xff) as f32 / 255.0]
}
/// (body, eye) colours for an element index (`0=Wood..5=Storm`), from the forge palette.
fn palette(element_idx: u8) -> (V3, V3) {
    match element_idx % NUM_ELEMENTS as u8 {
        0 => (hex(0x46532f), hex(0xb4f06a)), // Wood (green)
        1 => (hex(0x2b4456), hex(0x6ff0ff)), // Water
        2 => (hex(0x5b626c), hex(0xeaf6ff)), // Air
        3 => (hex(0x6a5636), hex(0xffcf7a)), // Dust
        4 => (hex(0x59302a), hex(0xffae5a)), // Fire
        _ => (hex(0x382d54), hex(0xc39bff)), // Storm
    }
}

/// The voxel dragon model produced from a genome.
#[derive(Clone, Debug)]
pub struct ForgeModel {
    /// Surface boxes (Z-merged) in model space (Z forward, Y up, feet near y=0).
    pub boxes: Vec<ForgeBox>,
    /// Edge length of a voxel cell, in model units.
    pub cell_size: f32,
    /// Body and eye colours (linear RGB 0..1).
    pub body_color: V3,
    pub eye_color: V3,
    /// Dark throat colour for interior mouth-cavity boxes (`ForgeBox::mouth`).
    pub mouth_color: V3,
    /// Model-space bounds of the voxel set.
    pub min: V3,
    pub max: V3,
    /// Skeleton joint pivots in model space (one per `FORGE_JOINTS`), used by
    /// [`skin`] to animate the boxes. Indexed by [`ForgeBox::joint`].
    pub joints: Vec<V3>,
}

/// Skeleton joint indices for the forge rig. Order is the layout of
/// [`ForgeModel::joints`]; [`FORGE_JOINTS`] is the count.
pub const J_SPINE: u8 = 0;
pub const J_HIPS: u8 = 1;
pub const J_NECK: u8 = 2;
pub const J_HEAD: u8 = 3;
pub const J_JAW: u8 = 4;
pub const J_TAIL: u8 = 5;
pub const J_LEG_FL: u8 = 6;
pub const J_LEG_FR: u8 = 7;
pub const J_LEG_BL: u8 = 8;
pub const J_LEG_BR: u8 = 9;
pub const J_WING_L: u8 = 10;
pub const J_WING_R: u8 = 11;
pub const FORGE_JOINTS: usize = 12;

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    crate::util::mathx::lerp(a, b, t) // r342 dedup
}
#[inline]
fn clamp01(x: f32) -> f32 {
    crate::util::mathx::saturate(x) // r343 dedup
}

// r332-dragon: bound an animated half-extent so the seam-bridge `grow` can never
// inflate a box beyond `rest * (1 + max_grow)`. The §4c balloon-then-shrink came
// from adding the world-space `grow` term (up to 0.9 model units) isotropically
// to every NON-leg box half regardless of its rest size — so a small,
// fast-moving head/wingtip box swelled to several times its rest extent at the
// fast part of the swing, then shrank back as it slowed. Capping the result to a
// small fraction over rest kills the balloon while leaving the relative puff and
// the arc-follow (centre rotation) motion untouched. NaN/negative → 0 (finite).
// (On the r332 base this can call `util::mathx::clamp` instead of the inline min.)
#[inline]
fn clamp_extent(rest: f32, animated: f32, max_grow: f32) -> f32 {
    if animated.is_nan() || animated < 0.0 {
        return 0.0;
    }
    let ceil = rest.max(0.0) * (1.0 + max_grow.max(0.0));
    if animated > ceil { ceil } else { animated }
}

/// Joint pivots in model space, inferred from the model bounds (Z forward toward
/// the head, Y up with feet near `min.y`, X lateral). Deterministic.
fn skeleton(mn: V3, mx: V3) -> Vec<V3> {
    let cx = (mn[0] + mx[0]) * 0.5;
    let (y0, yh) = (mn[1], mx[1]);
    let (zb, zf) = (mn[2], mx[2]); // tail .. head
    let midz = (zb + zf) * 0.5;
    let yl = |t: f32| lerp(y0, yh, t);
    let mut j = vec![[0.0f32; 3]; FORGE_JOINTS];
    j[J_SPINE as usize] = [cx, yl(0.55), midz];
    j[J_HIPS as usize] = [cx, yl(0.50), lerp(midz, zb, 0.4)];
    j[J_NECK as usize] = [cx, yl(0.70), lerp(midz, zf, 0.5)];
    j[J_HEAD as usize] = [cx, yl(0.82), lerp(midz, zf, 0.88)];
    j[J_JAW as usize] = [cx, yl(0.64), lerp(midz, zf, 0.95)];
    j[J_TAIL as usize] = [cx, yl(0.46), lerp(midz, zb, 0.25)]; // base near the hips
    // legs pivot at the HIP/shoulder height (not the foot), so the proximal end
    // stays with the trunk and the foot swings — no tearing at the hip seam.
    j[J_LEG_FL as usize] = [lerp(cx, mn[0], 0.6), yl(0.42), lerp(midz, zf, 0.5)];
    j[J_LEG_FR as usize] = [lerp(cx, mx[0], 0.6), yl(0.42), lerp(midz, zf, 0.5)];
    j[J_LEG_BL as usize] = [lerp(cx, mn[0], 0.6), yl(0.42), lerp(midz, zb, 0.5)];
    j[J_LEG_BR as usize] = [lerp(cx, mx[0], 0.6), yl(0.42), lerp(midz, zb, 0.5)];
    j[J_WING_L as usize] = [lerp(cx, mn[0], 0.7), yl(0.78), midz];
    j[J_WING_R as usize] = [lerp(cx, mx[0], 0.7), yl(0.78), midz];
    j
}

/// Assign a box to a skeleton joint by anatomical REGION, propagated from the
/// torso — not by raw nearest-pivot. This is the "wing barrier / leg barrier":
/// a box high and out to the side is wing membrane (it binds to a wing joint even
/// if a leg pivot is euclidean-closer), a box low and out to the side is a leg,
/// and everything central routes through the body chain. So a drooping wing voxel
/// never gets rigged — or counted — as a foot; only leg-region boxes are feet.
/// Glow (eye) boxes pin to the head.
fn assign_joint(joints: &[V3], mn: V3, mx: V3, c: V3, glow: bool) -> u8 {
    let cx = (mn[0] + mx[0]) * 0.5;
    let hw = ((mx[0] - mn[0]) * 0.5).max(1e-3);
    let lat = (c[0] - cx) / hw; // −1..1 lateral
    let ty = (c[1] - mn[1]) / (mx[1] - mn[1]).max(1e-3); // 0 ground .. 1 top
    let tz = (c[2] - mn[2]) / (mx[2] - mn[2]).max(1e-3); // 0 tail .. 1 head
    let lateral = lat.abs();

    // nearest among the central body joints (spine/hips/neck/head/jaw/tail).
    let nearest_central = || {
        const CENTRAL: [u8; 6] = [J_SPINE, J_HIPS, J_NECK, J_HEAD, J_JAW, J_TAIL];
        let mut best = J_SPINE;
        let mut bestd = f32::MAX;
        for &ji in CENTRAL.iter() {
            let j = joints[ji as usize];
            let d = (c[0] - j[0]).powi(2) + (c[1] - j[1]).powi(2) + (c[2] - j[2]).powi(2);
            if d < bestd {
                bestd = d;
                best = ji;
            }
        }
        best
    };

    if glow {
        // Eyes / head glow pin to the head. Everything else emissive — dorsal
        // back-spikes, tail tips, body accents — rides the nearest BODY joint, so a
        // glowing spine spike stays on the spine instead of swinging with the head
        // (and is never rigged as a foot).
        if tz > 0.70 && ty > 0.55 {
            return J_HEAD;
        }
        return nearest_central();
    }
    // WING: out to the side AND in the upper body. The height line ty≈0.4 is the
    // barrier between wing (above) and leg (below) for lateral boxes.
    if lateral > 0.45 && ty > 0.40 {
        return if lat < 0.0 { J_WING_L } else { J_WING_R };
    }
    // LEG: off-centre AND in the lower body — but ONLY if the box is genuinely
    // nearer a leg joint than the trunk (spine/hips). Without this, low ventral
    // "belly" boxes near the centreline get rigged to a leg and slide with the gait
    // (they sit on the TRUNK side of the hip, the feet on the LEG side). Comparing
    // xz-distance to a leg joint vs the trunk keeps the belly on the spine.
    if lateral > 0.18 && ty < 0.42 {
        let leg_joints = [J_LEG_FL, J_LEG_FR, J_LEG_BL, J_LEG_BR];
        let (mut leg_d, mut leg_j) = (f32::MAX, J_LEG_FL);
        for &ji in &leg_joints {
            let j = joints[ji as usize];
            let d = (c[0] - j[0]).powi(2) + (c[2] - j[2]).powi(2);
            if d < leg_d {
                leg_d = d;
                leg_j = ji;
            }
        }
        let trunk_d = [J_SPINE, J_HIPS]
            .iter()
            .map(|&ji| {
                let j = joints[ji as usize];
                (c[0] - j[0]).powi(2) + (c[2] - j[2]).powi(2)
            })
            .fold(f32::MAX, f32::min);
        if leg_d < trunk_d {
            return leg_j;
        }
    }
    // BODY chain.
    nearest_central()
}

/// Is this joint a foot/ground-contact joint? Locomotion + posture should derive
/// feet from these only — never from wings.
pub fn is_leg_joint(joint: u8) -> bool {
    matches!(joint, J_LEG_FL | J_LEG_FR | J_LEG_BL | J_LEG_BR)
}

/// How the brain wants the wings held this frame. Drives fold (tuck back), raise
/// (lift up), and beat amplitude in [`skin`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum WingPose {
    /// Out to the sides with a gentle idle beat (gliding / standing).
    #[default]
    Spread,
    /// Tucked back and up against the body (walking on the ground).
    Folded,
    /// Strong powered beat, raised (liftoff / climbing / flying).
    Beating,
    /// Spread wide and forward to brake (landing flare).
    Flared,
    /// Mantled — raised high and spread (threat / strike pose).
    Aggressive,
}

/// (fold-back, raise-up, beat-amplitude) for a wing pose.
fn wing_pose_params(p: WingPose) -> (f32, f32, f32) {
    match p {
        WingPose::Spread => (0.0, 0.05, 0.6),
        WingPose::Folded => (1.1, 0.6, 0.0),
        WingPose::Beating => (0.0, 0.35, 1.0),
        WingPose::Flared => (-0.45, -0.2, 0.25),
        WingPose::Aggressive => (-0.25, 1.0, 0.1),
    }
}

/// Animation inputs for [`skin`]. `gait_phase` (radians, advanced by distance)
/// drives the walk cycle; `anim_time` (seconds) drives cyclic idle motion
/// (wing-beat, jaw, breathing, blink). Both 0 (and no gaze) → rest pose.
#[derive(Clone, Copy, Debug, Default)]
pub struct ForgeAnim {
    pub gait_phase: f32,
    pub anim_time: f32,
    /// Explicit head gaze (rad, relative to body) from `dragon_observe`. When
    /// non-zero it drives the head/neck; when zero the head idle-scans on
    /// `anim_time` instead (so un-wired creatures look exactly as before).
    pub head_yaw: f32,
    pub head_pitch: f32,
    /// How the brain wants the wings held (walk vs strike vs liftoff vs land vs fly).
    pub wing_pose: WingPose,
}

// rotate point `c` about an axis-aligned pivot, in the plane of the other two axes
fn rot_about_x(c: V3, pv: V3, a: f32) -> V3 {
    let (s, co) = a.sin_cos();
    let (dy, dz) = (c[1]-pv[1], c[2]-pv[2]);
    [c[0], pv[1] + dy*co - dz*s, pv[2] + dy*s + dz*co]
}
fn rot_about_y(c: V3, pv: V3, a: f32) -> V3 {
    let (s, co) = a.sin_cos();
    let (dx, dz) = (c[0]-pv[0], c[2]-pv[2]);
    [pv[0] + dx*co + dz*s, c[1], pv[2] - dx*s + dz*co]
}
fn rot_about_z(c: V3, pv: V3, a: f32) -> V3 {
    let (s, co) = a.sin_cos();
    let (dx, dy) = (c[0]-pv[0], c[1]-pv[1]);
    [pv[0] + dx*co - dy*s, pv[1] + dx*s + dy*co, c[2]]
}

/// A plantable walk profile (the "claw zone"): during STANCE (first half of the
/// cycle) the leg sweeps backward at CONSTANT angular speed, so a claw planted on
/// the ground holds its spot as the body advances at matching speed — no slide.
/// During SWING (second half) it eases quickly forward to the next plant. Replaces
/// the pure sine, whose speed peaked mid-stroke and made the planted foot drift.
fn leg_swing(phase: f32, amp: f32) -> f32 {
    use std::f32::consts::PI;
    let p = phase.rem_euclid(2.0 * PI);
    if p < PI {
        amp * (1.0 - 2.0 * (p / PI)) // stance: +amp → −amp, linear
    } else {
        let s = (p - PI) / PI; // swing: −amp → +amp, smootherstep
        let e = s * s * s * (s * (s * 6.0 - 15.0) + 10.0);
        -amp + 2.0 * amp * e
    }
}

/// Foot lift during the SWING half — 0 while planted (stance), rising to a peak at
/// mid-swing and back to 0 at the next plant. So the swing foot clears the ground
/// (a step) instead of dragging through it.
fn leg_lift(phase: f32) -> f32 {
    use std::f32::consts::PI;
    let p = phase.rem_euclid(2.0 * PI);
    if p >= PI {
        (p - PI).sin().max(0.0) // 0 at toe-off/plant, 1 at mid-swing
    } else {
        0.0
    }
}

/// Apply the rig. Boxes are deformed toward their joint transform by a blend
/// weight that runs 0 at the limb's body attachment (so it moves with the trunk —
/// no gap) to 1 at the limb tip (full swing) — linear-blend skinning, so a leg or
/// wing bends and stretches as a continuous chain rather than disjoint cubes.
/// Breathing and a periodic blink ride on `anim_time`, so even a standing dragon
/// is visibly alive. Rest (`ForgeAnim::default()`) returns the boxes unchanged.
pub fn skin(model: &ForgeModel, anim: &ForgeAnim) -> Vec<ForgeBox> {
    use std::f32::consts::PI;
    const LIMB_STRETCH: f32 = 0.65; // world-space seam bridge per unit of travel (capped)
    const BASE_PUFF: f32 = 0.13; // small overlap on every (non-leg) box while animating
    const LEG_PUFF: f32 = 0.06; // legs stay slim laterally (girth check ≤ 1.1)
    // r332-dragon: hard ceiling on how far the seam-bridge may grow a NON-leg box
    // over its rest extent (§4c). BASE_PUFF (0.13) sits inside it, so the puff is
    // never clipped; only the unbounded world-space `grow` is. Kept "small" per
    // the brief so head/wings can never balloon (was up to +844 % on wings).
    const MAX_ANIM_GROW: f32 = 0.15;
    let t = anim.anim_time;
    let resting = anim.gait_phase == 0.0 && t == 0.0 && anim.head_yaw == 0.0 && anim.head_pitch == 0.0 && anim.wing_pose == WingPose::Spread;
    if resting {
        return model.boxes.clone();
    }
    let j = &model.joints;
    if j.len() < FORGE_JOINTS {
        return model.boxes.clone();
    }
    let (mn, mx) = (model.min, model.max);
    let cx = (mn[0] + mx[0]) * 0.5;
    let hw = ((mx[0] - mn[0]) * 0.5).max(1e-3);
    let hgt = (mx[1] - mn[1]).max(1e-3);
    let midz = (mn[2] + mx[2]) * 0.5;

    let gp = anim.gait_phase;
    let leg_a = leg_swing(gp, 0.32); // FL + BR — plantable (constant-speed stance)
    let leg_b = leg_swing(gp + PI, 0.32); // FR + BL (opposite phase)
    let lift_a = leg_lift(gp); // foot clearance during swing (FL + BR)
    let lift_b = leg_lift(gp + PI); // (FR + BL)
    const LEG_LIFT: f32 = 0.45; // world-units the swing foot rises at mid-step
    let tail_sway = 0.35 * (gp * 0.5 + t * 0.8).sin();
    let (w_fold, w_raise, w_beat) = wing_pose_params(anim.wing_pose);
    let wing_flap = w_beat * 0.6 * (t * 2.2).sin();
    let jaw_open = 0.22 * 0.5 * (1.0 - (t * 1.7).cos());
    let idle_scan = 0.12 * (t * 0.5).sin();
    let head_yaw = if anim.head_yaw != 0.0 { anim.head_yaw } else { idle_scan };
    let head_nod = 0.10 * (t * 1.3).sin() + 0.06 * gp.sin() + anim.head_pitch;

    // breathing: slow chest swell on the idle clock (zero at t=0)
    let breath_amt = 0.03 * (t * 0.7).sin(); // −0.03..0.03
    // blink: a quick eyelid close every ~3.4s (triangle 0→1→0 over a short window)
    let blink_phase = (t / 3.4).rem_euclid(1.0);
    let blink = if blink_phase < 0.07 {
        1.0 - (2.0 * (blink_phase / 0.07) - 1.0).abs()
    } else {
        0.0
    };
    let eye_y_scale = 1.0 - 0.85 * blink;

    let mut out = model.boxes.clone();
    for b in out.iter_mut() {
        let c0 = b.center;
        let pv = j[b.joint as usize];
        // blend weight: 0 at the body attach → 1 at the limb tip (elastic taper)
        let lat = ((c0[0] - cx) / hw).abs();
        let ty = (c0[1] - mn[1]) / hgt;
        let w = if is_leg_joint(b.joint) {
            clamp01((0.55 - ty) / 0.55) // hip line .. foot
        } else if b.joint == J_WING_L || b.joint == J_WING_R {
            clamp01((lat - 0.45) / 0.55) // root .. tip
        } else if b.joint == J_TAIL {
            clamp01((midz - c0[2]) / (midz - mn[2]).max(1e-3)) // base .. tip
        } else if b.joint == J_HEAD || b.joint == J_JAW {
            1.0
        } else if b.joint == J_NECK {
            0.5
        } else {
            0.0 // spine / hips: the stable trunk
        };
        // Rotate c0 about the joint pivot by (w × angle) — i.e. follow the ARC, with
        // the swing tapering from 0 at the attach to full at the tip. Previously this
        // lerped c0→fully-rotated, but a lerp is the CHORD of the arc and cuts inside
        // it, dragging every half-blended box toward the pivot (the "pulled-in"
        // wingtips/wing-base/tail). Scaling the angle keeps each box at its true
        // radius through the whole swing.
        let mut center = match b.joint {
            x if x == J_LEG_FL || x == J_LEG_BR => rot_about_x(c0, pv, leg_a * w),
            x if x == J_LEG_FR || x == J_LEG_BL => rot_about_x(c0, pv, leg_b * w),
            x if x == J_TAIL => rot_about_y(c0, pv, tail_sway * w),
            x if x == J_WING_L => {
                let c = rot_about_z(c0, pv, -(w_raise + wing_flap) * w);
                rot_about_y(c, pv, -w_fold * w)
            }
            x if x == J_WING_R => {
                let c = rot_about_z(c0, pv, (w_raise + wing_flap) * w);
                rot_about_y(c, pv, w_fold * w)
            }
            x if x == J_HEAD => {
                let c = rot_about_x(c0, pv, head_nod * w);
                rot_about_y(c, pv, head_yaw * w)
            }
            x if x == J_JAW => rot_about_x(c0, pv, -jaw_open * w),
            x if x == J_NECK => {
                let c = rot_about_x(c0, pv, head_nod * 0.4 * w);
                rot_about_y(c, pv, head_yaw * 0.4 * w)
            }
            _ => c0,
        };
        // foot clearance: lift the swinging leg, tapered so the foot rises and the
        // hip stays (w ≈ 1 at the foot, 0 at the hip).
        if is_leg_joint(b.joint) {
            let lift = if b.joint == J_LEG_FL || b.joint == J_LEG_BR { lift_a } else { lift_b };
            center[1] += LEG_LIFT * lift * w;
        }
        // breathing: swell trunk + neck outward from the spine axis
        if b.joint == J_SPINE || b.joint == J_HIPS || b.joint == J_NECK {
            center[0] += (c0[0] - cx) * breath_amt;
            center[1] += breath_amt * 0.5;
        }
        let c_moved = center;
        b.center = center;
        // stretch: bridge the seam a moving box leaves. A box that translated by
        // `disp` opens a gap up to `disp` wide on its trailing side, so grow each
        // half by a fraction of `disp` (world-space — works for thin wing membranes
        // too), plus a small relative puff so every seam's static side also overlaps.
        let disp = ((c_moved[0] - c0[0]).powi(2) + (c_moved[1] - c0[1]).powi(2) + (c_moved[2] - c0[2]).powi(2)).sqrt();
        let rest_half = b.half;
        let grow = (LIMB_STRETCH * disp).min(0.9);
        if is_leg_joint(b.joint) {
            // Legs tear open ALONG the bone when they bend, not sideways. Bridge the
            // seam by growing only the box's longest (bone) axis, and keep the
            // lateral radii barely puffed — so a walking leg never reads as fat
            // (guarded by `mob_limb_girth`, threshold 1.1).
            let puff = 1.0 + LEG_PUFF;
            let bone = if rest_half[0] >= rest_half[1] && rest_half[0] >= rest_half[2] {
                0
            } else if rest_half[1] >= rest_half[2] {
                1
            } else {
                2
            };
            let mut h = [rest_half[0] * puff, rest_half[1] * puff, rest_half[2] * puff];
            // Bridge the along-bone seam, but cap the growth to a fraction of the
            // segment so a swinging leg lengthens a little (no gap) instead of
            // ballooning to several times its length (the "swelling"). Cap scales
            // with the segment's own bone length, not the global 0.9.
            let leg_grow = grow.min(0.35 * rest_half[bone] + 0.06);
            h[bone] += leg_grow;
            b.half = h;
        } else {
            // Wings/tail/neck/head: isotropic bridge (thin membranes need lateral
            // fill), but bound each axis so the world-space `grow` can't balloon a
            // small fast-moving box past +MAX_ANIM_GROW of its rest extent (§4c).
            // The puff is inside the bound; the arc-follow (centre rotation above)
            // already keeps boxes adjacent through the swing, so capping the
            // extent costs no motion — it only stops the swell-then-shrink.
            let infl = 1.0 + BASE_PUFF;
            b.half = [
                clamp_extent(rest_half[0], rest_half[0] * infl + grow, MAX_ANIM_GROW),
                clamp_extent(rest_half[1], rest_half[1] * infl + grow, MAX_ANIM_GROW),
                clamp_extent(rest_half[2], rest_half[2] * infl + grow, MAX_ANIM_GROW),
            ];
        }
        // blink: squash the glowing eye boxes vertically (eyelid)
        if b.glow {
            b.half[1] *= eye_y_scale;
        }
    }
    out
}

/// Like [`skin`], but additionally droops the wing membrane using per-span offsets
/// from [`crate::sim::dragon_wing_flex`]. The verlet wing solver (stepped per tick
/// by the caller, pinned at root and tip) produces `offs_l`/`offs_r`; this nudges
/// each wing box by the sag at its span fraction (0 at the wing root .. 1 at the
/// tip). Passing empty slices yields exactly [`skin`]'s output — so un-wired
/// dragons are unchanged.
pub fn skin_with_flex(model: &ForgeModel, anim: &ForgeAnim, offs_l: &[[f32; 3]], offs_r: &[[f32; 3]]) -> Vec<ForgeBox> {
    let mut out = skin(model, anim);
    if offs_l.is_empty() && offs_r.is_empty() {
        return out;
    }
    let root_l = model.joints.get(J_WING_L as usize).copied().unwrap_or([0.0; 3]);
    let root_r = model.joints.get(J_WING_R as usize).copied().unwrap_or([0.0; 3]);
    // wing spans along x, from each root out to the model edge
    let span_l = (root_l[0] - model.min[0]).abs().max(1e-4);
    let span_r = (model.max[0] - root_r[0]).abs().max(1e-4);
    for b in out.iter_mut() {
        let (root, span, offs) = if b.joint == J_WING_L {
            (root_l, span_l, offs_l)
        } else if b.joint == J_WING_R {
            (root_r, span_r, offs_r)
        } else {
            continue;
        };
        if offs.is_empty() {
            continue;
        }
        let frac = ((b.center[0] - root[0]).abs() / span).max(0.0).min(1.0);
        let d = crate::sim::dragon_wing_flex::offset_at(offs, frac);
        b.center = [b.center[0] + d[0], b.center[1] + d[1], b.center[2] + d[2]];
    }
    out
}

/// Build the canonical forge dragon for a genome at voxel resolution `res`
/// (the forge slider tops out at 140; 256 is the high-fidelity in-world value).
/// Deterministic.
pub fn build(g: &Genome, res: u32) -> ForgeModel {
    let p = Proportions::from_genome(g);
    let (solids, emis) = build_solids(&p, true);
    let (mut boxes, mn, mx) = voxelize(&solids, &emis, res);
    let dim = (mx[0]-mn[0]).max(mx[1]-mn[1]).max(mx[2]-mn[2]).max(1.0);
    let cell_size = dim / res.max(8) as f32;
    // rig: a small skeleton inferred from the model bounds, each box bound to its
    // nearest joint so `skin` can animate legs/tail/wings/head/jaw.
    let joints = skeleton(mn, mx);
    for b in boxes.iter_mut() {
        b.joint = assign_joint(&joints, mn, mx, b.center, b.glow);
    }
    // dominant element → palette
    let mut idx = 0u8;
    let mut best = f32::MIN;
    for (i, &w) in g.element.iter().enumerate().take(NUM_ELEMENTS) {
        if w > best { best = w; idx = i as u8; }
    }
    let (body_color, eye_color) = palette(idx);
    // No interior "throat" box: any occluder big enough to fill the open maw also
    // protrudes from the head as a black slab (the operator rejected it twice). An
    // open jaw showing a few px of background is acceptable; a black box is not.
    let mouth_color = [0.04, 0.02, 0.03];
    ForgeModel { boxes, cell_size, body_color, eye_color, mouth_color, min: mn, max: mx, joints }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skin_with_flex_empty_equals_skin_and_sag_drops_the_membrane() {
        let m = build(&Genome::from_seed(4), 48);
        let anim = ForgeAnim { wing_pose: WingPose::Spread, ..Default::default() };
        let plain = skin(&m, &anim);
        // empty offsets ⇒ identical to skin
        let same = skin_with_flex(&m, &anim, &[], &[]);
        assert_eq!(plain.len(), same.len());
        for (a, b) in plain.iter().zip(same.iter()) {
            assert_eq!(a.center, b.center, "no offsets ⇒ unchanged");
        }
        // a downward mid-span sag (pinned ends) lowers the average wing-box height
        let sag = [[0.0, 0.0, 0.0], [0.0, -0.4, 0.0], [0.0, 0.0, 0.0]];
        let drooped = skin_with_flex(&m, &anim, &sag, &sag);
        let wing_y = |bs: &[ForgeBox]| -> f32 {
            let mut sum = 0.0;
            let mut n = 0.0;
            for b in bs.iter().filter(|b| b.joint == J_WING_L || b.joint == J_WING_R) {
                sum += b.center[1];
                n += 1.0;
            }
            if n > 0.0 { sum / n } else { 0.0 }
        };
        assert!(wing_y(&drooped) < wing_y(&plain), "sag drops the wing membrane");
    }

    #[test]
    fn builds_a_nonempty_shell_with_eyes() {
        let g = Genome::from_seed(1);
        let m = build(&g, 24);
        assert!(m.boxes.len() > 25, "expected a substantial shell, got {}", m.boxes.len());
        assert!(m.boxes.iter().any(|b| b.glow), "the dragon should have glowing eye boxes");
        assert!(m.cell_size > 0.0);
    }

    #[test]
    fn merge_reduces_box_count_vs_raw_cells() {
        // The body is coarsened to 2x2 super-cells and interior super-cells are
        // culled, so box count lands far below the ~5324 raw cells at res 128
        // (and well below the old Z-only ~2279). Eyes stay at fine resolution.
        let m = build(&Genome::from_seed(1), 128);
        assert!(m.boxes.len() > 100 && m.boxes.len() < 1500,
            "2x2 + interior cull should keep res-128 boxes near ~700, got {}", m.boxes.len());
        assert!(m.boxes.iter().any(|b| b.glow), "eyes survive the coarse body merge");
    }

    #[test]
    fn deterministic() {
        let g = Genome::from_seed(42);
        let a = build(&g, 20);
        let b = build(&g, 20);
        assert_eq!(a.boxes.len(), b.boxes.len());
        assert_eq!(a.boxes, b.boxes);
    }

    #[test]
    fn wood_is_green_fire_is_red() {
        let wood = build(&Genome::from_seed_element(3, 0), 16);
        assert!(wood.body_color[1] >= wood.body_color[0] && wood.body_color[1] >= wood.body_color[2],
            "Wood body should be green-ish: {:?}", wood.body_color);
        let fire = build(&Genome::from_seed_element(3, 4), 16);
        assert!(fire.body_color[0] > fire.body_color[2], "Fire body should be red-ish");
    }

    #[test]
    fn bulkier_genome_is_larger() {
        let mut small = Genome::from_seed(7); small.bulk = 0.0;
        let mut big = Genome::from_seed(7); big.bulk = 1.0;
        let s = build(&small, 20);
        let bld = build(&big, 20);
        let span = |m: &ForgeModel| m.max[1] - m.min[1];
        assert!(span(&bld) > span(&s), "more bulk → taller model");
    }

    #[test]
    fn feet_near_ground_no_cells_below() {
        let m = build(&Genome::from_seed(3), 20);
        // ground clip removes everything below ~half a cell.
        assert!(m.boxes.iter().all(|b| b.center[1] >= -m.cell_size), "no boxes far below ground");
    }

    #[test]
    fn no_horns_gene_still_builds() {
        let mut g = Genome::from_seed(9);
        g.horns = 0.0;
        let m = build(&g, 18);
        assert!(!m.boxes.is_empty());
    }

    #[test]
    fn build_assigns_joints_and_skeleton() {
        let m = build(&Genome::from_seed(5), 48);
        assert_eq!(m.joints.len(), FORGE_JOINTS);
        // every box maps to a valid joint
        assert!(m.boxes.iter().all(|b| (b.joint as usize) < FORGE_JOINTS));
        // Glow boxes are never rigged as feet (a glowing dorsal spike must not
        // animate as a leg), and at least some glow — the eyes — pin to the head.
        assert!(
            m.boxes.iter().filter(|b| b.glow).all(|b| !is_leg_joint(b.joint)),
            "no emissive box should be rigged to a leg joint"
        );
        assert!(
            m.boxes.iter().any(|b| b.glow && b.joint == J_HEAD),
            "the eyes should still pin to the head"
        );
        // legs actually capture some boxes (the rig isn't all-spine)
        let leggy = m.boxes.iter().filter(|b| matches!(b.joint, J_LEG_FL | J_LEG_FR | J_LEG_BL | J_LEG_BR)).count();
        assert!(leggy > 0, "expected some boxes rigged to legs");
    }

    #[test]
    fn rest_pose_is_identity() {
        let m = build(&Genome::from_seed(2), 32);
        let skinned = skin(&m, &ForgeAnim::default());
        assert_eq!(skinned, m.boxes, "ForgeAnim::default() must leave the model at rest");
    }

    #[test]
    fn walking_moves_legs_idle_moves_wings() {
        let m = build(&Genome::from_seed(2), 48);
        let walk = skin(&m, &ForgeAnim { gait_phase: 1.2, anim_time: 0.0, ..Default::default() });
        // a leg box should have moved vs rest
        let moved_leg = m.boxes.iter().zip(walk.iter()).any(|(r, a)| {
            matches!(r.joint, J_LEG_FL | J_LEG_FR | J_LEG_BL | J_LEG_BR) && r.center != a.center
        });
        assert!(moved_leg, "legs should swing while walking");

        let idle = skin(&m, &ForgeAnim { gait_phase: 0.0, anim_time: 0.7, ..Default::default() });
        let moved_wing = m.boxes.iter().zip(idle.iter()).any(|(r, a)| {
            matches!(r.joint, J_WING_L | J_WING_R) && r.center != a.center
        });
        assert!(moved_wing, "wings should beat on the idle clock");
        // body length is preserved (skin moves centres, doesn't grow the model)
        assert_eq!(walk.len(), m.boxes.len());
    }

    
    #[test]
    fn skin_is_deterministic() {
        let m = build(&Genome::from_seed(11), 32);
        let a = skin(&m, &ForgeAnim { gait_phase: 0.9, anim_time: 1.4, ..Default::default() });
        let b = skin(&m, &ForgeAnim { gait_phase: 0.9, anim_time: 1.4, ..Default::default() });
        assert_eq!(a, b);
    }

    #[test]
    fn wings_are_high_legs_are_low_barrier_holds() {
        let mut g = Genome::from_seed(5);
        g.wing_span = 0.9;
        g.wing_spread = 0.8;
        let m = build(&g, 64);
        let hgt = (m.max[1] - m.min[1]).max(1e-3);
        let ty = |b: &ForgeBox| (b.center[1] - m.min[1]) / hgt;
        // every leg-rigged box sits in the lower body...
        for b in m.boxes.iter().filter(|b| is_leg_joint(b.joint)) {
            assert!(ty(b) < 0.45, "a foot box is too high (ty {}) — wing leaked into legs", ty(b));
        }
        // ...and every wing-rigged box sits in the upper body (never a foot)
        for b in m.boxes.iter().filter(|b| matches!(b.joint, J_WING_L | J_WING_R)) {
            assert!(ty(b) > 0.38, "a wing box is too low (ty {}) — would be mistaken for a foot", ty(b));
        }
        assert!(m.boxes.iter().any(|b| matches!(b.joint, J_WING_L | J_WING_R)), "winged genome has wing boxes");
    }

    #[test]
    fn breathing_and_blink_animate_on_the_idle_clock() {
        let m = build(&Genome::from_seed(2), 48);
        // pick a time where breathing is near its peak and a blink is mid-window
        let t = 3.4 * 0.035 + 0.0; // blink_phase ≈ 0.035 → near full blink
        let a = skin(&m, &ForgeAnim { gait_phase: 0.0, anim_time: t, ..Default::default() });
        // a trunk box drifts from breathing
        let trunk_moved = m.boxes.iter().zip(a.iter()).any(|(r, s)| {
            matches!(r.joint, J_SPINE | J_HIPS) && (r.center[0] - s.center[0]).abs() > 1e-5
        });
        assert!(trunk_moved, "the chest should breathe");
        // the eyes squash (eyelid) during the blink
        let eye_blinked = m.boxes.iter().zip(a.iter()).any(|(r, s)| {
            r.glow && s.half[1] < r.half[1] - 1e-4
        });
        assert!(eye_blinked, "the eyes should blink");
    }

    #[test]
    fn wing_pose_changes_how_the_wings_are_held() {
        let m = build(&{ let mut g = Genome::from_seed(5); g.wing_span = 0.9; g }, 64);
        let wing_y = |pose: WingPose| {
            let s = skin(&m, &ForgeAnim { anim_time: 0.0, wing_pose: pose, ..Default::default() });
            let (mut sum, mut n) = (0.0f32, 0.0f32);
            for (r, a) in m.boxes.iter().zip(s.iter()) {
                if matches!(r.joint, J_WING_L | J_WING_R) {
                    sum += a.center[1];
                    n += 1.0;
                }
            }
            if n > 0.0 { sum / n } else { 0.0 }
        };
        let spread = wing_y(WingPose::Spread);
        let folded = wing_y(WingPose::Folded);
        let aggressive = wing_y(WingPose::Aggressive);
        assert!(folded > spread, "folded wings ride higher (tucked up) than spread");
        assert!(aggressive > spread, "an aggressive mantle raises the wings");
        // a non-default pose animates even while standing still (not early-returned)
        let standing_folded = skin(&m, &ForgeAnim { wing_pose: WingPose::Folded, ..Default::default() });
        assert_ne!(standing_folded, m.boxes, "a standing dragon still folds its wings");
    }

    #[test]
    fn no_oversized_mouth_slab() {
        // regression: the old dark-throat boxes sized to span_x*0.20 (wingspan-
        // inclusive), producing a body-wide slab across the head. A jaw-sized cavity
        // is allowed (it occludes the open maw); a wingspan-wide one is not.
        let m = build(&Genome::from_seed(5), 64);
        let span_x = m.max[0] - m.min[0];
        for b in m.boxes.iter().filter(|b| b.mouth) {
            assert!(
                b.half[0] * 2.0 < span_x * 0.25,
                "throat cavity is jaw-sized, not a body-wide slab ({} vs span {span_x})",
                b.half[0] * 2.0
            );
        }
        let widest_head = m
            .boxes
            .iter()
            .filter(|b| b.joint == J_HEAD)
            .map(|b| b.half[0] * 2.0)
            .fold(0.0f32, f32::max);
        assert!(widest_head < span_x * 0.6, "no head box spans most of the body ({widest_head} vs {span_x})");
    }

    #[test]
    fn belly_boxes_stay_on_the_trunk_not_legs() {
        // regression: low ventral "belly" boxes near the centreline used to rig to a
        // leg joint and slide with the gait. They must stay on the trunk.
        for seed in [1u64, 5, 13] {
            let m = build(&Genome::from_seed(seed), 64);
            let cx = (m.min[0] + m.max[0]) * 0.5;
            let hw = ((m.max[0] - m.min[0]) * 0.5).max(1e-3);
            for b in &m.boxes {
                let lateral = ((b.center[0] - cx) / hw).abs();
                let ty = (b.center[1] - m.min[1]) / (m.max[1] - m.min[1]).max(1e-3);
                if lateral < 0.12 && ty < 0.42 {
                    assert!(
                        !is_leg_joint(b.joint),
                        "seed {seed}: a central belly box (lateral={lateral:.2} ty={ty:.2}) was rigged to leg joint {}",
                        b.joint
                    );
                }
            }
        }
    }

    #[test]
    fn leg_lift_clears_during_swing() {
        use std::f32::consts::PI;
        assert_eq!(leg_lift(0.0), 0.0, "planted at cycle start");
        assert_eq!(leg_lift(PI * 0.5), 0.0, "no lift mid-stance");
        assert!(leg_lift(PI * 1.5) > 0.9, "near-max lift mid-swing");
        assert!(leg_lift(PI * 1.95) < 0.25, "coming back down before the next plant");
    }

    #[test]
    fn leg_swing_plants_then_swings() {
        use std::f32::consts::PI;
        // STANCE (first half): constant-speed linear sweep — the plantable signature.
        let stance: Vec<f32> = (0..=8).map(|i| leg_swing(PI * i as f32 / 8.0, 0.32)).collect();
        let sd: Vec<f32> = stance.windows(2).map(|w| w[1] - w[0]).collect();
        let smean = sd.iter().sum::<f32>() / sd.len() as f32;
        for d in &sd {
            assert!((d - smean).abs() < 1e-5, "stance sweeps at constant speed (Δ={d}, mean={smean})");
        }
        assert!(smean < 0.0, "stance sweeps backward (claw planted, body advancing)");
        // SWING (second half): returns forward FASTER than it planted.
        let swing: Vec<f32> = (8..=16).map(|i| leg_swing(PI * i as f32 / 8.0, 0.32)).collect();
        let wmax = swing.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        assert!(wmax > smean.abs() * 1.2, "swing returns faster than stance ({wmax} vs {})", smean.abs());
    }

    #[test]
    fn limb_blend_tapers_root_to_tip() {
        // a leg box near the hip line barely moves; a foot box swings fully — so the
        // limb bends continuously instead of separating into rigid cubes.
        let m = build(&Genome::from_seed(5), 64);
        let anim = ForgeAnim { gait_phase: 1.4, anim_time: 0.0, ..Default::default() };
        let s = skin(&m, &anim);
        let hgt = (m.max[1] - m.min[1]).max(1e-3);
        let mut root_disp = f32::MAX;
        let mut tip_disp = 0.0f32;
        for (r, a) in m.boxes.iter().zip(s.iter()).filter(|(r, _)| is_leg_joint(r.joint)) {
            let d = ((r.center[0] - a.center[0]).powi(2) + (r.center[1] - a.center[1]).powi(2) + (r.center[2] - a.center[2]).powi(2)).sqrt();
            let ty = (r.center[1] - m.min[1]) / hgt;
            if ty > 0.40 { root_disp = root_disp.min(d); } // near the hip
            if ty < 0.12 { tip_disp = tip_disp.max(d); } // near the foot
        }
        if root_disp.is_finite() && tip_disp > 0.0 {
            assert!(tip_disp > root_disp, "foot should swing more than the hip-root (continuous taper)");
        }
    }

    // ---- r332-dragon: §4c body-part balloon regression guards ----------------

    // The tolerance the fix enforces on NON-leg boxes: rest * (1 + MAX_ANIM_GROW).
    // (Mirrors `skin`'s const; kept here so the guard fails loudly if it drifts.)
    const NONLEG_TOL: f32 = 0.15;

    /// A spread of animation phases that exercise the worst swings (fast wing
    /// beats, mantled pose, head gaze, full gait).
    fn stress_anims() -> Vec<ForgeAnim> {
        use std::f32::consts::TAU;
        let poses = [
            WingPose::Spread, WingPose::Beating, WingPose::Aggressive,
            WingPose::Folded, WingPose::Flared,
        ];
        let mut v = Vec::new();
        for &wp in &poses {
            let mut gp = 0.0f32;
            while gp < TAU {
                for &t in &[0.0f32, 0.4, 0.9, 1.6, 2.3, 3.1, 4.0, 5.2] {
                    v.push(ForgeAnim {
                        gait_phase: gp,
                        anim_time: t,
                        head_yaw: 0.6 * (t * 1.1).sin(),
                        head_pitch: 0.4 * gp.cos(),
                        wing_pose: wp,
                    });
                }
                gp += 0.5;
            }
        }
        v
    }

    #[test]
    fn r332_nonleg_extents_bounded_across_cycle() {
        // The bug (pre-fix): wings ballooned to ~9.4x and the head ~4.3x rest
        // extent at the fast part of a swing, then shrank. Guard: no NON-leg box
        // ever exceeds rest * (1 + NONLEG_TOL) on any axis, across the whole
        // sweep. (Legs have their own, looser along-bone bridge — see below.)
        let m = build(&Genome::from_seed(5), 64);
        let rest = m.boxes.clone();
        let mut worst = 1.0f32;
        for anim in stress_anims() {
            let out = skin(&m, &anim);
            assert_eq!(out.len(), rest.len(), "skin preserves box order/count");
            for (i, b) in out.iter().enumerate() {
                if is_leg_joint(b.joint) {
                    continue;
                }
                for k in 0..3 {
                    let r = rest[i].half[k];
                    let ceil = r * (1.0 + NONLEG_TOL) + 1e-4;
                    assert!(
                        b.half[k] <= ceil,
                        "non-leg box {i} joint {} axis {k} ballooned: {} > {} (rest {})",
                        b.joint, b.half[k], ceil, r
                    );
                    if r > 1e-4 {
                        worst = worst.max(b.half[k] / r);
                    }
                }
            }
        }
        // sanity: the guard is actually exercised near the cap, not trivially slack.
        assert!(worst > 1.0 + NONLEG_TOL * 0.5, "expected growth near the cap, saw {worst}x");
    }

    #[test]
    fn r332_head_and_wings_specifically_bounded() {
        // The two parts §4c calls out, under the worst pose (powered beat + gaze).
        let m = build(&{ let mut g = Genome::from_seed(5); g.wing_span = 1.0; g }, 64);
        let rest = m.boxes.clone();
        let anim = ForgeAnim {
            gait_phase: 1.2, anim_time: 1.7, head_yaw: 0.7, head_pitch: 0.4,
            wing_pose: WingPose::Beating,
        };
        let out = skin(&m, &anim);
        let mut checked_head = false;
        let mut checked_wing = false;
        for (i, b) in out.iter().enumerate() {
            let is_head = b.joint == J_HEAD || b.joint == J_JAW;
            let is_wing = b.joint == J_WING_L || b.joint == J_WING_R;
            if !(is_head || is_wing) {
                continue;
            }
            for k in 0..3 {
                let r = rest[i].half[k];
                assert!(
                    b.half[k] <= r * (1.0 + NONLEG_TOL) + 1e-4,
                    "{} box {i} axis {k}: {} over rest {}",
                    if is_head { "head" } else { "wing" }, b.half[k], r
                );
            }
            checked_head |= is_head;
            checked_wing |= is_wing;
        }
        assert!(checked_head && checked_wing, "model must have head and wing boxes to guard");
    }

    #[test]
    fn r332_legs_stay_within_their_own_cap_unchanged() {
        // The leg path (its own along-bone bridge, untouched by this fix) must not
        // balloon either: bone axis <= rest*1.06 + (0.35*rest_bone + 0.06); other
        // axes only puffed by 1.06. Proves the fix left the working leg path alone.
        let m = build(&Genome::from_seed(5), 64);
        let rest = m.boxes.clone();
        for anim in stress_anims() {
            let out = skin(&m, &anim);
            for (i, b) in out.iter().enumerate() {
                if !is_leg_joint(b.joint) {
                    continue;
                }
                let rh = rest[i].half;
                let bone = if rh[0] >= rh[1] && rh[0] >= rh[2] { 0 } else if rh[1] >= rh[2] { 1 } else { 2 };
                for k in 0..3 {
                    let cap = if k == bone {
                        rh[k] * 1.06 + (0.35 * rh[bone] + 0.06)
                    } else {
                        rh[k] * 1.06
                    } + 1e-4;
                    assert!(b.half[k] <= cap, "leg box {i} axis {k}: {} > cap {}", b.half[k], cap);
                }
            }
        }
    }

    #[test]
    fn r332_rest_pose_is_identical_to_model() {
        // A static frame (rest anim) must equal the rest model exactly — the fix
        // must not perturb the un-animated dragon.
        let m = build(&Genome::from_seed(5), 64);
        let rest_anim = ForgeAnim { wing_pose: WingPose::Spread, ..Default::default() };
        let out = skin(&m, &rest_anim);
        assert_eq!(out, m.boxes, "rest pose must be byte-identical to the model");
    }

    #[test]
    fn r332_motion_is_preserved_not_frozen() {
        // The fix bounds EXTENT, not motion: box centres must still move under
        // animation (wing flap / gait / breathing), so the pose still animates.
        let m = build(&Genome::from_seed(5), 64);
        let rest = m.boxes.clone();
        let anim = ForgeAnim {
            gait_phase: 1.5, anim_time: 1.7, head_yaw: 0.5, head_pitch: 0.2,
            wing_pose: WingPose::Beating,
        };
        let out = skin(&m, &anim);
        let mut max_move = 0.0f32;
        for (r, b) in rest.iter().zip(out.iter()) {
            let d = ((r.center[0]-b.center[0]).powi(2)
                + (r.center[1]-b.center[1]).powi(2)
                + (r.center[2]-b.center[2]).powi(2)).sqrt();
            max_move = max_move.max(d);
        }
        assert!(max_move > 0.05, "animation should still move boxes (max centre move {max_move})");
    }

    #[test]
    fn r332_left_right_symmetry_preserved() {
        // On a mirror-symmetric synthetic wing pair under a yaw-free beat, the fix
        // must grow both wings identically (the clamp depends only on rest + disp,
        // and the L/R transforms are mirror images). Synthetic model avoids real
        // voxel L/R asymmetry so this isolates the fix's symmetry.
        let half = [0.05f32, 0.30, 0.45]; // a thin wing membrane box
        let pivot_y = 1.0;
        let mut model = ForgeModel {
            boxes: vec![
                ForgeBox { center: [-0.8, 1.0, 0.0], half, glow: false, mouth: false, joint: J_WING_L },
                ForgeBox { center: [ 0.8, 1.0, 0.0], half, glow: false, mouth: false, joint: J_WING_R },
            ],
            cell_size: 0.1,
            body_color: [0.5; 3], eye_color: [1.0; 3], mouth_color: [0.0; 3],
            min: [-1.0, 0.0, -1.0], max: [1.0, 2.0, 1.0],
            joints: vec![[0.0; 3]; FORGE_JOINTS],
        };
        model.joints[J_WING_L as usize] = [-0.2, pivot_y, 0.0];
        model.joints[J_WING_R as usize] = [ 0.2, pivot_y, 0.0];
        let anim = ForgeAnim {
            anim_time: 1.3, wing_pose: WingPose::Beating, head_yaw: 0.0, head_pitch: 0.0,
            gait_phase: 0.0,
        };
        let out = skin(&model, &anim);
        let l = out.iter().find(|b| b.joint == J_WING_L).map(|b| b.half);
        let r = out.iter().find(|b| b.joint == J_WING_R).map(|b| b.half);
        let (l, r) = (l.expect("L wing"), r.expect("R wing"));
        for k in 0..3 {
            assert!((l[k] - r[k]).abs() < 1e-5, "L/R wing extent axis {k} differ: {} vs {}", l[k], r[k]);
        }
    }

    #[test]
    fn r332_skin_is_deterministic() {
        let m = build(&Genome::from_seed(5), 64);
        let anim = ForgeAnim {
            gait_phase: 2.0, anim_time: 3.3, head_yaw: 0.4, head_pitch: 0.1,
            wing_pose: WingPose::Aggressive,
        };
        assert_eq!(skin(&m, &anim), skin(&m, &anim));
    }

    #[test]
    fn r332_clamp_extent_helper_bounds_and_sanitizes() {
        // ceiling: rest * (1 + max_grow)
        assert!((clamp_extent(1.0, 5.0, 0.15) - 1.15).abs() < 1e-6, "clamps to the ceiling");
        // passthrough below the ceiling
        assert!((clamp_extent(1.0, 1.05, 0.15) - 1.05).abs() < 1e-6, "in-range passes through");
        // NaN / negative animated → 0 (finite, non-negative)
        assert_eq!(clamp_extent(1.0, f32::NAN, 0.15), 0.0, "NaN ⇒ 0");
        assert_eq!(clamp_extent(1.0, -2.0, 0.15), 0.0, "negative ⇒ 0");
        // a negative max_grow is treated as 0 (never below rest from the ceiling).
        assert!((clamp_extent(2.0, 9.0, -1.0) - 2.0).abs() < 1e-6, "max_grow<0 ⇒ ceiling = rest");
        // a zero rest box stays zero (no floor reintroduces a balloon).
        assert_eq!(clamp_extent(0.0, 9.0, 0.15), 0.0, "zero rest ⇒ zero ceiling");
    }
}



use crate::sim::creature_from_image::SurfaceTraits;

/// Place additional emissive "glow" boxes from image-derived [`SurfaceTraits`]
/// (C3). Small + additive: each glow point becomes one small emissive box mapped
/// onto the creature's upper-dorsal surface and rigged to the nearest DORSAL
/// joint (head / neck / spine / tail — never a leg or wing), so the existing
/// glow-rigging invariant ("no emissive box rigged to a leg joint") is preserved.
/// No-op when there are no glow points. Pure (no render/IO/RNG).
pub fn place_surface_glow(model: &mut ForgeModel, surf: &SurfaceTraits) {
    if surf.glow_points.is_empty() {
        return;
    }
    let (mn, mx) = (model.min, model.max);
    let cx = (mn[0] + mx[0]) * 0.5;
    let span_x = (mx[0] - mn[0]).max(0.1);
    let mid_y = (mn[1] + mx[1]) * 0.5;
    let cell = model.cell_size.max(0.02);
    let dorsal = [J_HEAD, J_NECK, J_SPINE, J_TAIL];
    for gp in &surf.glow_points {
        let u = gp.u.clamp(0.0, 1.0);
        let v = gp.v.clamp(0.0, 1.0);
        let s = gp.strength.clamp(0.0, 1.0);
        // v → along the body long axis (z); u → lateral (x), kept near the spine;
        // sit on the upper-dorsal surface.
        let z = mn[2] + v * (mx[2] - mn[2]);
        let x = cx + (u - 0.5) * 0.5 * span_x;
        let y = mid_y + 0.72 * (mx[1] - mid_y);
        let center = [x, y, z];
        // nearest dorsal joint by squared distance (never legs/wings)
        let mut joint = J_HEAD;
        let mut best = f32::MAX;
        for &j in &dorsal {
            let p = model.joints[j as usize];
            let d = (p[0] - center[0]).powi(2) + (p[1] - center[1]).powi(2) + (p[2] - center[2]).powi(2);
            if d < best {
                best = d;
                joint = j;
            }
        }
        let r = cell * (1.0 + s);
        model.boxes.push(ForgeBox { center, half: [r, r, r], glow: true, mouth: false, joint });
    }
}

#[cfg(test)]
mod c3_surface_glow_tests {
    use super::*;
    use crate::sim::creature_from_image::{GlowPoint, SurfaceTraits};

    fn leg_joint(j: u8) -> bool {
        j == J_LEG_FL || j == J_LEG_FR || j == J_LEG_BL || j == J_LEG_BR || j == J_WING_L || j == J_WING_R
    }

    #[test]
    fn surface_glow_is_additive_dorsal_and_in_bounds() {
        let g = Genome::from_seed(1);
        let mut m = build(&g, 24);
        let before = m.boxes.len();
        let before_glow = m.boxes.iter().filter(|b| b.glow).count();
        let traits = SurfaceTraits {
            glow_points: vec![
                GlowPoint { u: 0.5, v: 0.2, strength: 0.9 },
                GlowPoint { u: 0.4, v: 0.55, strength: 0.6 },
                GlowPoint { u: 0.6, v: 0.85, strength: 0.5 },
            ],
            glow_strength: 0.7,
            roughness: 0.5,
        };
        place_surface_glow(&mut m, &traits);
        // additive: exactly +3 emissive boxes
        assert_eq!(m.boxes.len(), before + 3, "should add one box per glow point");
        assert_eq!(m.boxes.iter().filter(|b| b.glow).count(), before_glow + 3);
        // the added boxes are glow, rigged dorsally (never legs/wings), upper half, in-bounds
        for b in m.boxes.iter().skip(before) {
            assert!(b.glow, "added boxes are emissive");
            assert!(!leg_joint(b.joint), "C3 glow must not rig to a leg/wing joint (got {})", b.joint);
            assert!(b.center[1] >= (m.min[1] + m.max[1]) * 0.5, "glow sits on the upper body");
            assert!(b.center[2] >= m.min[2] - 1e-3 && b.center[2] <= m.max[2] + 1e-3, "within body length");
        }
    }

    #[test]
    fn empty_traits_is_noop() {
        let mut m = build(&Genome::from_seed(2), 24);
        let before = m.boxes.len();
        place_surface_glow(&mut m, &SurfaceTraits::neutral());
        assert_eq!(m.boxes.len(), before, "no glow points → no change");
    }
}
