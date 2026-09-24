//! rigged — cut a voxel body into parts, and turn a posed rig into the
//! matrices that draw them.
//!
//! The cats were one rigid mesh each, animated by sliding the whole thing up
//! and down. `creature-rig` has posed box anatomy for a while and `anim.rs`
//! has chosen gaits for it, but nothing joined them to a voxel body, so
//! neither was reachable from the screen. This is the join.
//!
//! ## Three layers, and what each one is allowed to know
//!
//! 1. [`CutPlan`] — DATA: boxes in voxel coordinates, each naming a [`Slot`].
//!    It knows nothing about posing. It is data rather than a closure so that
//!    it can be tested, printed, and eventually emitted by the same AI
//!    authoring path that emits camps.
//! 2. [`QuadRig`] — the rest anatomy handed to `creature_rig::pose`, in the
//!    rig's frame. It knows nothing about voxels.
//! 3. [`slot_transforms`] — posed boxes in, one `Mat4` per slot out, in MODEL
//!    space. It knows nothing about either, only how the two frames relate.
//!
//! ## The frame change is done once, by conjugation
//!
//! The rig is X-forward / Y-up / Z-right. The models face +Z, which makes
//! model-right **−X**. Every hand-derived sign for "pitch about rig Z, seen
//! from the model" is a coin flip, so none are derived: a delta transform is
//! built entirely in the rig frame and conjugated through [`rig_to_model`].
//! `tests::rotation_matrix_matches_the_rigs_own_rotate` pins the one
//! convention that does have to be transcribed by hand — the rig's yaw runs
//! from +X toward +Z, which is glam's `rotation_y(-yaw)`.
//!
//! ## Leg order is load-bearing
//!
//! `anim::Gait::leg_phases` is indexed `[FL, FR, HL, HR]` and the rig counts
//! legs in the order they appear in `parts`. Supplying them in another order
//! produces a different REAL gait rather than an error. [`QuadRig`] therefore
//! only has a constructor that takes the four legs by name.

use creature_rig::{PosedBox, RigPart, Role};
use glam::{Mat4, Vec3, Vec4};

use crate::voxel::{Grid, VoxelMesh};

/// A separately drawn piece of a quadruped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(usize)]
pub enum Slot {
    Torso = 0,
    LegFL = 1,
    LegFR = 2,
    LegHL = 3,
    LegHR = 4,
    Head = 5,
    Tail0 = 6,
    Tail1 = 7,
    Tail2 = 8,
    Tail3 = 9,
}

pub const SLOTS: usize = 10;
pub const LEG_SLOTS: [Slot; 4] = [Slot::LegFL, Slot::LegFR, Slot::LegHL, Slot::LegHR];
pub const TAIL_SLOTS: [Slot; 4] = [Slot::Tail0, Slot::Tail1, Slot::Tail2, Slot::Tail3];

// The rig emits this many chain boxes per tail; the slot table above has to
// agree or tail segments would be silently dropped.
const _: () = assert!(creature_rig::anim_rig::CHAIN_SEGMENTS == TAIL_SLOTS.len());

/// An inclusive box of voxels assigned to a slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub slot: Slot,
    pub min: [i32; 3],
    pub max: [i32; 3],
    /// `false`: the torso KEEPS these voxels too. That is how a joint is
    /// hidden — the top of a leg is duplicated inside the belly, so when the
    /// leg swings the belly has no notch and the leg has no flat stump.
    pub exclusive: bool,
}

impl Region {
    pub fn contains(&self, x: i32, y: i32, z: i32) -> bool {
        (self.min[0]..=self.max[0]).contains(&x)
            && (self.min[1]..=self.max[1]).contains(&y)
            && (self.min[2]..=self.max[2]).contains(&z)
    }
}

/// Which voxels go to which slot. Anything no region claims is torso.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CutPlan {
    pub regions: Vec<Region>,
}

impl CutPlan {
    pub fn in_slot(&self, slot: Slot, x: i32, y: i32, z: i32) -> bool {
        let mut claimed_exclusively = false;
        for r in &self.regions {
            if r.contains(x, y, z) {
                if r.slot == slot {
                    return true;
                }
                claimed_exclusively |= r.exclusive;
            }
        }
        slot == Slot::Torso && !claimed_exclusively
    }

