//! squeeze — a fit check for getting a body through a gap. Given how wide the body
//! is and how wide the opening is, decide: walk through (`Pass`), tuck/bend to fit
//! (`MustBend` with how hard), or it just won't go (`Stuck`). The substrate for a
//! big creature deciding whether a cave mouth or doorway is passable. OPTIONAL
//! task module; property-testable. Pure, std-only, deterministic.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Fit {
    /// The gap is at least the body's width — walk through.
    Pass,
    /// Tight: the body must bend/tuck by this angle (radians) to fit.
    MustBend(f32),
    /// Sealed or too small — impassable.
    Stuck,
}

/// Tuning for the fit check.
#[derive(Clone, Copy, Debug)]
pub struct SqueezeParams {
    /// Fraction of body width below which the gap is impassable (can't tuck that small).
    pub min_fraction: f32,
    /// Maximum bend the body can manage to squeeze (radians).
    pub max_bend: f32,
}

impl Default for SqueezeParams {
    fn default() -> SqueezeParams {
        SqueezeParams { min_fraction: 0.45, max_bend: 1.0 }
    }
}

/// Check fit. `body_width` is the body's full width across the squeeze axis;
/// `gap` is the open width of the opening (a sealed wall is `gap <= 0`). Returns
/// `Pass` when the gap is at least the body width, `MustBend(angle)` when the gap
/// is between `min_fraction × body_width` and the body width (more bend the
/// tighter it gets), and `Stuck` when narrower than that.
pub fn check(body_width: f32, gap: f32, p: &SqueezeParams) -> Fit {
    let w = body_width.max(1e-4);
    let min_w = w * p.min_fraction.clamp(0.0, 1.0);
    if gap >= w {
        Fit::Pass
    } else if gap >= min_w {
        // tightness in [0,1]: 0 at full width, 1 at the min passable width
        let tightness = ((w - gap) / (w - min_w).max(1e-4)).clamp(0.0, 1.0);
        Fit::MustBend((tightness * p.max_bend).clamp(0.0, p.max_bend))
    } else {
        Fit::Stuck
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        ((*seed >> 8) as f32) / ((1u32 << 24) as f32)
    }

    #[test]
    fn a_wide_opening_passes() {
        assert_eq!(check(1.0, 1.5, &SqueezeParams::default()), Fit::Pass);
        assert_eq!(check(1.0, 1.0, &SqueezeParams::default()), Fit::Pass);
    }

    #[test]
    fn a_tight_opening_must_bend() {
        let p = SqueezeParams::default();
        match check(1.0, 0.7, &p) {
            Fit::MustBend(a) => assert!(a > 0.0 && a <= p.max_bend, "bend in range: {a}"),
            other => panic!("expected MustBend, got {other:?}"),
        }
        // tighter → more bend
        let bend_tight = match check(1.0, 0.55, &p) {
            Fit::MustBend(a) => a,
            _ => -1.0,
        };
        let bend_loose = match check(1.0, 0.8, &p) {
            Fit::MustBend(a) => a,
            _ => -1.0,
        };
        assert!(bend_tight > bend_loose, "tighter gap needs more bend");
    }

    #[test]
    fn a_sealed_gap_is_stuck() {
        let p = SqueezeParams::default();
        assert_eq!(check(1.0, 0.1, &p), Fit::Stuck);
        assert_eq!(check(1.0, 0.0, &p), Fit::Stuck);
        assert_eq!(check(1.0, -1.0, &p), Fit::Stuck);
    }

    #[test]
    fn prop_fit_is_consistent_and_bend_bounded() {
        let p = SqueezeParams::default();
        let mut seed = 0x5A0E_E2E1u32;
        for _ in 0..5000 {
            let w = 0.2 + lcg(&mut seed) * 3.0;
            let gap = lcg(&mut seed) * 4.0 - 0.5;
            match check(w, gap, &p) {
                Fit::Pass => assert!(gap >= w - 1e-6, "Pass only when gap >= width"),
                Fit::MustBend(a) => {
                    assert!(a.is_finite() && a >= 0.0 && a <= p.max_bend + 1e-6, "bend bounded: {a}");
                    assert!(gap < w && gap >= w * p.min_fraction - 1e-6, "Bend only in the tight band");
                }
                Fit::Stuck => assert!(gap < w * p.min_fraction + 1e-6, "Stuck only when below min"),
            }
        }
    }
}
