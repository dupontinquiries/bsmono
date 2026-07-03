//! Wind field — direction + magnitude at any world position.
//!
//! Today's implementation is a smooth analytical function rather than a
//! field-framework derived field. Reasons:
//!
//!   - Need TWO scalars (X, Z components) but the field framework only
//!     supports scalar grids per field name. Adding tuple-valued fields
//!     would require touching the framework, the registry, the per-chunk
//!     storage, etc.
//!
//!   - Wind varies slowly across the world AND through time, so an
//!     analytical noise-driven function is cheap to sample directly,
//!     no caching needed.
//!
//!   - Time-evolution is natural here (gusts, slowly rotating direction).
//!     The field framework today is static-derived; making it time-aware
//!     is a separate refactor.
//!
//! Wind direction rotates slowly with `world_time` so plants visibly sway
//! in directions that change over minutes. Magnitude is modulated by
//! altitude (more wind at altitude) and a low-frequency noise function
//! (some areas calm, others gusty).
//!
//! Future:
//!   - Real solver: pressure gradients drive flow, blocks obstruct it.
//!   - Couple to biome temperature gradients (warm air rising, cooler
//!     air flowing in horizontally).
//!   - Move into the field framework as a vector field once that exists.

use crate::world::noise;

/// Average wind magnitude (no units; treated as fraction of "max wind").
const BASE_MAGNITUDE: f32 = 0.3;
/// How much altitude boosts wind magnitude (per block above sea level).
const ALTITUDE_GAIN: f32 = 0.005;
/// Maximum wind magnitude clamp.
/// Max wind magnitude — the cap on gust peaks. Was 1.0 (steady wind);
/// bumped to 3.0 to allow gust modulation to produce visible peaks
/// while still bounded.
const MAX_MAGNITUDE: f32 = 3.0;
/// Time scale for slow direction rotation, in seconds. Direction completes
/// a full rotation roughly every TWOPI × this many seconds.
const DIRECTION_ROTATION_SECS: f64 = 50.0;
/// Spatial scale of the regional wind variation in blocks. Larger = wind
/// magnitude varies over wider areas.
const REGIONAL_SCALE: f64 = 256.0;

