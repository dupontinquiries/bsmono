//! `sim::ecology_population` — trophic population dynamics per region.
//!
//! WID r78-04-ecology-population. Makes fauna populations *live*: a bounded
//! grass → herbivore → carnivore model so spawn rates emerge from local
//! ecology instead of being constant. This is the continuous-field companion to
//! the spatial agent sim in [`crate::sim::forage_sim`] (`ForageWorld`/`Grazer`
//! prove the foraging *loop* keeps individuals alive; this proves the
//! *aggregate* populations stay bounded and settle). Concepts are reused —
//! grass as the renewable base, herbivores grazing it, per-region carrying
//! capacity — but the math is a clamped Lotka–Volterra-ish update rather than
//! agent stepping. Spawn pressure derived here feeds the world spawner that
//! places `fauna_behavior` creatures.
//!
//! Spec: `physical_mob_engine.docx` (trophic/ecology section); composes the
//! concepts in `sim::fauna_behavior` (the creatures these populations spawn).
//!
//! # Model
//!
//! State [`Trophic`] holds three levels, each a population in *its own units*:
//! `grass01 ∈ [0,1]` (fraction of max ground cover), `herbivores ∈ [0,k_herb]`,
//! `carnivores ∈ [0,k_carn]`. One [`step`] advances `dt` seconds:
//!
//! ```text
//! grass'      += regrow * grass * (1 - grass)            (logistic regrowth)
//!             -= graze  * grass * herb_frac              (grazing pressure)
//! herbivores' += graze  * grass * herbivores * (1 - herb/k_herb)   (logistic, food-gated)
//!             -= predation * herbivores * carn_frac      (predation loss)
//! carnivores' += predation * herb_frac * carnivores * (1 - carn/k_carn) (logistic, prey-gated)
//!             -= starve * carnivores * (1 - herb_avail)  (starve when prey scarce)
//! ```
//!
//! Every level is **clamped to `[0, K]` after each step** — the model can never
//! blow up or go negative regardless of parameters or `dt`. `herb_frac` /
//! `carn_frac` are the levels normalised by their K, keeping the coupling terms
//! dimensionless and bounded.
//!
//! # Invariants (pinned by tests)
//!
//! - All populations finite, `≥ 0`, `≤ K` after any number of steps.
//! - Deterministic (no RNG, no clock).
//! - No predators → herbivores rise toward `k_herb` then plateau (logistic).
//! - Predators decline when herbivores are scarce.
//! - Mid params settle into a bounded band (a stable region), never NaN or
//!   diverging.
//! - `spawn_pressure` outputs in `[0,1]`.
//! - Desert `k_*` < rainforest `k_*`.

// ── Parameters ────────────────────────────────────────────────────────────────

/// Per-region ecology parameters. Carrying capacities `k_*` cap each level; the
/// rate terms govern the coupling. All are expected `≥ 0`; [`step`] sanitises
/// defensively regardless.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EcoParams {
    /// Max ground cover (always 1.0 in `grass01` units; kept for symmetry/clarity).
    pub k_grass: f32,
    /// Herbivore carrying capacity (population units).
    pub k_herb: f32,
    /// Carnivore carrying capacity (population units).
    pub k_carn: f32,
    /// Logistic grass regrowth rate.
    pub regrow: f32,
    /// Grazing rate (grass consumed per herbivore-fraction).
    pub graze: f32,
    /// Predation rate (herbivores taken per carnivore-fraction).
    pub predation: f32,
    /// Carnivore starvation rate when prey is scarce.
    pub starve: f32,
}

