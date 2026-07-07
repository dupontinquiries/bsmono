//! r675: bird/dragon flight-mode selection — from
//! `04_Creatures_and_Dragons/flying_animal_behaviour.docx` §2.1.
//!
//! `sim::wing_loading` already gives the size-dependent envelope (stall speed,
//! regime, updraft need); this is the **behavioural FSM** on top of it: every
//! tick a flier picks one of seven flight modes based on its energy state, the
//! wind property `W`, and whether a thermal/ridge/gradient is available — the
//! "primary flight FSM decision" the doc describes. Each mode has a relative
//! power cost (the energy-saving modes are why birds don't just flap constantly).
//!
//! Pure, std-only, deterministic. The caller supplies the environment (wind,
//! thermal/ridge presence) and the species' capabilities as flags, so this
//! module needs no world/creature deps.

/// The seven flight modes (doc §2.1), plus a plain wings-extended `Glide` used
/// when descending toward a target with no lift source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlightMode {
    /// Continuous wingbeats — takeoff + sustained level flight. Full power.
    PoweredFlapping,
    /// Flap bursts + wing-folded ballistic arcs (small birds). ~15% cheaper.
    Bounding,
    /// Flap + wings-extended glide (woodpecker/kestrel). ~30% cheaper.
    Undulating,
    /// Circling a thermal column for free altitude. Needs `W ≥ 0.10` + thermal.
    ThermalSoaring,
    /// Banking into a ridge updraft. Needs a ridge + `W ≥ 0.20`.
    RidgeSoaring,
    /// S-curves across a wind gradient (albatross). Needs `W ≥ 0.50` + gradient.
    DynamicSoaring,
    /// Folded-wing high-angle dive for a prey strike / threat response.
    Stooping,
    /// Passive wings-out descent toward a target (no lift source).
    Glide,
}

/// Wind thresholds for the soaring modes (doc §2.1).
pub const W_THERMAL: f32 = 0.10;
pub const W_RIDGE: f32 = 0.20;
pub const W_DYNAMIC: f32 = 0.50;

impl FlightMode {
    /// Relative power cost vs. continuous powered flapping (1.0). The soaring
    /// modes are near-free; a committed stoop spends nothing (gravity does the
    /// work). Bounding/undulating encode the doc's ~15% / ~30% savings.
    pub fn power_factor(self) -> f32 {
        match self {
            FlightMode::PoweredFlapping => 1.0,
            FlightMode::Bounding => 0.85,
            FlightMode::Undulating => 0.70,
            FlightMode::ThermalSoaring => 0.05,
            FlightMode::RidgeSoaring => 0.05,
            FlightMode::DynamicSoaring => 0.02,
            FlightMode::Stooping => 0.0,
            FlightMode::Glide => 0.05,
        }
    }

    /// Does this mode gain altitude at (near) zero flap cost?
    pub fn is_soaring(self) -> bool {
        matches!(
            self,
            FlightMode::ThermalSoaring | FlightMode::RidgeSoaring | FlightMode::DynamicSoaring
        )
    }
}

/// The momentary flight environment the FSM reads.
#[derive(Clone, Copy, Debug, Default)]
pub struct Env {
    /// Wind property `W` in `[0,1]` (from the world/bending-field wind).
    pub wind_w: f32,
    /// A thermal column is available here.
    pub thermal: bool,
    /// A ridge updraft is available (flying parallel to a cliff edge).
    pub ridge: bool,
    /// A measurable wind gradient over 5+ blocks (dynamic soaring).
    pub wind_gradient: bool,
    /// The flier is committed to a stoop (diving on prey / fleeing down).
    pub committed_dive: bool,
    /// The flier is descending toward a landing/target (no need to hold altitude).
    pub descending_to_target: bool,
}

/// Which modes a species can use (wing type + size). A sparrow bounds; an eagle
/// thermal-soars; an albatross dynamic-soars; a dragon rides updrafts.
#[derive(Clone, Copy, Debug)]
pub struct Caps {
    pub can_bound: bool,
    pub can_undulate: bool,
    pub can_thermal: bool,
    pub can_ridge: bool,
    pub can_dynamic: bool,
    pub can_stoop: bool,
}

impl Caps {
    /// A generic mid-size flier: undulates, thermal- and ridge-soars, can stoop;
    /// doesn't bound (that's for tiny birds) or dynamic-soar (albatross only).
    pub fn generic() -> Caps {
        Caps { can_bound: false, can_undulate: true, can_thermal: true, can_ridge: true, can_dynamic: false, can_stoop: true }
    }
    /// A small bird: bounds + undulates, can stoop; too small for big soaring.
    pub fn small_bird() -> Caps {
        Caps { can_bound: true, can_undulate: true, can_thermal: false, can_ridge: false, can_dynamic: false, can_stoop: true }
    }
    /// A heavy soarer (eagle / dragon): all soaring, stoops, no bounding.
    pub fn soarer() -> Caps {
        Caps { can_bound: false, can_undulate: true, can_thermal: true, can_ridge: true, can_dynamic: true, can_stoop: true }
    }
}

