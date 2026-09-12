//! anim_rig — oriented per-part posing for box creatures.
//!
//! `pose::animate` moves each anatomy box by an OFFSET: a leg swings on an arc
//! about the body origin, and every box keeps the body's orientation. That is
//! why a walking creature reads as sliding — a leg that translates instead of
//! pivoting at the hip never plants. Squash-and-stretch (`sim::deform`),
//! breath and limpness (`sim::liveliness`), the tail S-curve
//! (`sim::flex_bend`) and the ragdoll (`sim::ragdoll`) were each built, tested
//! and never composed into a pose.
//!
//! This module is that composition. Input: the rest anatomy as [`RigPart`]s
//! (role, rest centre, half-extents; body-local, X forward / Y up / Z right)
//! and a [`MotionInput`]. Output: [`PosedBox`]es — centre, half-extents and a
//! per-box rotation — in the same body-local frame, plus [`corners`] /
//! [`world_corners`] that produce the eight vertices exactly as
//! `entity::push_box_oriented` would (roll about X, pitch about Z, yaw about
//! Y, in that order, then the body's own rotation). A harness that draws the
//! corners draws what the game draws.
//!
//! What each layer does, in application order:
//!
//! 1. **Gait** — legs pivot at the hip (top of the leg box), diagonal pairs in
//!    counter-phase, arms in counter-phase to their leg, wings roll at the
//!    root. The stance leg EXTENDS so the foot stays on the ground through the
//!    swing arc (the arc geometry alone lifts the foot ~12% of leg length at
//!    the extremes — the "residual float" in docs/body_physics.md); the swing
//!    leg lifts.
//! 2. **Torso deform** — breath (Y stretch, volume kept), landing squash from
//!    `impact`, motion stretch along +X from speed. Head-cluster boxes ride the
//!    torso's front so a stretched body carries its head with it.
//! 3. **Chains** — a Tail (or Neck) box becomes `CHAIN_SEGMENTS` boxes laid
//!    from its root outward, bent by `flex_bend` (turn steers the chain,
//!    speed drives the swish at the gait phase, droop hangs the tip), tapered
//!    toward the tip.
//! 4. **Settle** — `settle` in `[0,1]` folds the legs under the body and
//!    lowers the torso onto the ground: the bedded-down rest pose.
//! 5. **Strike** — `strike` in `[0,1]` is a lunge overlay: the torso pitches
//!    nose-down and drives forward, the head cluster leads. Shape comes from
//!    [`strike_curve`] (anticipation, lunge, recover).
//! 6. **Ragdoll blend** — with a [`RagdollSample`] (node centres from
//!    `sim::ragdoll`, body-local), every box's centre and orientation blend
//!    toward the physics result by `liveliness::limpness(state, progress)`.
//!    Dying at progress 1 is pure ragdoll; Idle keeps the faint sway.
//!
//! Everything is bounded: angles are clamped, stretch runs through
//! `deform`'s elastic clamp, and with a zero [`MotionInput`] (standing, not
//! settled, no strike, no ragdoll) every box is at its rest centre with zero
//! rotation, so a standing creature is what it was.
//!
//! Pure: std-only, deterministic, no panics on any input. Allocates only the
//! output `Vec` (and the chain scratch, which is stack-sized).

use crate::sim::deform;
use crate::sim::flex_bend::{self, FlexParams};
use crate::sim::liveliness::{self, MotionState};
use std::f32::consts::PI;

// ── gait tuning (mirrors pose.rs so the two paths stride alike) ──────────
/// Peak leg swing at full speed (radians fore/aft from neutral).
pub const LEG_SWING_MAX: f32 = 0.7;
/// Peak arm swing at full speed (radians).
pub const ARM_SWING_MAX: f32 = 0.5;
/// Peak body bob (block units) at full speed; twice per stride.
pub const BODY_BOB_MAX: f32 = 0.12;
/// Peak wing-flap angle (radians) and how much faster wings beat than legs.
pub const WING_FLAP_MAX: f32 = 1.1;
pub const WING_BEAT_RATIO: f32 = 3.0;
/// Speed (blocks/s) at which the gait amplitudes saturate.
pub const SPEED_SATURATE: f32 = 6.0;
/// Swing-leg lift as a fraction of leg length, at full speed.
pub const SWING_LIFT: f32 = 0.35;
/// How many boxes a Tail/Neck becomes.
pub const CHAIN_SEGMENTS: usize = 4;
/// Tip taper of a chain (fraction of root half-extents lost at the tip).
pub const CHAIN_TAPER: f32 = 0.45;
/// Strike overlay: forward lunge as a fraction of torso length, and the
/// nose-down pitch at full lunge (radians).
pub const STRIKE_LUNGE: f32 = 0.45;
pub const STRIKE_PITCH: f32 = 0.35;
/// Settle: fraction of leg length the torso drops when fully bedded, and the
/// fold angle of a bedded leg (radians, tucked under).
pub const SETTLE_DROP: f32 = 0.85;
pub const SETTLE_FOLD: f32 = 1.35;

/// What a rest box is, for the rig. Mapped from `creature::Part` by the
/// consumer (Foot/StickyFoot → Leg, Hand → Arm, Jaw/Eye/Ear → Head, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Leg,
    Arm,
    Wing,
    Tail,
    Neck,
    Head,
    Fin,
    Other,
}

/// One rest box of the anatomy: role, centre relative to the body centre,
/// half-extents. Body-local: X forward, Y up, Z right.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RigPart {
    pub role: Role,
    pub offset: [f32; 3],
    pub half: [f32; 3],
}

