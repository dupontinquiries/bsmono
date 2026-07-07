//! `snow_line` — snow coverage by altitude and temperature.
//!
//! Worker-03 / WORLD pure core, round 19. Returns snow coverage in [0,1]:
//! none well below the snow line, full well above it, with a soft blend across
//! it. Warmer temperatures raise the effective line (snow only clings higher
//! up). Feeds surface material / albedo selection.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

const TEMP_LIFT: f32 = 200.0; // warm pushes the snow line this much higher
const BLEND: f32 = 40.0; // altitude band over which snow fades in

/// Snow coverage in [0,1] at `altitude`, ambient `temp` (0..1), with base snow
/// `line`. Warmer temps lift the effective line; a `BLEND`-wide band softens it.
pub fn snow(altitude: f32, temp: f32, line: f32) -> f32 {
    let eff_line = line + clamp01(temp) * TEMP_LIFT;
    let lo = eff_line - BLEND * 0.5;
    let t = (altitude - lo) / BLEND;
    clamp01(t)
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn above_line_is_snow() {
        assert!((snow(2000.0, 0.0, 1000.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn below_line_is_bare() {
        assert_eq!(snow(0.0, 0.0, 1000.0), 0.0);
    }

    #[test]
    fn warm_melts_snow() {
        // Just above a cold line there's some snow; the same spot warm has less.
        let cold = snow(1010.0, 0.1, 1000.0);
        let warm = snow(1010.0, 0.9, 1000.0);
        assert!(cold > warm);
    }

    #[test]
    fn blend_near_line() {
        // At the effective line (temp 0 → eff_line = line) coverage is mid.
        let v = snow(1000.0, 0.0, 1000.0);
        assert!(v > 0.0 && v < 1.0);
        assert!((v - 0.5).abs() < 1e-5);
    }

    #[test]
    fn bounded() {
        for a in 0..3000 {
            for ti in 0..=10 {
                let v = snow(a as f32, ti as f32 / 10.0, 1000.0);
                assert!(v >= 0.0 && v <= 1.0);
            }
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(snow(1200.0, 0.3, 1000.0), snow(1200.0, 0.3, 1000.0));
    }
}
