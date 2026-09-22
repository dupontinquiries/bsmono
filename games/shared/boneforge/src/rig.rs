//! The skeleton: fixed-length bones, joint angles, forward kinematics.
//!
//! Bone LENGTHS never change. That is the property that makes a pose
//! hand-authorable -- there is no such thing as an elbow angle when the
//! forearm's length is a function of time -- and it is what a growth
//! animation dressed as a skeleton cannot offer.

pub type Deg = f32;

#[derive(Debug, Clone, Copy)]
pub struct Bone {
    pub name: &'static str,
    pub parent: Option<usize>,
    pub len: f32,
    /// Direction the bone points when its angles are zero, in its parent's
    /// frame. NOT always +y: arms and legs are (0,-1,0), clavicles (0,0,-1),
    /// feet (-1,0,0). Writing `(0, t*len, 0)` instead of following this is
    /// the single mistake that has cost the most here -- it put cladding past
    /// the shoulder, muscle attachments off the wrong end, and a whole shell
    /// on the wrong side of every limb.
    pub rest: [f32; 3],
    pub radius: f32,
}

/// A point `t` of the way along a bone, in its own frame.
///
/// The helper that should exist so nobody writes the tuple by hand again.
pub fn along(b: &Bone, t: f32) -> [f32; 3] {
    [b.rest[0] * b.len * t, b.rest[1] * b.len * t, b.rest[2] * b.len * t]
}

pub type Mat3 = [[f32; 3]; 3];

pub fn rot(rx: Deg, ry: Deg, rz: Deg) -> Mat3 {
    let (cx, sx) = (rx.to_radians().cos(), rx.to_radians().sin());
    let (cy, sy) = (ry.to_radians().cos(), ry.to_radians().sin());
    let (cz, sz) = (rz.to_radians().cos(), rz.to_radians().sin());
    [[cz * cy, cz * sy * sx - sz * cx, cz * sy * cx + sz * sx],
     [sz * cy, sz * sy * sx + cz * cx, sz * sy * cx - cz * sx],
     [-sy,     cy * sx,                cy * cx]]
}

pub fn mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut o = [[0.0f32; 3]; 3];
    for i in 0..3 { for j in 0..3 {
        o[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
    }}
    o
}

pub fn apply(m: &Mat3, v: [f32; 3]) -> [f32; 3] {
    [m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
     m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
     m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2]]
}

#[derive(Debug, Clone)]
pub struct Pose {
    pub root_pos: [f32; 3],
    pub root_rot: [Deg; 3],
    /// Applied OUTERMOST, about world x, after the root rotation. The root's
    /// euler is ZYX, so with a lay-down in rz any rx is applied first and
    /// rotates the body's front within the horizontal plane -- it can face
    /// left or right but never DOWN. No value of the three can express "lay
    /// it flat, then roll it onto its front".
    pub root_roll: Deg,
    pub angles: Vec<[Deg; 3]>,
}

impl Pose {
    pub fn rest(n: usize) -> Self {
        Pose { root_pos: [0.0; 3], root_rot: [0.0; 3], root_roll: 0.0,
               angles: vec![[0.0; 3]; n] }
    }
    pub fn lerp(&self, other: &Pose, f: f32) -> Pose {
        let m = |a: f32, b: f32| a + (b - a) * f;
        Pose {
            root_pos: [m(self.root_pos[0], other.root_pos[0]),
                       m(self.root_pos[1], other.root_pos[1]),
                       m(self.root_pos[2], other.root_pos[2])],
            root_rot: [m(self.root_rot[0], other.root_rot[0]),
                       m(self.root_rot[1], other.root_rot[1]),
                       m(self.root_rot[2], other.root_rot[2])],
            root_roll: m(self.root_roll, other.root_roll),
            angles: self.angles.iter().zip(&other.angles)
                .map(|(a, b)| [m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2])])
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Joint {
    pub head: [f32; 3],
    pub tail: [f32; 3],
    pub mat: Mat3,
}

/// Forward kinematics. No grounding: a caller that wants the body on the
/// floor grounds on the MESH, because a foot's geometry hangs below its ankle
/// and a joint-grounded rig renders sunk.
pub fn solve(bones: &[Bone], p: &Pose) -> Vec<Joint> {
    let mut rr = rot(p.root_rot[0], p.root_rot[1], p.root_rot[2]);
    if p.root_roll.abs() > 1e-6 {
        rr = mul(&rot(p.root_roll, 0.0, 0.0), &rr);
    }
    let mut out: Vec<Joint> = Vec::with_capacity(bones.len());
    for (i, b) in bones.iter().enumerate() {
        let (head, pm) = match b.parent {
            None => (p.root_pos, rr),
            Some(pi) => (out[pi].tail, out[pi].mat),
        };
        let a = p.angles.get(i).copied().unwrap_or([0.0; 3]);
        let m = mul(&pm, &rot(a[0], a[1], a[2]));
        let d = apply(&m, b.rest);
        out.push(Joint {
            head,
            tail: [head[0] + d[0] * b.len, head[1] + d[1] * b.len, head[2] + d[2] * b.len],
            mat: m,
        });
    }
    out
}

/// Angle at a joint between a bone and its parent, signed about `axis`.
pub fn flex(js: &[Joint], parent: usize, child: usize, axis: usize) -> Deg {
    let pv = sub(js[parent].tail, js[parent].head);
    let cv = sub(js[child].tail, js[child].head);
    let (pl, cl) = (norm(pv).max(1e-6), norm(cv).max(1e-6));
    let d = (pv[0] * cv[0] + pv[1] * cv[1] + pv[2] * cv[2]) / (pl * cl);
    let ang = d.clamp(-1.0, 1.0).acos().to_degrees();
    let cr = match axis {
        0 => pv[1] * cv[2] - pv[2] * cv[1],
        1 => pv[2] * cv[0] - pv[0] * cv[2],
        _ => pv[0] * cv[1] - pv[1] * cv[0],
    };
    if cr >= 0.0 { ang } else { -ang }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] { [a[0]-b[0], a[1]-b[1], a[2]-b[2]] }
fn norm(v: [f32; 3]) -> f32 { (v[0]*v[0] + v[1]*v[1] + v[2]*v[2]).sqrt() }
