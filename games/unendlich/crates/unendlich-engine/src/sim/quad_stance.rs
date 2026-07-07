//! quad_stance — the connected, four-foot layer on top of `foot_reach`.
//!
//! A point-walker only needs one ground probe. A dragon needs all four feet to
//! find footholds *at once*, within each foot's own reach, for a given body
//! pose — and if any one foot is stranded over a gap, that pose is simply not
//! viable and the body must reposition. That coupling ("the feet are connected
//! through the body") is what this module enforces: it is the solver the
//! `foot_reach` doc header pointed at.
//!
//! Pipeline per candidate pose:
//!   1. map each hip's body-local offset into the world (pose translate+rotate),
//!   2. `foot_reach::find_foothold` under each hip — any `None` ⇒ whole stance
//!      `None` (reposition the body, don't force a step),
//!   3. fit the four planted feet with `large_body_grounding::solve` for the
//!      rest height and the size-clamped lean.
//!
//! Pure + deterministic; terrain is a sampler closure, same as `foot_reach`.
//! The four footholds then drive `foot_ik::solve_leg` per leg downstream.

use crate::sim::foot_reach::{find_foothold, Foothold, ReachBounds};
use crate::sim::large_body_grounding::{solve as ground_solve, FootSample, Grounding};

/// A four-footed body: hip offsets in body-local space (heading 0), each foot's
/// reach envelope, and the body scale that clamps the grounding lean.
#[derive(Clone, Copy, Debug)]
pub struct Quadruped {
    /// Body-local `(x, z)` of the four hips at heading 0 (any consistent order,
    /// e.g. front-left, front-right, hind-left, hind-right).
    pub hips: [[f32; 2]; 4],
    /// Per-foot reach bounds (front/hind can differ).
    pub bounds: [ReachBounds; 4],
    /// Body scale fed to `large_body_grounding` (1 = baseline; bigger ⇒ less tilt).
    pub size: f32,
}

/// A viable stance: where all four feet planted, plus the body grounding.
#[derive(Clone, Copy, Debug)]
pub struct Stance {
    /// The four chosen footholds, in the same order as `Quadruped::hips`.
    pub feet: [Foothold; 4],
    /// Rest height + size-clamped pitch/roll from `large_body_grounding`.
    pub grounding: Grounding,
}

