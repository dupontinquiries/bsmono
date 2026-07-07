//! `sim::horn_charge` — horns accrue charge over time and discharge when full
//! (see `HORN_SYSTEM`).
//!
//! [`charge`] advances the stored charge by `rate · dt`, clamped to
//! `[0, capacity]`, so a horn fills (or, with a negative rate, drains) toward its
//! cap and never past it. [`try_discharge`] fires only when the horn is full,
//! returning the released energy (and the caller resets the charge to 0);
//! otherwise it returns `None`. A non-positive capacity holds nothing and never
//! discharges.
//!
//! Pure and deterministic: a clamped accumulate and a threshold test — no clock,
//! no randomness, no cross-imports.

/// Advance stored charge by `rate · dt`, clamped to `[0, capacity]`.
///
/// `capacity` is floored at 0; a `dt` of 0 leaves the charge unchanged (within
/// range). A negative `rate` drains.
pub fn charge(cur: f32, dt: f32, rate: f32, capacity: f32) -> f32 {
    let cap = if capacity > 0.0 { capacity } else { 0.0 };
    let next = cur + rate * dt;
    if next < 0.0 {
        0.0
    } else if next > cap {
        cap
    } else {
        next
    }
}

/// Discharge the horn iff it is full.
///
/// Returns `Some(cur)` (the energy released) when `capacity > 0` and `cur` has
/// reached `capacity`; otherwise `None`.
pub fn try_discharge(cur: f32, capacity: f32) -> Option<f32> {
    if capacity > 0.0 && cur >= capacity {
        Some(cur)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    #[test]
    fn accrues_by_rate_times_dt() {
        assert_eq!(charge(0.0, 1.0, 2.0, 10.0), 2.0);
        assert_eq!(charge(3.0, 0.5, 4.0, 10.0), 5.0);
    }

    #[test]
    fn capped_at_capacity() {
        assert_eq!(charge(9.0, 10.0, 2.0, 10.0), 10.0);
        // and floored at 0 with a draining rate
        assert_eq!(charge(1.0, 10.0, -2.0, 10.0), 0.0);
    }

    #[test]
    fn discharge_only_when_full() {
        assert_eq!(try_discharge(5.0, 10.0), None);
        assert_eq!(try_discharge(10.0, 10.0), Some(10.0));
        assert_eq!(try_discharge(11.0, 10.0), Some(11.0)); // at or above full
    }

    #[test]
    fn dt_zero_unchanged() {
        let mut seed = 0x0001_u32;
        for _ in 0..1000 {
            let cur = (lcg(&mut seed) % 1001) as f32 / 100.0; // [0,10]
            assert_eq!(charge(cur, 0.0, 5.0, 10.0), cur);
        }
    }

    #[test]
    fn capacity_zero_guard() {
        assert_eq!(charge(5.0, 1.0, 10.0, 0.0), 0.0);
        assert_eq!(charge(0.0, 1.0, 10.0, -5.0), 0.0);
        assert_eq!(try_discharge(5.0, 0.0), None);
        assert_eq!(try_discharge(0.0, 0.0), None);
    }

    #[test]
    fn charge_bounded_and_deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..4000 {
            let cur = (lcg(&mut seed) % 2001) as f32 / 100.0 - 5.0;
            let dt = (lcg(&mut seed) % 2001) as f32 / 100.0 - 5.0;
            let rate = (lcg(&mut seed) % 2001) as f32 / 100.0 - 10.0;
            let cap = (lcg(&mut seed) % 2001) as f32 / 100.0 - 5.0;
            let r = charge(cur, dt, rate, cap);
            let hi = if cap > 0.0 { cap } else { 0.0 };
            assert!(r >= 0.0 && r <= hi, "{} not in [0,{}]", r, hi);
            assert_eq!(r, charge(cur, dt, rate, cap));
        }
    }
}
