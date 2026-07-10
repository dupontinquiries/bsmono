//! `sim::deeptime_convergence` — classify a system by how it behaves under deep
//! time, so the fast-forward simulator decides *per system* what to compute past
//! a horizon instead of using one global max-depth flag.
//!
//! From `12_Deep_Time_Simulation/deeptime_convergence_classes.docx` (operator
//! notes r253). The doc's resolution: don't pick between a global "100 years →
//! steady-state → stop" flag and "LOD statistical edges." Classify each
//! generator/system by its deep-time behaviour and let the class decide what
//! happens past the horizon — the max-depth flag is just the *convergent* class.
//!
//! ## The three classes
//! - **A — Procedural / resettable (no history).** Pure function of
//!   `(pos, seed, time)`. Deep-time cost is O(1): you don't fast-forward, you
//!   *evaluate at the target time*. Travelling 1M years = recompute from seed.
//!   No checkpoints, no divergence. (e.g. base terrain height, noise fields.)
//! - **B — Convergent (history that reaches a steady state).** Accumulates but
//!   provably settles: state stops changing meaningfully after some horizon.
//!   This is the "100 years → steady-state → stop" flag. Past the horizon, snap
//!   to the settled value. (e.g. fire/heat fields that cap, erosion that flattens.)
//! - **C — Chaotic / path-dependent (never fully settles).** Genuinely keeps
//!   evolving and the fine trajectory can matter: wandering-climate migration, a
//!   river system that keeps re-routing, a named dragon's territory history. Must
//!   be simulated at full stage resolution across the span.
//!
//! ## Waypoint density (hybrid step model)
//! Per `hybrid_step_model.docx`: Class-C channels set the *minimum* stage count
//! (they can't be culled); Class-B channels ride the same fill for free and merge
//! at intermediates; Class-A channels contribute nothing (evaluated directly).

/// How a system behaves under deep-time fast-forward.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ConvergenceClass {
    /// A — pure function of `(pos, seed, time)`, O(1) to evaluate at any time.
    ProceduralResettable,
    /// B — accumulates but provably settles to a steady state after a horizon.
    Convergent,
    /// C — path-dependent / chaotic; the fine trajectory matters.
    Chaotic,
}

impl ConvergenceClass {
    /// Does this system need accumulated history (checkpoints) to reach a target
    /// time? Class A doesn't — it's evaluated directly from the seed.
    pub const fn needs_history(self) -> bool {
        !matches!(self, ConvergenceClass::ProceduralResettable)
    }

    /// Does the system reach a steady state we can snap to past a horizon?
    /// A settles trivially (stateless); B settles by definition; C never does.
    pub const fn settles(self) -> bool {
        !matches!(self, ConvergenceClass::Chaotic)
    }

    /// Can a "snap at the horizon" (seeded-PRNG statistical edge) replace the tail
    /// of the sim? True only for B — the always-converges cull target. A needs no
    /// tail (direct eval); C's tail is load-bearing.
    pub const fn snappable_at_horizon(self) -> bool {
        matches!(self, ConvergenceClass::Convergent)
    }

    /// Does this class drive the hybrid step model's *minimum* waypoint count?
    /// Only C — it can't be culled, so it pins the stage resolution. A contributes
    /// nothing (evaluated directly); B rides C's fill for free.
    pub const fn pins_stage_count(self) -> bool {
        matches!(self, ConvergenceClass::Chaotic)
    }

    /// Short tag for logs/debug.
    pub const fn tag(self) -> &'static str {
        match self {
            ConvergenceClass::ProceduralResettable => "A/procedural",
            ConvergenceClass::Convergent => "B/convergent",
            ConvergenceClass::Chaotic => "C/chaotic",
        }
    }
}

/// The compute strategy the fast-forward simulator should use for one system over
/// a requested time span.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DeepTimePlan {
    /// Class A — evaluate the generator directly at the target time. O(1).
    EvaluateAt,
    /// Class B, span beyond the settle horizon — fast-forward (or snap) to the
    /// horizon, then hold the steady state for the remainder. `horizon_years` is
    /// the point past which further simulation is wasted.
    SnapAtHorizon { horizon_years: f64 },
    /// Class B within its horizon, or Class C — simulate across `stages` staged
    /// waypoints (Class C pins the minimum; B rides the same fill).
    FullSim { stages: u32 },
}

/// Plan the deep-time compute for one system.
///
/// - `class`: its convergence class.
/// - `span_years`: how far forward we're fast-forwarding.
/// - `horizon_years`: the settle horizon (only meaningful for Class B).
/// - `stage_hint`: desired waypoints for a full sim of this span (density knob).
pub fn plan(
    class: ConvergenceClass,
    span_years: f64,
    horizon_years: f64,
    stage_hint: u32,
) -> DeepTimePlan {
    match class {
        ConvergenceClass::ProceduralResettable => DeepTimePlan::EvaluateAt,
        ConvergenceClass::Convergent => {
            if span_years > horizon_years && horizon_years > 0.0 {
                DeepTimePlan::SnapAtHorizon { horizon_years }
            } else {
                // Still inside the settle window — must sim up to the target.
                DeepTimePlan::FullSim { stages: stage_hint.max(1) }
            }
        }
        ConvergenceClass::Chaotic => DeepTimePlan::FullSim { stages: stage_hint.max(1) },
    }
}

