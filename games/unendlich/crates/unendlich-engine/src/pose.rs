//! Procedural animation poses for articulated creatures. Instead of authored
//! keyframes, a creature's pose is a pure function of a *gait phase* plus its
//! [`crate::creature::GradedCapabilities`]: legs swing in alternating phase,
//! the body bobs, wings flap. Three payoffs of doing it this way:
//!
//! - **Cheap & deterministic** — no animation assets, no per-frame state
//!   beyond a single phase float that advances with movement speed.
//! - **Damage-reactive for free** — amplitudes scale by `move_speed`, so a
//!   creature with a hurt leg automatically limps (small, asymmetric swing)
//!   and a torn-winged flyer stops flapping, with no special cases.
//! - **Interpolation-friendly** — the pose at tick T and T+1 blend cleanly,
//!   so it rides the `SimDriver`'s snapshot interpolation for smooth display.
//!
//! Render-side and visual: the pose tells the renderer how to place each
//! anatomy part; it is not part of the authoritative simulation. `f32`.

use std::f32::consts::{PI, TAU};

/// Stride cadence: radians of gait phase per (block/sec · second). Tuned so a
/// creature moving ~4 blocks/sec takes a brisk, natural stride.
pub const CADENCE: f32 = 2.2;
/// Peak leg swing at full speed (radians fore/aft from neutral).
pub const LEG_SWING_MAX: f32 = 0.7;
/// Peak arm swing at full speed (radians). Gentler than the legs and in
/// counter-phase, so a walking biped's arms swing opposite its stride.
pub const ARM_SWING_MAX: f32 = 0.5;
/// Peak tail sway at full speed (radians, side-to-side about the vertical axis).
/// The tail swings laterally with the stride, lending momentum to the gait.
pub const TAIL_SWAY_MAX: f32 = 0.38;
/// Peak body bob (vertical, in block units) at full speed.
pub const BODY_BOB_MAX: f32 = 0.12;
/// Peak wing-flap angle (radians) and how much faster wings beat than legs.
pub const WING_FLAP_MAX: f32 = 1.1;
pub const WING_BEAT_RATIO: f32 = 3.0;

/// Locomotion inputs the pose reads — decoupled from creature internals so the
/// math is testable on its own. Build directly, or via [`Locomotion::walking`]
/// from a creature's graded capabilities.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Locomotion {
    /// Ground speed in blocks/sec (drives stride length + amplitude).
    pub speed: f32,
    /// Ground-move capability in `[0,1]` — `move_speed` from grading. A limp
    /// damps the swing; 0 means the legs don't drive at all.
    pub leg_health: f32,
    /// Whether the creature is currently flying (gates wing flap).
    pub flying: bool,
    /// Lift fraction in `[0,1]` when flying (scales flap amplitude).
    pub fly_power: f32,
}

impl Locomotion {
    /// Build from a creature's graded capabilities and current ground speed.
    pub fn from_graded(g: &crate::creature::GradedCapabilities, speed: f32) -> Locomotion {
        Locomotion {
            speed,
            leg_health: g.move_speed,
            flying: g.can_fly,
            fly_power: g.fly_power,
        }
    }

    /// A convenient ground-walker at `speed` with full health.
    pub fn walking(speed: f32) -> Locomotion {
        Locomotion { speed, leg_health: 1.0, flying: false, fly_power: 0.0 }
    }
}

/// The per-tick pose: how far each limb is swung, etc. Lengths are radians
/// (swings/flap) or block units (bob). The renderer turns these into part
/// transforms.
#[derive(Debug, Clone, PartialEq)]
pub struct CreaturePose {
    /// Fore/aft swing per leg, in declaration order. Adjacent legs alternate.
    pub leg_swings: Vec<f32>,
    /// Vertical body displacement (gait bob).
    pub body_bob: f32,
    /// Wing flap angle (0 when not flying).
    pub wing_flap: f32,
}

/// Advance the gait phase by `dt`, with stride cadence proportional to `speed`.
/// A stationary creature's phase doesn't move (no marching in place). Wraps to
/// `[0, TAU)` so it never grows unbounded.
pub fn advance_gait_phase(phase: f32, speed: f32, dt: f32) -> f32 {
    let next = phase + CADENCE * speed.max(0.0) * dt;
    next.rem_euclid(TAU)
}

/// Phase offset for leg `i` of `n_legs`. Even/odd legs go in counter-phase
/// (the diagonal gait of most quadrupeds and the simple alternation of a
/// biped): leg 0 forward while leg 1 is back.
fn leg_phase_offset(i: usize, _n_legs: usize) -> f32 {
    if i % 2 == 0 {
        0.0
    } else {
        PI
    }
}

