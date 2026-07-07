//! quad_reposition — what to do when `quad_stance::solve_stance` says `None`.
//!
//! `solve_stance` reports that a candidate body pose strands a foot, but it
//! doesn't fix it. This closes that loop: given the body's *desired* center, it
//! returns the NEAREST center at which all four feet find footholds — the
//! "step back / shuffle over until you can plant" a real animal does at a cliff
//! edge. If the desired pose is already viable it returns it unchanged
//! (`shifted == 0`); if nowhere within the search window works, it returns
//! `None` (the spot is genuinely unstandable — back off entirely).
//!
//! Pure + deterministic. Same terrain-closure shape as `foot_reach` /
//! `quad_stance`. This is meant to be called only when a naive stance fails,
//! not every frame for every entity.

use crate::sim::quad_stance::{solve_stance, Quadruped, Stance};

/// A settled stance: the (possibly nudged) pose plus how far it had to move.
#[derive(Clone, Copy, Debug)]
pub struct Settled {
    /// The center the body actually stands at (== desired if `shifted == 0`).
    pub center: [f32; 2],
    /// The body reference height used (unchanged from the request here).
    pub body_y: f32,
    /// The viable four-foot stance at `center`.
    pub stance: Stance,
    /// Horizontal distance from the desired center (0 = desired worked as-is).
    pub shifted: f32,
}

/// Golden-angle spiral, equal-area, deterministic in the sample index.
const GOLDEN_ANGLE: f32 = 2.399_963_2;

/// Find the nearest viable stance to `desired_center`.
///
/// Tries the desired pose first; if it strands a foot, probes centers on an
/// outward spiral within `search_radius` (`search_steps` of them) and returns
/// the FIRST viable one — and because the spiral grows monotonically outward,
/// first ≈ nearest. `heading`, `body_y`, `samples_per_foot`, `max_lean`, and
/// `ground` pass straight through to `solve_stance`.
///
/// Returns `None` only if neither the desired center nor any probe stands —
/// i.e. there is no foothold set for the body anywhere in the window.
#[allow(clippy::too_many_arguments)]
pub fn settle_stance(
    quad: &Quadruped,
    desired_center: [f32; 2],
    heading: f32,
    body_y: f32,
    search_radius: f32,
    search_steps: u32,
    samples_per_foot: u32,
    max_lean: f32,
    ground: impl Fn(f32, f32) -> Option<f32> + Copy,
) -> Option<Settled> {
    // Desired pose first — the common case, zero shift.
    if let Some(stance) =
        solve_stance(quad, desired_center, heading, body_y, samples_per_foot, max_lean, ground)
    {
        return Some(Settled { center: desired_center, body_y, stance, shifted: 0.0 });
    }

    // Spiral outward from the desired center; first hit is ~nearest.
    let radius = search_radius.max(0.0);
    let n = search_steps.max(1);
    for i in 0..n {
        let t = (i as f32 + 0.5) / n as f32;
        let r = radius * t.sqrt();
        let a = i as f32 * GOLDEN_ANGLE;
        let center = [desired_center[0] + r * a.cos(), desired_center[1] + r * a.sin()];
        if let Some(stance) =
            solve_stance(quad, center, heading, body_y, samples_per_foot, max_lean, ground)
        {
            return Some(Settled { center, body_y, stance, shifted: r });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::foot_reach::ReachBounds;

    fn quad() -> Quadruped {
        let b = ReachBounds { radius: 1.2, max_up: 1.5, max_down: 2.5 };
        Quadruped {
            hips: [[-2.0, 3.0], [2.0, 3.0], [-2.0, -3.0], [2.0, -3.0]],
            bounds: [b, b, b, b],
            size: 4.0,
        }
    }

    #[test]
    fn desired_pose_works_means_zero_shift() {
        let s = settle_stance(&quad(), [0.0, 0.0], 0.0, 0.0, 6.0, 200, 48, 0.6, |_, _| Some(0.0))
            .expect("flat ground stands as-is");
        assert_eq!(s.shifted, 0.0);
        assert_eq!(s.center, [0.0, 0.0]);
    }

    #[test]
    fn shuffles_off_a_cliff_edge_to_the_nearest_solid_stance() {
        // Everything with x > 0 is open air (a cliff to the east). At the desired
        // center the east hips sit at world x≈2; the nearest solid (x≤0) is 2
        // blocks away — beyond the foot's 1.2 reach — so those feet are stranded.
        // Repositioning WEST until all four are over solid ground should succeed.
        let g = |x: f32, _z: f32| if x > 0.0 { None } else { Some(0.0) };
        let s = settle_stance(&quad(), [0.0, 0.0], 0.0, 0.0, 8.0, 400, 48, 0.6, g)
            .expect("a viable stance exists to the west");
        assert!(s.shifted > 0.0, "had to move off the edge");
        // Every foot must now be over solid ground (world x ≤ 0).
        for f in &s.stance.feet {
            assert!(f.pos[0] <= 1e-3, "every foot on solid ground, foot x={}", f.pos[0]);
        }
    }

    #[test]
    fn no_standable_spot_in_window_returns_none() {
        // A pillar-top smaller than the footprint: only a tiny solid disc, the
        // body can't fit all four feet anywhere near it.
        let g = |x: f32, z: f32| if x.hypot(z) < 0.5 { Some(0.0) } else { None };
        assert!(settle_stance(&quad(), [0.0, 0.0], 0.0, 0.0, 4.0, 200, 48, 0.6, g).is_none());
    }

    #[test]
    fn deterministic() {
        let g = |x: f32, _z: f32| if x > 1.0 { None } else { Some(0.0) };
        let a = settle_stance(&quad(), [0.5, 0.0], 0.0, 0.0, 8.0, 300, 48, 0.6, g);
        let b = settle_stance(&quad(), [0.5, 0.0], 0.0, 0.0, 8.0, 300, 48, 0.6, g);
        assert_eq!(format!("{a:?}"), format!("{b:?}"));
    }
}
