//! `sim::bending_heavy_brine` — the Heavy Brine discipline (pure technique resolver).
//!
//! Per `docs/specs/resonance_bending_upgrade.docx` §"Heavy Brine · Mineral
//! Suspension Discipline": Heavy Brine bends mass-laden mineral fluids
//! (hydrothermal vent brine, saline deposits) that are "denser, slower, and
//! carry charge in ways that pure water cannot." It sinks, persists, coats
//! surfaces, and has high inertia — it can crush, anchor, and drag.
//!
//! This module owns ONLY the technique-effect decision: given brine state and a
//! technique + power level, it returns the effect parameters. There is NO fluid
//! sim here — the bin owns water/brine cells; this is logic in → data out.
//!
//! Pure: no RNG, IO, GPU, wall-clock; no `unwrap`/`expect`/`panic!`; degenerate
//! or out-of-range inputs are clamped and never produce NaN. Deterministic.

// ---------------------------------------------------------------------------
// Density model (cited: §Heavy Brine "significantly denser than seawater")
// ---------------------------------------------------------------------------

/// Density of fresh water, kg/m³ (reference baseline).
pub const DENSITY_FRESH: f32 = 1000.0;
/// Density of fully-saturated heavy brine, kg/m³ (mineral-laden, ~hydrothermal).
/// Chosen above seawater (~1025) to reflect the "significantly denser" spec note.
pub const DENSITY_BRINE_MAX: f32 = 1240.0;

/// Map salinity `[0,1]` to fluid density (kg/m³). Monotone increasing:
/// `salinity=0` → fresh water, `salinity=1` → saturated heavy brine.
/// Out-of-range salinity is clamped. Always finite.
pub fn density(salinity01: f32) -> f32 {
    let s = clamp01(salinity01);
    DENSITY_FRESH + (DENSITY_BRINE_MAX - DENSITY_FRESH) * s
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A parcel of brine the bender is acting on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrineState {
    /// Salinity / mineral load `[0,1]`: 0 = fresh water, 1 = saturated brine.
    pub salinity01: f32,
    /// Mass of the parcel in kilograms (≥ 0; negatives clamped to 0).
    pub mass_kg: f32,
}

/// The four Heavy Brine techniques (§"What Makes It Different From Water":
/// crush, anchor, drag — plus a defensive Wall of suspended mass).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrineTechnique {
    /// Compress mass onto a target — force scales with mass × density × power.
    Crush,
    /// Pin a target in place — raises `anchor01` (the brine's "it stays" property).
    Anchor,
    /// Slow a target moving through the medium — raises `slow01`.
    Drag,
    /// Raise a suspended-mass barrier — moderate force + moderate slow, low anchor.
    Wall,
}

/// The resolved effect parameters of a technique.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrineEffect {
    /// Crushing / pushing force in newtons-ish engine units (≥ 0, finite).
    pub force: f32,
    /// How much the target is slowed `[0,1]` (1 = fully halted).
    pub slow01: f32,
    /// How strongly the target is anchored in place `[0,1]`.
    pub anchor01: f32,
}

// ---------------------------------------------------------------------------
// Inertia (cited: §Heavy Brine "denser, slower ... higher-inertia")
// ---------------------------------------------------------------------------

/// Inertia of a brine parcel: `mass × density_factor`, where the density factor
/// is `density(salinity)/DENSITY_FRESH` (≥ 1). Higher salinity and higher mass
/// both raise inertia — the parcel is slower to start and stop. Always ≥ 0, finite.
pub fn inertia(state: &BrineState) -> f32 {
    let mass = state.mass_kg.max(0.0);
    let mass = if mass.is_finite() { mass } else { 0.0 };
    let density_factor = density(state.salinity01) / DENSITY_FRESH; // ≥ 1
    mass * density_factor
}

// ---------------------------------------------------------------------------
// Effect resolver
// ---------------------------------------------------------------------------

// Tuning constants (documented).
/// Newtons of crush force per (kg × density_factor × power) unit.
const CRUSH_FORCE_K: f32 = 9.81;
/// Force contribution of a Wall relative to Crush.
const WALL_FORCE_FRAC: f32 = 0.4;
/// Inertia (kg·factor) at which slow/anchor reach ~half their max (soft knee).
const INERTIA_HALF: f32 = 200.0;

