//! `sim::fauna_gait` — body-plan proportions and gait selection.
//!
//! Generalises `dragon_locomotion`'s gait idea to all 20 fauna body plans.
//! Proportion ratios are relative to `torso = 1.0`. The rendering rig (forge)
//! consumes these to size joints; this module is pure data — no render deps.
//!
//! Proportion values sourced from `docs/specs/fauna_locomotion.docx §D`.

use core::f32::consts::TAU;

// ── Body plan ─────────────────────────────────────────────────────────────────

/// Top-level locomotion archetype.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyPlan {
    /// Four-legged ground animal (deer, wolf, horse, etc.).
    Quadruped,
    /// Two-legged ground animal (goblin, human NPC, etc.).
    Biped,
    /// Legless sinuous animal (snake, eel, worm).
    Serpentine,
    /// Winged flier (bird, pterosaur).
    Avian,
    /// Aquatic swimmer (fish, dolphin, sea-drake).
    Aquatic,
}

// ── Proportions ───────────────────────────────────────────────────────────────

/// Body segment proportions as ratios of `torso = 1.0`.
///
/// Values from `fauna_locomotion.docx §D` proportion tables.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Proportions {
    /// Head length ratio.
    pub head: f32,
    /// Neck length ratio (0 for serpentine / aquatic that lack a distinct neck).
    pub neck: f32,
    /// Primary limb (leg or wing) length ratio.
    pub limb: f32,
    /// Tail length ratio.
    pub tail: f32,
}

/// Return the canonical body proportions for `plan`.
///
/// All returned ratios are finite and positive.
pub fn proportions(plan: BodyPlan) -> Proportions {
    match plan {
        BodyPlan::Quadruped => Proportions {
            head: 0.35,
            neck: 0.40,
            limb: 0.80,
            tail: 0.60,
        },
        BodyPlan::Biped => Proportions {
            head: 0.45,
            neck: 0.25,
            limb: 1.10,
            tail: 0.10, // vestigial or absent
        },
        BodyPlan::Serpentine => Proportions {
            head: 0.20,
            neck: 0.05, // no distinct neck segment
            limb: 0.00, // no limbs; kept as 0 (forge ignores limb for serpentine)
            tail: 2.50, // very long tail/body extension
        },
        BodyPlan::Avian => Proportions {
            head: 0.30,
            neck: 0.50,
            limb: 1.20, // wing span arm
            tail: 0.45,
        },
        BodyPlan::Aquatic => Proportions {
            head: 0.40,
            neck: 0.10,
            limb: 0.70, // pectoral fin / flipper
            tail: 1.00, // strong propulsive tail
        },
    }
}

// ── Gait ──────────────────────────────────────────────────────────────────────

/// Discrete locomotion mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GaitKind {
    /// Stationary / idle.
    Stand,
    /// Slow ground walk.
    Walk,
    /// Medium ground trot.
    Trot,
    /// Fast ground gallop.
    Gallop,
    /// Stotting pronk (alarm/play leap).
    Pronk,
    /// Sinuous body-wave locomotion (serpentine).
    Slither,
    /// Aquatic undulation / fin propulsion.
    Swim,
    /// Wing-beat flight.
    Flap,
}