/// Minimum waypoint (stage) count for a hybrid step across a set of co-simulated
/// systems: the max stage requirement over the Class-C channels (which pin it),
/// falling back to `base` when there are none (B rides for free, A contributes
/// nothing). Per `hybrid_step_model.docx`.
pub fn min_stage_count<I>(classes_with_hints: I, base: u32) -> u32
where
    I: IntoIterator<Item = (ConvergenceClass, u32)>,
{
    let mut pinned = 0u32;
    for (c, hint) in classes_with_hints {
        if c.pins_stage_count() {
            pinned = pinned.max(hint);
        }
    }
    pinned.max(base)
}

/// The effective compute cost order of a plan, for the scheduler's budgeting:
/// direct-eval and horizon-snap are O(1) in the span; full sim is O(stages).
pub fn plan_stage_cost(plan: DeepTimePlan) -> u32 {
    match plan {
        DeepTimePlan::EvaluateAt => 1,
        DeepTimePlan::SnapAtHorizon { .. } => 1,
        DeepTimePlan::FullSim { stages } => stages,
    }
}

/// r710: convenience classifier for the engine's known deep-time systems, so
/// callers get a consistent class without re-deriving it. Extend as systems are
/// wired into the fast-forward simulator.
pub fn classify_system(name: &str) -> Option<ConvergenceClass> {
    Some(match name {
        // Class A — pure procedural, evaluated from seed at the target time.
        "terrain_height" | "base_noise" | "biome_field" | "cave_carve" => {
            ConvergenceClass::ProceduralResettable
        }
        // Class B — accumulate then settle (the doc's cited examples + engine caps).
        "fire_field" | "heat_field" | "erosion" | "smoke_field" | "water_settle" => {
            ConvergenceClass::Convergent
        }
        // Class C — path-dependent histories that must be simulated in full.
        "climate_migration" | "river_routing" | "dragon_territory" | "population" => {
            ConvergenceClass::Chaotic
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_predicates() {
        use ConvergenceClass::*;
        assert!(!ProceduralResettable.needs_history());
        assert!(Convergent.needs_history() && Chaotic.needs_history());
        assert!(ProceduralResettable.settles() && Convergent.settles());
        assert!(!Chaotic.settles());
        // Only B can be replaced by a horizon snap.
        assert!(Convergent.snappable_at_horizon());
        assert!(!ProceduralResettable.snappable_at_horizon() && !Chaotic.snappable_at_horizon());
        // Only C pins the stage count.
        assert!(Chaotic.pins_stage_count());
        assert!(!ProceduralResettable.pins_stage_count() && !Convergent.pins_stage_count());
    }

    #[test]
    fn plan_per_class() {
        use ConvergenceClass::*;
        // A is always a direct evaluate, regardless of span.
        assert_eq!(plan(ProceduralResettable, 1_000_000.0, 100.0, 8), DeepTimePlan::EvaluateAt);
        // B past its horizon snaps.
        assert_eq!(
            plan(Convergent, 5000.0, 100.0, 8),
            DeepTimePlan::SnapAtHorizon { horizon_years: 100.0 }
        );
        // B inside its horizon must sim.
        assert_eq!(plan(Convergent, 50.0, 100.0, 8), DeepTimePlan::FullSim { stages: 8 });
        // C always sims at full resolution.
        assert_eq!(plan(Chaotic, 1_000_000.0, 100.0, 8), DeepTimePlan::FullSim { stages: 8 });
    }

    #[test]
    fn horizon_snap_is_o1_but_c_is_linear() {
        use ConvergenceClass::*;
        assert_eq!(plan_stage_cost(plan(ProceduralResettable, 9e9, 1.0, 64)), 1);
        assert_eq!(plan_stage_cost(plan(Convergent, 9e9, 1.0, 64)), 1);
        assert_eq!(plan_stage_cost(plan(Chaotic, 9e9, 1.0, 64)), 64);
    }

    #[test]
    fn c_channels_pin_the_stage_count() {
        use ConvergenceClass::*;
        // Only the Chaotic channel's hint (12) pins the count; A/B are ignored.
        let chans = [(ProceduralResettable, 99), (Convergent, 40), (Chaotic, 12)];
        assert_eq!(min_stage_count(chans, 4), 12);
        // No C channels → falls back to base.
        assert_eq!(min_stage_count([(Convergent, 40), (ProceduralResettable, 99)], 4), 4);
    }

    #[test]
    fn known_systems_classified() {
        assert_eq!(classify_system("terrain_height"), Some(ConvergenceClass::ProceduralResettable));
        assert_eq!(classify_system("fire_field"), Some(ConvergenceClass::Convergent));
        assert_eq!(classify_system("dragon_territory"), Some(ConvergenceClass::Chaotic));
        assert_eq!(classify_system("nonexistent"), None);
    }
}
