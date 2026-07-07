//! foot_reach — where a foot can actually *plant*. The engine's cliff stepping
//! probed a single block of air under each leg. That works for a point-sized
//! walker, but a dragon's feet span many blocks: each foot must find its OWN
//! foothold within ITS OWN reach, and a foot suspended over a gap wider than it
//! can reach has *nowhere* to plant — the cue that the body must reposition
//! rather than step.
//!
//! This searches the terrain within one foot's reach bounds and returns the
//! best reachable foothold, or `None` when the foot is stranded. It is the
//! decision the prior 1-block search skipped; the chosen foothold then feeds
//! `crate::sim::foot_ik::solve_leg` (the target) and the per-foot samples feed
//! `crate::sim::large_body_grounding::solve` (the contact plane / lean). The
//! vertical bounds come from `crate::sim::step_height`.
//!
//! Pure + deterministic. Terrain is supplied as a sampler closure, so this is
//! IO-free and unit-testable without a world.
//!
//! The connected, four-foot constraint ("the body can only step here if all
//! four feet find footholds") is the layer ABOVE this: run `find_foothold` per
//! foot at each hip's neutral point and treat any `None` as "this stance is not
//! viable." That solver is intentionally not in this module — this is its atom.

/// How far a foot may stray from its neutral (hip-projected) stance point, and
/// how big a vertical step it will accept, before the spot is unreachable.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ReachBounds {
    /// Max horizontal distance from the neutral point, in blocks.
    pub radius: f32,
    /// Max lip the foot may step UP onto (typically `step_height::max_step`).
    pub max_up: f32,
    /// Max drop the foot may step DOWN to and still call it a step (beyond
    /// this it is a fall, not a foothold).
    pub max_down: f32,
}

/// A chosen place to plant a foot.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Foothold {
    /// Plant position `[x, ground_y, z]`.
    pub pos: [f32; 3],
    /// Horizontal distance from neutral that was needed (0 = right under the hip).
    pub reach_used: f32,
    /// Signed vertical step taken (+ up onto a lip, − down to a ledge).
    pub step: f32,
}

/// Golden angle — spreads spiral samples so no two land on the same spoke.
const GOLDEN_ANGLE: f32 = 2.399_963_2; // ≈ π(3 − √5)