/// Swing of a single leg at `phase`. Amplitude scales with both how fast the
/// creature moves (up to a normalized cap) and its leg health (a limp).
pub fn leg_swing(phase: f32, i: usize, n_legs: usize, speed: f32, leg_health: f32) -> f32 {
    // Normalize speed into a 0..1 amplitude factor (saturates ~6 blocks/sec).
    let amp = (speed / 6.0).clamp(0.0, 1.0) * leg_health.clamp(0.0, 1.0) * LEG_SWING_MAX;
    amp * (phase + leg_phase_offset(i, n_legs)).sin()
}

/// The full pose for a creature with `n_legs` at gait `phase`.
pub fn pose_for(phase: f32, n_legs: usize, loco: &Locomotion) -> CreaturePose {
    let leg_swings = (0..n_legs)
        .map(|i| leg_swing(phase, i, n_legs, loco.speed, loco.leg_health))
        .collect();

    // The body dips twice per stride (once per footfall), so bob runs at 2×
    // the leg frequency. Scales with speed and leg health like the legs.
    let move_amp = (loco.speed / 6.0).clamp(0.0, 1.0) * loco.leg_health.clamp(0.0, 1.0);
    let body_bob = move_amp * BODY_BOB_MAX * (2.0 * phase).sin();

    // Wings only flap while flying; faster beat, amplitude from lift power.
    let wing_flap = if loco.flying {
        loco.fly_power.clamp(0.0, 1.0) * WING_FLAP_MAX * (WING_BEAT_RATIO * phase).sin()
    } else {
        0.0
    };

    CreaturePose { leg_swings, body_bob, wing_flap }
}

// ───────────────────────────────────────────────────────────────────────────
// Applying a pose to an anatomy → animated part placements.
//
// Limbs are placed by a body-local `offset` (X forward, Y up, Z right) and the
// existing renderer treats them as offset-only boxes. So animation just moves
// those offsets: a leg swings fore/aft on an arc about the body origin, a wing
// rolls about the forward axis, and every part rides the vertical body bob. No
// new rotation concept for the renderer — same boxes, animated centers.
// ───────────────────────────────────────────────────────────────────────────

/// A body part placed for the current animation frame: its role + half-extents
/// (unchanged from the resting [`crate::creature::PartState`]) and its animated
/// body-local center `offset`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnimatedPart {
    pub part: crate::creature::Part,
    pub offset: [f32; 3],
    pub half_extents: [f32; 3],
}

/// Rotate a body-local vector fore/aft (in the X-up-Y plane, about the lateral
/// Z axis) — a leg swinging forward (+angle → +X) or back.
fn swing_xy(v: [f32; 3], a: f32) -> [f32; 3] {
    let (s, c) = a.sin_cos();
    [v[0] * c - v[1] * s, v[0] * s + v[1] * c, v[2]]
}

/// Rotate a body-local vector about the forward X axis (in the Y-up-Z plane) —
/// a wing flapping up/down.
fn roll_yz(v: [f32; 3], a: f32) -> [f32; 3] {
    let (s, c) = a.sin_cos();
    [v[0], v[1] * c - v[2] * s, v[1] * s + v[2] * c]
}

/// Rotate a body-local vector about the vertical Y axis (in the X-forward /
/// Z-right plane) — a tail swinging side to side.
fn yaw_xz(v: [f32; 3], a: f32) -> [f32; 3] {
    let (s, c) = a.sin_cos();
    [v[0] * c + v[2] * s, v[1], -v[0] * s + v[2] * c]
}