    /// Cut `grid` into one mesh per slot.
    pub fn cut(&self, grid: &Grid, voxel: f32) -> [VoxelMesh; SLOTS] {
        std::array::from_fn(|i| {
            let slot = slot_from_index(i);
            grid.mesh_where(voxel, true, &|x, y, z| self.in_slot(slot, x, y, z))
        })
    }
}

fn slot_from_index(i: usize) -> Slot {
    [
        Slot::Torso, Slot::LegFL, Slot::LegFR, Slot::LegHL, Slot::LegHR,
        Slot::Head, Slot::Tail0, Slot::Tail1, Slot::Tail2, Slot::Tail3,
    ][i]
}

/// Rest anatomy for `creature_rig::pose`, plus where the rig's origin sits
/// in the model.
#[derive(Debug, Clone, PartialEq)]
pub struct QuadRig {
    parts: Vec<RigPart>,
    pub torso_half: [f32; 3],
    /// The rig's origin (the body centre) in MODEL space.
    pub body_centre: Vec3,
}

impl QuadRig {
    /// Legs are taken by name so the `[FL, FR, HL, HR]` order cannot be got
    /// wrong at a call site. All boxes are `(offset, half)` in the RIG frame.
    pub fn new(
        body_centre: Vec3,
        torso_half: [f32; 3],
        fl: ([f32; 3], [f32; 3]),
        fr: ([f32; 3], [f32; 3]),
        hl: ([f32; 3], [f32; 3]),
        hr: ([f32; 3], [f32; 3]),
        head: ([f32; 3], [f32; 3]),
        tail: ([f32; 3], [f32; 3]),
    ) -> QuadRig {
        let p = |role, (offset, half): ([f32; 3], [f32; 3])| RigPart { role, offset, half };
        QuadRig {
            parts: vec![
                p(Role::Leg, fl), p(Role::Leg, fr), p(Role::Leg, hl), p(Role::Leg, hr),
                p(Role::Head, head), p(Role::Tail, tail),
            ],
            torso_half,
            body_centre,
        }
    }

    pub fn parts(&self) -> &[RigPart] {
        &self.parts
    }

    /// Hip of leg `i` (`[FL, FR, HL, HR]`) in the rig frame: top-centre of
    /// the rest box, which is where `creature_rig::pose` pivots it.
    pub fn hip(&self, i: usize) -> [f32; 3] {
        let p = &self.parts[i];
        [p.offset[0], p.offset[1] + p.half[1], p.offset[2]]
    }

    /// Rest sole height of leg `i` below the body centre (a negative number).
    pub fn sole(&self, i: usize) -> f32 {
        let p = &self.parts[i];
        p.offset[1] - p.half[1]
    }

    /// Root of the tail in the rig frame: the end nearest the body.
    pub fn tail_root(&self) -> [f32; 3] {
        let p = &self.parts[5];
        [p.offset[0] + p.half[0], p.offset[1], p.offset[2]]
    }

    /// Rest centre of tail chain segment `k`, mirroring `anim_rig::chain`:
    /// the box is split along its long axis from the root outward.
    fn tail_rest_centre(&self, k: usize) -> [f32; 3] {
        let p = &self.parts[5];
        let seg = p.half[0] * 2.0 / TAIL_SLOTS.len() as f32;
        let root = self.tail_root();
        [root[0] - seg * (k as f32 + 0.5), root[1], root[2]]
    }
}

/// rig (X fwd, Y up, Z right) → model (+Z fwd, Y up, −X right). A proper
/// rotation (det +1), so rotations conjugate to rotations.
pub fn rig_to_model() -> Mat4 {
    Mat4::from_cols(
        Vec4::new(0.0, 0.0, 1.0, 0.0),
        Vec4::new(0.0, 1.0, 0.0, 0.0),
        Vec4::new(-1.0, 0.0, 0.0, 0.0),
        Vec4::W,
    )
}

/// The rig's `[roll X, pitch Z, yaw Y]` as a matrix, matching
/// `creature_rig::anim_rig::rotate` exactly (see the module header on yaw).
pub fn rig_rotation(rot: [f32; 3]) -> Mat4 {
    Mat4::from_rotation_y(-rot[2]) * Mat4::from_rotation_z(rot[1]) * Mat4::from_rotation_x(rot[0])
}

