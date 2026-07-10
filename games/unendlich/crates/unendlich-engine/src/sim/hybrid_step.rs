//! `sim::hybrid_step` — R2 plans waypoints, R1 fills between them.
//!
//! From `12_Deep_Time_Simulation/hybrid_step_model.docx` (operator notes r257).
//! The live driver for deep-time fast-forward, and the first real caller of
//! [`super::deeptime_convergence`]:
//!
//! - **R2 (planner, not oracle):** proposes a handful of coarse *waypoints* the
//!   region passes through over the span. Cheap, and can even be a search,
//!   because it is not trusted as truth.
//! - **R1 (authoritative fill):** integrates deterministically *between* waypoints
//!   — seeded by `hash(seed, region, tick, digest(state))` — at a coarse sub-step
//!   cadence (a few exact evaluations per stage, each advancing several ticks), so
//!   it does far fewer exact steps than long-form while still landing exactly.
//!
//! The waypoint count (the "vector length") is the **minimum over the modelled
//! systems' fidelity horizons** — which is exactly the convergence-class rule:
//! Class-C (genome/population) channels pin the stage count, Class-B ride the same
//! fill for free, Class-A contribute nothing (evaluated directly). Per-system, the
//! plan then decides evaluate-directly / snap-at-horizon / full-sim.

use super::deeptime_convergence::{min_stage_count, plan, ConvergenceClass, DeepTimePlan};

/// A planned deep-time step schedule for one span.
#[derive(Clone, Debug, PartialEq)]
pub struct StepSchedule {
    /// Times (years from the span start) of the R2 waypoints, endpoint inclusive.
    /// `stages() == waypoints.len() - 1`.
    pub waypoints: Vec<f64>,
    /// R1 exact evaluations between each pair of waypoints.
    pub substeps_per_stage: u32,
}

impl StepSchedule {
    pub fn stages(&self) -> u32 {
        self.waypoints.len().saturating_sub(1) as u32
    }
    /// Total R1 exact evaluations if every stage is fully filled.
    pub fn full_exact_evals(&self) -> u32 {
        self.stages() * self.substeps_per_stage.max(1)
    }
}

/// **R2** — plan the waypoint schedule for a span co-simulating `systems` (each a
/// `(class, stage_hint)`). Stage count = `min_stage_count(systems, base_stages)`,
/// so Class-C channels pin it, B rides free, A adds nothing. Waypoints are evenly
/// spaced across `[0, span_years]`.
pub fn plan_schedule(
    span_years: f64,
    systems: &[(ConvergenceClass, u32)],
    base_stages: u32,
    substeps_per_stage: u32,
) -> StepSchedule {
    let stages = min_stage_count(systems.iter().copied(), base_stages).max(1);
    let span = span_years.max(0.0);
    let mut waypoints = Vec::with_capacity(stages as usize + 1);
    let mut k = 0u32;
    while k <= stages {
        waypoints.push(span * (k as f64) / (stages as f64));
        k += 1;
    }
    StepSchedule { waypoints, substeps_per_stage: substeps_per_stage.max(1) }
}