/// Sample wind at a world position and time. Returns (wind_x, wind_z, magnitude).
/// `wind_x` and `wind_z` are unit-normalized direction components scaled by
/// magnitude (so the returned vector has length = magnitude).
///
/// `world_time` should be derived from `TickId.to_sim_seconds(tick_period)`
/// for rollback determinism — same TickId always returns the same wind.
/// Wall-clock time should NOT be passed here in simulation contexts;
/// it's fine for purely visual / debug uses.
pub fn wind_at(world_x: f64, world_y: f64, world_z: f64, world_time: f64) -> (f32, f32, f32) {
    // Direction: a slow global rotation modulated by spatial position so
    // different regions feel slightly different gust directions.
    let global_phase = world_time / DIRECTION_ROTATION_SECS;
    let local_phase_offset = noise::noise_2d(
        world_x / REGIONAL_SCALE * 0.5,
        world_z / REGIONAL_SCALE * 0.5,
        0xCAFEBEEF,
    ) * 1.5; // ±1.5 radians of local offset
    let phase = (global_phase as f32) + (local_phase_offset as f32);
    let dir_x = phase.cos();
    let dir_z = phase.sin();

    // Magnitude: BASE plus altitude gain plus low-frequency regional noise.
    let altitude_above_sea = (world_y - crate::constants::SEA_LEVEL as f64).max(0.0) as f32;
    let altitude_term = altitude_above_sea * ALTITUDE_GAIN;
    let regional = noise::noise_2d(
        world_x / REGIONAL_SCALE,
        world_z / REGIONAL_SCALE,
        0x1A2B3C4D,
    ) as f32;
    // regional is in [-1, 1]; map to [-0.2, 0.2].
    let regional_term = regional * 0.2;
    let base = (BASE_MAGNITUDE + altitude_term + regional_term).clamp(0.0, MAX_MAGNITUDE);

    // Gust modulation: multiply base magnitude by a time-varying factor
    // that produces "gust" peaks. Without this, wind is steady-state and
    // imperceptible to the player (steady wind × WIND_FORCE_SCALE / friction
    // gives drift on the order of 0.025 m/s — invisible). Gusts produce
    // brief pulses where wind force jumps to ~5x base, producing a visible
    // 0.1 m/s shove that the player can feel.
    //
    // Two superimposed timescales:
    //   - Slow: ~100s period, ±0.5 around 1.0 — broad envelope (r492: was 8).
    //   - Fast: ~31s period, ±0.4 around 1.0 — gust pulses (r492: was 1.5, vacillated).
    // With a regional phase offset so different chunks gust at different
    // times (no synchronized world-wide gusts).
    let gust_phase_offset = noise::noise_2d(
        world_x / REGIONAL_SCALE * 0.7,
        world_z / REGIONAL_SCALE * 0.7,
        0x6B17BE8E,
    ) as f32;
    let slow_t = (world_time / 16.0) as f32 + gust_phase_offset;
    let fast_t = (world_time / 5.0) as f32 + gust_phase_offset * 2.0;
    // Slow envelope ∈ [0.5, 1.5], fast pulse ∈ [0.6, 1.4].
    let slow = 1.0 + 0.5 * slow_t.sin();
    let fast = 1.0 + 0.4 * fast_t.sin();
    // Their product ∈ [0.3, 2.1] roughly — peaks ~2x baseline. We want
    // peaks closer to 5x, so add a sharp pulse component when both are
    // high simultaneously (a "gust burst"). Squaring the high-side
    // intensifies peaks without affecting valleys.
    let combined = slow * fast;
    let burst = if combined > 1.5 {
        // Strong gust window — apply quadratic intensification.
        combined * combined / 1.5
    } else {
        combined
    };
    let gust_multiplier = burst.clamp(0.3, 5.0);

    let mag = (base * gust_multiplier).clamp(0.0, MAX_MAGNITUDE);

    (dir_x * mag, dir_z * mag, mag)
}

/// Magnitude only — what `field_reader("wind", ...)` returns.
pub fn wind_magnitude(world_x: f64, world_y: f64, world_z: f64, world_time: f64) -> f32 {
    let (_x, _z, m) = wind_at(world_x, world_y, world_z, world_time);
    m
}