/// The best soaring mode available in `env` for a flier with `caps`, if any.
/// Preference: thermal → ridge → dynamic (cheapest / most sustainable first that
/// its conditions allow). Returns `None` when no lift source is usable.
pub fn best_soar(env: &Env, caps: &Caps) -> Option<FlightMode> {
    if caps.can_thermal && env.thermal && env.wind_w >= W_THERMAL {
        Some(FlightMode::ThermalSoaring)
    } else if caps.can_ridge && env.ridge && env.wind_w >= W_RIDGE {
        Some(FlightMode::RidgeSoaring)
    } else if caps.can_dynamic && env.wind_gradient && env.wind_w >= W_DYNAMIC {
        Some(FlightMode::DynamicSoaring)
    } else {
        None
    }
}

/// Select the flight mode this tick.
///
/// - `energy01` is the flier's energy reserve in `[0,1]`.
/// - `covering_distance` = true when the bird wants range (flap efficiently);
///   false when it's loitering/conserving (soar if it can).
///
/// Priority: a committed dive stoops; otherwise, when conserving OR low on
/// energy, take a soaring mode if the environment offers one; failing that, use
/// the cheapest flap mode the species has (undulating < bounding < powered); a
/// descent toward a target glides. Distance-covering with energy to spare uses
/// powered flapping (or undulating as its efficient cruise).
pub fn select_mode(env: &Env, caps: &Caps, energy01: f32, covering_distance: bool) -> FlightMode {
    if env.committed_dive && caps.can_stoop {
        return FlightMode::Stooping;
    }
    let conserving = !covering_distance || energy01 < 0.35;
    if conserving {
        if let Some(soar) = best_soar(env, caps) {
            return soar;
        }
        if env.descending_to_target {
            return FlightMode::Glide;
        }
        if caps.can_undulate {
            return FlightMode::Undulating;
        }
        if caps.can_bound {
            return FlightMode::Bounding;
        }
        return FlightMode::PoweredFlapping;
    }
    // Covering distance with energy: efficient cruise if available, else power.
    if caps.can_undulate {
        FlightMode::Undulating
    } else {
        FlightMode::PoweredFlapping
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn committed_dive_stoops() {
        let env = Env { committed_dive: true, ..Default::default() };
        assert_eq!(select_mode(&env, &Caps::soarer(), 1.0, true), FlightMode::Stooping);
    }

    #[test]
    fn conserving_soarer_takes_thermal_when_wind_allows() {
        let mut env = Env { thermal: true, wind_w: 0.05, ..Default::default() };
        // too little wind → no soaring, falls to a cheap flap mode
        assert_eq!(select_mode(&env, &Caps::soarer(), 0.9, false), FlightMode::Undulating);
        env.wind_w = 0.12; // now above the thermal threshold
        assert_eq!(select_mode(&env, &Caps::soarer(), 0.9, false), FlightMode::ThermalSoaring);
    }

    #[test]
    fn soar_preference_thermal_over_ridge_over_dynamic() {
        let env = Env { thermal: true, ridge: true, wind_gradient: true, wind_w: 0.9, ..Default::default() };
        assert_eq!(best_soar(&env, &Caps::soarer()), Some(FlightMode::ThermalSoaring));
        let env2 = Env { ridge: true, wind_gradient: true, wind_w: 0.9, ..Default::default() };
        assert_eq!(best_soar(&env2, &Caps::soarer()), Some(FlightMode::RidgeSoaring));
        let env3 = Env { wind_gradient: true, wind_w: 0.9, ..Default::default() };
        assert_eq!(best_soar(&env3, &Caps::soarer()), Some(FlightMode::DynamicSoaring));
    }

    #[test]
    fn small_bird_bounds_when_conserving_without_lift() {
        let env = Env::default();
        // small bird can't undulate-soar; with no lift + conserving it undulates
        // first (cheaper), but a pure bounder (no undulate) bounds.
        let bound_only = Caps { can_undulate: false, ..Caps::small_bird() };
        assert_eq!(select_mode(&env, &bound_only, 0.9, false), FlightMode::Bounding);
    }

    #[test]
    fn low_energy_forces_conserving_even_when_covering_distance() {
        let env = Env { thermal: true, wind_w: 0.5, ..Default::default() };
        // wants distance but nearly exhausted → soars to recover
        assert_eq!(select_mode(&env, &Caps::soarer(), 0.2, true), FlightMode::ThermalSoaring);
    }

    #[test]
    fn power_factors_encode_the_savings() {
        assert!(FlightMode::Undulating.power_factor() < FlightMode::Bounding.power_factor());
        assert!(FlightMode::Bounding.power_factor() < FlightMode::PoweredFlapping.power_factor());
        assert!(FlightMode::ThermalSoaring.is_soaring());
        assert_eq!(FlightMode::Stooping.power_factor(), 0.0);
    }
}