/// **R1** fill seed: `hash(world_seed, region_id, tick, state_digest)` — the doc's
/// deterministic sub-step seeding, so the integration between two waypoints is
/// reproducible regardless of how R2's proposal was reached.
pub fn substep_seed(world_seed: u64, region_id: u64, tick: u64, state_digest: u64) -> u64 {
    let mut h = world_seed;
    let mut i = 0;
    let xs = [region_id, tick, state_digest];
    while i < xs.len() {
        h ^= xs[i]
            .wrapping_add(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(h << 6)
            .wrapping_add(h >> 2);
        i += 1;
    }
    h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^ (h >> 27)
}

/// The exact-evaluation cost of running `class` over the span on this schedule,
/// via its convergence plan:
/// - **A** → 0 (evaluated directly at the target time, no fill).
/// - **B past horizon** → only the stages up to the settle horizon are filled,
///   then the tail is snapped (cost scales with `horizon/span`).
/// - **B within horizon, or C** → the full schedule (`stages × substeps`).
///
/// Summing this over a system set gives the scheduler its deep-time budget.
pub fn system_exact_evals(
    sched: &StepSchedule,
    class: ConvergenceClass,
    span_years: f64,
    horizon_years: f64,
) -> u32 {
    match plan(class, span_years, horizon_years, sched.stages()) {
        DeepTimePlan::EvaluateAt => 0,
        DeepTimePlan::SnapAtHorizon { horizon_years } => {
            let frac = if span_years > 0.0 {
                (horizon_years / span_years).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let stages_to_horizon = (sched.stages() as f64 * frac).ceil() as u32;
            stages_to_horizon.max(1) * sched.substeps_per_stage.max(1)
        }
        DeepTimePlan::FullSim { .. } => sched.full_exact_evals(),
    }
}

/// Total exact evaluations for a whole co-simulated system set over the span —
/// the headline number the fast-forward budget is compared against. Demonstrates
/// the win: a set that's mostly Class-A/B costs far less than `systems × full`.
pub fn total_exact_evals(
    sched: &StepSchedule,
    systems: &[(ConvergenceClass, u32)],
    span_years: f64,
    horizon_years: f64,
) -> u32 {
    systems
        .iter()
        .map(|&(c, _)| system_exact_evals(sched, c, span_years, horizon_years))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::deeptime_convergence::ConvergenceClass::*;

    #[test]
    fn class_c_pins_the_waypoint_count() {
        // A Chaotic channel wants 12 stages; A/B hints are ignored.
        let systems = [(ProceduralResettable, 99), (Convergent, 40), (Chaotic, 12)];
        let s = plan_schedule(1000.0, &systems, 4, 3);
        assert_eq!(s.stages(), 12);
        assert_eq!(s.waypoints.len(), 13);
        // Endpoints are exact.
        assert_eq!(s.waypoints[0], 0.0);
        assert_eq!(*s.waypoints.last().unwrap(), 1000.0);
        // No C channel → falls back to base_stages.
        let s2 = plan_schedule(1000.0, &[(Convergent, 40)], 4, 3);
        assert_eq!(s2.stages(), 4);
    }

    #[test]
    fn substep_seed_is_deterministic_and_sensitive() {
        let a = substep_seed(1, 2, 3, 4);
        assert_eq!(a, substep_seed(1, 2, 3, 4));
        // Changing any input changes the seed.
        assert_ne!(a, substep_seed(1, 2, 3, 5));
        assert_ne!(a, substep_seed(9, 2, 3, 4));
    }

    #[test]
    fn cost_reflects_the_convergence_plan() {
        let systems = [(Chaotic, 8)];
        let s = plan_schedule(10_000.0, &systems, 4, 2); // 8 stages × 2 = 16 full
        // Class A is free (evaluate at target).
        assert_eq!(system_exact_evals(&s, ProceduralResettable, 10_000.0, 100.0), 0);
        // Class C pays the full schedule.
        assert_eq!(system_exact_evals(&s, Chaotic, 10_000.0, 100.0), 16);
        // Class B past its horizon pays only the horizon fraction (100/10000 → 1 stage).
        let b = system_exact_evals(&s, Convergent, 10_000.0, 100.0);
        assert!(b < s.full_exact_evals(), "B past horizon must be cheaper than full");
        assert_eq!(b, 2); // ceil(8 * 0.01)=1 stage × 2 substeps
        // Class B within its horizon pays full.
        assert_eq!(system_exact_evals(&s, Convergent, 50.0, 100.0), s.full_exact_evals());
    }

    #[test]
    fn mixed_set_is_far_cheaper_than_naive() {
        // 1 chaotic + 3 convergent(past horizon) + 4 procedural over a long span.
        let systems = [
            (Chaotic, 6),
            (Convergent, 6), (Convergent, 6), (Convergent, 6),
            (ProceduralResettable, 6), (ProceduralResettable, 6),
            (ProceduralResettable, 6), (ProceduralResettable, 6),
        ];
        let s = plan_schedule(100_000.0, &systems, 4, 4); // 6 stages × 4 = 24 full each
        let naive = systems.len() as u32 * s.full_exact_evals(); // if everything ran full
        let actual = total_exact_evals(&s, &systems, 100_000.0, 100.0);
        assert!(actual < naive / 3, "hybrid should be <1/3 of naive: {actual} vs {naive}");
    }
}