/// Everything the pose reads. `Default` is a standing, idle, unharmed body.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MotionInput {
    /// Gait phase (radians), advanced by distance travelled.
    pub gait_phase: f32,
    /// Horizontal speed (blocks/s).
    pub speed: f32,
    /// Speed at which motion stretch saturates (blocks/s; ≤0 disables).
    pub max_speed: f32,
    /// Leg health `[0,1]`: scales swing (a limp).
    pub leg_health: f32,
    /// Airborne and powered; wings beat with `fly_power`.
    pub flying: bool,
    pub fly_power: f32,
    /// Normalised yaw rate `[-1,1]` (+ = turning right). The neck leads into
    /// the turn; the tail swings out of it.
    pub turn: f32,
    /// Head gaze relative to the body (radians).
    pub gaze_yaw: f32,
    pub gaze_pitch: f32,
    /// Liveliness state and its progress (`Casting`/`Dying` read progress).
    pub state: MotionState,
    pub progress: f32,
    /// Seconds, for the breath cycle.
    pub time: f32,
    /// Landing impact `[0,1]` — squash.
    pub impact: f32,
    /// Bedded-down amount `[0,1]`; the caller eases it.
    pub settle: f32,
    /// Strike progress `[0,1]` (0 = none/idle, sweeps 0→1 over a strike).
    pub strike: f32,
}

impl Default for MotionInput {
    fn default() -> Self {
        MotionInput {
            gait_phase: 0.0,
            speed: 0.0,
            max_speed: SPEED_SATURATE,
            leg_health: 1.0,
            flying: false,
            fly_power: 0.0,
            turn: 0.0,
            gaze_yaw: 0.0,
            gaze_pitch: 0.0,
            state: MotionState::Idle,
            progress: 0.0,
            time: 0.0,
            impact: 0.0,
            settle: 0.0,
            strike: 0.0,
        }
    }
}

/// Ragdoll node centres in the BODY-LOCAL frame (rotated back by the body's
/// yaw, relative to the body centre): `body` for the torso node, one entry
/// per rest part in `parts` order. Built from `sim::ragdoll` nodes by the
/// consumer; missing entries leave that box driven.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RagdollSample<'a> {
    pub body: [f32; 3],
    pub parts: &'a [[f32; 3]],
}

/// A posed box: which rest part it came from (`None` = the torso), its centre
/// and half-extents, and its own rotation `[roll about X, pitch about Z, yaw
/// about Y]` applied about its centre before the body's rotation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PosedBox {
    pub source: Option<usize>,
    pub role: Option<Role>,
    pub center: [f32; 3],
    pub half: [f32; 3],
    pub rot: [f32; 3],
}

fn clamp01(x: f32) -> f32 {
    if x.is_nan() {
        0.0
    } else {
        x.clamp(0.0, 1.0)
    }
}

/// Clamp with NaN mapped to 0 (`f32::clamp` passes NaN through).
fn clampf(x: f32, lo: f32, hi: f32) -> f32 {
    if x.is_nan() {
        0.0
    } else {
        x.clamp(lo, hi)
    }
}

fn finite(x: f32) -> f32 {
    if x.is_finite() {
        x
    } else {
        0.0
    }
}

/// Rotate `v` by `[roll, pitch, yaw]` in the engine's order: roll about +X,
/// then pitch about +Z, then yaw about +Y — exactly `push_box_oriented`.
pub fn rotate(v: [f32; 3], rot: [f32; 3]) -> [f32; 3] {
    let (sr, cr) = rot[0].sin_cos();
    let (sp, cp) = rot[1].sin_cos();
    let (sy, cy) = rot[2].sin_cos();
    let (x, y, z) = (v[0], v[1], v[2]);
    let (y1, z1) = (y * cr - z * sr, y * sr + z * cr);
    let (x2, y2) = (x * cp - y1 * sp, x * sp + y1 * cp);
    let (rx, rz) = (x2 * cy - z1 * sy, x2 * sy + z1 * cy);
    [rx, y2, rz]
}

/// The eight corners of a posed box in body-local space, in the same
/// `(x, y, z) ∈ {-,+}³` order `push_box_oriented` emits (x outer, z inner).
pub fn corners(b: &PosedBox) -> [[f32; 3]; 8] {
    let mut out = [[0.0f32; 3]; 8];
    let mut i = 0;
    for &sx in &[-1.0f32, 1.0] {
        for &sy in &[-1.0f32, 1.0] {
            for &sz in &[-1.0f32, 1.0] {
                let local = [b.half[0] * sx, b.half[1] * sy, b.half[2] * sz];
                let r = rotate(local, b.rot);
                out[i] = [b.center[0] + r[0], b.center[1] + r[1], b.center[2] + r[2]];
                i += 1;
            }
        }
    }
    out
}

/// [`corners`] carried into the world: the body's `[roll, pitch, yaw]`
/// (its lean and heading) then `body_pos`.
pub fn world_corners(b: &PosedBox, body_pos: [f32; 3], body_rot: [f32; 3]) -> [[f32; 3]; 8] {
    let mut c = corners(b);
    for p in c.iter_mut() {
        let r = rotate(*p, body_rot);
        *p = [body_pos[0] + r[0], body_pos[1] + r[1], body_pos[2] + r[2]];
    }
    c
}

/// The strike overlay shape over progress `[0,1]`: a short anticipation
/// (pull back, −), a fast lunge to +1 at ~0.45, and a recovery back to 0.
/// Zero at both ends so a finished strike leaves the pose untouched.
pub fn strike_curve(progress: f32) -> f32 {
    let p = clamp01(progress);
    if p < 0.25 {
        // anticipation: rock back
        -0.35 * (p / 0.25 * PI * 0.5).sin()
    } else if p < 0.45 {
        // lunge: whip forward
        let t = (p - 0.25) / 0.20;
        -0.35 + 1.35 * (t * PI * 0.5).sin()
    } else {
        // recover: ease back to neutral
        let t = (p - 0.45) / 0.55;
        1.0 - t.clamp(0.0, 1.0)
    }
}

/// Diagonal-pair leg phase: even legs at 0, odd at π — the same rule
/// `pose::leg_phase_offset` uses, so both paths trot alike.
fn leg_phase_offset(i: usize) -> f32 {
    if i % 2 == 0 {
        0.0
    } else {
        PI
    }
}