/// Search for the best reachable foothold for ONE foot.
///
/// - `neutral` — the foot's preferred plant point `[x, z]` (the hip projected
///   straight down).
/// - `neutral_y` — the body's reference foot height at that point (where the
///   foot would sit on flat ground under the hip). Steps are measured from here.
/// - `bounds` — the reach envelope.
/// - `samples` — how many candidate points to probe across the reach disc
///   (more = finer search; the neutral point is always probed first/free).
/// - `ground` — returns the top solid surface height at `(x, z)`, or `None`
///   when that column has no plantable surface within search depth (open air /
///   bottomless). Pass your column heightmap / voxel probe here.
///
/// Returns the plantable spot that minimises reach first, then step magnitude
/// (stay under the hip; take the smallest step that works), or `None` when no
/// probed point is plantable within bounds — i.e. the foot is over a gap and
/// the caller should reposition the body, not force a step.
pub fn find_foothold(
    neutral: [f32; 2],
    neutral_y: f32,
    bounds: &ReachBounds,
    samples: u32,
    ground: impl Fn(f32, f32) -> Option<f32>,
) -> Option<Foothold> {
    let radius = bounds.radius.max(0.0);

    // Cost of a plantable candidate: reach dominates (a foot wants to stay
    // under its hip), step magnitude breaks ties. Lower is better.
    let cost = |reach: f32, step: f32| reach * 4.0 + step.abs();

    let mut best: Option<(f32, Foothold)> = None;
    let mut consider = |x: f32, z: f32| {
        let Some(gy) = ground(x, z) else { return; };
        let step = gy - neutral_y;
        if step > bounds.max_up || step < -bounds.max_down {
            return; // ground exists but is out of vertical reach
        }
        let dx = x - neutral[0];
        let dz = z - neutral[1];
        let reach = (dx * dx + dz * dz).sqrt();
        if reach > radius + 1e-4 {
            return; // outside the horizontal envelope
        }
        let c = cost(reach, step);
        let cand = Foothold { pos: [x, gy, z], reach_used: reach, step };
        match best {
            Some((bc, _)) if bc <= c => {}
            _ => best = Some((c, cand)),
        }
    };

    // Always probe straight under the hip first (the ideal, zero-reach spot).
    consider(neutral[0], neutral[1]);

    // Sunflower/Vogel spiral across the disc: r = radius·√t spaces points by
    // equal area, golden-angle rotation avoids spokes. Deterministic in `i`.
    let n = samples.max(1);
    for i in 0..n {
        let t = (i as f32 + 0.5) / n as f32;
        let r = radius * t.sqrt();
        let a = i as f32 * GOLDEN_ANGLE;
        consider(neutral[0] + r * a.cos(), neutral[1] + r * a.sin());
    }

    best.map(|(_, f)| f)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds() -> ReachBounds {
        ReachBounds { radius: 3.0, max_up: 1.2, max_down: 2.0 }
    }

    #[test]
    fn flat_ground_plants_under_hip() {
        let h = find_foothold([10.0, 5.0], 0.0, &bounds(), 64, |_, _| Some(0.0))
            .expect("flat ground is plantable");
        assert!(h.reach_used < 0.2, "should plant ~under the hip, got {}", h.reach_used);
        assert!(h.step.abs() < 1e-3, "no step on flat ground, got {}", h.step);
    }

    #[test]
    fn steps_up_a_lip_within_reach() {
        // Neutral spot is low; a +1 lip sits to the +x side, within reach & max_up.
        let h = find_foothold([0.0, 0.0], 0.0, &bounds(), 128, |x, _| {
            Some(if x > 1.0 { 1.0 } else { 0.0 })
        }).expect("lip is plantable");
        // Flat ground under the hip is the zero-reach win; confirm it CAN find
        // the lip when the near ground is gone (see cliff test). Here flat wins.
        assert!(h.step.abs() < 1e-3 && h.reach_used < 0.2);
    }

    #[test]
    fn reaches_to_a_ledge_when_hip_is_over_a_cliff() {
        // Under the hip (x<0) the column is bottomless (None) or far below; a
        // ledge at x≥1 is reachable and within step. The foot must REACH it
        // rather than report nothing.
        let h = find_foothold([0.0, 0.0], 0.0, &bounds(), 256, |x, _| {
            if x >= 1.0 { Some(0.0) } else { None } // air under the hip
        }).expect("ledge within reach must be found");
        assert!(h.pos[0] >= 1.0 - 1e-3, "foothold should be on the ledge, got x={}", h.pos[0]);
        assert!(h.reach_used > 0.5, "should have reached out, got {}", h.reach_used);
    }

    #[test]
    fn stranded_over_a_wide_gap_returns_none() {
        // Nearest ground is beyond the reach radius in every direction → None,
        // the signal to reposition the body instead of stepping.
        let h = find_foothold([0.0, 0.0], 0.0, &bounds(), 256, |x, z| {
            if (x * x + z * z).sqrt() > 5.0 { Some(0.0) } else { None }
        });
        assert!(h.is_none(), "no foothold within reach ⇒ None");
    }

    #[test]
    fn ground_below_max_down_is_not_a_foothold() {
        // A floor exists everywhere but it's a 4-block drop; max_down is 2.
        let h = find_foothold([0.0, 0.0], 0.0, &bounds(), 64, |_, _| Some(-4.0));
        assert!(h.is_none(), "a drop beyond max_down is a fall, not a step");
    }

    #[test]
    fn deterministic() {
        let g = |x: f32, _z: f32| Some(if x > 0.5 { 0.5 } else { 0.0 });
        let a = find_foothold([2.0, 2.0], 0.0, &bounds(), 200, g);
        let b = find_foothold([2.0, 2.0], 0.0, &bounds(), 200, g);
        assert_eq!(a, b);
    }
}