/// Extra per-leg control layered on the rig's gait, applied about the hip.
///
/// The rig swings legs by gait phase and knows nothing else. Reaching over a
/// root, a forepaw swipe, and tucking up in flight are all "this leg, more",
/// so they are expressed as a delta on top rather than by forking the rig.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LegAdjust {
    /// Added pitch, radians. Positive swings the foot FORWARD.
    pub pitch: f32,
    /// Fraction of leg length folded away, `-0.3..=0.7`. The hip stays put
    /// and the foot rises; negative REACHES, for a paw feeling for ground
    /// lower than the body expects.
    pub fold: f32,
    /// Splay outward from the body, radians.
    pub splay: f32,
    /// Where the sole must BE, in the rig frame after the whole-body
    /// adjustments (see [`world_to_rig`]). When set, the leg is solved to
    /// reach it from its hip and `pitch`/`fold`/`splay` are ignored: a
    /// planted paw is placed, not swung (`footfall`).
    pub plant: Option<[f32; 3]>,
}

/// Whole-body and tail deltas from the locomotion layer.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BodyAdjust {
    /// Metres the body centre drops (stalk, gather, landing).
    pub drop: f32,
    /// Nose-up positive, radians.
    pub pitch: f32,
    /// Right-side-down positive, radians.
    pub roll: f32,
    /// Tail raised about its root, radians.
    pub tail_lift: f32,
    /// Tail swung sideways about its root, radians.
    pub tail_lash: f32,
    /// Front half vs hind half vertical shear (spine arch in flight).
    pub arch: f32,
    /// Whole body turned about the vertical, radians, positive to the
    /// RIGHT. With `sway` this is the slither: a body curving round a stem.
    pub yaw: f32,
    /// Whole body shifted sideways, world units, positive to the right.
    pub sway: f32,
    /// Front half rolled this much FURTHER toward upright than the hind
    /// half, radians. The air-righting reflex is two turns: head and
    /// forelegs come round first, the hindquarters follow.
    pub twist: f32,
    /// Where pitch and roll pivot, `[x, y]` in the rig frame. The body
    /// centre for a tilt over terrain; the hind hips for a rear, or the
    /// haunches sink through the floor as the chest comes up.
    pub pivot: [f32; 2],
}