/// Animate every part of `anatomy` for gait `phase` and locomotion `loco`. The
/// number of legs is taken from the anatomy's foot count, so the gait matches
/// the body plan. Returns one [`AnimatedPart`] per `anatomy.parts`, in order.
///
/// - Feet (`Foot`/`StickyFoot`) swing fore/aft on an arc, adjacent feet in
///   counter-phase, so the creature appears to step.
/// - Wings roll about the forward axis by the flap angle.
/// - All parts then ride the vertical body bob.
///
/// A damaged creature limps automatically: `loco.leg_health` shrinks the swing,
/// and a torn wing (`loco.flying == false` or low `fly_power`) stops flapping.
pub fn animate(
    anatomy: &crate::creature::Anatomy,
    phase: f32,
    loco: &Locomotion,
) -> Vec<AnimatedPart> {
    use crate::creature::Part;
    let n_legs = anatomy
        .parts
        .iter()
        .filter(|p| matches!(p.part, Part::Foot | Part::StickyFoot))
        .count();
    let pose = pose_for(phase, n_legs, loco);

    let mut leg_i = 0usize;
    let mut hand_i = 0usize;
    // Arm swing amplitude scales with speed + leg health, like the legs.
    let arm_amp = (loco.speed / 6.0).clamp(0.0, 1.0) * loco.leg_health.clamp(0.0, 1.0) * ARM_SWING_MAX;
    anatomy
        .parts
        .iter()
        .map(|ps| {
            let mut offset = ps.offset;
            match ps.part {
                Part::Foot | Part::StickyFoot => {
                    let swing = pose.leg_swings.get(leg_i).copied().unwrap_or(0.0);
                    leg_i += 1;
                    offset = swing_xy(offset, swing);
                }
                Part::Hand => {
                    // Arms swing fore/aft in COUNTER-phase to the same-index leg
                    // (contralateral gait): right arm forward as the right leg goes
                    // back. r485 — previously static.
                    let off = PI + leg_phase_offset(hand_i, 0);
                    hand_i += 1;
                    offset = swing_xy(offset, arm_amp * (phase + off).sin());
                }
                Part::Wing => {
                    offset = roll_yz(offset, pose.wing_flap);
                }
                Part::Tail => {
                    // Tail swings side to side with the stride (one sway per stride),
                    // pivoting about the body's vertical axis. r486 — previously static.
                    let sway = arm_amp / ARM_SWING_MAX * TAIL_SWAY_MAX * phase.sin();
                    offset = yaw_xz(offset, sway);
                }
                _ => {}
            }
            // Every part rides the body bob.
            offset[1] += pose.body_bob;
            AnimatedPart { part: ps.part, offset, half_extents: ps.half_extents }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_advances_with_speed_and_wraps() {
        // Stationary: phase frozen.
        assert_eq!(advance_gait_phase(1.0, 0.0, 0.016), 1.0);
        // Faster moves the phase further in the same dt.
        let slow = advance_gait_phase(0.0, 1.0, 0.1);
        let fast = advance_gait_phase(0.0, 4.0, 0.1);
        assert!(fast > slow, "faster → larger phase advance");
        // Always wrapped into [0, TAU).
        let wrapped = advance_gait_phase(TAU - 0.01, 10.0, 1.0);
        assert!((0.0..TAU).contains(&wrapped), "phase wraps: {wrapped}");
    }

    #[test]
    fn adjacent_legs_swing_in_counter_phase() {
        // At a phase where sin is clearly nonzero, leg 0 and leg 1 oppose.
        let phase = PI / 2.0;
        let l0 = leg_swing(phase, 0, 4, 4.0, 1.0);
        let l1 = leg_swing(phase, 1, 4, 4.0, 1.0);
        assert!(l0 > 0.1, "leg 0 forward");
        assert!(l1 < -0.1, "leg 1 back");
        assert!((l0 + l1).abs() < 1e-5, "exactly opposite");
    }

    #[test]
    fn a_limp_damps_the_swing() {
        let phase = PI / 2.0;
        let healthy = leg_swing(phase, 0, 4, 4.0, 1.0);
        let hurt = leg_swing(phase, 0, 4, 4.0, 0.3); // injured leg
        assert!(hurt.abs() < healthy.abs(), "hurt leg swings less");
        assert!(hurt.abs() > 0.0, "but still moves a little");
    }

    #[test]
    fn idle_creature_barely_moves() {
        let pose = pose_for(PI / 2.0, 4, &Locomotion::walking(0.0));
        assert!(pose.leg_swings.iter().all(|s| s.abs() < 1e-6), "no marching in place");
        assert!(pose.body_bob.abs() < 1e-6, "no bob when still");
    }

    #[test]
    fn wings_flap_only_while_flying() {
        let phase = PI / (2.0 * WING_BEAT_RATIO); // make the flap sine nonzero
        let grounded = pose_for(phase, 2, &Locomotion { speed: 3.0, leg_health: 1.0, flying: false, fly_power: 0.0 });
        assert_eq!(grounded.wing_flap, 0.0, "no flap on the ground");

        let aloft = pose_for(phase, 2, &Locomotion { speed: 3.0, leg_health: 1.0, flying: true, fly_power: 1.0 });
        assert!(aloft.wing_flap.abs() > 0.1, "flapping while flying");

        // A weakened wing flaps less.
        let weak = pose_for(phase, 2, &Locomotion { speed: 3.0, leg_health: 1.0, flying: true, fly_power: 0.4 });
        assert!(weak.wing_flap.abs() < aloft.wing_flap.abs(), "weak wing, smaller beat");
    }

    #[test]
    fn from_graded_maps_capabilities() {
        let g = crate::creature::GradedCapabilities {
            vision: 1.0, move_speed: 0.5, can_fly: true, fly_power: 0.8,
            climb: 1.0, swim: 1.0, bite: 1.0, grasp: 1.0, hearing: 1.0,
        };
        let loco = Locomotion::from_graded(&g, 4.0);
        assert_eq!(loco.leg_health, 0.5);
        assert!(loco.flying);
        assert_eq!(loco.fly_power, 0.8);
        // The half-health legs produce a smaller swing than full health would.
        let hurt = pose_for(PI / 2.0, 4, &loco);
        let full = pose_for(PI / 2.0, 4, &Locomotion::walking(4.0));
        assert!(hurt.leg_swings[0].abs() < full.leg_swings[0].abs());
    }

    use crate::creature::{Anatomy, Part, PartState};

    fn quadruped() -> Anatomy {
        // Four feet at the corners (X forward, Y up, Z right), plus an eye.
        let foot = |x: f32, z: f32| PartState::new(Part::Foot, 10, [x, -1.0, z], [0.1, 0.5, 0.1]);
        Anatomy {
            parts: vec![
                foot(0.5, -0.5),  // front-left  (leg 0)
                foot(0.5, 0.5),   // front-right (leg 1)
                foot(-0.5, -0.5), // back-left   (leg 2)
                foot(-0.5, 0.5),  // back-right  (leg 3)
                PartState::new(Part::Eye, 4, [0.8, 0.4, -0.2], [0.05, 0.05, 0.05]),
            ],
        }
    }

    #[test]
    fn idle_animation_is_the_resting_pose() {
        let a = quadruped();
        let framed = animate(&a, std::f32::consts::PI / 2.0, &Locomotion::walking(0.0));
        for (ap, ps) in framed.iter().zip(a.parts.iter()) {
            for k in 0..3 {
                assert!((ap.offset[k] - ps.offset[k]).abs() < 1e-6,
                    "{:?} axis {k}: animated {} vs rest {}", ap.part, ap.offset[k], ps.offset[k]);
            }
        }
    }

    #[test]
    fn walking_steps_adjacent_feet_in_opposition() {
        let a = quadruped();
        let framed = animate(&a, std::f32::consts::PI / 2.0, &Locomotion::walking(4.0));
        // Foot 0 (front-left) swings forward (+X past rest 0.5); foot 1 back.
        assert!(framed[0].offset[0] > 0.5, "front-left steps forward");
        assert!(framed[1].offset[0] < 0.5, "front-right steps back");
        // The eye doesn't swing in X — it only rides the bob.
        assert!((framed[4].offset[0] - 0.8).abs() < 1e-6, "eye doesn't stride");
    }

    #[test]
    fn a_limp_shortens_the_step() {
        let a = quadruped();
        let phase = std::f32::consts::PI / 2.0;
        let healthy = animate(&a, phase, &Locomotion::walking(4.0));
        let hurt = animate(&a, phase, &Locomotion { speed: 4.0, leg_health: 0.3, flying: false, fly_power: 0.0 });
        let disp = |v: &[AnimatedPart]| (v[0].offset[0] - 0.5).abs();
        assert!(disp(&hurt) < disp(&healthy), "limp takes a shorter step");
        assert!(disp(&hurt) > 0.0, "but still steps");
    }

    #[test]
    fn flying_rolls_the_wing_off_its_rest() {
        let a = Anatomy {
            parts: vec![
                PartState::new(Part::Wing, 12, [0.0, 0.2, 1.0], [0.4, 0.05, 0.8]),
                PartState::new(Part::Foot, 10, [0.0, -1.0, 0.2], [0.1, 0.5, 0.1]),
            ],
        };
        let phase = std::f32::consts::PI / (2.0 * WING_BEAT_RATIO);
        let aloft = animate(&a, phase, &Locomotion { speed: 0.0, leg_health: 1.0, flying: true, fly_power: 1.0 });
        let rest = [0.0, 0.2, 1.0];
        let moved = (0..3).any(|k| (aloft[0].offset[k] - rest[k]).abs() > 0.05);
        assert!(moved, "wing rolls away from its rest position while flying");
        // Grounded: wing stays put (no flap).
        let grounded = animate(&a, phase, &Locomotion::walking(0.0));
        for k in 0..3 {
            assert!((grounded[0].offset[k] - rest[k]).abs() < 1e-6, "grounded wing is still");
        }
    }
}
