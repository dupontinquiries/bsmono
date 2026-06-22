//! `game_speed` — authoritative game-speed (time-dilation) model.
//!
//! A **server-side** speed multiplier (0x–10x) lets a world run frozen, slow,
//! normal, or fast. It is *not* a client-local effect: a client applying its
//! own dilation would desync from the authoritative sim. So every participant
//! states a *preference*, and the world runs at the single value the group
//! reconciles to.
//!
//! **Reconciliation rule — closest to 1x wins.** Collect every participant's
//! preferred multiplier and pick the one nearest to normal speed (1x). So:
//! - everyone picks 10x → the world runs 10x;
//! - everyone picks 0x (freeze) → 0x;
//! - mixed (someone 1x, someone 10x) → 1x, because 1x is closest to normal.
//!
//! This is deliberately conservative: a single player who wants normal speed
//! pulls the whole session back toward normal, so no one can unilaterally
//! speed up or freeze a shared world. (On a dedicated host with one operator
//! preference, that preference simply wins.)
//!
//! The slider is **logarithmic** around 1x at its midpoint, with a hard 0x
//! (pause) at the far-left detent. Pure + deterministic — unit-tested below.

/// Far-left slider detent (and anything below it) means a hard freeze. Kept
/// just above 0 so the log mapping never sees a true zero position.
const FREEZE_POS: f32 = 0.012;

/// Map a slider position `pos` in `[0, 1]` to a speed multiplier.
///
/// - `pos <= FREEZE_POS` → `0.0` (hard pause)
/// - `pos == 0.5` → `1.0` (normal — the default)
/// - `pos == 1.0` → `10.0`
/// - in between → logarithmic: `10^((pos - 0.5) * 2)`, so each half of the
///   track is one decade (0.1x … 1x … 10x).
pub fn pos_to_multiplier(pos: f32) -> f32 {
    let p = pos.clamp(0.0, 1.0);
    if p <= FREEZE_POS {
        return 0.0;
    }
    10f32.powf((p - 0.5) * 2.0)
}

/// Inverse of [`pos_to_multiplier`]: the slider position that yields `mult`.
/// `0.0` maps to the freeze detent; values are clamped to the 0.1x–10x range
/// the track can express.
pub fn multiplier_to_pos(mult: f32) -> f32 {
    if mult <= 0.0 {
        return 0.0;
    }
    let m = mult.clamp(0.1, 10.0);
    (m.log10() / 2.0 + 0.5).clamp(0.0, 1.0)
}

/// The default position (1x normal speed) — the slider's midpoint.
pub const DEFAULT_POS: f32 = 0.5;

/// Reconcile every participant's preferred multiplier to the single value the
/// world runs at: **the one closest to 1x**. Ties (two values equidistant
/// from 1x, e.g. 0.5x and 2x are *not* equidistant linearly — see note) break
/// toward the *slower* value, which is the more conservative choice for a
/// shared world. Empty input → 1x (normal).
///
/// Note on distance: closeness is measured in *log space* so that 0.5x and 2x
/// are treated as equally far from 1x (one decade each way), matching how the
/// slider is laid out. A literal 1x preference always wins outright.
pub fn reconcile(prefs: &[f32]) -> f32 {
    if prefs.is_empty() {
        return 1.0;
    }
    // log-distance from 1x; 0x (freeze) is treated as maximally far below
    // (it only wins if *everyone* wants it — handled by the all-equal case).
    let logdist = |m: f32| -> f32 {
        if m <= 0.0 {
            // freeze: one decade below 0.1x, so any running speed beats it
            // unless every preference is freeze.
            return (0.1f32.log10() - 1.0).abs(); // = 2.0 decades from 1x
        }
        m.log10().abs()
    };
    // if every preference is an exact freeze, the world freezes.
    if prefs.iter().all(|&m| m <= 0.0) {
        return 0.0;
    }
    let mut best = prefs[0];
    let mut best_d = logdist(best);
    for &m in &prefs[1..] {
        let d = logdist(m);
        // closer to 1x wins; tie → slower (smaller multiplier) is more
        // conservative for a shared world.
        if d < best_d - 1e-6 || (((d - best_d).abs() <= 1e-6) && m < best) {
            best = m;
            best_d = d;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn midpoint_is_normal_speed() {
        assert!(approx(pos_to_multiplier(DEFAULT_POS), 1.0));
        assert!(approx(multiplier_to_pos(1.0), 0.5));
    }

    #[test]
    fn ends_are_freeze_and_ten_x() {
        assert_eq!(pos_to_multiplier(0.0), 0.0); // hard pause
        assert!(approx(pos_to_multiplier(1.0), 10.0));
        // half-decade points
        assert!(approx(pos_to_multiplier(0.75), 10f32.powf(0.5))); // ~3.16x
        assert!(approx(pos_to_multiplier(0.25), 10f32.powf(-0.5))); // ~0.316x
    }

    #[test]
    fn pos_multiplier_roundtrips() {
        // values comfortably inside the running range (0.1x sits right at the
        // freeze boundary in position space, so it's excluded as degenerate).
        for &m in &[0.2f32, 0.5, 1.0, 2.0, 3.16, 10.0] {
            let p = multiplier_to_pos(m);
            assert!(approx(pos_to_multiplier(p), m), "roundtrip {m} -> {p} -> {}", pos_to_multiplier(p));
        }
    }

    #[test]
    fn everyone_fast_runs_fast() {
        assert!(approx(reconcile(&[10.0, 10.0, 10.0]), 10.0));
    }

    #[test]
    fn everyone_frozen_freezes() {
        assert_eq!(reconcile(&[0.0, 0.0]), 0.0);
    }

    #[test]
    fn one_normal_pulls_session_to_normal() {
        // mixed: a 10x and a 1x → 1x wins (closest to normal)
        assert!(approx(reconcile(&[10.0, 1.0]), 1.0));
        // a freeze + a 10x → 10x (running beats freeze unless all freeze)
        assert!(approx(reconcile(&[0.0, 10.0]), 10.0));
        // 0.5x and 2x are equidistant in log space → slower (0.5x) wins
        assert!(approx(reconcile(&[2.0, 0.5]), 0.5));
    }

    #[test]
    fn closest_to_one_among_many() {
        // 0.3x, 5x, 1.5x → 1.5x is nearest to 1x in log space
        let r = reconcile(&[0.3, 5.0, 1.5]);
        assert!(approx(r, 1.5), "got {r}");
    }

    #[test]
    fn empty_is_normal() {
        assert!(approx(reconcile(&[]), 1.0));
    }
}
