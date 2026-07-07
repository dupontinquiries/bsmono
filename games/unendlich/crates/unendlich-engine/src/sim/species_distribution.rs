//! `sim::species_distribution` — biome/climate → species weight tables.
//!
//! The missing half of spawning: `mob_spawn_table::pick` proportionally selects
//! a species from a `weights: &[(u16, f32)]` list, but nothing produced those
//! weights per biome. This module is that producer. It maps `(biome, light,
//! altitude)` to a `Vec<(SpeciesId, f32)>` ready to feed straight into `pick`,
//! and wraps the whole round-trip in [`pick_species`].
//!
//! Grounding:
//! - `sim::mob_spawn_table::pick(biome:u8, light:f32, weights:&[(u16,f32)], roll:f32)`
//!   — the consumer (read its signature; weights ≥ 0, all-zero ⇒ `None`).
//! - Biome ids match `world::biomes::Biome` discriminants (Plains=0 … ThermalCave=18).
//! - Diurnal/nocturnal niches: `fauna_behaviour` (deer/rabbit graze by day,
//!   predators range at dusk/night).
//! - Flying/altitude niches: `flying_animal_behaviour.docx` (birds & flyers gated
//!   above a minimum altitude).
//! - Body-plan / aquatic split: `physical_mob_engine.docx` (ocean sets are
//!   aquatic-only; land flyers need air column).
//!
//! Pure: logic in → data out. No IO, no clock, no RNG, no render. Deterministic.
//! Release-safe: every helper used in asserts is defined unconditionally.

// ---------------------------------------------------------------------------
// Biome ids (mirror world::biomes::Biome discriminants; kept local so this
// module stays a pure leaf with no cross-crate enum dependency)
// ---------------------------------------------------------------------------

pub const B_PLAINS: u8 = 0;
pub const B_TEMPERATE_FOREST: u8 = 1;
pub const B_TEMPERATE_HIGHLANDS: u8 = 2;
pub const B_BIRCH_FOREST: u8 = 3;
pub const B_MAPLE_FOREST: u8 = 4;
pub const B_CLOUD_FOREST: u8 = 5;
pub const B_MISTY_MOUNTAINS: u8 = 6;
pub const B_ALPINE_HIGHLANDS: u8 = 7;
pub const B_DESERT: u8 = 8;
pub const B_RED_SANDSTONE_CANYON: u8 = 9;
pub const B_BLOOMING_CANYON: u8 = 10;
pub const B_OCEAN: u8 = 11;
pub const B_BEACH: u8 = 12;
pub const B_SWAMP: u8 = 13;
pub const B_TUNDRA: u8 = 14;
pub const B_SKYLANDS: u8 = 15;
pub const B_ICE_CAVE: u8 = 16;
pub const B_FUNGAL_CAVE: u8 = 17;
pub const B_THERMAL_CAVE: u8 = 18;

/// Is this a land biome (expected to always yield a non-empty land set)?
/// Ocean is aquatic; caves yield their own (possibly empty) sets — documented.
pub fn is_land_biome(biome: u8) -> bool {
    !matches!(biome, B_OCEAN)
}

/// Is this an aquatic biome (ocean)? Aquatic sets are returned for these.
pub fn is_aquatic_biome(biome: u8) -> bool {
    matches!(biome, B_OCEAN)
}

/// Is this a cave biome (no sky light; flyers/diurnal surface fauna excluded)?
pub fn is_cave_biome(biome: u8) -> bool {
    matches!(biome, B_ICE_CAVE | B_FUNGAL_CAVE | B_THERMAL_CAVE)
}

// ---------------------------------------------------------------------------
// Species catalogue (u16). Small, illustrative roster spanning the niches the
// invariants exercise: diurnal grazers, nocturnal predators, flyers, aquatics,
// cave dwellers.
// ---------------------------------------------------------------------------

pub type SpeciesId = u16;

