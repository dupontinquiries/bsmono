//! `sim::bending_thermal_phase` — the Thermal Phase bending discipline.
//!
//! Per `resonance_bending_upgrade.docx`: a bender changes the *phase* of matter
//! by moving heat — freeze water → ice, boil → steam, soften stone. This module
//! is the pure physics: classify a material's phase, compute the signed heat to
//! drive it to a target phase (crossing latent-heat transitions), apply heat,
//! and price the technique for the resonance economy (r66-02 spends it). The bin
//! applies the resulting block/state changes. Pure, deterministic, finite,
//! panic-free (R1/R6/R11); standalone `rustc --test`-able, release-safe.

/// Phase of matter.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Solid,
    Liquid,
    Gas,
}

/// A material's thermal state. Temperatures in °C.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Material {
    pub freeze_c: f32,
    pub boil_c: f32,
    pub temp_c: f32,
}

/// Heat per °C (specific-heat proxy). Magnitudes are budget units, not real J.
const SPECIFIC_HEAT: f32 = 1.0;
/// Latent heat per phase boundary crossed (melt/freeze, boil/condense).
const LATENT: f32 = 100.0;
/// Sane temperature clamp: absolute zero up to a plasma-ish ceiling.
const TEMP_MIN: f32 = -273.15;
const TEMP_MAX: f32 = 1.0e6;

fn phase_index(p: Phase) -> i32 {
    match p {
        Phase::Solid => 0,
        Phase::Liquid => 1,
        Phase::Gas => 2,
    }
}

/// Classify a material's current phase from its temperature.
/// `temp < freeze` ⇒ Solid; `temp >= boil` ⇒ Gas; otherwise Liquid. The Solid
/// check is first, so it stays total even for a degenerate `freeze > boil`.
pub fn phase_of(m: &Material) -> Phase {
    if m.temp_c < m.freeze_c {
        Phase::Solid
    } else if m.temp_c >= m.boil_c {
        Phase::Gas
    } else {
        Phase::Liquid
    }
}

/// Signed heat to push `m` to `target` (negative = remove heat / cool). `0` when
/// `m` is already in `target`. Accounts for crossing transition points: the
/// sensible-heat ramp to the target band's edge plus `LATENT` per phase boundary
/// crossed, signed by the direction of the phase change.
pub fn heat_to_phase(m: &Material, target: Phase) -> f32 {
    let cur = phase_of(m);
    if cur == target {
        return 0.0;
    }
    // temperature edge that just enters the target band
    let edge = match target {
        Phase::Solid => m.freeze_c, // cool down to the freeze point
        Phase::Gas => m.boil_c,     // heat up to the boil point
        Phase::Liquid => {
            if phase_index(cur) < phase_index(Phase::Liquid) {
                m.freeze_c // coming up from Solid → melt
            } else {
                m.boil_c // coming down from Gas → condense
            }
        }
    };
    let base = SPECIFIC_HEAT * (edge - m.temp_c);
    let jumps = (phase_index(target) - phase_index(cur)).abs() as f32;
    // direction from the phase change itself, so the sign is right even when the
    // edge equals the current temperature (e.g. temp exactly at freeze_c).
    let dir = if phase_index(target) < phase_index(cur) { -1.0 } else { 1.0 };
    base + dir * jumps * LATENT
}

/// Move `m`'s temperature by `delta_c` (clamped to a sane range). Non-finite
/// deltas are ignored. Phase is not stored — it follows from the new temp via
/// [`phase_of`].
pub fn apply_heat(m: &mut Material, delta_c: f32) {
    if !delta_c.is_finite() {
        return;
    }
    let t = m.temp_c + delta_c;
    m.temp_c = t.max(TEMP_MIN).min(TEMP_MAX);
}