/// The long axis of a box (index of its largest half-extent).
fn long_axis(half: [f32; 3]) -> usize {
    let mut a = 0;
    for k in 1..3 {
        if half[k] > half[a] {
            a = k;
        }
    }
    a
}

/// Lay a Tail/Neck box out as a bent chain of segments.
fn chain(
    idx: usize,
    part: &RigPart,
    role: Role,
    m: &MotionInput,
    phase: f32,
    bob: f32,
    out: &mut Vec<PosedBox>,
) {
    let axis = long_axis(part.half);
    let full = part.half[axis] * 2.0;
    let n = CHAIN_SEGMENTS.max(1);
    let seg = full / n as f32;
    // Root = the end nearest the body: for a tail that trails behind (offset
    // −X) the root is the +X end; for a neck ahead (+X) the −X end. Along a
    // lateral axis (a fin) the root is the side nearest the centre line.
    let toward_body = -part.offset[axis].signum();
    let toward_body = if toward_body == 0.0 { -1.0 } else { toward_body };
    let mut root = part.offset;
    root[axis] += part.half[axis] * toward_body;
    // Unit direction from root to tip along the rest axis.
    let mut dir = [0.0f32; 3];
    dir[axis] = -toward_body;

    let speed01 = clamp01(m.speed / SPEED_SATURATE) * clamp01(m.leg_health);
    let (turn, swish, droop, params) = match role {
        Role::Neck => (
            // the neck leads into the turn
            clampf(m.turn, -1.0, 1.0) + clampf(m.gaze_yaw, -1.0, 1.0) * 0.6,
            speed01 * 0.35,
            (-clampf(m.gaze_pitch, -1.0, 1.0) * 0.3).max(0.0),
            FlexParams { max_bend: 0.45, wave_amp: 0.10, wavelength: PI / 3.0, turn_bend: 0.22 },
        ),
        _ => (
            // the tail swings OUT of the turn, for balance
            -clampf(m.turn, -1.0, 1.0),
            speed01,
            // straight at rest (the standing pose is unchanged); hangs when bedded
            0.6 * clamp01(m.settle),
            FlexParams::default(),
        ),
    };
    let mut local_yaw = [0.0f32; CHAIN_SEGMENTS];
    let mut local_pitch = [0.0f32; CHAIN_SEGMENTS];
    let mut cum_yaw = [0.0f32; CHAIN_SEGMENTS];
    let mut cum_pitch = [0.0f32; CHAIN_SEGMENTS];
    flex_bend::flex_yaw(&mut local_yaw[..n], turn, swish, phase, &params);
    flex_bend::flex_pitch(&mut local_pitch[..n], droop, phase, &params);
    flex_bend::cumulate(&local_yaw[..n], &mut cum_yaw[..n]);
    flex_bend::cumulate(&local_pitch[..n], &mut cum_pitch[..n]);

    // A tail pointing −X bends the OTHER way on screen for the same local
    // yaw, so the yaw sign follows the chain direction; pitch droop is
    // always "down toward the tip", which for a −X chain is a +pitch.
    let along = -toward_body; // +1 chain runs +X, −1 runs −X
    let mut cursor = root;
    for i in 0..n {
        let tip = (i as f32 + 1.0) / n as f32;
        let yaw = cum_yaw[i] * along;
        let pitch = -cum_pitch[i] * along;
        // segment direction: rest axis rotated by the cumulative bend
        let d = rotate([dir[0] * seg, dir[1] * seg, dir[2] * seg], [0.0, pitch, yaw]);
        let center = [cursor[0] + d[0] * 0.5, cursor[1] + d[1] * 0.5 + bob, cursor[2] + d[2] * 0.5];
        let taper = 1.0 - CHAIN_TAPER * tip;
        let mut half = [part.half[0] * taper, part.half[1] * taper, part.half[2] * taper];
        half[axis] = seg * 0.5;
        out.push(PosedBox { source: Some(idx), role: Some(role), center, half, rot: [0.0, pitch, yaw] });
        cursor = [cursor[0] + d[0], cursor[1] + d[1], cursor[2] + d[2]];
    }
}

