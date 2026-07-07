//! Temperature field — scalar atmospheric temperature in °C, the
//! third axis of the world's dynamic state alongside wind and
//! moisture.
//!
//! ## The triangle
//!
//! - **Wind** (vector field, in `wind.rs`): air motion.
//! - **Moisture** (scalar fields, in `cloud.rs` and `water_field.rs`):
//!   how much water is suspended in air or pooled in cells.
//! - **Temperature** (scalar field, this module): how hot or cold
//!   each cell is.
//!
//! Everything interesting in the world's atmosphere emerges from
//! the COUPLINGS between these three:
//!   - temperature → wind: warm air rises (vertical convection).
//!   - temperature × moisture: water above 100°C boils to vapor
//!     (joins moisture); below 0°C freezes to ice. Cool moist air
//!     condenses → clouds form. Cool moisture-saturated cloud →
//!     rain.
//!   - temperature → friction: cold + water = ice (slippery).
//!     Hot stone = lava-hot. Affects motion_factor.
//!   - wind × temperature: wind transports heat (advection).
//!     A warm front blowing into a cold valley raises local temp.
//!
//! All of these are emergent. We don't script "make it snow in cold
//! biomes" — we just say "cold air condenses moisture to ice" and
//! the snow appears wherever cold + moisture coexist.
//!
//! ## What's HERE
//!
//! Step 1: temperature SAMPLING. A pure function `temperature_at(wx,
//! wy, wz)` that returns the local equilibrium temperature in °C.
//! Driven by:
//!   - **Latitude proxy**: world_z modulates a base temperature
//!     (north is colder than south, but cyclic to avoid edges).
//!   - **Lapse rate**: temperature drops ~6.5°C per km of altitude
//!     (real-world rate). 1 block = 1m, so -6.5°C per 1000 blocks.
//!     At Earth's troposphere a 10000m mountain peak is ~65°C colder
//!     than sea level. We use the same.
//!   - **Local noise**: ±5°C per-position noise gives spatial
//!     variation (microclimates).
//!
//! NOT yet: heat sources (fires, lava), absorption (sun warming
//! exposed surfaces, shade cooling), advection (wind moves heat),
//! or persistence (a grid we mutate over time).
//!
//! ## Future
//!
//! - Full sparse 3D scalar grid like `water_field`.
//! - Tick handler that diffuses heat between cells.
//! - Heat sources: fire blocks emit heat (per-tick increment to
//!   nearby cells); player body emits a tiny amount; lava cells
//!   emit a lot.
//! - Sun driver: cells with sky exposure gain temperature during
//!   day (sky's day_phase), lose at night. Tracks diurnal cycle.

/// Lapse rate in °C per block of altitude. Real Earth lapse rate
/// is 6.5°C/km. Our 1 block = 1m so it's 6.5e-3 °C/block.
/// Capped to avoid pathological extremes at extreme altitudes.
pub const LAPSE_RATE_PER_BLOCK: f32 = 6.5e-3;

/// Sea-level baseline temperature in °C at the world's "equator"
/// (z = 0). Approximately Earth's tropical sea-level average.
pub const BASE_TEMPERATURE_C: f32 = 22.0;

/// Latitude span: how many blocks of Z corresponds to "pole to pole".
/// At z = 0 we're equatorial; at z = ± LATITUDE_SPAN/2 we're polar.
/// Using a large value so a typical play area stays in temperate
/// climate (no instant glacier 100 blocks north).
pub const LATITUDE_SPAN: f32 = 16384.0;

/// Temperature swing from equator to pole, in °C. ±25°C means
/// equatorial sea-level is BASE_TEMPERATURE_C, polar sea-level is
/// BASE_TEMPERATURE_C - 25 = -3°C (just below freezing).
pub const LATITUDE_SWING_C: f32 = 25.0;