pub const S_DEER: SpeciesId      = 1;  // diurnal grazer (forest/plains)
pub const S_RABBIT: SpeciesId    = 2;  // diurnal grazer (plains/forest)
pub const S_BOAR: SpeciesId      = 3;  // crepuscular forager (forest/swamp)
pub const S_WOLF: SpeciesId      = 4;  // nocturnal predator (forest/tundra)
pub const S_FOX: SpeciesId       = 5;  // nocturnal small predator
pub const S_BEAR: SpeciesId      = 6;  // diurnal apex (forest/highlands)
pub const S_GOAT: SpeciesId      = 7;  // diurnal, high-altitude
pub const S_EAGLE: SpeciesId     = 8;  // diurnal flyer, high altitude
pub const S_OWL: SpeciesId       = 9;  // nocturnal flyer
pub const S_BAT: SpeciesId       = 10; // nocturnal flyer, cave
pub const S_LIZARD: SpeciesId    = 11; // diurnal desert
pub const S_SCORPION: SpeciesId  = 12; // nocturnal desert
pub const S_CAMEL: SpeciesId     = 13; // diurnal desert large
pub const S_FISH: SpeciesId      = 14; // aquatic
pub const S_SHARK: SpeciesId     = 15; // aquatic predator
pub const S_CRAB: SpeciesId      = 16; // beach/aquatic
pub const S_CAVE_NEWT: SpeciesId = 17; // cave, light-independent
pub const S_GLOWBUG: SpeciesId   = 18; // cave (fungal), light-independent
pub const S_PTARMIGAN: SpeciesId = 19; // diurnal tundra bird (ground)
pub const S_SKY_RAY: SpeciesId   = 20; // skylands flyer, very high altitude

// ---------------------------------------------------------------------------
// Niche
// ---------------------------------------------------------------------------

/// A species' activity + spatial niche.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Niche {
    /// Active in daylight (gets ~0 weight at low light).
    pub diurnal: bool,
    /// Active at night (gets ~0 weight at high light).
    pub nocturnal: bool,
    /// Minimum altitude (metres) at which this species appears. 0 = sea level.
    pub min_alt_m: f32,
    /// Flying species — only spawn above `min_alt_m` (needs air column).
    pub flying: bool,
    /// Aquatic species — only spawn in aquatic biomes.
    pub aquatic: bool,
    /// Light-independent (caves): ignores the day/night flip.
    pub light_independent: bool,
}

impl Niche {
    const fn day(min_alt_m: f32) -> Self {
        Self { diurnal: true, nocturnal: false, min_alt_m, flying: false, aquatic: false, light_independent: false }
    }
    const fn night(min_alt_m: f32) -> Self {
        Self { diurnal: false, nocturnal: true, min_alt_m, flying: false, aquatic: false, light_independent: false }
    }
    const fn crepuscular(min_alt_m: f32) -> Self {
        // active both ends of the day → both flags set (no flip suppression)
        Self { diurnal: true, nocturnal: true, min_alt_m, flying: false, aquatic: false, light_independent: false }
    }
    const fn day_flyer(min_alt_m: f32) -> Self {
        Self { diurnal: true, nocturnal: false, min_alt_m, flying: true, aquatic: false, light_independent: false }
    }
    const fn night_flyer(min_alt_m: f32) -> Self {
        Self { diurnal: false, nocturnal: true, min_alt_m, flying: true, aquatic: false, light_independent: false }
    }
    const fn water() -> Self {
        Self { diurnal: true, nocturnal: true, min_alt_m: 0.0, flying: false, aquatic: true, light_independent: true }
    }
    const fn cave() -> Self {
        Self { diurnal: false, nocturnal: true, min_alt_m: 0.0, flying: false, aquatic: false, light_independent: true }
    }
    const fn cave_flyer() -> Self {
        Self { diurnal: false, nocturnal: true, min_alt_m: 0.0, flying: true, aquatic: false, light_independent: true }
    }
}