/// Select the appropriate gait for `plan` at normalised speed `speed01 ∈ [0,1]`.
///
/// `speed01` is clamped to `[0,1]` internally. NaN is treated as 0 (Stand).
///
/// Speed bands (quadruped reference):
///
/// | speed01 | Quadruped | Biped | Serpentine | Avian | Aquatic |
/// |---|---|---|---|---|---|
/// | < 0.05  | Stand | Stand | Stand | Stand | Stand |
/// | 0.05–0.3 | Walk | Walk | Slither | Walk | Swim |
/// | 0.3–0.65 | Trot | Trot | Slither | Flap | Swim |
/// | 0.65–0.85 | Gallop | Gallop | Slither | Flap | Swim |
/// | > 0.85  | Gallop | Pronk | Slither | Flap | Swim |
pub fn gait_for(plan: BodyPlan, speed01: f32) -> GaitKind {
    let s = if speed01.is_finite() { speed01.clamp(0.0, 1.0) } else { 0.0 };

    match plan {
        BodyPlan::Serpentine => {
            if s < 0.05 { GaitKind::Stand } else { GaitKind::Slither }
        }
        BodyPlan::Aquatic => {
            if s < 0.05 { GaitKind::Stand } else { GaitKind::Swim }
        }
        BodyPlan::Avian => {
            if s < 0.05 { GaitKind::Stand } else { GaitKind::Flap }
        }
        BodyPlan::Quadruped => {
            if      s < 0.05 { GaitKind::Stand }
            else if s < 0.30 { GaitKind::Walk }
            else if s < 0.65 { GaitKind::Trot }
            else              { GaitKind::Gallop }
        }
        BodyPlan::Biped => {
            if      s < 0.05 { GaitKind::Stand }
            else if s < 0.30 { GaitKind::Walk }
            else if s < 0.65 { GaitKind::Trot }
            else if s < 0.85 { GaitKind::Gallop }
            else              { GaitKind::Pronk }
        }
    }
}

// ── Phase advance ─────────────────────────────────────────────────────────────