/// One model-space matrix per slot for this pose.
///
/// `posed` is `creature_rig::pose`'s output for `rig.parts()`. A slot the
/// rig did not emit keeps the identity, which draws it at rest rather than
/// not at all — a missing limb is a worse failure than a stiff one.
pub fn slot_transforms(rig: &QuadRig, posed: &[PosedBox], legs: &[LegAdjust; 4], body: &BodyAdjust) -> [Mat4; SLOTS] {
    let mut rig_space = [Mat4::IDENTITY; SLOTS];
    let mut tail_k = 0usize;
    for b in posed {
        let (slot, rest_c, rest_h, scaled) = match b.source {
            None => (Slot::Torso, [0.0; 3], rig.torso_half, true),
            Some(i) => {
                let Some(p) = rig.parts.get(i) else { continue };
                match p.role {
                    Role::Leg if i < 4 => (LEG_SLOTS[i], p.offset, p.half, true),
                    Role::Head => (Slot::Head, p.offset, p.half, false),
                    Role::Tail => {
                        if tail_k >= TAIL_SLOTS.len() {
                            continue;
                        }
                        let k = tail_k;
                        tail_k += 1;
                        // Tail segments taper in the rig; the voxels do not
                        // want squeezing, so no scale.
                        (TAIL_SLOTS[k], rig.tail_rest_centre(k), p.half, false)
                    }
                    _ => continue,
                }
            }
        };
        let scale = if scaled {
            Vec3::new(ratio(b.half[0], rest_h[0]), ratio(b.half[1], rest_h[1]), ratio(b.half[2], rest_h[2]))
        } else {
            Vec3::ONE
        };
        rig_space[slot as usize] = Mat4::from_translation(Vec3::from_array(b.center))
            * rig_rotation(b.rot)
            * Mat4::from_scale(scale)
            * Mat4::from_translation(-Vec3::from_array(rest_c));
    }

    // Whole body: arch shears front up / hind down about the centre, then
    // pitch/roll/drop. Applied to every slot, so the cat moves as one.
    let arch = Mat4::from_cols(
        Vec4::new(1.0, body.arch, 0.0, 0.0),
        Vec4::Y,
        Vec4::Z,
        Vec4::W,
    );
    let pivot = Vec3::new(body.pivot[0], body.pivot[1], 0.0);
    let whole = Mat4::from_translation(Vec3::new(0.0, -body.drop, body.sway) + pivot)
        * rig_rotation([body.roll, body.pitch, body.yaw])
        * Mat4::from_translation(-pivot)
        * arch;

    // The twist splits the body along the spine (the rig X axis through the
    // centre): front slots lead by half of it, hind slots lag by half. The
    // torso is one mesh and takes neither, which is where a real cat's spine
    // would be doing the work.
    let half = |sign: f32| rig_rotation([-body.twist * 0.5 * sign, 0.0, 0.0]);
    let (front, hind) = (half(1.0), half(-1.0));
    // Leg deltas, about the hip.
    for (i, adj) in legs.iter().enumerate() {
        if *adj == LegAdjust::default() {
            continue;
        }
        if let Some(target) = adj.plant {
            // Undo what will be done to this leg after its own transform --
            // the whole-body tilt and this half's twist -- so the leg is
            // solved in its own frame and lands on the target once they are
            // applied.
            let part = if i < 2 { front } else { hind };
            let pre = (whole * part).inverse().transform_point3(Vec3::from_array(target));
            if let Some(m) = plant_leg(rig, i, pre) {
                rig_space[LEG_SLOTS[i] as usize] = m;
                continue;
            }
        }
        let hip = Vec3::from_array(rig.hip(i));
        // Outward is −Z for a left leg and +Z for a right one. Roll about
        // +X by θ sends a foot hanging at −Y to z = +sin θ... no: to
        // z = −h·sin θ with y = −h, i.e. POSITIVE z for NEGATIVE θ. So a
        // right leg (side +1) splays with −θ. I had this backwards and
        // `leg_adjust_pitch_is_forward_fold_is_up_splay_is_out` said so.
        let side = if rig.parts[i].offset[2] >= 0.0 { 1.0 } else { -1.0 };
        let m = Mat4::from_translation(hip)
            * rig_rotation([-adj.splay * side, adj.pitch, 0.0])
            * Mat4::from_scale(Vec3::new(1.0, 1.0 - adj.fold.clamp(-0.3, 0.7), 1.0))
            * Mat4::from_translation(-hip);
        let s = LEG_SLOTS[i] as usize;
        rig_space[s] = m * rig_space[s];
    }

    // Tail deltas, about the root, applied to every segment so the chain
    // stays joined.
    if body.tail_lift != 0.0 || body.tail_lash != 0.0 {
        let root = Vec3::from_array(rig.tail_root());
        // The tail points −X, so raising the tip is a NEGATIVE pitch about Z.
        let m = Mat4::from_translation(root)
            * rig_rotation([0.0, -body.tail_lift, body.tail_lash])
            * Mat4::from_translation(-root);
        for s in TAIL_SLOTS {
            rig_space[s as usize] = m * rig_space[s as usize];
        }
    }

    let c = rig_to_model();
    let to_model = Mat4::from_translation(rig.body_centre) * c;
    let from_model = c.transpose() * Mat4::from_translation(-rig.body_centre);
    std::array::from_fn(|i| {
        let part = match slot_from_index(i) {
            Slot::Head | Slot::LegFL | Slot::LegFR => front,
            Slot::Torso => Mat4::IDENTITY,
            _ => hind,
        };
        to_model * whole * part * rig_space[i] * from_model
    })
}