/// Carrying capacities and rates for a biome id.
///
/// Biome ids follow the engine's convention; the mapping here is by climate
/// productivity. Deserts get low carrying capacity, rainforests high, with
/// temperate/savanna in between. Unknown ids fall back to a temperate default.
///
/// | biome | example | k_herb | k_carn |
/// |---|---|---|---|
/// | 0 | desert / barren | low | low |
/// | 1 | tundra | low | low |
/// | 2 | savanna / grassland | high herb | mid carn |
/// | 3 | temperate forest | mid | mid |
/// | 4 | rainforest | highest | highest |
/// | _ | default temperate | mid | mid |
pub fn params_for_biome(biome: u8) -> EcoParams {
    // regrow/graze/predation/starve are climate-tuned but kept in a stable band.
    match biome {
        0 => EcoParams { // desert / barren — sparse, slow
            k_grass: 1.0, k_herb: 8.0, k_carn: 1.5,
            regrow: 0.10, graze: 0.30, predation: 0.25, starve: 0.20,
        },
        1 => EcoParams { // tundra — low, slow
            k_grass: 1.0, k_herb: 10.0, k_carn: 2.0,
            regrow: 0.12, graze: 0.30, predation: 0.25, starve: 0.20,
        },
        2 => EcoParams { // savanna / grassland — herbivore-rich
            k_grass: 1.0, k_herb: 60.0, k_carn: 8.0,
            regrow: 0.35, graze: 0.40, predation: 0.30, starve: 0.18,
        },
        3 => EcoParams { // temperate forest — balanced
            k_grass: 1.0, k_herb: 40.0, k_carn: 7.0,
            regrow: 0.30, graze: 0.35, predation: 0.30, starve: 0.18,
        },
        4 => EcoParams { // rainforest — richest
            k_grass: 1.0, k_herb: 90.0, k_carn: 14.0,
            regrow: 0.45, graze: 0.40, predation: 0.32, starve: 0.16,
        },
        _ => EcoParams { // default temperate
            k_grass: 1.0, k_herb: 40.0, k_carn: 7.0,
            regrow: 0.30, graze: 0.35, predation: 0.30, starve: 0.18,
        },
    }
}

// ── State ─────────────────────────────────────────────────────────────────────

/// The three trophic levels for one region.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Trophic {
    /// Ground cover fraction in `[0, 1]`.
    pub grass01: f32,
    /// Herbivore population in `[0, k_herb]`.
    pub herbivores: f32,
    /// Carnivore population in `[0, k_carn]`.
    pub carnivores: f32,
}