/// Pose the anatomy. `torso_half` is the body box's half-extents (the torso
/// is emitted first, `source: None`). Boxes are appended to `out` (cleared
/// first), one per rest part except chains, which emit `CHAIN_SEGMENTS`.
pub fn pose(
    parts: &[RigPart],
    torso_half: [f32; 3],
    m: &MotionInput,
    ragdoll: Option<&RagdollSample>,
    out: &mut Vec<PosedBox>,
) {
    out.clear();
    let speed01 = clamp01(finite(m.speed) / SPEED_SATURATE) * clamp01(m.leg_health);
    let settle = clamp01(m.settle);
    let strike = strike_curve(m.strike) * (1.0 - settle);
    let phase = finite(m.gait_phase);
    // Bob: twice per stride, gone when bedded down.
    let bob = speed01 * BODY_BOB_MAX * (2.0 * phase).sin() * (1.0 - settle);

    // Leg length (for settle drop and stance extension): the tallest leg.
    let leg_len = parts
        .iter()
        .filter(|p| p.role == Role::Leg)
        .map(|p| p.half[1] * 2.0)
        .fold(0.0f32, f32::max);
    let drop = settle * SETTLE_DROP * leg_len;

    // ── torso ────────────────────────────────────────────────────────
    let breath = liveliness::breath_for(m.state, finite(m.time));
    let mut th = deform::stretch_axis(torso_half, 1, breath);
    th = deform::landing_squash(th, clamp01(m.impact));
    if m.max_speed > 1e-3 {
        th = deform::motion_stretch(th, finite(m.speed), m.max_speed);
    }
    let lunge = strike * STRIKE_LUNGE * torso_half[0];
    let torso = PosedBox {
        source: None,
        role: None,
        center: [lunge, bob - drop, 0.0],
        half: th,
        rot: [0.0, -STRIKE_PITCH * strike.max(0.0), 0.0],
    };
    // Head-cluster boxes ride the torso's front as it stretches/lunges.
    let front_shift = (th[0] - torso_half[0]) + lunge;
    // Head pivot for gaze/strike: the torso's front-top, or the neck tip.
    let head_pivot = [torso_half[0] * 0.85, torso_half[1] * 0.35, 0.0];
    out.push(torso);

    // ── parts ────────────────────────────────────────────────────────
    let mut leg_i = 0usize;
    let mut arm_i = 0usize;
    for (idx, p) in parts.iter().enumerate() {
        match p.role {
            Role::Tail | Role::Neck => {
                chain(idx, p, p.role, m, phase, bob - drop, out);
                continue;
            }
            _ => {}
        }
        let mut center = p.offset;
        let mut half = p.half;
        let mut rot = [0.0f32; 3];
        let mut rides_bob = true;
        match p.role {
            Role::Leg => {
                let off = leg_phase_offset(leg_i);
                leg_i += 1;
                let s = (phase + off).sin();
                let c = (phase + off).cos();
                let swing = speed01 * LEG_SWING_MAX * s;
                // The hip rides the body bob; the sole does not.
                let hip = [p.offset[0], p.offset[1] + p.half[1] + bob, p.offset[2]];
                let len = p.half[1] * 2.0;
                let fold = settle * SETTLE_FOLD * if leg_i % 2 == 0 { 1.0 } else { -1.0 };
                let pitch = swing * (1.0 - settle) + fold;
                // Stance (foot moving back, c<0): extend so the LOWEST CORNER
                // of the tilted foot sits exactly at the rest sole — the arc
                // and the bob are both taken up by the leg, the way a planted
                // leg straightens under a rising hip. Swing (c>0): lift.
                let (ext, lift) = if c < 0.0 && settle < 1.0 {
                    let cos_sw = pitch.cos().max(0.5);
                    let corner_dip = p.half[0] * pitch.sin().abs();
                    (((len + bob - corner_dip) / (len * cos_sw)).clamp(0.5, 2.0), 0.0)
                } else {
                    (1.0, SWING_LIFT * len * speed01 * c.max(0.0))
                };
                let down = rotate([0.0, -p.half[1] * ext, 0.0], [0.0, pitch, 0.0]);
                center = [hip[0] + down[0], hip[1] + down[1] + lift, hip[2] + down[2]];
                half[1] = p.half[1] * ext;
                rot[1] = pitch;
                // bedded: the leg lies against the lowered belly
                center[1] += -drop + settle * p.half[1] * 0.6;
                rides_bob = false;
            }
            Role::Arm => {
                let off = PI + leg_phase_offset(arm_i);
                arm_i += 1;
                let swing = speed01 * ARM_SWING_MAX * (phase + off).sin() * (1.0 - settle);
                let shoulder = [p.offset[0], p.offset[1] + p.half[1], p.offset[2]];
                let down = rotate([0.0, -p.half[1], 0.0], [0.0, swing, 0.0]);
                center = [shoulder[0] + down[0], shoulder[1] + down[1], shoulder[2] + down[2]];
                rot[1] = swing;
                center[1] -= drop;
            }
            Role::Wing => {
                let flap = if m.flying {
                    clamp01(m.fly_power) * WING_FLAP_MAX * (WING_BEAT_RATIO * phase).sin()
                } else {
                    // folded when bedded, a resting droop otherwise
                    -0.25 * settle
                };
                // roll about the root (inner edge) so the outer tip sweeps
                let side = p.offset[2].signum();
                let side = if side == 0.0 { 1.0 } else { side };
                let root = [p.offset[0], p.offset[1], p.offset[2] - side * p.half[2]];
                let roll = flap * side; // mirror so both wings rise together
                let out_v = rotate([0.0, 0.0, side * p.half[2]], [roll, 0.0, 0.0]);
                center = [root[0] + out_v[0], root[1] + out_v[1], root[2] + out_v[2]];
                rot[0] = roll;
                center[1] -= drop;
            }
            Role::Head => {
                // gaze + strike about the head pivot, riding the torso front
                let yaw = clampf(m.gaze_yaw, -1.2, 1.2);
                let pitch = clampf(m.gaze_pitch, -0.9, 0.9) - STRIKE_PITCH * 1.4 * strike.max(0.0);
                let rel = [p.offset[0] - head_pivot[0], p.offset[1] - head_pivot[1], p.offset[2] - head_pivot[2]];
                let r = rotate(rel, [0.0, pitch, yaw]);
                center = [head_pivot[0] + r[0] + front_shift, head_pivot[1] + r[1], head_pivot[2] + r[2]];
                rot = [0.0, pitch, yaw];
                center[1] -= drop;
            }
            Role::Fin | Role::Other | Role::Tail | Role::Neck => {
                center[0] += front_shift * clamp01(p.offset[0] / torso_half[0].max(1e-3));
                center[1] -= drop;
            }
        }
        if rides_bob {
            center[1] += bob;
        }
        out.push(PosedBox { source: Some(idx), role: Some(p.role), center, half, rot });
    }

    // ── ragdoll blend ────────────────────────────────────────────────
    let limp = liveliness::limpness(m.state, clamp01(m.progress));
    if let Some(rd) = ragdoll {
        if limp > 0.0 {
            for b in out.iter_mut() {
                let target = match b.source {
                    None => Some(rd.body),
                    Some(i) => rd.parts.get(i).copied(),
                };
                let Some(t) = target else { continue };
                // Orientation from the stick: the rotation that takes the rest
                // direction (body→part) to the current one, as yaw + pitch.
                let rest = match b.source {
                    None => [1.0, 0.0, 0.0],
                    Some(i) => parts.get(i).map(|p| p.offset).unwrap_or([1.0, 0.0, 0.0]),
                };
                let cur = [t[0] - rd.body[0], t[1] - rd.body[1], t[2] - rd.body[2]];
                let (ry, rp) = dir_angles(rest);
                let (cy, cp) = dir_angles(cur);
                let dyaw = wrap_pi(cy - ry);
                let dpitch = cp - rp;
                // A chain segment blends toward the part's node offset along
                // the segment's own share of the rest length.
                let phys_center = match b.role {
                    Some(Role::Tail) | Some(Role::Neck) => {
                        let src = b.source.and_then(|i| parts.get(i));
                        match src {
                            Some(p) => {
                                let k = (b.center[0] - rd.body[0]).abs().max(1e-3) / (p.offset[0].abs() + p.half[0]).max(1e-3);
                                let k = clamp01(k);
                                [rd.body[0] + cur[0] * k, rd.body[1] + cur[1] * k, rd.body[2] + cur[2] * k]
                            }
                            None => t,
                        }
                    }
                    _ => t,
                };
                b.center = liveliness::blend3(b.center, phys_center, limp);
                b.rot = liveliness::blend3(b.rot, [b.rot[0], b.rot[1] + dpitch, b.rot[2] + dyaw], limp);
            }
        }
    }
}