/// The rest leg `i`, turned about its hip to point at `sole` and stretched
/// or shortened to reach it. One bone, so the solve is exact: direction and
/// length, nothing to iterate. `None` for a target that is not finite or is
/// on the hip itself. Length is held to what a voxel leg can show
/// (`0.3..=1.3` of rest), so an impossible target bends the leg toward it
/// rather than tearing it off the body.
pub fn plant_leg(rig: &QuadRig, i: usize, sole: Vec3) -> Option<Mat4> {
    let p = rig.parts.get(i)?;
    let hip = Vec3::from_array(rig.hip(i));
    let rest_len = (p.half[1] * 2.0).max(1e-3);
    let d = sole - hip;
    let len = d.length();
    if !(len.is_finite() && len > 1e-4) {
        return None;
    }
    let rot = glam::Quat::from_rotation_arc(Vec3::NEG_Y, d / len);
    let k = (len / rest_len).clamp(0.3, 1.3);
    Some(Mat4::from_translation(hip) * Mat4::from_quat(rot) * Mat4::from_scale(Vec3::new(1.0, k, 1.0)) * Mat4::from_translation(-hip))
}

/// A world point in the rig frame [`LegAdjust::plant`] wants, given the
/// model-to-world matrix the cat is drawn with.
pub fn world_to_rig(rig: &QuadRig, model_to_world: Mat4, p: Vec3) -> [f32; 3] {
    let c = rig_to_model();
    let from_model = c.transpose() * Mat4::from_translation(-rig.body_centre);
    (from_model * model_to_world.inverse()).transform_point3(p).to_array()
}