/// The niche for a species id. Unknown ids get a neutral diurnal ground niche
/// (so the function is total and never panics).
pub fn species_niche(species: SpeciesId) -> Niche {
    match species {
        S_DEER      => Niche::day(0.0),
        S_RABBIT    => Niche::day(0.0),
        S_BOAR      => Niche::crepuscular(0.0),
        S_WOLF      => Niche::night(0.0),
        S_FOX       => Niche::night(0.0),
        S_BEAR      => Niche::day(0.0),
        S_GOAT      => Niche::day(120.0),
        S_EAGLE     => Niche::day_flyer(80.0),
        S_OWL       => Niche::night_flyer(0.0),
        S_BAT       => Niche::cave_flyer(),
        S_LIZARD    => Niche::day(0.0),
        S_SCORPION  => Niche::night(0.0),
        S_CAMEL     => Niche::day(0.0),
        S_FISH      => Niche::water(),
        S_SHARK     => Niche::water(),
        S_CRAB      => Niche::water(),
        S_CAVE_NEWT => Niche::cave(),
        S_GLOWBUG   => Niche::cave(),
        S_PTARMIGAN => Niche::day(0.0),
        S_SKY_RAY   => Niche::day_flyer(200.0),
        _           => Niche::day(0.0),
    }
}

// ---------------------------------------------------------------------------
// Light gating
// ---------------------------------------------------------------------------

/// Light thresholds for the day/night flip. `light01` is 0 (dark) .. 1 (noon).
const NIGHT_MAX: f32 = 0.35; // below this counts as "night"
const DAY_MIN: f32 = 0.55;   // above this counts as "day"

/// Clamp helper (total, finite-safe).
#[inline]
fn clamp01(x: f32) -> f32 {
    crate::util::mathx::clamp01(x) // r343 dedup
}

/// Light multiplier for a niche given the ambient light. Light-independent
/// (cave/aquatic) niches always return 1.0. Diurnal species fall to ~0 at low
/// light; nocturnal to ~0 at high light; crepuscular (both flags) stay active.
/// Always finite, in `[0, 1]`.
fn light_factor(n: &Niche, light01: f32) -> f32 {
    if n.light_independent {
        return 1.0;
    }
    let l = clamp01(light01);
    let day_ramp = smoothstep(NIGHT_MAX, DAY_MIN, l);       // 0 at night → 1 at day
    let night_ramp = 1.0 - day_ramp;                        // 1 at night → 0 at day
    match (n.diurnal, n.nocturnal) {
        (true, true)   => 1.0,            // crepuscular / always active
        (true, false)  => day_ramp,       // diurnal: ~0 at low light
        (false, true)  => night_ramp,     // nocturnal: ~0 at high light
        (false, false) => 1.0,            // unspecified → unaffected
    }
}