/// Advance gait phase by the distance covered in `dt` at `speed`, normalised by
/// `stride_len`.
///
/// Returns the new phase in `[0, TAU)`.
///
/// Formula: `phase = (prev_phase + speed * dt / stride_len * TAU) % TAU`
///
/// - `stride_len ≤ 0` or non-finite: treated as 1.0 (no divide-by-zero).
/// - Non-finite `speed` or `dt`: treated as 0 (phase unchanged).
/// - Non-finite `prev_phase`: treated as 0.
pub fn stride_phase(prev_phase: f32, speed: f32, stride_len: f32, dt: f32) -> f32 {
    let phase0  = if prev_phase.is_finite() { prev_phase } else { 0.0 };
    let spd     = if speed.is_finite() && speed >= 0.0 { speed } else { 0.0 };
    let dt_safe = if dt.is_finite() && dt >= 0.0 { dt } else { 0.0 };
    let slen    = if stride_len.is_finite() && stride_len > 0.0 { stride_len } else { 1.0 };

    let delta = spd * dt_safe / slen * TAU;
    let raw   = phase0 + delta;

    // Wrap to [0, TAU). Use rem_euclid on f32 via manual approach.
    let wrapped = raw % TAU;
    if wrapped < 0.0 { wrapped + TAU } else { wrapped }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── proportions: finite positive ratios ──

    #[test]
    fn proportions_all_plans_finite_positive() {
        for plan in [
            BodyPlan::Quadruped, BodyPlan::Biped, BodyPlan::Serpentine,
            BodyPlan::Avian, BodyPlan::Aquatic,
        ] {
            let p = proportions(plan);
            assert!(p.head.is_finite() && p.head >= 0.0, "{:?} head={}", plan, p.head);
            assert!(p.neck.is_finite() && p.neck >= 0.0, "{:?} neck={}", plan, p.neck);
            assert!(p.limb.is_finite() && p.limb >= 0.0, "{:?} limb={}", plan, p.limb);
            assert!(p.tail.is_finite() && p.tail >  0.0, "{:?} tail={}", plan, p.tail);
        }
    }

    #[test]
    fn proportions_head_positive_for_all() {
        for plan in [BodyPlan::Quadruped, BodyPlan::Biped, BodyPlan::Serpentine,
                     BodyPlan::Avian, BodyPlan::Aquatic] {
            assert!(proportions(plan).head > 0.0);
        }
    }

    // ── gait_for: Quadruped speed bands ──

    #[test]
    fn quadruped_stand_at_zero() {
        assert_eq!(gait_for(BodyPlan::Quadruped, 0.0), GaitKind::Stand);
        assert_eq!(gait_for(BodyPlan::Quadruped, 0.04), GaitKind::Stand);
    }

    #[test]
    fn quadruped_walk() {
        assert_eq!(gait_for(BodyPlan::Quadruped, 0.05), GaitKind::Walk);
        assert_eq!(gait_for(BodyPlan::Quadruped, 0.20), GaitKind::Walk);
    }

    #[test]
    fn quadruped_trot() {
        assert_eq!(gait_for(BodyPlan::Quadruped, 0.30), GaitKind::Trot);
        assert_eq!(gait_for(BodyPlan::Quadruped, 0.50), GaitKind::Trot);
    }

    #[test]
    fn quadruped_gallop() {
        assert_eq!(gait_for(BodyPlan::Quadruped, 0.65), GaitKind::Gallop);
        assert_eq!(gait_for(BodyPlan::Quadruped, 1.00), GaitKind::Gallop);
    }

    #[test]
    fn quadruped_monotone_ascending() {
        let gaits = [GaitKind::Stand, GaitKind::Walk, GaitKind::Trot, GaitKind::Gallop];
        let speeds = [0.0f32, 0.1, 0.4, 0.8];
        for (&s, &expected) in speeds.iter().zip(gaits.iter()) {
            assert_eq!(gait_for(BodyPlan::Quadruped, s), expected, "at speed {}", s);
        }
    }

    // ── gait_for: Biped speed bands ──

    #[test]
    fn biped_stand_walk_trot_gallop_pronk() {
        assert_eq!(gait_for(BodyPlan::Biped, 0.0),  GaitKind::Stand);
        assert_eq!(gait_for(BodyPlan::Biped, 0.1),  GaitKind::Walk);
        assert_eq!(gait_for(BodyPlan::Biped, 0.5),  GaitKind::Trot);
        assert_eq!(gait_for(BodyPlan::Biped, 0.75), GaitKind::Gallop);
        assert_eq!(gait_for(BodyPlan::Biped, 0.9),  GaitKind::Pronk);
    }

    // ── gait_for: Serpentine → Slither ──

    #[test]
    fn serpentine_slither_above_threshold() {
        assert_eq!(gait_for(BodyPlan::Serpentine, 0.0),  GaitKind::Stand);
        assert_eq!(gait_for(BodyPlan::Serpentine, 0.05), GaitKind::Slither);
        assert_eq!(gait_for(BodyPlan::Serpentine, 1.0),  GaitKind::Slither);
    }

    // ── gait_for: Aquatic → Swim ──

    #[test]
    fn aquatic_swim_above_threshold() {
        assert_eq!(gait_for(BodyPlan::Aquatic, 0.0),  GaitKind::Stand);
        assert_eq!(gait_for(BodyPlan::Aquatic, 0.1),  GaitKind::Swim);
        assert_eq!(gait_for(BodyPlan::Aquatic, 1.0),  GaitKind::Swim);
    }

    // ── gait_for: Avian → Flap ──

    #[test]
    fn avian_flap_above_threshold() {
        assert_eq!(gait_for(BodyPlan::Avian, 0.0),  GaitKind::Stand);
        assert_eq!(gait_for(BodyPlan::Avian, 0.1),  GaitKind::Flap);
        assert_eq!(gait_for(BodyPlan::Avian, 1.0),  GaitKind::Flap);
    }

    // ── gait_for: clamping / NaN ──

    #[test]
    fn gait_speed_nan_gives_stand() {
        for plan in [BodyPlan::Quadruped, BodyPlan::Biped, BodyPlan::Serpentine,
                     BodyPlan::Avian, BodyPlan::Aquatic] {
            assert_eq!(gait_for(plan, f32::NAN), GaitKind::Stand);
        }
    }

    #[test]
    fn gait_speed_above_one_clamped_to_one() {
        // > 1.0 should clamp to 1.0, which is Gallop/Pronk/Slither/Swim/Flap
        assert_eq!(gait_for(BodyPlan::Quadruped, 99.0), GaitKind::Gallop);
        assert_eq!(gait_for(BodyPlan::Serpentine, 99.0), GaitKind::Slither);
    }

    #[test]
    fn gait_speed_negative_clamped_to_zero() {
        assert_eq!(gait_for(BodyPlan::Quadruped, -1.0), GaitKind::Stand);
    }

    // ── stride_phase: basic advance ──

    #[test]
    fn zero_speed_holds_phase() {
        let phase = 1.23f32;
        assert!((stride_phase(phase, 0.0, 1.0, 0.1) - phase).abs() < 1e-5);
    }

    #[test]
    fn zero_dt_holds_phase() {
        let phase = 0.5f32;
        assert!((stride_phase(phase, 5.0, 2.0, 0.0) - phase).abs() < 1e-5);
    }

    #[test]
    fn phase_wraps_within_tau() {
        let mut seed = 0xFACE_u32;
        let lcg = |s: &mut u32| -> f32 {
            *s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            (*s >> 8) as f32 / (1u32 << 24) as f32
        };
        for _ in 0..500 {
            let prev  = lcg(&mut seed) * TAU;
            let speed = lcg(&mut seed) * 10.0;
            let slen  = 0.1 + lcg(&mut seed) * 5.0;
            let dt    = lcg(&mut seed) * 2.0;
            let p = stride_phase(prev, speed, slen, dt);
            assert!(p >= 0.0 && p < TAU + 1e-5,
                "phase={} out of [0,TAU) for prev={} speed={} slen={} dt={}", p, prev, speed, slen, dt);
        }
    }

    #[test]
    fn phase_advances_proportional_to_distance() {
        // After one full stride (speed*dt == stride_len), phase advances by TAU.
        let prev = 0.0f32;
        let stride_len = 2.0f32;
        let speed = 4.0f32;
        let dt = 0.5f32; // distance = speed*dt = 2.0 = one full stride
        let phase = stride_phase(prev, speed, stride_len, dt);
        assert!((phase % TAU).abs() < 1e-4, "expected 0, got {}", phase);
    }

    #[test]
    fn phase_half_stride_advances_pi() {
        let phase = stride_phase(0.0, 1.0, 2.0, 1.0); // distance = 0.5 stride → TAU/2
        let expected = TAU / 2.0;
        assert!((phase - expected).abs() < 1e-4, "expected {} got {}", expected, phase);
    }

    // ── stride_phase: safety inputs ──

    #[test]
    fn stride_len_zero_treated_as_one() {
        let p = stride_phase(0.0, 1.0, 0.0, 1.0); // stride_len→1.0, advances TAU
        assert!(p.is_finite() && p >= 0.0 && p < TAU + 1e-5);
    }

    #[test]
    fn stride_len_negative_treated_as_one() {
        let p = stride_phase(0.0, 1.0, -5.0, 1.0);
        assert!(p.is_finite() && p >= 0.0 && p < TAU + 1e-5);
    }

    #[test]
    fn nan_speed_holds_phase() {
        let phase = 1.0f32;
        let p = stride_phase(phase, f32::NAN, 1.0, 1.0);
        assert!((p - phase).abs() < 1e-5);
    }

    #[test]
    fn nan_prev_phase_treated_as_zero() {
        let p = stride_phase(f32::NAN, 0.0, 1.0, 1.0);
        assert!(p.is_finite() && p >= 0.0);
    }

    // ── Determinism ──

    #[test]
    fn gait_for_deterministic() {
        for plan in [BodyPlan::Quadruped, BodyPlan::Biped, BodyPlan::Serpentine,
                     BodyPlan::Avian, BodyPlan::Aquatic] {
            for i in 0..=10 {
                let s = i as f32 / 10.0;
                assert_eq!(gait_for(plan, s), gait_for(plan, s));
            }
        }
    }

    #[test]
    fn stride_phase_deterministic() {
        let a = stride_phase(1.0, 2.0, 1.5, 0.1);
        let b = stride_phase(1.0, 2.0, 1.5, 0.1);
        assert_eq!(a.to_bits(), b.to_bits());
    }

    #[test]
    fn proportions_deterministic() {
        for plan in [BodyPlan::Quadruped, BodyPlan::Biped, BodyPlan::Serpentine,
                     BodyPlan::Avian, BodyPlan::Aquatic] {
            let p1 = proportions(plan);
            let p2 = proportions(plan);
            assert_eq!(p1.head.to_bits(), p2.head.to_bits());
        }
    }
}