fn ratio(a: f32, b: f32) -> f32 {
    if b.abs() < 1e-6 || !a.is_finite() { 1.0 } else { (a / b).clamp(0.25, 4.0) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use creature_rig::{pose, MotionInput};

    fn cat() -> QuadRig {
        let leg = |x: f32, z: f32| ([x, -0.7, z], [0.1, 0.45, 0.08]);
        QuadRig::new(
            Vec3::new(0.0, 1.2, 0.0),
            [0.9, 0.37, 0.35],
            leg(0.55, -0.25), leg(0.55, 0.25), leg(-0.5, -0.25), leg(-0.5, 0.25),
            ([1.05, 0.4, 0.0], [0.27, 0.25, 0.25]),
            ([-1.1, 0.15, 0.0], [0.22, 0.07, 0.07]),
        )
    }

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-4
    }

    /// The one hand-transcribed convention. If the rig's `rotate` and this
    /// matrix disagree, every limb bends the wrong way and nothing else
    /// would say so.
    #[test]
    fn rotation_matrix_matches_the_rigs_own_rotate() {
        let mut checked = 0;
        for rot in [[0.3, 0.0, 0.0], [0.0, 0.5, 0.0], [0.0, 0.0, 0.7], [0.2, -0.4, 1.1]] {
            for v in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.3, -0.8, 0.5]] {
                let want = Vec3::from_array(creature_rig::anim_rig::rotate(v, rot));
                let got = rig_rotation(rot).transform_vector3(Vec3::from_array(v));
                assert!(close(want, got), "rot {rot:?} v {v:?}: rig {want} vs matrix {got}");
                checked += 1;
            }
        }
        assert_eq!(checked, 16);
    }

    #[test]
    fn frame_change_is_a_proper_rotation_and_forward_is_plus_z() {
        let c = rig_to_model();
        assert!((c.determinant() - 1.0).abs() < 1e-6, "a reflection would mirror every pose");
        assert!(close(c.transform_vector3(Vec3::X), Vec3::Z), "rig forward is model +Z");
        assert!(close(c.transform_vector3(Vec3::Z), -Vec3::X), "rig right is model -X");
    }

    /// At rest every slot must be (near) identity, or the cut body would not
    /// line up with the uncut one. Breath moves the torso a hair; nothing
    /// may move by more than that.
    #[test]
    fn a_standing_cat_is_drawn_where_it_was_authored() {
        let rig = cat();
        let mut out = Vec::new();
        pose(rig.parts(), rig.torso_half, &MotionInput::default(), None, &mut out);
        let m = slot_transforms(&rig, &out, &[LegAdjust::default(); 4], &BodyAdjust::default());
        let probe = Vec3::new(0.2, 0.9, 0.4);
        for (i, t) in m.iter().enumerate() {
            let moved = (t.transform_point3(probe) - probe).length();
            assert!(moved < 0.03, "slot {i} moved {moved} at rest");
        }
    }

    /// A walking cat's legs must actually differ from one another and from
    /// rest — the failure this module exists to end is "all slots identity".
    #[test]
    fn walking_moves_the_feet_and_not_the_hips() {
        let rig = cat();
        let mut out = Vec::new();
        let m_in = MotionInput { speed: 5.0, gait_phase: 1.0, leg_phases: Some(crate::anim::Gait::Trot.leg_phases()), ..MotionInput::default() };
        pose(rig.parts(), rig.torso_half, &m_in, None, &mut out);
        let m = slot_transforms(&rig, &out, &[LegAdjust::default(); 4], &BodyAdjust::default());
        let c = rig_to_model();
        let mut foot_travel = [0.0f32; 4];
        for i in 0..4 {
            let hip = rig.body_centre + c.transform_vector3(Vec3::from_array(rig.hip(i)));
            let sole = hip + Vec3::new(0.0, -0.9, 0.0);
            let t = m[LEG_SLOTS[i] as usize];
            foot_travel[i] = (t.transform_point3(sole) - sole).length();
            // The rig RETRACTS a swinging leg upward into the torso (its
            // SWING_LIFT), so the hip legitimately rises. What it must not
            // do is slide along the body or sink below it.
            let d = t.transform_point3(hip) - hip;
            assert!(d.x.abs() < 0.02 && d.z.abs() < 0.02, "leg {i} hip slid along the body: {d}");
            assert!((-0.13..0.32).contains(&d.y), "leg {i} hip left the body vertically: {d}");
        }
        assert!(foot_travel.iter().all(|d| *d > 0.05), "feet did not move: {foot_travel:?}");
        // Trot: diagonal pairs match, lateral pairs do not.
        let fl = m[Slot::LegFL as usize];
        let hr = m[Slot::LegHR as usize];
        let fr = m[Slot::LegFR as usize];
        let z = Vec3::Z * 0.3;
        assert!(close(fl.transform_vector3(z), hr.transform_vector3(z)), "FL and HR swing together in a trot");
        assert!(!close(fl.transform_vector3(z), fr.transform_vector3(z)), "FL and FR must not");
    }

    /// A planted sole lands where it was told, in WORLD space, through the
    /// whole chain the scene draws with: body pitch, roll, drop, twist, and
    /// the model-to-world placement. A plant solved in the wrong frame is
    /// right at rest and wrong the moment the body tilts, so it is checked
    /// tilted.
    #[test]
    fn a_planted_sole_lands_on_its_world_target_under_a_tilted_body() {
        let rig = cat();
        let mut out = Vec::new();
        pose(rig.parts(), rig.torso_half, &MotionInput { speed: 4.0, gait_phase: 0.7, ..Default::default() }, None, &mut out);
        let body = BodyAdjust { drop: 0.12, pitch: 0.2, roll: -0.1, twist: 0.15, arch: 0.05, ..Default::default() };
        let base = Mat4::from_translation(Vec3::new(10.0, 0.3, -4.0)) * Mat4::from_axis_angle(Vec3::Y, 0.6);
        let c = rig_to_model();
        let rest_sole = |i: usize| rig.body_centre + c.transform_vector3(Vec3::new(rig.hip(i)[0], rig.sole(i), rig.hip(i)[2]));
        let mut legs = [LegAdjust::default(); 4];
        let mut targets = [Vec3::ZERO; 4];
        for i in 0..4 {
            // Somewhere near where the paw is at rest, pushed forward a bit.
            let w = base.transform_point3(rest_sole(i)) + Vec3::new(0.15, -0.05, 0.1);
            targets[i] = w;
            // The puppet ALSO computes a fold and pitch for a walking leg;
            // a plant must win over them, not be applied beneath them.
            legs[i] = LegAdjust { pitch: 0.3, fold: 0.2, splay: 0.1, plant: Some(world_to_rig(&rig, base, w)) };
        }
        let m = slot_transforms(&rig, &out, &legs, &body);
        for i in 0..4 {
            let got = base.transform_point3(m[LEG_SLOTS[i] as usize].transform_point3(rest_sole(i)));
            assert!((got - targets[i]).length() < 1e-3, "leg {i}: sole at {got}, told {}", targets[i]);
        }
        // A plant beyond reach bends toward it and stays attached.
        let far = base.transform_point3(rest_sole(0)) + Vec3::new(3.0, -2.0, 0.0);
        let mut legs = [LegAdjust::default(); 4];
        legs[0].plant = Some(world_to_rig(&rig, base, far));
        let m = slot_transforms(&rig, &out, &legs, &BodyAdjust::default());
        let hip_m = rig.body_centre + c.transform_vector3(Vec3::from_array(rig.hip(0)));
        let moved_hip = (m[LEG_SLOTS[0] as usize].transform_point3(hip_m) - hip_m).length();
        assert!(moved_hip < 1e-3, "the hip stays on the body: {moved_hip}");
        assert!(m[LEG_SLOTS[0] as usize].is_finite());
    }

    #[test]
    fn leg_adjust_pitch_is_forward_fold_is_up_splay_is_out() {
        let rig = cat();
        let mut out = Vec::new();
        pose(rig.parts(), rig.torso_half, &MotionInput::default(), None, &mut out);
        let c = rig_to_model();
        let sole = |i: usize| rig.body_centre + c.transform_vector3(Vec3::new(rig.hip(i)[0], rig.sole(i), rig.hip(i)[2]));
        let run = |adj: LegAdjust, i: usize| {
            let mut legs = [LegAdjust::default(); 4];
            legs[i] = adj;
            let m = slot_transforms(&rig, &out, &legs, &BodyAdjust::default());
            m[LEG_SLOTS[i] as usize].transform_point3(sole(i)) - sole(i)
        };
        assert!(run(LegAdjust { pitch: 0.5, ..Default::default() }, 0).z > 0.2, "positive pitch reaches forward (+Z)");
        assert!(run(LegAdjust { fold: 0.5, ..Default::default() }, 0).y > 0.3, "fold lifts the foot");
        // FL is the LEFT leg: model-left is +X.
        assert!(run(LegAdjust { splay: 0.4, ..Default::default() }, 0).x > 0.1, "left leg splays to model +X");
        assert!(run(LegAdjust { splay: 0.4, ..Default::default() }, 1).x < -0.1, "right leg splays to model -X");
    }

    #[test]
    fn body_adjust_directions() {
        let rig = cat();
        let mut out = Vec::new();
        pose(rig.parts(), rig.torso_half, &MotionInput::default(), None, &mut out);
        let nose = rig.body_centre + Vec3::Z * 1.2;
        let tip = rig.body_centre + Vec3::new(0.0, 0.15, -1.3);
        let run = |b: BodyAdjust, slot: Slot, p: Vec3| {
            let m = slot_transforms(&rig, &out, &[LegAdjust::default(); 4], &b);
            let rest = slot_transforms(&rig, &out, &[LegAdjust::default(); 4], &BodyAdjust::default());
            m[slot as usize].transform_point3(p) - rest[slot as usize].transform_point3(p)
        };
        assert!(run(BodyAdjust { pitch: 0.3, ..Default::default() }, Slot::Head, nose).y > 0.2, "pitch is nose-up");
        assert!(run(BodyAdjust { drop: 0.3, ..Default::default() }, Slot::Torso, nose).y < -0.29);
        assert!(run(BodyAdjust { tail_lift: 0.6, ..Default::default() }, Slot::Tail3, tip).y > 0.05, "tail_lift raises the tip");
        let lash = run(BodyAdjust { tail_lash: 0.6, ..Default::default() }, Slot::Tail3, tip);
        assert!(lash.x.abs() > 0.05, "tail_lash swings it sideways, got {lash}");
    }

    #[test]
    fn cut_plan_shares_and_excludes() {
        let plan = CutPlan {
            regions: vec![
                Region { slot: Slot::LegFL, min: [0, 0, 0], max: [1, 3, 1], exclusive: true },
                Region { slot: Slot::LegFL, min: [0, 4, 0], max: [1, 5, 1], exclusive: false },
            ],
        };
        assert!(plan.in_slot(Slot::LegFL, 0, 2, 0) && !plan.in_slot(Slot::Torso, 0, 2, 0), "exclusive: leg only");
        assert!(plan.in_slot(Slot::LegFL, 0, 5, 0) && plan.in_slot(Slot::Torso, 0, 5, 0), "shared: both, hides the joint");
        assert!(plan.in_slot(Slot::Torso, 9, 9, 9) && !plan.in_slot(Slot::Head, 9, 9, 9), "unclaimed is torso");
    }
}