impl Trophic {
    /// A reasonable mid-density starting state for `p`.
    pub fn seeded(p: &EcoParams) -> Trophic {
        Trophic {
            grass01: 0.6,
            herbivores: 0.4 * p.k_herb.max(0.0),
            carnivores: 0.3 * p.k_carn.max(0.0),
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

#[inline]
fn fin(v: f32) -> f32 {
    if v.is_finite() { v } else { 0.0 }
}

#[inline]
fn clamp(v: f32, lo: f32, hi: f32) -> f32 {
    let v = fin(v);
    let hi = if hi < lo { lo } else { hi };
    if v < lo { lo } else if v > hi { hi } else { v }
}

// ── Step ──────────────────────────────────────────────────────────────────────

/// Advance the trophic state by `dt` seconds under `p`. Returns the new state
/// with every level clamped to `[0, K]`.
///
/// The update is an explicit Euler step of a logistic predator/prey system, but
/// **every output level is hard-clamped to its carrying capacity**, so the
/// system is unconditionally bounded — large `dt` or extreme params can make it
/// crude, never divergent or NaN.
pub fn step(t: Trophic, p: &EcoParams, dt: f32) -> Trophic {
    let dt = clamp(dt, 0.0, 10.0); // guard against absurd/negative dt

    let k_grass = clamp(p.k_grass, 1e-6, f32::MAX).min(1.0).max(1e-6);
    let k_herb = clamp(p.k_herb, 0.0, f32::MAX);
    let k_carn = clamp(p.k_carn, 0.0, f32::MAX);

    let regrow = clamp(p.regrow, 0.0, f32::MAX);
    let graze = clamp(p.graze, 0.0, f32::MAX);
    let predation = clamp(p.predation, 0.0, f32::MAX);
    let starve = clamp(p.starve, 0.0, f32::MAX);

    // Current levels, sanitised into range.
    let grass = clamp(t.grass01, 0.0, 1.0);
    let herb = clamp(t.herbivores, 0.0, k_herb);
    let carn = clamp(t.carnivores, 0.0, k_carn);

    // Dimensionless fractions for coupling (0 when K==0).
    let herb_frac = if k_herb > 0.0 { herb / k_herb } else { 0.0 };
    let carn_frac = if k_carn > 0.0 { carn / k_carn } else { 0.0 };

    // ── Grass: logistic regrowth toward cover=1, minus grazing pressure. ──
    let grass_regrow = regrow * grass * (1.0 - grass / k_grass);
    let grass_grazed = graze * grass * herb_frac;
    let mut grass_next = grass + dt * (grass_regrow - grass_grazed);
    grass_next = clamp(grass_next, 0.0, 1.0);

    // ── Herbivores: logistic growth gated by available grass, minus predation. ──
    let herb_growth = graze * grass * herb * (1.0 - herb_frac);
    let herb_predated = predation * herb * carn_frac;
    let mut herb_next = herb + dt * (herb_growth - herb_predated);
    herb_next = clamp(herb_next, 0.0, k_herb);

    // ── Carnivores: logistic growth gated by available prey, minus starvation. ──
    // "prey availability" = herb_frac; carnivores starve as it drops toward 0.
    let carn_growth = predation * herb_frac * carn * (1.0 - carn_frac);
    let carn_starved = starve * carn * (1.0 - herb_frac);
    let mut carn_next = carn + dt * (carn_growth - carn_starved);
    carn_next = clamp(carn_next, 0.0, k_carn);

    Trophic {
        grass01: grass_next,
        herbivores: herb_next,
        carnivores: carn_next,
    }
}

// ── Spawn pressure ──────────────────────────────────────────────────────────────

/// Map the current trophic state to spawn pressures in `[0, 1]` for
/// `(herbivores, carnivores)`. The world spawner multiplies these by its base
/// rates: a region near herbivore capacity wants more herbivore spawns, and
/// likewise for carnivores. Pressure rises with how full each level is relative
/// to a healthy reference, saturating at 1.0.
///
/// Both outputs are always finite and in `[0, 1]`.
pub fn spawn_pressure(t: &Trophic) -> (f32, f32) {
    // We don't know K here, so express pressure off the levels' own structure:
    // herbivore pressure rises with grass abundance AND existing herd presence
    // (a healthy, grassy region should keep spawning grazers); carnivore
    // pressure rises with herbivore abundance (prey present → predators viable).
    //
    // Normalise herbivores/carnivores by a soft reference so the result is in
    // [0,1] without needing K: x/(1+x) saturates any non-negative magnitude.
    let grass = clamp(t.grass01, 0.0, 1.0);
    let herb = clamp(t.herbivores, 0.0, f32::MAX);
    let carn = clamp(t.carnivores, 0.0, f32::MAX);

    // Herbivore spawn pressure: grass drives it; presence of some herd sustains it.
    let herb_presence = herb / (1.0 + herb); // [0,1)
    let herb_pressure = clamp(0.5 * grass + 0.5 * herb_presence, 0.0, 1.0);

    // Carnivore spawn pressure: scales with prey availability.
    let prey_presence = herb / (4.0 + herb); // needs more prey before saturating
    let carn_presence = carn / (1.0 + carn);
    let carn_pressure = clamp(0.7 * prey_presence + 0.3 * carn_presence, 0.0, 1.0);

    (herb_pressure, carn_pressure)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn mid() -> EcoParams {
        params_for_biome(3) // temperate
    }

    fn finite_in_range(t: &Trophic, p: &EcoParams) -> bool {
        t.grass01.is_finite() && t.grass01 >= 0.0 && t.grass01 <= 1.0 + 1e-4
            && t.herbivores.is_finite() && t.herbivores >= 0.0 && t.herbivores <= p.k_herb + 1e-3
            && t.carnivores.is_finite() && t.carnivores >= 0.0 && t.carnivores <= p.k_carn + 1e-3
    }

    // ── Boundedness: never blows up, never goes negative or NaN ──

    #[test]
    fn stays_bounded_over_long_run() {
        let p = mid();
        let mut t = Trophic::seeded(&p);
        for _ in 0..100_000 {
            t = step(t, &p, 0.1);
            assert!(finite_in_range(&t, &p), "out of range: {:?}", t);
        }
    }

    #[test]
    fn bounded_under_extreme_params_and_dt() {
        // Cranked rates + huge dt must still clamp, not diverge.
        let p = EcoParams {
            k_grass: 1.0, k_herb: 50.0, k_carn: 9.0,
            regrow: 10.0, graze: 10.0, predation: 10.0, starve: 10.0,
        };
        let mut t = Trophic { grass01: 1.0, herbivores: 50.0, carnivores: 9.0 };
        for _ in 0..10_000 {
            t = step(t, &p, 5.0);
            assert!(finite_in_range(&t, &p), "blew up: {:?}", t);
        }
    }

    #[test]
    fn negative_and_nan_inputs_sanitised() {
        let p = mid();
        let t = Trophic { grass01: -5.0, herbivores: -10.0, carnivores: f32::NAN };
        let out = step(t, &p, 0.1);
        assert!(finite_in_range(&out, &p));
        assert!(out.grass01 >= 0.0 && out.herbivores >= 0.0 && out.carnivores >= 0.0);
    }

    #[test]
    fn over_capacity_inputs_clamped_down() {
        let p = mid();
        let t = Trophic { grass01: 99.0, herbivores: 9999.0, carnivores: 9999.0 };
        let out = step(t, &p, 0.1);
        assert!(out.grass01 <= 1.0 + 1e-4);
        assert!(out.herbivores <= p.k_herb + 1e-3);
        assert!(out.carnivores <= p.k_carn + 1e-3);
    }

    // ── Logistic herbivores with no predators ──

    #[test]
    fn no_predators_herbivores_rise_to_k_then_plateau() {
        let p = mid();
        // Start with grass, some herbivores, ZERO carnivores.
        let mut t = Trophic { grass01: 1.0, herbivores: 1.0, carnivores: 0.0 };
        let mut last = t.herbivores;
        let mut rose = false;
        for _ in 0..20_000 {
            t = step(t, &p, 0.1);
            assert_eq!(t.carnivores, 0.0, "no carnivores can appear from nothing");
            if t.herbivores > last + 1e-6 { rose = true; }
            last = t.herbivores;
        }
        assert!(rose, "herbivores should have risen at some point");
        // Plateau near k_herb (logistic ceiling), not above.
        assert!(t.herbivores > 0.5 * p.k_herb,
            "herbivores should approach K, got {}", t.herbivores);
        assert!(t.herbivores <= p.k_herb + 1e-3);
        // And it should be ~settled: one more step barely moves it.
        let after = step(t, &p, 0.1);
        assert!((after.herbivores - t.herbivores).abs() < 0.5,
            "herbivores should have plateaued: {} -> {}", t.herbivores, after.herbivores);
    }

    #[test]
    fn zero_carnivores_stay_zero() {
        let p = mid();
        let mut t = Trophic { grass01: 1.0, herbivores: 30.0, carnivores: 0.0 };
        for _ in 0..1000 {
            t = step(t, &p, 0.1);
            assert_eq!(t.carnivores, 0.0);
        }
    }

    // ── Predators decline when prey scarce ──

    #[test]
    fn predators_decline_when_herbivores_scarce() {
        let p = mid();
        // Lots of carnivores, almost no herbivores → carnivores must fall.
        let mut t = Trophic { grass01: 0.2, herbivores: 0.5, carnivores: p.k_carn };
        let start = t.carnivores;
        for _ in 0..200 {
            t = step(t, &p, 0.1);
        }
        assert!(t.carnivores < start,
            "carnivores should decline with scarce prey: {} -> {}", start, t.carnivores);
    }

    #[test]
    fn predators_starve_toward_zero_with_no_prey() {
        let p = mid();
        let mut t = Trophic { grass01: 0.0, herbivores: 0.0, carnivores: p.k_carn };
        for _ in 0..5000 {
            t = step(t, &p, 0.1);
        }
        assert!(t.carnivores < 0.05 * p.k_carn,
            "with no prey carnivores should crash toward 0, got {}", t.carnivores);
    }

    // ── Stable fixed point for mid params ──

    #[test]
    fn mid_params_settle_into_a_band() {
        let p = mid();
        let mut t = Trophic::seeded(&p);
        // Burn in.
        for _ in 0..50_000 {
            t = step(t, &p, 0.1);
        }
        // Now sample a window and assert it stays in a band (not diverging,
        // not NaN, not oscillating to the rails).
        let mut min_h = f32::INFINITY;
        let mut max_h = f32::NEG_INFINITY;
        let mut min_c = f32::INFINITY;
        let mut max_c = f32::NEG_INFINITY;
        for _ in 0..20_000 {
            t = step(t, &p, 0.1);
            assert!(finite_in_range(&t, &p));
            min_h = min_h.min(t.herbivores);
            max_h = max_h.max(t.herbivores);
            min_c = min_c.min(t.carnivores);
            max_c = max_c.max(t.carnivores);
        }
        // The band should be narrow relative to K (settled, not full-swing).
        let herb_band = (max_h - min_h) / p.k_herb;
        let carn_band = (max_c - min_c) / p.k_carn;
        assert!(herb_band < 0.5, "herbivore band too wide ({:.3} of K) — not settling", herb_band);
        assert!(carn_band < 0.5, "carnivore band too wide ({:.3} of K) — not settling", carn_band);
        // And a non-trivial population persists (ecosystem didn't collapse).
        assert!(max_h > 0.05 * p.k_herb, "herbivores collapsed");
    }

    // ── Determinism ──

    #[test]
    fn deterministic() {
        let p = mid();
        let run = || {
            let mut t = Trophic::seeded(&p);
            for _ in 0..1000 { t = step(t, &p, 0.1); }
            t
        };
        let a = run();
        let b = run();
        assert_eq!(a.grass01.to_bits(), b.grass01.to_bits());
        assert_eq!(a.herbivores.to_bits(), b.herbivores.to_bits());
        assert_eq!(a.carnivores.to_bits(), b.carnivores.to_bits());
    }

    // ── spawn_pressure ──

    #[test]
    fn spawn_pressure_in_range() {
        // Sweep a grid of states; both pressures must stay in [0,1].
        let mut seed = 0x5EEDu32;
        let mut lcg = || { seed = seed.wrapping_mul(1664525).wrapping_add(1013904223); seed >> 8 };
        for _ in 0..5000 {
            let t = Trophic {
                grass01: (lcg() % 1000) as f32 / 1000.0,
                herbivores: (lcg() % 2000) as f32 / 10.0,
                carnivores: (lcg() % 500) as f32 / 10.0,
            };
            let (h, c) = spawn_pressure(&t);
            assert!(h >= 0.0 && h <= 1.0, "herb pressure {} out of [0,1]", h);
            assert!(c >= 0.0 && c <= 1.0, "carn pressure {} out of [0,1]", c);
            assert!(h.is_finite() && c.is_finite());
        }
    }

    #[test]
    fn spawn_pressure_rises_with_abundance() {
        let lean = Trophic { grass01: 0.1, herbivores: 1.0, carnivores: 0.5 };
        let rich = Trophic { grass01: 0.9, herbivores: 40.0, carnivores: 6.0 };
        let (hl, cl) = spawn_pressure(&lean);
        let (hr, cr) = spawn_pressure(&rich);
        assert!(hr > hl, "herb pressure should rise with abundance: {} vs {}", hr, hl);
        assert!(cr > cl, "carn pressure should rise with prey: {} vs {}", cr, cl);
    }

    #[test]
    fn spawn_pressure_handles_degenerate_state() {
        let (h, c) = spawn_pressure(&Trophic { grass01: f32::NAN, herbivores: -5.0, carnivores: f32::INFINITY });
        assert!(h >= 0.0 && h <= 1.0 && h.is_finite());
        assert!(c >= 0.0 && c <= 1.0 && c.is_finite());
    }

    // ── Biome carrying capacity ordering ──

    #[test]
    fn desert_k_less_than_rainforest() {
        let desert = params_for_biome(0);
        let rainforest = params_for_biome(4);
        assert!(desert.k_herb < rainforest.k_herb,
            "desert k_herb {} should be < rainforest {}", desert.k_herb, rainforest.k_herb);
        assert!(desert.k_carn < rainforest.k_carn,
            "desert k_carn {} should be < rainforest {}", desert.k_carn, rainforest.k_carn);
    }

    #[test]
    fn biome_params_all_nonneg_and_finite() {
        for b in 0u8..=10 {
            let p = params_for_biome(b);
            for v in [p.k_grass, p.k_herb, p.k_carn, p.regrow, p.graze, p.predation, p.starve] {
                assert!(v.is_finite() && v >= 0.0, "biome {} param {} bad", b, v);
            }
        }
    }

    #[test]
    fn unknown_biome_falls_back_to_temperate() {
        assert_eq!(params_for_biome(200), params_for_biome(3));
    }

    // ── Richer biome sustains more life ──

    #[test]
    fn rainforest_sustains_more_herbivores_than_desert() {
        let settle = |biome: u8| -> f32 {
            let p = params_for_biome(biome);
            let mut t = Trophic::seeded(&p);
            for _ in 0..30_000 { t = step(t, &p, 0.1); }
            t.herbivores
        };
        let desert = settle(0);
        let rainforest = settle(4);
        assert!(rainforest > desert,
            "rainforest should sustain more herbivores: {} vs {}", rainforest, desert);
    }
}