/// Resonance cost in `[0,1]` to perform the phase change, bigger for larger
/// jumps (0 ⇒ same phase, 0.5 ⇒ one boundary, 1.0 ⇒ solid↔gas). Feeds the
/// r66-02 resonance spend.
pub fn technique_cost01(m: &Material, target: Phase) -> f32 {
    let jumps = (phase_index(target) - phase_index(phase_of(m))).abs() as f32;
    (jumps / 2.0).max(0.0).min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn water(temp: f32) -> Material {
        Material { freeze_c: 0.0, boil_c: 100.0, temp_c: temp }
    }

    #[test]
    fn phase_of_matches_thresholds() {
        assert_eq!(phase_of(&water(-10.0)), Phase::Solid);
        assert_eq!(phase_of(&water(0.0)), Phase::Liquid, "at freeze ⇒ liquid");
        assert_eq!(phase_of(&water(50.0)), Phase::Liquid);
        assert_eq!(phase_of(&water(100.0)), Phase::Gas, "at boil ⇒ gas");
        assert_eq!(phase_of(&water(250.0)), Phase::Gas);
    }

    #[test]
    fn apply_heat_transitions_across_both_points() {
        let mut m = water(-20.0);
        assert_eq!(phase_of(&m), Phase::Solid);
        apply_heat(&mut m, 30.0); // -20 → 10
        assert_eq!(phase_of(&m), Phase::Liquid, "melted");
        apply_heat(&mut m, 120.0); // 10 → 130
        assert_eq!(phase_of(&m), Phase::Gas, "boiled");
        apply_heat(&mut m, -200.0); // 130 → -70
        assert_eq!(phase_of(&m), Phase::Solid, "refroze");
    }

    #[test]
    fn heat_to_phase_sign_and_zero() {
        // already in target ⇒ 0
        assert_eq!(heat_to_phase(&water(50.0), Phase::Liquid), 0.0);
        assert_eq!(heat_to_phase(&water(-5.0), Phase::Solid), 0.0);
        // cooling liquid → solid is negative
        assert!(heat_to_phase(&water(50.0), Phase::Solid) < 0.0);
        // heating liquid → gas is positive
        assert!(heat_to_phase(&water(50.0), Phase::Gas) > 0.0);
        // solid → gas (two jumps) positive and larger magnitude than solid → liquid
        let solid = water(-50.0);
        let to_liq = heat_to_phase(&solid, Phase::Liquid);
        let to_gas = heat_to_phase(&solid, Phase::Gas);
        assert!(to_liq > 0.0 && to_gas > to_liq, "bigger jump ⇒ more heat");
        // edge case: temp exactly at freeze, target Solid ⇒ must remove heat
        assert!(heat_to_phase(&water(0.0), Phase::Solid) < 0.0);
    }

    #[test]
    fn technique_cost_in_range_and_monotone_in_jump() {
        let liquid = water(50.0);
        let c0 = technique_cost01(&liquid, Phase::Liquid); // same phase
        let c1 = technique_cost01(&liquid, Phase::Solid); // one jump
        let c1b = technique_cost01(&liquid, Phase::Gas); // one jump
        let solid = water(-50.0);
        let c2 = technique_cost01(&solid, Phase::Gas); // two jumps
        assert_eq!(c0, 0.0);
        assert!(c1 > c0 && c1b > c0);
        assert!(c2 > c1, "two-jump cost should exceed one-jump");
        for c in [c0, c1, c1b, c2] {
            assert!((0.0..=1.0).contains(&c), "cost out of range: {}", c);
        }
    }

    #[test]
    fn apply_heat_clamps_and_is_nan_safe() {
        let mut hot = water(0.0);
        apply_heat(&mut hot, 1.0e30); // clamps to TEMP_MAX
        assert!(hot.temp_c.is_finite() && hot.temp_c <= TEMP_MAX);
        let mut cold = water(0.0);
        apply_heat(&mut cold, -1.0e30); // clamps to TEMP_MIN
        assert!(cold.temp_c.is_finite() && cold.temp_c >= TEMP_MIN);
        // NaN/inf delta ignored
        let mut m = water(20.0);
        apply_heat(&mut m, f32::NAN);
        apply_heat(&mut m, f32::INFINITY);
        assert_eq!(m.temp_c, 20.0);
    }

    #[test]
    fn deterministic_and_no_panic_over_random() {
        let mut s = 0x66FA_u64; // documented seed (R20)
        let mut rng = || {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
            (s >> 40) as f32 / ((1u64 << 24) as f32)
        };
        let phases = [Phase::Solid, Phase::Liquid, Phase::Gas];
        for _ in 0..4000 {
            let f = rng() * 100.0 - 20.0;
            let b = f + 1.0 + rng() * 300.0; // boil above freeze
            let m = Material { freeze_c: f, boil_c: b, temp_c: rng() * 500.0 - 100.0 };
            for &tgt in &phases {
                let h = heat_to_phase(&m, tgt);
                assert!(h.is_finite(), "heat non-finite");
                let c = technique_cost01(&m, tgt);
                assert!((0.0..=1.0).contains(&c));
                // sign sanity: nonzero heat must agree with the phase direction
                let cur = phase_of(&m);
                if cur != tgt {
                    let want_cool = phase_index(tgt) < phase_index(cur);
                    assert_eq!(h < 0.0, want_cool, "sign mismatch {:?}→{:?}: {}", cur, tgt, h);
                }
            }
            // determinism
            assert_eq!(heat_to_phase(&m, Phase::Gas), heat_to_phase(&m, Phase::Gas));
        }
    }
}