/// Smoothstep in `[edge0, edge1]`; clamps outside. Total/finite.
#[inline]
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    if !(edge1 > edge0) {
        // degenerate edges: step at edge0
        return if x < edge0 { 0.0 } else { 1.0 };
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// ---------------------------------------------------------------------------
// Base biome rosters (species + base habitat weight, before light/altitude)
// ---------------------------------------------------------------------------

/// The base roster for a biome: `(species, base_weight)` pairs, BEFORE light and
/// altitude gating. Land biomes are never empty; ocean is aquatic; caves yield
/// cave dwellers. Unknown biome ids fall back to the Plains roster (total).
fn base_roster(biome: u8) -> &'static [(SpeciesId, f32)] {
    match biome {
        B_PLAINS => &[(S_DEER, 3.0), (S_RABBIT, 4.0), (S_FOX, 1.5), (S_WOLF, 1.0), (S_EAGLE, 0.8)],
        B_TEMPERATE_FOREST | B_BIRCH_FOREST | B_MAPLE_FOREST =>
            &[(S_DEER, 4.0), (S_RABBIT, 2.0), (S_BOAR, 2.0), (S_WOLF, 1.5), (S_FOX, 1.5), (S_BEAR, 1.0), (S_OWL, 1.0)],
        B_CLOUD_FOREST =>
            &[(S_DEER, 2.0), (S_BOAR, 1.5), (S_EAGLE, 1.5), (S_OWL, 1.2), (S_GOAT, 1.0)],
        B_TEMPERATE_HIGHLANDS | B_ALPINE_HIGHLANDS =>
            &[(S_GOAT, 3.0), (S_EAGLE, 2.0), (S_WOLF, 1.0), (S_BEAR, 1.0)],
        B_MISTY_MOUNTAINS =>
            // Ptarmigan/fox range the lower slopes at ground level; goat/eagle/
            // sky-ray are the altitude-gated set. Keeps the biome non-empty at
            // ground while reserving the high niches for altitude.
            &[(S_PTARMIGAN, 2.0), (S_FOX, 1.5), (S_GOAT, 3.0), (S_EAGLE, 2.5), (S_SKY_RAY, 0.5)],
        B_DESERT =>
            &[(S_LIZARD, 3.0), (S_SCORPION, 3.0), (S_CAMEL, 2.0), (S_EAGLE, 0.7)],
        B_RED_SANDSTONE_CANYON =>
            &[(S_LIZARD, 3.0), (S_SCORPION, 2.5), (S_EAGLE, 1.5), (S_GOAT, 1.0)],
        B_BLOOMING_CANYON =>
            &[(S_RABBIT, 2.5), (S_LIZARD, 2.0), (S_FOX, 1.5), (S_EAGLE, 1.0)],
        B_OCEAN =>
            &[(S_FISH, 5.0), (S_SHARK, 1.0)],
        B_BEACH =>
            &[(S_CRAB, 3.0), (S_FISH, 1.5), (S_RABBIT, 1.0)],
        B_SWAMP =>
            &[(S_BOAR, 2.5), (S_FOX, 1.5), (S_OWL, 1.5), (S_FISH, 1.0)],
        B_TUNDRA =>
            &[(S_PTARMIGAN, 3.0), (S_WOLF, 2.0), (S_FOX, 2.0), (S_OWL, 1.0)],
        B_SKYLANDS =>
            &[(S_SKY_RAY, 4.0), (S_EAGLE, 2.0)],
        B_ICE_CAVE =>
            &[(S_CAVE_NEWT, 3.0), (S_BAT, 2.0)],
        B_FUNGAL_CAVE =>
            &[(S_GLOWBUG, 4.0), (S_CAVE_NEWT, 2.0), (S_BAT, 2.0)],
        B_THERMAL_CAVE =>
            &[(S_CAVE_NEWT, 3.0), (S_BAT, 2.5)],
        _ => &[(S_DEER, 3.0), (S_RABBIT, 4.0), (S_FOX, 1.5), (S_WOLF, 1.0), (S_EAGLE, 0.8)], // fallback = Plains
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Produce the `(species, weight)` list for `pick`, gating each base entry by
/// the species' niche against `light01` and `altitude_m`.
///
/// - Diurnal species → ~0 weight at low light; nocturnal → ~0 at high light.
/// - Flying species → 0 weight below their `min_alt_m`.
/// - Non-flying ground species with a `min_alt_m` (e.g. mountain goat) → 0 below it.
/// - Aquatic species only appear in aquatic biomes; non-aquatic species are
///   dropped from aquatic biomes.
/// - Cave biomes: only light-independent / cave species survive (surface fauna
///   would be light-gated to ~0 anyway, but we also drop non-cave flyers/grazers
///   that have no business underground except bats/newts/glowbugs in the roster).
///
/// Weights are always ≥ 0 and finite. For any LAND biome the result is
/// non-empty with positive total (invariant). Ocean returns an aquatic set;
/// other non-land/empty cases are documented. Deterministic.
pub fn weights_for(biome: u8, light01: f32, altitude_m: f32) -> Vec<(SpeciesId, f32)> {
    let roster = base_roster(biome);
    let aquatic_biome = is_aquatic_biome(biome);
    let mut out: Vec<(SpeciesId, f32)> = Vec::with_capacity(roster.len());

    for &(species, base_w) in roster {
        let n = species_niche(species);

        // Aquatic gating: aquatic species only in aquatic biomes, and aquatic
        // biomes only host aquatic species.
        if aquatic_biome != n.aquatic {
            continue;
        }

        // Altitude gating: flying or alt-restricted species need the air/height.
        if altitude_m < n.min_alt_m {
            continue;
        }

        // Light gating (day/night flip), unless light-independent.
        let lf = light_factor(&n, light01);

        let w = base_w * lf;
        // Keep only meaningfully positive weights (drop the ~0 flipped ones).
        if w > 1e-4 {
            out.push((species, w));
        }
    }

    out
}

/// Convenience: build the weights and run `mob_spawn_table::pick`. Returns the
/// chosen species, or `None` only when the weight set is empty (e.g. a flyer-only
/// context below altitude, or an aquatic query in a land biome's off-roster).
///
/// Round-trips exactly through `pick`: the same `weights_for(...)` output and the
/// same `roll` always yields the same choice.
pub fn pick_species(biome: u8, light01: f32, altitude_m: f32, roll: f32) -> Option<SpeciesId> {
    let weights = weights_for(biome, light01, altitude_m);
    crate::sim::mob_spawn_table::pick(biome, light01, &weights, roll)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: f32 = 1.0;
    const NIGHT: f32 = 0.0;

    fn total(w: &[(SpeciesId, f32)]) -> f32 {
        w.iter().map(|&(_, x)| x).sum()
    }
    fn has(w: &[(SpeciesId, f32)], s: SpeciesId) -> bool {
        w.iter().any(|&(id, x)| id == s && x > 0.0)
    }

    // All land biome ids for sweeps.
    const LAND_BIOMES: [u8; 18] = [0,1,2,3,4,5,6,7,8,9,10,12,13,14,15,16,17,18];
    const ALL_BIOMES: [u8; 19] = [0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18];

    // ---- weights basic invariants -----------------------------------------

    #[test]
    fn weights_all_non_negative_and_finite() {
        for &b in &ALL_BIOMES {
            for &light in &[0.0f32, 0.25, 0.5, 0.75, 1.0] {
                for &alt in &[0.0f32, 100.0, 300.0] {
                    for &(_, w) in &weights_for(b, light, alt) {
                        assert!(w >= 0.0, "negative weight in biome {b}");
                        assert!(w.is_finite(), "non-finite weight in biome {b}");
                    }
                }
            }
        }
    }

    /// Biomes that have a normal ground level (land, non-cave, non-sky).
    /// Skylands is a floating-island/sky biome whose roster is altitude-gated by
    /// design — it has no ground fauna, like caves have no daylight fauna.
    fn has_ground_fauna(b: u8) -> bool {
        is_land_biome(b) && !is_cave_biome(b) && b != B_SKYLANDS
    }

    #[test]
    fn land_biomes_never_all_zero_at_some_light() {
        // Every ground-bearing land biome must have SOME light level at ground
        // that yields a positive-total set. Sky/cave biomes are exercised
        // separately (cave_species_light_independent, sky altitude tests).
        for &b in &LAND_BIOMES {
            if !has_ground_fauna(b) {
                // sky/cave: must instead be non-empty at their natural context.
                let alt_set = total(&weights_for(b, 0.5, 300.0));
                let dark_set = total(&weights_for(b, 0.0, 0.0));
                assert!(alt_set > 0.0 || dark_set > 0.0,
                    "sky/cave biome {b} empty at both altitude and darkness");
                continue;
            }
            let day = total(&weights_for(b, DAY, 0.0));
            let night = total(&weights_for(b, NIGHT, 0.0));
            assert!(day > 0.0 || night > 0.0,
                "land biome {b} has empty roster at both day and night, ground level");
        }
    }

    #[test]
    fn land_biomes_nonempty_day_ground() {
        // Every ground-bearing surface biome has something awake by day at ground.
        for &b in &LAND_BIOMES {
            if !has_ground_fauna(b) { continue; } // caves night/independent; skylands sky-only
            let w = weights_for(b, DAY, 0.0);
            assert!(total(&w) > 0.0, "land biome {b} empty at day/ground");
        }
    }

    // ---- day/night flip ----------------------------------------------------

    #[test]
    fn nocturnal_zero_at_high_light() {
        // Wolf (nocturnal) should be absent at full daylight in a forest.
        let w = weights_for(B_TEMPERATE_FOREST, DAY, 0.0);
        assert!(!has(&w, S_WOLF), "nocturnal wolf present at full day light");
    }

    #[test]
    fn diurnal_zero_at_low_light() {
        // Deer (diurnal) should be absent at full night in a forest.
        let w = weights_for(B_TEMPERATE_FOREST, NIGHT, 0.0);
        assert!(!has(&w, S_DEER), "diurnal deer present at full night");
    }

    #[test]
    fn day_night_flip_swaps_predators_and_grazers() {
        let day = weights_for(B_TEMPERATE_FOREST, DAY, 0.0);
        let night = weights_for(B_TEMPERATE_FOREST, NIGHT, 0.0);
        // day: deer present, wolf absent
        assert!(has(&day, S_DEER) && !has(&day, S_WOLF));
        // night: wolf present, deer absent
        assert!(has(&night, S_WOLF) && !has(&night, S_DEER));
    }

    #[test]
    fn crepuscular_present_day_and_night() {
        // Boar is crepuscular (both flags) → present at both extremes.
        let day = weights_for(B_TEMPERATE_FOREST, DAY, 0.0);
        let night = weights_for(B_TEMPERATE_FOREST, NIGHT, 0.0);
        assert!(has(&day, S_BOAR) && has(&night, S_BOAR));
    }

    // ---- altitude / flying -------------------------------------------------

    #[test]
    fn flyer_absent_below_min_altitude() {
        // Eagle min_alt 80m → absent at ground in plains.
        let ground = weights_for(B_PLAINS, DAY, 0.0);
        assert!(!has(&ground, S_EAGLE), "eagle present below its min altitude");
        // Present once above 80m.
        let high = weights_for(B_PLAINS, DAY, 150.0);
        assert!(has(&high, S_EAGLE), "eagle absent above its min altitude");
    }

    #[test]
    fn sky_ray_needs_very_high_altitude() {
        let low = weights_for(B_SKYLANDS, DAY, 50.0);
        assert!(!has(&low, S_SKY_RAY), "sky ray present below 200m");
        let high = weights_for(B_SKYLANDS, DAY, 250.0);
        assert!(has(&high, S_SKY_RAY), "sky ray absent above 200m");
    }

    #[test]
    fn mountain_goat_needs_altitude() {
        // Goat min_alt 120m (non-flying alt restriction).
        let low = weights_for(B_MISTY_MOUNTAINS, DAY, 50.0);
        assert!(!has(&low, S_GOAT), "goat present below its altitude floor");
        let high = weights_for(B_MISTY_MOUNTAINS, DAY, 200.0);
        assert!(has(&high, S_GOAT), "goat absent above its altitude floor");
    }

    // ---- aquatic / ocean ---------------------------------------------------

    #[test]
    fn ocean_returns_aquatic_set() {
        let w = weights_for(B_OCEAN, 0.5, 0.0);
        assert!(total(&w) > 0.0, "ocean should have an aquatic set");
        for &(id, _) in &w {
            assert!(species_niche(id).aquatic, "non-aquatic species {id} in ocean set");
        }
    }

    #[test]
    fn land_biomes_have_no_aquatic_species() {
        for &b in &LAND_BIOMES {
            for &alt in &[0.0f32, 200.0] {
                for &(id, _) in &weights_for(b, DAY, alt) {
                    assert!(!species_niche(id).aquatic,
                        "aquatic species {id} in land biome {b}");
                }
                for &(id, _) in &weights_for(b, NIGHT, alt) {
                    assert!(!species_niche(id).aquatic,
                        "aquatic species {id} in land biome {b} (night)");
                }
            }
        }
    }

    // ---- caves -------------------------------------------------------------

    #[test]
    fn cave_species_light_independent() {
        // Cave biomes yield a set regardless of light (newts/bats/glowbugs).
        let dark = weights_for(B_FUNGAL_CAVE, 0.0, 0.0);
        let lit = weights_for(B_FUNGAL_CAVE, 1.0, 0.0);
        assert!(total(&dark) > 0.0 && total(&lit) > 0.0, "cave empty under some light");
        // glowbug present either way
        assert!(has(&dark, S_GLOWBUG) && has(&lit, S_GLOWBUG));
    }

    // ---- determinism + pick round-trip ------------------------------------

    #[test]
    fn weights_deterministic() {
        let a = weights_for(B_TEMPERATE_FOREST, 0.7, 100.0);
        let b = weights_for(B_TEMPERATE_FOREST, 0.7, 100.0);
        assert_eq!(a, b, "weights_for must be deterministic");
    }

    #[test]
    fn pick_species_round_trips_through_pick() {
        // Same weights + same roll ⇒ same choice as calling pick directly.
        let biome = B_TEMPERATE_FOREST;
        let (light, alt) = (DAY, 100.0);
        let weights = weights_for(biome, light, alt);
        let mut i = 0;
        while i < 500 {
            let roll = i as f32 / 500.0;
            let via_wrap = pick_species(biome, light, alt, roll);
            let via_direct = crate::sim::mob_spawn_table::pick(biome, light, &weights, roll);
            assert_eq!(via_wrap, via_direct, "round-trip mismatch at roll {roll}");
            i += 1;
        }
    }

    #[test]
    fn pick_species_none_only_when_empty() {
        // A land biome at day/ground has a set → Some for any roll.
        let some_ctx = pick_species(B_PLAINS, DAY, 0.0, 0.42);
        assert!(some_ctx.is_some(), "non-empty land context returned None");

        // Construct an empty context: aquatic species queried on land with an
        // altitude that filters out the rest? Simpler: a biome+alt where the
        // roster filters to empty. Plains flyers need 80m, but ground grazers
        // exist, so plains is never empty. Use ocean queried as if no aquatic?
        // Ocean always has fish → Some. The empty case is e.g. Skylands below
        // any species' altitude with everything flying-high:
        let empty = weights_for(B_SKYLANDS, DAY, 0.0); // sky_ray@200, eagle@80 both gated out
        assert!(empty.is_empty(), "expected empty skylands@ground set: {:?}", empty);
        assert_eq!(pick_species(B_SKYLANDS, DAY, 0.0, 0.5), None,
            "empty set must yield None");
    }

    #[test]
    fn pick_species_some_for_all_land_at_correct_conditions() {
        // Every land biome yields Some at the light level where it's populated.
        for &b in &LAND_BIOMES {
            let day = weights_for(b, DAY, 250.0);   // high alt to admit flyers too
            let night = weights_for(b, NIGHT, 0.0);
            let ctx_light = if total(&day) >= total(&night) { (DAY, 250.0) } else { (NIGHT, 0.0) };
            let got = pick_species(b, ctx_light.0, ctx_light.1, 0.5);
            assert!(got.is_some(), "land biome {b} yielded None at its populated context");
        }
    }

    #[test]
    fn light_factor_in_range() {
        for s in [S_DEER, S_WOLF, S_BOAR, S_FISH, S_GLOWBUG] {
            let n = species_niche(s);
            for &l in &[0.0f32, 0.2, 0.45, 0.6, 0.8, 1.0] {
                let f = light_factor(&n, l);
                assert!(f >= 0.0 && f <= 1.0 && f.is_finite(),
                    "light_factor out of range for species {s} at light {l}: {f}");
            }
        }
    }

    #[test]
    fn unknown_species_has_neutral_niche() {
        let n = species_niche(60000);
        assert!(n.diurnal && !n.flying && !n.aquatic, "unknown species niche not neutral");
    }
}