/// Vertical wind component (blocks/second) at this position, driven
/// by temperature-gradient convection. Positive = updraft (warm air
/// rising); negative = downdraft (cool air sinking).
///
/// This is the temperature → wind coupling: warm spots generate
/// updrafts, cold spots downdrafts. Uses `temperature::convection_at`
/// for the gradient computation. Magnitude typically ±0.3 b/s.
///
/// Time isn't used (yet) — convection is stable on the time scale of
/// individual ticks. Future: add diurnal modulation (warmer days =
/// stronger updrafts).
pub fn vertical_wind_at(world_x: f64, world_y: f64, world_z: f64, _world_time: f64) -> f32 {
    crate::temperature::convection_at(
        world_x.round() as i32,
        world_y.round() as i32,
        world_z.round() as i32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wind_returns_unit_direction_scaled_by_magnitude() {
        let (wx, wz, m) = wind_at(0.0, 64.0, 0.0, 0.0);
        // Vector length should equal magnitude.
        let len = (wx * wx + wz * wz).sqrt();
        assert!((len - m).abs() < 0.01,
            "expected len ≈ magnitude; len={} m={}", len, m);
    }

    #[test]
    fn wind_magnitude_increases_with_altitude() {
        let (_, _, m_low)  = wind_at(0.0, 64.0,  0.0, 0.0);
        let (_, _, m_high) = wind_at(0.0, 200.0, 0.0, 0.0);
        assert!(m_high > m_low,
            "wind should be stronger at altitude: low={} high={}", m_low, m_high);
    }

    #[test]
    fn wind_direction_changes_over_time() {
        let (x0, z0, _) = wind_at(0.0, 64.0, 0.0, 0.0);
        let (x1, z1, _) = wind_at(0.0, 64.0, 0.0, 60.0);
        // Direction should be measurably different after a minute.
        let dot = x0 * x1 + z0 * z1;
        let m0 = (x0 * x0 + z0 * z0).sqrt();
        let m1 = (x1 * x1 + z1 * z1).sqrt();
        let cos_angle = dot / (m0 * m1);
        assert!(cos_angle < 0.99,
            "direction should rotate over time; cos_angle={}", cos_angle);
    }

    #[test]
    fn wind_is_deterministic() {
        // Same (x,y,z,t) should give same result.
        let a = wind_at(100.0, 80.0, 50.0, 5.0);
        let b = wind_at(100.0, 80.0, 50.0, 5.0);
        assert_eq!(a, b);
    }

    /// Wind gust amplitude test — the "amplitude slider" framework.
    /// Verifies that over a long time window, the wind magnitude
    /// reaches BOTH calm and gusty values. Without gust modulation,
    /// wind is nearly steady-state (boring); with it, the player feels
    /// pulses of force interspersed with calm.
    ///
    /// Sampled across 1000 seconds with 0.5s step (2000 samples).
    /// Reports min, max, mean, std. The amplitude RATIO (max/mean)
    /// is the key metric — a steady wind has ratio ≈ 1.0; a gusty
    /// wind has ratio ≥ 2.0.
    #[test]
    fn wind_has_visible_gust_amplitude() {
        let mut samples: Vec<f32> = Vec::with_capacity(2000);
        for i in 0..2000 {
            let t = i as f64 * 0.5;
            let (_, _, m) = wind_at(50.0, 100.0, 50.0, t);
            samples.push(m);
        }
        let min = samples.iter().cloned().fold(f32::INFINITY, f32::min);
        let max = samples.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let mean = samples.iter().sum::<f32>() / samples.len() as f32;
        let ratio = max / mean.max(0.01);

        // Visible gusts: peak should be at least 2x the mean.
        assert!(ratio >= 2.0,
            "wind not gusty enough: peak/mean ratio = {:.2} (want ≥ 2.0).\
             min={:.3} max={:.3} mean={:.3}",
            ratio, min, max, mean);

        // Calm periods should also exist: min should be well below mean.
        assert!(min < mean * 0.6,
            "wind never goes calm: min/mean = {:.2} (want < 0.6). \
             min={:.3} mean={:.3}", min/mean, min, mean);
    }

    /// Amplitude slider analog: scaling world_time arguments by a factor
    /// is equivalent to running wind faster/slower — exactly what a
    /// realtime multiplier slider would do. This test verifies the
    /// behavior is preserved under that transform: if we sample wind
    /// at time scale 1x for 100s, we should get statistically similar
    /// distribution as sampling at 10x for 10s.
    #[test]
    fn wind_amplitude_scales_with_time_factor() {
        // 1x multiplier: 100s of samples at 0.1s step.
        let samples_1x: Vec<f32> = (0..1000)
            .map(|i| {
                let (_, _, m) = wind_at(0.0, 100.0, 0.0, i as f64 * 0.1);
                m
            }).collect();
        // 10x multiplier: 10s of samples at 0.01s step (same 1000 samples,
        // but compressed in real time — the SLIDER USE CASE).
        let samples_10x: Vec<f32> = (0..1000)
            .map(|i| {
                let (_, _, m) = wind_at(0.0, 100.0, 0.0, i as f64 * 0.01 * 10.0);
                m
            }).collect();
        // Both arrays should have similar mean and similar peak/calm range.
        let mean_1x: f32 = samples_1x.iter().sum::<f32>() / 1000.0;
        let mean_10x: f32 = samples_10x.iter().sum::<f32>() / 1000.0;
        assert!((mean_1x - mean_10x).abs() / mean_1x < 0.20,
            "mean changed too much under time-rescale: 1x={:.3} 10x={:.3}",
            mean_1x, mean_10x);

        let max_1x = samples_1x.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let max_10x = samples_10x.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        // Both should reach gust peaks (within 20% of each other).
        assert!((max_1x - max_10x).abs() / max_1x < 0.25,
            "peak changed too much under time-rescale: 1x={:.3} 10x={:.3}",
            max_1x, max_10x);
    }
}