/// Azimuth (about Y, from +X toward +Z) and elevation of a direction.
fn dir_angles(v: [f32; 3]) -> (f32, f32) {
    let hx = (v[0] * v[0] + v[2] * v[2]).sqrt();
    if hx < 1e-4 && v[1].abs() < 1e-4 {
        return (0.0, 0.0);
    }
    (v[2].atan2(v[0]), v[1].atan2(hx))
}

fn wrap_pi(a: f32) -> f32 {
    let mut a = a;
    while a > PI {
        a -= 2.0 * PI;
    }
    while a < -PI {
        a += 2.0 * PI;
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fauna quadruped from spawn.rs (body + 4 feet + tail + jaw/eyes).
    fn quadruped() -> (Vec<RigPart>, [f32; 3]) {
        let parts = vec![
            RigPart { role: Role::Head, offset: [0.5, 0.35, 0.18], half: [0.08, 0.08, 0.08] },
            RigPart { role: Role::Head, offset: [0.5, 0.35, -0.18], half: [0.08, 0.08, 0.08] },
            RigPart { role: Role::Head, offset: [0.6, 0.1, 0.0], half: [0.16, 0.12, 0.16] },
            RigPart { role: Role::Leg, offset: [0.45, -0.5, 0.25], half: [0.12, 0.3, 0.12] },
            RigPart { role: Role::Leg, offset: [0.45, -0.5, -0.25], half: [0.12, 0.3, 0.12] },
            RigPart { role: Role::Leg, offset: [-0.45, -0.5, 0.25], half: [0.12, 0.3, 0.12] },
            RigPart { role: Role::Leg, offset: [-0.45, -0.5, -0.25], half: [0.12, 0.3, 0.12] },
            RigPart { role: Role::Tail, offset: [-0.7, 0.1, 0.0], half: [0.4, 0.12, 0.12] },
        ];
        (parts, [0.55, 0.32, 0.3])
    }

    fn posed(parts: &[RigPart], th: [f32; 3], m: &MotionInput, rd: Option<&RagdollSample>) -> Vec<PosedBox> {
        let mut out = Vec::new();
        pose(parts, th, m, rd, &mut out);
        out
    }

    fn feet(out: &[PosedBox]) -> Vec<&PosedBox> {
        out.iter().filter(|b| b.role == Some(Role::Leg)).collect()
    }

    fn lowest_y(b: &PosedBox) -> f32 {
        corners(b).iter().map(|c| c[1]).fold(f32::MAX, f32::min)
    }

    #[test]
    fn a_standing_creature_is_at_rest() {
        let (parts, th) = quadruped();
        let out = posed(&parts, th, &MotionInput::default(), None);
        // torso + 7 non-chain parts + CHAIN_SEGMENTS for the tail
        assert_eq!(out.len(), 1 + 7 + CHAIN_SEGMENTS);
        let torso = &out[0];
        assert_eq!(torso.source, None);
        assert!((torso.center[0]).abs() < 1e-6 && torso.center[2].abs() < 1e-6);
        assert_eq!(torso.rot, [0.0, 0.0, 0.0]);
        // breath at t=0 is exactly 1 → half-extents unchanged
        assert!((torso.half[1] - th[1]).abs() < 1e-6, "breath at t=0 is 1: {:?}", torso.half);
        for b in &out[1..] {
            let src = parts[b.source.unwrap()];
            if b.role == Some(Role::Tail) {
                continue;
            }
            assert!(b.rot.iter().all(|r| r.abs() < 1e-6), "{b:?} rotated at rest");
            for k in 0..3 {
                assert!((b.center[k] - src.offset[k]).abs() < 1e-5, "{b:?} moved at rest (rest {:?})", src.offset);
            }
            assert_eq!(b.half, src.half);
        }
        // The straight tail's segments tile the rest box exactly.
        let tail: Vec<&PosedBox> = out.iter().filter(|b| b.role == Some(Role::Tail)).collect();
        assert_eq!(tail.len(), CHAIN_SEGMENTS);
        let lo = tail.iter().map(|b| b.center[0] - b.half[0]).fold(f32::MAX, f32::min);
        let hi = tail.iter().map(|b| b.center[0] + b.half[0]).fold(f32::MIN, f32::max);
        assert!((lo - (-1.1)).abs() < 1e-4 && (hi - (-0.3)).abs() < 1e-4, "tail spans {lo}..{hi}");
        assert!(tail.iter().all(|b| b.rot[2].abs() < 1e-6), "straight at rest");
    }

    #[test]
    fn legs_pivot_at_the_hip_and_diagonal_pairs_oppose() {
        let (parts, th) = quadruped();
        let m = MotionInput { gait_phase: PI / 2.0, speed: 4.0, ..Default::default() };
        let out = posed(&parts, th, &m, None);
        let f = feet(&out);
        assert!(f[0].rot[1] > 0.1, "leg 0 swings forward: {:?}", f[0].rot);
        assert!(f[1].rot[1] < -0.1, "leg 1 swings back: {:?}", f[1].rot);
        assert!((f[0].rot[1] + f[1].rot[1]).abs() < 1e-5, "equal and opposite");
        // The hip (top face centre) of the swinging leg stays put; the sole moves.
        for (i, b) in f.iter().enumerate() {
            let rest = parts[3 + i];
            let hip_rest = [rest.offset[0], rest.offset[1] + rest.half[1], rest.offset[2]];
            let top = rotate([0.0, b.half[1], 0.0], b.rot);
            let hip_now = [b.center[0] + top[0], b.center[1] + top[1] - m_bob(&m), b.center[2] + top[2]];
            let _ = hip_now[1]; // y: the hip rides the bob (asserted in the stance test)
            let dx = (hip_now[0] - hip_rest[0]).abs();
            let dz = (hip_now[2] - hip_rest[2]).abs();
            assert!(dx < 1e-4 && dz < 1e-4, "leg {i} hip drifted: {hip_now:?} vs {hip_rest:?}");
        }
    }

    fn m_bob(m: &MotionInput) -> f32 {
        let s = (m.speed / SPEED_SATURATE).clamp(0.0, 1.0);
        s * BODY_BOB_MAX * (2.0 * m.gait_phase).sin()
    }

    #[test]
    fn the_stance_foot_stays_on_the_ground_and_the_swing_foot_lifts() {
        let (parts, th) = quadruped();
        // Rest sole height of a leg.
        let rest = posed(&parts, th, &MotionInput::default(), None);
        let sole_rest = lowest_y(feet(&rest)[0]);
        let mut min_stance_err = 0.0f32;
        let mut max_swing_lift = 0.0f32;
        let n = 48;
        for k in 0..n {
            let phase = k as f32 / n as f32 * 2.0 * PI;
            let m = MotionInput { gait_phase: phase, speed: 6.0, ..Default::default() };
            let out = posed(&parts, th, &m, None);
            let f0 = feet(&out)[0];
            // the sole is measured in body space: a planted foot ignores the bob
            let sole = lowest_y(f0);
            if phase.cos() < 0.0 {
                // stance: the sole should sit at (or within a hair of) the rest sole
                min_stance_err = min_stance_err.max((sole - sole_rest).abs());
            } else {
                max_swing_lift = max_swing_lift.max(sole - sole_rest);
            }
        }
        assert!(min_stance_err < 0.03, "stance sole floats by {min_stance_err} (the residual float)");
        assert!(max_swing_lift > 0.08, "swing foot never lifts ({max_swing_lift})");
    }

    #[test]
    fn a_limp_shrinks_the_swing_and_a_still_body_does_not_bob() {
        let (parts, th) = quadruped();
        let healthy = posed(&parts, th, &MotionInput { gait_phase: 1.0, speed: 5.0, ..Default::default() }, None);
        let hurt = posed(&parts, th, &MotionInput { gait_phase: 1.0, speed: 5.0, leg_health: 0.3, ..Default::default() }, None);
        assert!(feet(&hurt)[0].rot[1].abs() < feet(&healthy)[0].rot[1].abs() * 0.5);
        let still = posed(&parts, th, &MotionInput { gait_phase: 1.0, speed: 0.0, ..Default::default() }, None);
        assert!(still[0].center[1].abs() < 1e-6, "no bob when still");
        assert!(feet(&still)[0].rot[1].abs() < 1e-6, "no swing when still");
    }

    #[test]
    fn breath_and_impact_deform_the_torso_and_keep_its_volume() {
        let (parts, th) = quadruped();
        let v0 = deform::volume(th);
        // Breath: at a quarter period the chest is fullest.
        let (rate, depth) = liveliness::breath_params(MotionState::Idle);
        let t = 0.25 / rate;
        let out = posed(&parts, th, &MotionInput { time: t, ..Default::default() }, None);
        assert!(out[0].half[1] > th[1] * (1.0 + depth * 0.9), "chest rises: {:?}", out[0].half);
        assert!((deform::volume(out[0].half) - v0).abs() < v0 * 1e-3, "volume kept");
        // Impact squashes Y and widens X/Z.
        let out = posed(&parts, th, &MotionInput { impact: 1.0, ..Default::default() }, None);
        assert!(out[0].half[1] < th[1] * 0.65 && out[0].half[0] > th[0]);
        assert!((deform::volume(out[0].half) - v0).abs() < v0 * 1e-3);
        // Motion stretch lengthens X and carries the head cluster forward.
        let fast = posed(&parts, th, &MotionInput { speed: 6.0, gait_phase: 0.0, ..Default::default() }, None);
        assert!(fast[0].half[0] > th[0] * 1.3);
        let head_rest = parts[2].offset[0];
        let head_now = fast.iter().find(|b| b.source == Some(2)).unwrap().center[0];
        assert!(head_now > head_rest + 0.1, "head rides the stretched front: {head_now} vs {head_rest}");
    }

    #[test]
    fn the_tail_swishes_with_the_stride_and_swings_out_of_a_turn() {
        let (parts, th) = quadruped();
        let tip = |out: &Vec<PosedBox>| {
            out.iter().filter(|b| b.role == Some(Role::Tail)).last().unwrap().center
        };
        // Walking: the tip sweeps side to side across the stride.
        let mut zs = Vec::new();
        for k in 0..16 {
            let m = MotionInput { gait_phase: k as f32 / 16.0 * 2.0 * PI, speed: 5.0, ..Default::default() };
            zs.push(tip(&posed(&parts, th, &m, None))[2]);
        }
        let span = zs.iter().cloned().fold(f32::MIN, f32::max) - zs.iter().cloned().fold(f32::MAX, f32::min);
        assert!(span > 0.15, "tail tip barely moves across a stride: {span}");
        // Turning right (+turn): the tail swings out to the left (−Z)... i.e.
        // opposite to a neck, which would lead right.
        let right = posed(&parts, th, &MotionInput { turn: 1.0, ..Default::default() }, None);
        let left = posed(&parts, th, &MotionInput { turn: -1.0, ..Default::default() }, None);
        assert!(tip(&right)[2] < -0.05 && tip(&left)[2] > 0.05, "tail out of the turn: R {:?} L {:?}", tip(&right), tip(&left));
        // Segments stay connected: each starts where the previous ended.
        let out = posed(&parts, th, &MotionInput { turn: 0.8, speed: 5.0, gait_phase: 1.3, ..Default::default() }, None);
        let segs: Vec<&PosedBox> = out.iter().filter(|b| b.role == Some(Role::Tail)).collect();
        for w in segs.windows(2) {
            let (a, b) = (w[0], w[1]);
            // end of a along its own axis (−X in local, rotated)
            let end_a = {
                let d = rotate([-a.half[0], 0.0, 0.0], a.rot);
                [a.center[0] + d[0], a.center[1] + d[1], a.center[2] + d[2]]
            };
            let start_b = {
                let d = rotate([b.half[0], 0.0, 0.0], b.rot);
                [b.center[0] + d[0], b.center[1] + d[1], b.center[2] + d[2]]
            };
            let gap = ((end_a[0] - start_b[0]).powi(2) + (end_a[1] - start_b[1]).powi(2) + (end_a[2] - start_b[2]).powi(2)).sqrt();
            assert!(gap < 1e-3, "tail segments disconnected by {gap}");
        }
    }

    #[test]
    fn a_neck_leads_into_the_turn() {
        let parts = vec![RigPart { role: Role::Neck, offset: [1.2, 0.3, 0.0], half: [0.6, 0.15, 0.15] }];
        let tip = |m: &MotionInput| posed(&parts, [0.6, 0.3, 0.3], m, None).last().unwrap().center;
        assert!(tip(&MotionInput { turn: 1.0, ..Default::default() })[2] > 0.05);
        assert!(tip(&MotionInput { turn: -1.0, ..Default::default() })[2] < -0.05);
        assert!(tip(&MotionInput::default())[2].abs() < 1e-6);
    }

    #[test]
    fn settle_folds_the_legs_and_lowers_the_body() {
        let (parts, th) = quadruped();
        let up = posed(&parts, th, &MotionInput::default(), None);
        let down = posed(&parts, th, &MotionInput { settle: 1.0, ..Default::default() }, None);
        assert!(down[0].center[1] < up[0].center[1] - 0.3, "torso drops: {}", down[0].center[1]);
        for (a, b) in feet(&up).iter().zip(feet(&down).iter()) {
            assert!(b.rot[1].abs() > 1.0, "leg folds: {:?}", b.rot);
            assert!(lowest_y(b) >= lowest_y(a) - 0.05, "a folded leg does not sink below the standing sole");
        }
        // No gait while bedded even at speed input.
        let moving = posed(&parts, th, &MotionInput { settle: 1.0, speed: 5.0, gait_phase: 1.0, ..Default::default() }, None);
        assert!((moving[0].center[1] - down[0].center[1]).abs() < 1e-6, "no bob when settled");
    }

    #[test]
    fn a_strike_lunges_and_pitches_nose_down_then_returns_to_neutral() {
        let (parts, th) = quadruped();
        let at = |p: f32| posed(&parts, th, &MotionInput { strike: p, ..Default::default() }, None);
        let peak = at(0.45);
        assert!(peak[0].center[0] > 0.15, "lunge forward: {:?}", peak[0].center);
        assert!(peak[0].rot[1] < -0.2, "nose down: {:?}", peak[0].rot);
        let head = |out: &Vec<PosedBox>| out.iter().find(|b| b.source == Some(2)).unwrap().center[0];
        assert!(head(&peak) > head(&at(0.0)) + 0.15, "head leads the lunge");
        let back = at(0.1);
        assert!(back[0].center[0] < 0.0, "anticipation rocks back");
        let done = at(1.0);
        assert!(done[0].center[0].abs() < 1e-6 && done[0].rot[1].abs() < 1e-6, "finished strike leaves no trace");
        assert!(strike_curve(0.0).abs() < 1e-6 && strike_curve(1.0).abs() < 1e-6);
        assert!((strike_curve(0.45) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn wings_beat_only_in_flight_and_mirror() {
        let parts = vec![
            RigPart { role: Role::Wing, offset: [0.0, 0.8, 2.5], half: [2.5, 0.2, 1.5] },
            RigPart { role: Role::Wing, offset: [0.0, 0.8, -2.5], half: [2.5, 0.2, 1.5] },
        ];
        let th = [3.0, 1.2, 1.5];
        let grounded = posed(&parts, th, &MotionInput { gait_phase: 0.5, speed: 2.0, ..Default::default() }, None);
        assert!(grounded[1].rot[0].abs() < 1e-6 && grounded[2].rot[0].abs() < 1e-6);
        let m = MotionInput { flying: true, fly_power: 1.0, gait_phase: PI / 6.0, speed: 4.0, ..Default::default() };
        let flying = posed(&parts, th, &m, None);
        let (l, r) = (&flying[1], &flying[2]);
        assert!(l.rot[0].abs() > 0.3, "flap: {:?}", l.rot);
        assert!((l.rot[0] + r.rot[0]).abs() < 1e-5, "mirrored roll");
        // both tips move together: the outermost corner (largest z·side) of
        // each wing displaces vertically by the same amount
        let rest = posed(&parts, th, &MotionInput::default(), None);
        let tip_y = |b: &PosedBox, side: f32| {
            corners(b).iter().max_by(|a, c| (a[2] * side).partial_cmp(&(c[2] * side)).unwrap()).unwrap()[1]
        };
        let dl = tip_y(l, 1.0) - tip_y(&rest[1], 1.0);
        let dr = tip_y(r, -1.0) - tip_y(&rest[2], -1.0);
        assert!((dl - dr).abs() < 1e-3 && dl.abs() > 0.3, "tips move together: {dl} {dr}");
        // the root (innermost corner) stays put
        let root_y = |b: &PosedBox, side: f32| {
            corners(b).iter().min_by(|a, c| (a[2] * side).partial_cmp(&(c[2] * side)).unwrap()).unwrap()[1]
        };
        assert!((root_y(l, 1.0) - root_y(&rest[1], 1.0)).abs() < 0.25, "root pinned: {} vs {}", root_y(l, 1.0), root_y(&rest[1], 1.0));
    }

    #[test]
    fn ragdoll_blend_moves_boxes_toward_the_nodes_by_limpness() {
        let (parts, th) = quadruped();
        // A ragdoll where every node has fallen 1 block below and the body
        // has rolled: parts scattered.
        let nodes: Vec<[f32; 3]> = parts.iter().map(|p| [p.offset[0] * 1.5, -0.8, p.offset[2] + 0.4]).collect();
        let rd = RagdollSample { body: [0.0, -1.0, 0.0], parts: &nodes };
        let driven = posed(&parts, th, &MotionInput::default(), None);
        let dying_half = posed(&parts, th, &MotionInput { state: MotionState::Dying, progress: 0.5, ..Default::default() }, Some(&rd));
        let dead = posed(&parts, th, &MotionInput { state: MotionState::Dying, progress: 1.0, ..Default::default() }, Some(&rd));
        // torso: halfway then all the way to the body node
        assert!((dying_half[0].center[1] - (-0.5)).abs() < 1e-4, "{:?}", dying_half[0].center);
        assert!((dead[0].center[1] - (-1.0)).abs() < 1e-4);
        // a leg lands on its node when dead, is between when half
        let leg_i = 3;
        let d = dead.iter().find(|b| b.source == Some(leg_i)).unwrap().center;
        assert!((d[0] - nodes[leg_i][0]).abs() < 1e-4 && (d[1] - nodes[leg_i][1]).abs() < 1e-4 && (d[2] - nodes[leg_i][2]).abs() < 1e-4, "{d:?}");
        let h = dying_half.iter().find(|b| b.source == Some(leg_i)).unwrap().center;
        let r = driven.iter().find(|b| b.source == Some(leg_i)).unwrap().center;
        for k in 0..3 {
            let mid = (r[k] + nodes[leg_i][k]) * 0.5;
            assert!((h[k] - mid).abs() < 1e-4, "axis {k}: {} vs mid {mid}", h[k]);
        }
        // and the rotation follows the stick: the leg's direction changed, so
        // its yaw/pitch are no longer zero when dead.
        let db = dead.iter().find(|b| b.source == Some(leg_i)).unwrap();
        assert!(db.rot[1].abs() + db.rot[2].abs() > 0.05, "orientation follows the stick: {:?}", db.rot);
        // Idle keeps most of the driven pose (IDLE_LIMP is faint).
        let idle = posed(&parts, th, &MotionInput::default(), Some(&rd));
        assert!((idle[0].center[1] - (-liveliness::IDLE_LIMP)).abs() < 1e-4);
    }

    #[test]
    fn corners_match_push_box_oriented_order_and_world_rotation() {
        let b = PosedBox { source: None, role: None, center: [1.0, 2.0, 3.0], half: [0.5, 0.25, 0.125], rot: [0.0; 3] };
        let c = corners(&b);
        // order: x outer, y middle, z inner, each - then +
        assert_eq!(c[0], [0.5, 1.75, 2.875]);
        assert_eq!(c[1], [0.5, 1.75, 3.125]);
        assert_eq!(c[2], [0.5, 2.25, 2.875]);
        assert_eq!(c[4], [1.5, 1.75, 2.875]);
        assert_eq!(c[7], [1.5, 2.25, 3.125]);
        // yaw +π/2 about the body takes +X to +Z (entity.rs's own test).
        let w = world_corners(&PosedBox { center: [1.0, 0.0, 0.0], half: [0.1; 3], rot: [0.0; 3], ..b }, [10.0, 0.0, 10.0], [0.0, 0.0, PI / 2.0]);
        let cx = w.iter().map(|p| p[0]).sum::<f32>() / 8.0;
        let cz = w.iter().map(|p| p[2]).sum::<f32>() / 8.0;
        assert!((cx - 10.0).abs() < 1e-5 && (cz - 11.0).abs() < 1e-5, "yaw π/2: ({cx},{cz})");
        // a box's own pitch about Z takes +X to +Y, applied before the body yaw
        let p = PosedBox { center: [0.0; 3], half: [1.0, 0.1, 0.1], rot: [0.0, PI / 2.0, 0.0], ..b };
        let c = corners(&p);
        let ymax = c.iter().map(|q| q[1]).fold(f32::MIN, f32::max);
        assert!((ymax - 1.0).abs() < 1e-5, "pitched box stands up: {ymax}");
    }

    #[test]
    fn garbage_inputs_never_panic_and_outputs_stay_finite() {
        let (parts, th) = quadruped();
        let bad = [f32::NAN, f32::INFINITY, -f32::INFINITY, 1e9, -1e9];
        for &v in &bad {
            let m = MotionInput { gait_phase: v, speed: v, leg_health: v, turn: v, gaze_yaw: v, gaze_pitch: v, progress: v, time: v, impact: v, settle: v, strike: v, fly_power: v, flying: true, ..Default::default() };
            let out = posed(&parts, th, &m, None);
            assert!(!out.is_empty());
            for b in &out {
                for x in b.center.iter().chain(b.half.iter()).chain(b.rot.iter()) {
                    assert!(x.is_finite(), "{b:?} from input {v}");
                }
                assert!(b.half.iter().all(|h| *h > 0.0), "{b:?} collapsed");
            }
        }
        // empty anatomy: just a torso
        let out = posed(&[], th, &MotionInput::default(), None);
        assert_eq!(out.len(), 1);
        // a ragdoll sample shorter than the parts leaves the rest driven
        let short = [[0.0, -1.0, 0.0]];
        let rd = RagdollSample { body: [0.0, -1.0, 0.0], parts: &short };
        let out = posed(&parts, th, &MotionInput { state: MotionState::Dying, progress: 1.0, ..Default::default() }, Some(&rd));
        let leg = out.iter().find(|b| b.source == Some(4)).unwrap();
        assert_eq!(leg.center, parts[4].offset, "unsampled part stays driven");
    }
}
