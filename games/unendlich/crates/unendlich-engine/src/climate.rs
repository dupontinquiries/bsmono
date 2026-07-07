//! climate.rs — pure diurnal temperature core for the fog / moisture sim.
//!
//! [`temp_for`] returns a normalized temperature in `[0, 1]`: a smooth diurnal
//! curve (coldest just before dawn, warmest mid-afternoon) minus an altitude
//! lapse term. Deterministic and allocation-free — no clocks, no RNG (R11/R21).
//!
//! This is the pure core of the "diurnal temperature cycle" backlog item. The
//! coordinator wires it into the moisture tick later: temperature drives the
//! sim's saturation capacity (`capacity = capacity_base * exp(k * T)`), which is
//! why fog pools in cold valleys at night and burns off in the warm afternoon.

use core::f32::consts::PI;

/// Day length used when a caller passes a non-positive `day_len`.
const DEFAULT_DAY_LEN: f32 = 1.0;

/// Phase (fraction of day) of the coldest moment — just before dawn.
const TROUGH_PHASE: f32 = 0.21;

/// Normalized diurnal temperature in `[0, 1]`.
///
/// * `time_of_day` — seconds into the day (any real value; wrapped by `day_len`).
/// * `day_len`     — length of a full day in seconds (<= 0 falls back to 1.0).
/// * `altitude`    — metres; negative altitudes are treated as 0.
/// * `lapse`       — per-metre cooling subtracted from the curve (negative
///                   values are treated as 0 so altitude never *warms*).
///
/// The base curve is a cosine troughing near dawn (`TROUGH_PHASE`) and peaking a
/// half-day later (mid-afternoon). Altitude subtracts `lapse * altitude`. The
/// result is clamped to `[0, 1]` and is always finite, including for infinite or
/// extreme inputs.
pub fn temp_for(time_of_day: f32, day_len: f32, altitude: f32, lapse: f32) -> f32 {
    let day = if day_len > 0.0 { day_len } else { DEFAULT_DAY_LEN };
    let mut phase = (time_of_day / day).rem_euclid(1.0);
    if !phase.is_finite() {
        phase = 0.0; // infinite/NaN time -> treat as start of day
    }
    // Cosine diurnal curve in [0, 1]: 0 at the trough (dawn), 1 a half-day later.
    let base = 0.5 - 0.5 * (2.0 * PI * (phase - TROUGH_PHASE)).cos();
    let alt = lapse.max(0.0) * altitude.max(0.0);
    (base - alt).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prop_in_unit_range() {
        let mut t = -10.0;
        while t <= 10.0 {
            let mut a = -100.0;
            while a <= 500.0 {
                let v = temp_for(t, 600.0, a, 0.002);
                assert!(v >= 0.0 && v <= 1.0, "out of range: {}", v);
                a += 37.0;
            }
            t += 0.7;
        }
    }

    #[test]
    fn prop_periodic_in_time() {
        let day = 600.0;
        let mut t = 0.0;
        while t < day {
            let a = temp_for(t, day, 50.0, 0.001);
            let b = temp_for(t + day, day, 50.0, 0.001);
            let c = temp_for(t + 3.0 * day, day, 50.0, 0.001);
            assert!((a - b).abs() < 1e-5, "not periodic over +1 day at t={}", t);
            assert!((a - c).abs() < 1e-5, "not periodic over +3 days at t={}", t);
            t += 11.0;
        }
    }

    #[test]
    fn prop_cooler_with_altitude() {
        // Monotone non-increasing in altitude: higher is never warmer.
        let day = 600.0;
        let mut t = 0.0;
        while t < day {
            let mut a = 0.0;
            while a < 400.0 {
                let lo = temp_for(t, day, a, 0.002);
                let hi = temp_for(t, day, a + 25.0, 0.002);
                assert!(hi <= lo + 1e-6, "warmer higher up at t={} a={}", t, a);
                a += 25.0;
            }
            t += 23.0;
        }
    }

    #[test]
    fn prop_finite_on_extremes() {
        let times = [0.0f32, -1.0, 1e9, f32::INFINITY, f32::NEG_INFINITY];
        let days = [600.0f32, 0.0, -5.0];
        let alts = [0.0f32, 1e6, -10.0];
        for &t in &times {
            for &d in &days {
                for &a in &alts {
                    let v = temp_for(t, d, a, 0.002);
                    assert!(v.is_finite(), "non-finite for t={} d={} a={}", t, d, a);
                    assert!(v >= 0.0 && v <= 1.0);
                }
            }
        }
    }

    #[test]
    fn dawn_colder_than_afternoon() {
        let day = 1000.0;
        let dawn = temp_for(TROUGH_PHASE * day, day, 0.0, 0.0);
        let afternoon = temp_for((TROUGH_PHASE + 0.5) * day, day, 0.0, 0.0);
        assert!(afternoon > dawn, "afternoon {} should beat dawn {}", afternoon, dawn);
    }
}