/// Sample the local equilibrium temperature in °C at the given world
/// position. Pure function — no state.
///
/// Components:
///   - Latitude: cosine of (z / LATITUDE_SPAN) modulates a swing of
///     LATITUDE_SWING_C around BASE_TEMPERATURE_C. World z=0 is
///     warmest, ±LATITUDE_SPAN/2 is coldest.
///   - Altitude: lapse rate drops temperature with Y.
///   - Local noise: ±5°C of trig-based pseudo-noise for microclimate
///     variation (sunny side of mountain warmer, etc.)
///
/// Sea level (Y = 64) at "equator" (Z = 0) returns ~BASE_TEMPERATURE_C.
/// Mountain peak (Y = 200) at same latitude returns
/// ~BASE_TEMPERATURE_C - (200-64)*0.0065 ≈ 21°C. Polar sea level
/// (Z = 8000) returns ~ -3°C.
pub fn temperature_at(wx: i32, wy: i32, wz: i32) -> f32 {
    // Latitude factor in [0, 1]: 0 at equator, 1 at pole.
    // cos(z/span * TAU) is 1 at equator, -1 at pole/2; we want
    // 0 at equator and 1 at pole, so use (1 - cos)/2.
    let lat_phase = (wz as f32) / LATITUDE_SPAN * std::f32::consts::TAU;
    let polar_factor = (1.0 - lat_phase.cos()) * 0.5;
    let lat_temp = BASE_TEMPERATURE_C - polar_factor * LATITUDE_SWING_C;

    // Altitude factor: lapse rate drops temperature linearly with Y.
    // Reference: Y = 64 (sea level) is the lapse zero point.
    let altitude_drop = (wy as f32 - 64.0) * LAPSE_RATE_PER_BLOCK;

    // Local noise: ±2.5°C perturbations including Y-dependence so
    // adjacent altitudes have different microclimates (drives the
    // convection model — without Y noise the above-vs-here contrast
    // is purely lapse rate, which is constant and gives zero anomaly).
    let noise = ((wx as f32 * 0.07).sin()
               + (wz as f32 * 0.11).cos()
               + (wy as f32 * 0.05).sin() * 0.7) * 1.25;

    lat_temp - altitude_drop + noise
}

/// Like `temperature_at` but also adds the heat contribution from
/// dynamic heat sources (fires, lava, ice, etc.) registered in the
/// given `HeatSourceMap`.
///
/// Use this where a heat source map is available (e.g. in the bin's
/// per-frame loop). Tests and pure contexts use the base
/// `temperature_at` which doesn't take a map.
pub fn temperature_at_with_heat(
    wx: i32, wy: i32, wz: i32,
    heat_sources: &crate::heat_source::HeatSourceMap,
) -> f32 {
    temperature_at(wx, wy, wz)
        + heat_sources.total_heat_at(wx as f32, wy as f32, wz as f32)
}

/// Convenience: is this position cold enough for water to freeze?
/// (At or below 0°C.)
#[inline]
pub fn is_freezing_at(wx: i32, wy: i32, wz: i32) -> bool {
    temperature_at(wx, wy, wz) <= 0.0
}

/// Convenience: is this position hot enough for water to boil?
/// (At or above 100°C.)
#[inline]
pub fn is_boiling_at(wx: i32, wy: i32, wz: i32) -> bool {
    temperature_at(wx, wy, wz) >= 100.0
}

/// Vertical convection wind component in blocks/second at this
/// position. Warm spots (above local average) generate updrafts;
/// cold spots generate downdrafts. Coupled to temperature gradient,
/// not absolute temperature — what matters for convection is the
/// CONTRAST between the cell and its surroundings.
///
/// Cheap approximation: difference between this cell and the cell
/// 16 blocks above. If THIS is warmer, air rises here.
///
/// Returns blocks/second of vertical wind. Magnitude rarely exceeds
/// ±0.3 in normal terrain. Hot spots (lava, fire) would push higher.
pub fn convection_at(wx: i32, wy: i32, wz: i32) -> f32 {
    // Compare to a cell 16 blocks above. If this cell is significantly
    // warmer, that drives an updraft. Conversely, cool cells under
    // warm air sink.
    let here = temperature_at(wx, wy, wz);
    let above = temperature_at(wx, wy + 16, wz);
    // Without the lapse rate, "above is colder" is the natural state
    // (lapse rate makes it 16 * 0.0065 = 0.104°C colder). So we
    // subtract the expected lapse-driven gap to find ANOMALY.
    let expected_above = here - 16.0 * LAPSE_RATE_PER_BLOCK;
    let anomaly = expected_above - above;
    // Anomaly in °C → wind speed. Tuned so 5°C anomaly gives ~0.15
    // blocks/sec updraft (gentle but visible on the wind arrow).
    anomaly * 0.03
}