/// Soft saturating curve mapping a non-negative magnitude to `[0,1)`:
/// `x / (x + half)`. Monotone increasing, 0 at x=0, → 1 as x → ∞.
#[inline]
fn saturate(x: f32, half: f32) -> f32 {
    let x = x.max(0.0);
    let half = half.max(1e-6);
    x / (x + half)
}

/// Clamp to `[0,1]`, mapping NaN to 0.
#[inline]
fn clamp01(x: f32) -> f32 {
    crate::util::mathx::clamp01(x) // r343 dedup
}

/// Resolve the [`BrineEffect`] for a technique at `power01 ∈ [0,1]`.
///
/// - **Crush**: force scales with `inertia × power` (mass × density × power);
///   negligible slow/anchor.
/// - **Anchor**: raises `anchor01` with inertia × power; low force, some slow.
/// - **Drag**: raises `slow01` with inertia × power; low force, little anchor.
/// - **Wall**: a balance — moderate force, moderate slow, low anchor.
///
/// All outputs finite, non-negative, `[0,1]` where applicable. Deterministic;
/// out-of-range inputs clamped; never panics.
pub fn effect(tech: BrineTechnique, state: &BrineState, power01: f32) -> BrineEffect {
    let power = clamp01(power01);
    let i = inertia(state);                 // ≥ 0
    let drive = i * power;                  // mass×density×power
    let crush_force = CRUSH_FORCE_K * drive;

    match tech {
        BrineTechnique::Crush => BrineEffect {
            force:    crush_force,
            slow01:   clamp01(0.15 * saturate(drive, INERTIA_HALF)),
            anchor01: clamp01(0.10 * saturate(drive, INERTIA_HALF)),
        },
        BrineTechnique::Anchor => BrineEffect {
            force:    0.2 * crush_force,
            slow01:   clamp01(0.4 * saturate(drive, INERTIA_HALF)),
            anchor01: clamp01(saturate(drive, INERTIA_HALF)),
        },
        BrineTechnique::Drag => BrineEffect {
            force:    0.15 * crush_force,
            slow01:   clamp01(saturate(drive, INERTIA_HALF)),
            anchor01: clamp01(0.2 * saturate(drive, INERTIA_HALF)),
        },
        BrineTechnique::Wall => BrineEffect {
            force:    WALL_FORCE_FRAC * crush_force,
            slow01:   clamp01(0.5 * saturate(drive, INERTIA_HALF)),
            anchor01: clamp01(0.25 * saturate(drive, INERTIA_HALF)),
        },
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn state(sal: f32, mass: f32) -> BrineState {
        BrineState { salinity01: sal, mass_kg: mass }
    }

    // ---- density -----------------------------------------------------------

    #[test]
    fn density_monotone_in_salinity() {
        let mut prev = -1.0f32;
        let mut s = 0.0f32;
        while s <= 1.0 {
            let d = density(s);
            assert!(d >= prev - 1e-4, "density not monotone at s={s}: {prev} -> {d}");
            prev = d;
            s += 0.05;
        }
    }

    #[test]
    fn density_endpoints() {
        assert!((density(0.0) - DENSITY_FRESH).abs() < 1e-3, "salinity 0 = fresh");
        assert!((density(1.0) - DENSITY_BRINE_MAX).abs() < 1e-3, "salinity 1 = max brine");
    }

    #[test]
    fn brine_denser_than_fresh() {
        assert!(density(0.5) > density(0.0), "brine should be denser than fresh water");
    }

    #[test]
    fn density_clamps_out_of_range() {
        assert!((density(-1.0) - DENSITY_FRESH).abs() < 1e-3);
        assert!((density(5.0) - DENSITY_BRINE_MAX).abs() < 1e-3);
    }

    // ---- inertia -----------------------------------------------------------

    #[test]
    fn higher_salinity_higher_inertia() {
        let low  = inertia(&state(0.1, 100.0));
        let high = inertia(&state(0.9, 100.0));
        assert!(high > low, "higher salinity → higher inertia: {high} !> {low}");
    }

    #[test]
    fn higher_mass_higher_inertia() {
        let light = inertia(&state(0.5, 50.0));
        let heavy = inertia(&state(0.5, 500.0));
        assert!(heavy > light, "higher mass → higher inertia");
    }

    #[test]
    fn inertia_non_negative_and_finite() {
        for &(s, m) in &[(0.0, 0.0), (0.5, 100.0), (1.0, 1e6), (-1.0, -5.0), (2.0, f32::NAN)] {
            let i = inertia(&state(s, m));
            assert!(i >= 0.0, "inertia negative for s={s} m={m}: {i}");
            assert!(i.is_finite(), "inertia non-finite for s={s} m={m}");
        }
    }

    // ---- effect: technique semantics --------------------------------------

    #[test]
    fn crush_force_scales_with_mass_and_power() {
        let s = state(0.8, 200.0);
        let low_p  = effect(BrineTechnique::Crush, &s, 0.2).force;
        let high_p = effect(BrineTechnique::Crush, &s, 0.9).force;
        assert!(high_p > low_p, "crush force should rise with power");

        let light = effect(BrineTechnique::Crush, &state(0.8, 50.0),  0.9).force;
        let heavy = effect(BrineTechnique::Crush, &state(0.8, 500.0), 0.9).force;
        assert!(heavy > light, "crush force should rise with mass");
    }

    #[test]
    fn anchor_raises_anchor01() {
        let s = state(0.7, 300.0);
        let anchor = effect(BrineTechnique::Anchor, &s, 0.9);
        let crush  = effect(BrineTechnique::Crush,  &s, 0.9);
        assert!(anchor.anchor01 > crush.anchor01,
            "Anchor should raise anchor01 above Crush: {} !> {}", anchor.anchor01, crush.anchor01);
    }

    #[test]
    fn drag_raises_slow01() {
        let s = state(0.7, 300.0);
        let drag  = effect(BrineTechnique::Drag,  &s, 0.9);
        let crush = effect(BrineTechnique::Crush, &s, 0.9);
        assert!(drag.slow01 > crush.slow01,
            "Drag should raise slow01 above Crush: {} !> {}", drag.slow01, crush.slow01);
    }

    #[test]
    fn all_effect_outputs_in_range() {
        let techs = [BrineTechnique::Crush, BrineTechnique::Anchor, BrineTechnique::Drag, BrineTechnique::Wall];
        let states = [state(0.0, 0.0), state(0.5, 200.0), state(1.0, 1e5), state(-1.0, -10.0)];
        for &t in &techs {
            for &st in &states {
                for &p in &[-1.0f32, 0.0, 0.5, 1.0, 2.0] {
                    let e = effect(t, &st, p);
                    assert!(e.force >= 0.0 && e.force.is_finite(), "force bad: {:?} {:?} p={p}", t, st);
                    assert!(e.slow01 >= 0.0 && e.slow01 <= 1.0, "slow01 out of range: {}", e.slow01);
                    assert!(e.anchor01 >= 0.0 && e.anchor01 <= 1.0, "anchor01 out of range: {}", e.anchor01);
                    assert!(e.slow01.is_finite() && e.anchor01.is_finite());
                }
            }
        }
    }

    #[test]
    fn zero_power_zero_force() {
        let e = effect(BrineTechnique::Crush, &state(1.0, 1000.0), 0.0);
        assert!(e.force.abs() < 1e-4, "zero power → zero force: {}", e.force);
    }

    #[test]
    fn zero_mass_zero_force() {
        let e = effect(BrineTechnique::Crush, &state(1.0, 0.0), 1.0);
        assert!(e.force.abs() < 1e-4, "zero mass → zero force: {}", e.force);
    }

    #[test]
    fn effect_deterministic() {
        let s = state(0.6, 250.0);
        let a = effect(BrineTechnique::Wall, &s, 0.7);
        let b = effect(BrineTechnique::Wall, &s, 0.7);
        assert_eq!(a, b, "effect must be deterministic");
    }

    #[test]
    fn nan_inputs_no_nan_output() {
        let e = effect(BrineTechnique::Crush, &state(f32::NAN, f32::NAN), f32::NAN);
        assert!(e.force.is_finite() && e.slow01.is_finite() && e.anchor01.is_finite(),
            "NaN inputs must not produce NaN outputs");
    }

    #[test]
    fn slow_and_anchor_monotone_in_power() {
        let s = state(0.8, 300.0);
        let lo = effect(BrineTechnique::Drag, &s, 0.2);
        let hi = effect(BrineTechnique::Drag, &s, 0.9);
        assert!(hi.slow01 >= lo.slow01, "slow01 should rise with power");
    }
}