/// Try to stand `quad` with its center at `center` `[x, z]`, facing `heading`
/// (radians), expecting the ground near `body_y` (the reference foot plane;
/// steps are measured from here). `samples_per_foot` is the per-foot search
/// resolution; `max_lean` caps the grounding tilt. `ground(x, z)` returns the
/// top solid height at a column, or `None` for open air.
///
/// Returns `Some(Stance)` only when ALL four feet find a foothold within their
/// bounds — the connected constraint. Returns `None` the moment any foot is
/// stranded, which the caller treats as "this pose is invalid, nudge the body."
pub fn solve_stance(
    quad: &Quadruped,
    center: [f32; 2],
    heading: f32,
    body_y: f32,
    samples_per_foot: u32,
    max_lean: f32,
    ground: impl Fn(f32, f32) -> Option<f32>,
) -> Option<Stance> {
    let (s, c) = heading.sin_cos();

    // Placeholder feet; every slot is overwritten before use (guarded by the
    // early `?` returns — a missing foothold aborts the whole solve).
    let zero = Foothold { pos: [0.0; 3], reach_used: 0.0, step: 0.0 };
    let mut feet = [zero; 4];
    let mut samples = [FootSample { x: 0.0, z: 0.0, ground: 0.0 }; 4];

    for i in 0..4 {
        let (hx, hz) = (quad.hips[i][0], quad.hips[i][1]);
        // Body-local → world (rotate by heading, translate to center).
        let wx = center[0] + hx * c - hz * s;
        let wz = center[1] + hx * s + hz * c;

        // The connected constraint: any foot with no foothold kills the pose.
        let foot = find_foothold([wx, wz], body_y, &quad.bounds[i], samples_per_foot, &ground)?;

        // For the lean fit, use the foot's ACTUAL contact point mapped back into
        // body-local space (a reached-out foot shifts the contact, which is what
        // tilts the body) rather than the neutral hip.
        let dx = foot.pos[0] - center[0];
        let dz = foot.pos[2] - center[1];
        samples[i] = FootSample {
            x: dx * c + dz * s,   // rotate by -heading
            z: -dx * s + dz * c,
            ground: foot.pos[1],
        };
        feet[i] = foot;
    }

    let grounding = ground_solve(&samples, quad.size, max_lean);
    Some(Stance { feet, grounding })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad() -> Quadruped {
        // A 4×6 footprint, generous reach, big body.
        let b = ReachBounds { radius: 3.0, max_up: 1.5, max_down: 2.5 };
        Quadruped {
            hips: [[-2.0, 3.0], [2.0, 3.0], [-2.0, -3.0], [2.0, -3.0]],
            bounds: [b, b, b, b],
            size: 4.0,
        }
    }

    #[test]
    fn flat_ground_is_level_and_viable() {
        let st = solve_stance(&quad(), [0.0, 0.0], 0.0, 0.0, 64, 0.6, |_, _| Some(0.0))
            .expect("flat ground ⇒ all four feet plant");
        for f in &st.feet {
            assert!(f.reach_used < 0.2 && f.step.abs() < 1e-3, "feet plant under hips");
        }
        assert!(st.grounding.pitch.abs() < 1e-3 && st.grounding.roll.abs() < 1e-3, "level");
        assert!((st.grounding.height - 0.0).abs() < 1e-3);
    }

    #[test]
    fn one_foot_over_a_gap_invalidates_the_whole_stance() {
        // A pit beyond reach under the front-right hip (≈ world (2,3)); the other
        // three are on flat ground. One stranded foot ⇒ the pose is not viable.
        let g = |x: f32, z: f32| {
            if (x - 2.0).hypot(z - 3.0) < 0.8 { None } else { Some(0.0) }
        };
        // radius 3 would let it reach around an 0.8 pit — shrink that foot's reach
        // so the gap genuinely strands it.
        let mut q = quad();
        q.bounds[1] = ReachBounds { radius: 0.5, max_up: 1.5, max_down: 2.5 };
        assert!(solve_stance(&q, [0.0, 0.0], 0.0, 0.0, 128, 0.6, g).is_none(),
            "a stranded foot must invalidate the connected stance");
    }

    #[test]
    fn slope_produces_bounded_lean() {
        // Ground tilts along x; body should pitch/roll but stay clamped & viable.
        let st = solve_stance(&quad(), [0.0, 0.0], 0.0, 0.0, 64, 0.6, |x, _| Some(0.2 * x))
            .expect("a gentle slope is plantable");
        let lean = st.grounding.pitch.abs().max(st.grounding.roll.abs());
        assert!(lean > 0.0, "a slope should tilt the body");
        assert!(lean <= 0.6 + 1e-4, "lean stays within max_lean");
    }

    #[test]
    fn heading_rotates_the_footprint() {
        // Facing +90°, the hips swing but flat ground still grounds all four.
        let st = solve_stance(&quad(), [10.0, -4.0], std::f32::consts::FRAC_PI_2, 1.0, 64, 0.6,
            |_, _| Some(1.0)).expect("rotated pose on flat ground is viable");
        assert!((st.grounding.height - 1.0).abs() < 1e-3);
    }

    #[test]
    fn deterministic() {
        let g = |x: f32, z: f32| Some(0.1 * x - 0.05 * z);
        let a = solve_stance(&quad(), [3.0, 1.0], 0.7, 0.0, 96, 0.6, g);
        let b = solve_stance(&quad(), [3.0, 1.0], 0.7, 0.0, 96, 0.6, g);
        assert_eq!(format!("{a:?}"), format!("{b:?}"));
    }
}