/// Heat-aware convection: like `convection_at` but uses
/// `temperature_at_with_heat` so dynamic heat sources contribute
/// to the gradient anomaly. A fire creates a strong local updraft
/// (chimney effect); an ice block creates a downdraft.
///
/// This is what the bin's wind arrow / vertical_wind reads when a
/// HeatSourceMap is available. Pure-function callers (tests, mesh
/// pipeline) use the non-heat variant.
pub fn convection_at_with_heat(
    wx: i32, wy: i32, wz: i32,
    heat_sources: &crate::heat_source::HeatSourceMap,
) -> f32 {
    let here = temperature_at_with_heat(wx, wy, wz, heat_sources);
    let above = temperature_at_with_heat(wx, wy + 16, wz, heat_sources);
    let expected_above = here - 16.0 * LAPSE_RATE_PER_BLOCK;
    let anomaly = expected_above - above;
    anomaly * 0.03
}

// ============================================================================
// Solar warming — the diurnal cycle's contribution to local temperature.
// ============================================================================

/// Maximum solar warming at noon, in °C. A sky-exposed surface cell
/// at noon is up to this many degrees warmer than its base climate.
/// Tuned to be visible on the F4 convection arrow (creates a gradient
/// anomaly large enough to drive a noticeable updraft) without
/// dominating the climate (so latitude/altitude still matter).
const SOLAR_WARMING_PEAK_C: f32 = 12.0;

/// The Y-band of cells that get solar warming. Sun heats SURFACES,
/// not air; high-altitude cells well above ground don't get the
/// warming. Cells underground (Y < SEA_LEVEL - 5) are shaded;
/// cells far above (Y > SEA_LEVEL + 30) are too high in the air
/// column to be warmed by surface sun.
///
/// This is a deliberate approximation. A true model would ray-trace
/// from each cell to the sky and check for occluders. That's a
/// future project (probably tied to a sky-exposure cache in
/// `chunk_field_cache` once that lands). For now: a Y-band proxy.
const SOLAR_BAND_BOTTOM_OFFSET: i32 = -5;
const SOLAR_BAND_TOP_OFFSET: i32 = 30;

/// Solar warming contribution at this position, given the current
/// day phase (from `Sky::day_phase()`). Returns °C.
///
/// - Phase 0.0 (sunrise): sin(0) = 0. No warming.
/// - Phase 0.25 (noon):   sin(π/2) = 1. Peak warming.
/// - Phase 0.5 (sunset):  sin(π) = 0. No warming.
/// - Phase 0.5..1.0 (night): zero.
///
/// Outside the Y-band: zero.
///
/// This is the **temperature × diurnal coupling** the design has
/// been pointing toward. Walking the same square of ground at noon
/// vs. midnight should feel different in the F4 wind arrow:
///   - Noon: arrow tilts up (yellow), sun-warmed ground updrafts.
///   - Midnight: arrow horizontal or even down (blue), surface
///     radiates heat back to sky.
pub fn solar_warming_at(wx: i32, wy: i32, wz: i32, day_phase: f32) -> f32 {
    use crate::constants::SEA_LEVEL;
    let band_bottom = SEA_LEVEL + SOLAR_BAND_BOTTOM_OFFSET;
    let band_top = SEA_LEVEL + SOLAR_BAND_TOP_OFFSET;
    if wy < band_bottom || wy > band_top { return 0.0; }

    // Daytime intensity curve. Phase in [0, 0.5] is daytime;
    // sin(phase * 2π) gives a half-cycle peaking at noon (0.25).
    // Phase in [0.5, 1.0] is night → zero contribution.
    let intensity = if day_phase < 0.5 {
        (day_phase * std::f32::consts::TAU).sin()
    } else {
        0.0
    };

    // Within the band, the warming is also strongest near sea level
    // (where the surface IS) and falls off with altitude. This
    // approximates: high-altitude air, even if sky-exposed, doesn't
    // have a thermal mass below it that the sun is heating, so it
    // doesn't warm as much. Linear falloff is fine here; we're not
    // simulating radiative transfer.
    let altitude_above_sea = (wy - SEA_LEVEL).max(0) as f32;
    let altitude_factor = (1.0 - altitude_above_sea / SOLAR_BAND_TOP_OFFSET as f32).max(0.0);

    // Tiny noise so the boundary isn't a perfect stripe. Same noise
    // shape as `temperature_at` for consistency.
    let noise = ((wx as f32 * 0.07).sin() + (wz as f32 * 0.11).cos()) * 0.5;

    SOLAR_WARMING_PEAK_C * intensity * altitude_factor + noise * intensity
}

/// All-coupled temperature: base climate + heat sources + solar.
/// This is the canonical runtime temperature query for systems that
/// have access to all the inputs (the bin's per-frame loop).
pub fn temperature_at_full(
    wx: i32, wy: i32, wz: i32,
    heat_sources: &crate::heat_source::HeatSourceMap,
    day_phase: f32,
) -> f32 {
    temperature_at_with_heat(wx, wy, wz, heat_sources)
        + solar_warming_at(wx, wy, wz, day_phase)
}

/// All-coupled convection: gradient anomaly using the full
/// temperature model (climate + heat sources + solar). What the F4
/// wind arrow reads.
pub fn convection_at_full(
    wx: i32, wy: i32, wz: i32,
    heat_sources: &crate::heat_source::HeatSourceMap,
    day_phase: f32,
) -> f32 {
    let here = temperature_at_full(wx, wy, wz, heat_sources, day_phase);
    let above = temperature_at_full(wx, wy + 16, wz, heat_sources, day_phase);
    let expected_above = here - 16.0 * LAPSE_RATE_PER_BLOCK;
    let anomaly = expected_above - above;
    anomaly * 0.03
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Equator at sea level should be near BASE_TEMPERATURE_C
    /// (within local noise).
    #[test]
    fn equator_sea_level_is_warm() {
        let t = temperature_at(0, 64, 0);
        assert!((t - BASE_TEMPERATURE_C).abs() < 10.0,
            "equator sea level should be near {}°C, got {}", BASE_TEMPERATURE_C, t);
        assert!(t > 10.0, "equator should be well above freezing, got {}", t);
    }

    /// Lapse rate: temperature drops with altitude.
    #[test]
    fn temperature_drops_with_altitude() {
        let sea = temperature_at(0, 64, 0);
        let mountain = temperature_at(0, 200, 0);
        let high = temperature_at(0, 1000, 0);
        assert!(mountain < sea, "mountain colder than sea: {} vs {}", mountain, sea);
        assert!(high < mountain, "high colder than mountain: {} vs {}", high, mountain);
        // 1000 blocks above sea = -6.5°C from lapse alone; should be
        // colder by at least 4°C (allowing local noise to muddle).
        let drop = sea - high;
        assert!(drop > 4.0, "expected ≥4°C drop, got {}", drop);
    }

    /// Latitude: poles are colder than equator at same altitude.
    #[test]
    fn poles_are_colder_than_equator() {
        let mut warm_count = 0;
        let mut total = 0;
        for x in [0, 100, -100, 500] {
            let equator = temperature_at(x, 64, 0);
            let pole = temperature_at(x, 64, (LATITUDE_SPAN / 2.0) as i32);
            if equator > pole { warm_count += 1; }
            total += 1;
        }
        // Should hold for most positions (noise can flip occasional
        // ones at small lat differences).
        assert!(warm_count == total,
            "equator should be warmer than pole at all tested x, got {}/{}",
            warm_count, total);
    }

    /// Convection: a hot cell with cool air above generates an
    /// updraft. We can't fabricate this from `temperature_at` alone
    /// (it's deterministic), but we CAN test that convection_at
    /// returns nonzero values across the world (real anomalies exist
    /// from the noise component).
    #[test]
    fn convection_produces_nonzero_values() {
        let mut nonzero = 0;
        let mut total = 0;
        for x in -100..=100 {
            for z in -100..=100 {
                let c = convection_at(x, 64, z);
                if c.abs() > 0.001 { nonzero += 1; }
                total += 1;
            }
        }
        let ratio = nonzero as f32 / total as f32;
        assert!(ratio > 0.5,
            "expected most positions to have nonzero convection, got {} ({}/{})",
            ratio, nonzero, total);
    }

    /// Convection should be roughly symmetric: positive and negative
    /// values both occur (some places have updrafts, some downdrafts).
    /// We sample many Y altitudes because the Y-noise component
    /// varies with altitude and the sign of "above is hotter or
    /// colder than expected" rotates through the noise wavelength.
    #[test]
    fn convection_has_both_signs() {
        let mut up = 0;
        let mut down = 0;
        for x in -50..=50 {
            for z in -50..=50 {
                for y in [40, 64, 80, 100, 120] {
                    let c = convection_at(x, y, z);
                    if c > 0.01 { up += 1; }
                    if c < -0.01 { down += 1; }
                }
            }
        }
        assert!(up > 0, "expected some updrafts, got 0");
        assert!(down > 0, "expected some downdrafts, got 0");
    }

    /// Boiling/freezing predicates align with the temperature field.
    #[test]
    fn predicates_match_temperature() {
        for x in [-50, 0, 50] {
            for y in [60, 64, 100, 1000] {
                for z in [-1000, 0, 1000] {
                    let t = temperature_at(x, y, z);
                    assert_eq!(is_freezing_at(x, y, z), t <= 0.0);
                    assert_eq!(is_boiling_at(x, y, z), t >= 100.0);
                }
            }
        }
    }

    /// Smoothness: adjacent cells have similar temperatures (no
    /// salt-and-pepper).
    #[test]
    fn temperature_field_is_smooth() {
        let mut max_jump: f32 = 0.0;
        for x in -50..=50 {
            for z in -50..=50 {
                let here = temperature_at(x, 64, z);
                let east = temperature_at(x + 1, 64, z);
                max_jump = max_jump.max((east - here).abs());
            }
        }
        // Adjacent cells should differ by < 0.5°C (noise + tiny
        // gradient terms).
        assert!(max_jump < 0.5,
            "temperature field too jagged: max neighbor diff = {}°C", max_jump);
    }

    /// Heat-aware: an empty heat-source map produces the same
    /// temperature as the base function.
    #[test]
    fn heat_aware_with_empty_map_matches_base() {
        let map = crate::heat_source::HeatSourceMap::new();
        for x in [-100, 0, 100] {
            for y in [40, 64, 200] {
                for z in [-100, 0, 100] {
                    let base = temperature_at(x, y, z);
                    let with_heat = temperature_at_with_heat(x, y, z, &map);
                    assert!((base - with_heat).abs() < 1e-3,
                        "empty map should not change temperature: ({},{},{}) base={} with={}",
                        x, y, z, base, with_heat);
                }
            }
        }
    }

    /// Heat-aware: a fire near a cell makes that cell warmer.
    #[test]
    fn fire_warms_nearby_cells() {
        let map = crate::heat_source::HeatSourceMap::new();
        map.add(crate::heat_source::HeatSource::new(
            [0.0, 64.0, 0.0], 500.0, 4.0));
        let cold = temperature_at(0, 64, 0);
        let warm = temperature_at_with_heat(0, 64, 0, &map);
        assert!(warm > cold + 100.0,
            "fire should significantly warm cell at source: cold={} warm={}",
            cold, warm);
    }

    /// Heat-aware convection: a fire creates an updraft (chimney).
    #[test]
    fn fire_creates_updraft() {
        let map = crate::heat_source::HeatSourceMap::new();
        map.add(crate::heat_source::HeatSource::new(
            [0.0, 64.0, 0.0], 500.0, 4.0));
        let conv = convection_at_with_heat(0, 64, 0, &map);
        assert!(conv > 1.0,
            "fire should create strong updraft, got {} b/s", conv);
    }

    /// Heat-aware convection: cold source creates a downdraft.
    #[test]
    fn ice_block_creates_downdraft() {
        let map = crate::heat_source::HeatSourceMap::new();
        map.add(crate::heat_source::HeatSource::new(
            [0.0, 64.0, 0.0], -50.0, 4.0));
        let conv = convection_at_with_heat(0, 64, 0, &map);
        assert!(conv < 0.0,
            "ice should create downdraft, got {} b/s", conv);
    }

    // ========================================================================
    // Solar warming tests.
    // ========================================================================

    /// Noon (phase 0.25) gives peak solar warming at sea level.
    #[test]
    fn noon_warms_surface() {
        // Sea level cell at noon should get close to SOLAR_WARMING_PEAK_C.
        let warming = solar_warming_at(0, 64, 0, 0.25);
        // Peak is 12°C; allow ±1.5 for the noise component.
        assert!(warming > 10.0 && warming < 14.0,
            "noon at sea level should be ~12°C warming, got {}", warming);
    }

    /// Midnight (phase 0.75) gives no solar warming.
    #[test]
    fn midnight_has_no_solar_warming() {
        for x in [-100, 0, 100] {
            for y in [60, 64, 80] {
                for z in [-100, 0, 100] {
                    let w = solar_warming_at(x, y, z, 0.75);
                    assert_eq!(w, 0.0,
                        "midnight should give no solar warming at ({},{},{}): got {}",
                        x, y, z, w);
                }
            }
        }
    }

    /// Sunrise (phase 0.0) and sunset (phase ~0.5) give zero (or
    /// near-zero) solar warming.
    #[test]
    fn sunrise_sunset_give_no_warming() {
        // Phase 0.0: sin(0) = 0. No warming intensity.
        // The noise term is ALSO multiplied by intensity, so total is 0.
        assert_eq!(solar_warming_at(0, 64, 0, 0.0), 0.0);
        // Phase 0.5: sin(π) ≈ 0. Effectively no warming.
        let sunset = solar_warming_at(0, 64, 0, 0.4999);
        assert!(sunset.abs() < 0.1, "sunset should be ~0, got {}", sunset);
    }

    /// Underground cells (Y < SEA_LEVEL - 5) get no solar warming.
    #[test]
    fn underground_is_shaded() {
        // Y = 50 is way below SEA_LEVEL - 5 = 59.
        for hour in [0.0, 0.25, 0.5, 0.75] {
            assert_eq!(solar_warming_at(0, 50, 0, hour), 0.0,
                "underground at phase {} should be shaded", hour);
        }
    }

    /// High altitude (above the solar band) gets no warming.
    /// The sun heats SURFACES, not air; cells far above ground don't
    /// have enough thermal contact.
    #[test]
    fn high_altitude_gets_no_warming() {
        for hour in [0.0, 0.25, 0.5] {
            // Y = 200 is way above SEA_LEVEL + 30 = 94.
            assert_eq!(solar_warming_at(0, 200, 0, hour), 0.0,
                "high-altitude at phase {} should not be warmed", hour);
        }
    }

    /// Solar warming decreases with altitude WITHIN the band.
    /// A cell at sea level + 0 should get more warming than a cell
    /// at sea level + 25, both at noon.
    #[test]
    fn solar_warming_decreases_with_altitude_in_band() {
        let low = solar_warming_at(0, 64, 0, 0.25);
        let mid = solar_warming_at(0, 64 + 15, 0, 0.25);
        let high = solar_warming_at(0, 64 + 25, 0, 0.25);
        assert!(low > mid, "sea-level ({}) should be warmer than mid ({})",
            low, mid);
        assert!(mid > high, "mid ({}) should be warmer than high ({})",
            mid, high);
    }

    /// Diurnal convection: noon creates a strong updraft over warmed
    /// ground (solar makes here warmer; the +16 cell is partly
    /// outside the band so doesn't get as much).
    #[test]
    fn noon_creates_solar_updraft() {
        let map = crate::heat_source::HeatSourceMap::new();
        let here = 64; // sea level
        let day_conv = convection_at_full(0, here, 0, &map, 0.25);
        let night_conv = convection_at_full(0, here, 0, &map, 0.75);
        // Day should produce more upward (or less downward) convection
        // than night at the same position.
        assert!(day_conv > night_conv,
            "noon convection ({}) should exceed midnight convection ({})",
            day_conv, night_conv);
        // And specifically: noon should drive an UPDRAFT (positive).
        assert!(day_conv > 0.0,
            "noon should produce updraft, got {}", day_conv);
    }

    /// `temperature_at_full` is `temperature_at_with_heat + solar`.
    /// Verify the composition is additive.
    #[test]
    fn temperature_at_full_composes_components() {
        let map = crate::heat_source::HeatSourceMap::new();
        map.add(crate::heat_source::HeatSource::new(
            [0.0, 64.0, 0.0], 100.0, 3.0));
        let phase_noon = 0.25_f32;
        let composed = temperature_at_full(0, 64, 0, &map, phase_noon);
        let manual = temperature_at_with_heat(0, 64, 0, &map)
            + solar_warming_at(0, 64, 0, phase_noon);
        assert!((composed - manual).abs() < 1e-3,
            "full should equal heat + solar: full={}, manual={}",
            composed, manual);
    }
}
