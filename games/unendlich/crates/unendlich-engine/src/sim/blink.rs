//! `sim::blink` — the occasional eyelid blink that keeps a face from looking
//! dead-eyed.
//!
//! [`blink`] returns an eyelid value (`0` open .. `1` fully shut) for a creature
//! at time `t`, blinking at roughly `rate_hz` with per-creature timing taken
//! from `seed`. The eye is open almost all the time; each ~`1/rate_hz` window
//! holds one brief shut-and-open pulse, placed at a seed-jittered offset so two
//! creatures don't blink in lockstep. `rate_hz <= 0` means never.
//!
//! Presentation-tier and pure: the blink time is a hash of `(seed, interval)`,
//! the pulse is a half-sine — no clock, no RNG state, no cross-imports. The same
//! `(t, rate_hz, seed)` always gives the same eyelid value on every peer.

use std::f32::consts::PI;

/// How long one blink takes, in seconds (the shut→open pulse width).
const BLINK_DUR: f32 = 0.12;
const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;

/// SplitMix64 finalizer — pure scramble used only to place blinks.
fn mix64(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^= x >> 31;
    x
}

/// Deterministic `[0,1)` from a seed and an interval index.
fn hash01(seed: u64, n: u64) -> f32 {
    let h = mix64(seed ^ n.wrapping_mul(GOLDEN));
    (h >> 40) as f32 / (1u64 << 24) as f32
}

/// Eyelid value at time `t` for a creature blinking at `rate_hz` with timing
/// from `seed`. `0` = open, `1` = shut. Returns `0` for non-positive rate.
pub fn blink(t: f32, rate_hz: f32, seed: u64) -> f32 {
    if !(rate_hz > 0.0) {
        return 0.0;
    }
    let phase = t * rate_hz; // cycles elapsed
    let n = phase.floor();
    let frac = phase - n; // position within this blink interval, [0,1)

    // fraction of the interval occupied by the blink pulse
    let w = (BLINK_DUR * rate_hz).clamp(1e-4, 1.0);
    // seed-jittered start, kept so the whole pulse fits inside the interval
    let start = hash01(seed, n as i64 as u64) * (1.0 - w);

    if frac >= start && frac < start + w {
        let u = (frac - start) / w; // [0,1) through the pulse
        (PI * u).sin() // 0 → 1 → 0, fully shut at the midpoint
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mostly_open_over_time() {
        let rate = 0.3;
        let seed = 0xA1;
        let mut open = 0;
        let mut total = 0;
        let mut i = 0;
        while i < 20_000 {
            let t = i as f32 * 0.005; // 100 s of samples
            if blink(t, rate, seed) < 0.05 {
                open += 1;
            }
            total += 1;
            i += 1;
        }
        // eye open the large majority of the time
        assert!(open as f32 / total as f32 > 0.8, "open frac {}", open as f32 / total as f32);
    }

    #[test]
    fn blinks_actually_occur() {
        let mut maxv = 0.0f32;
        let mut i = 0;
        while i < 20_000 {
            let t = i as f32 * 0.005;
            let v = blink(t, 0.5, 0x55);
            if v > maxv {
                maxv = v;
            }
            i += 1;
        }
        assert!(maxv > 0.9, "never fully blinked, max {}", maxv);
    }

    #[test]
    fn rate_scales_frequency() {
        fn blink_fraction(rate: f32) -> f32 {
            let mut shut = 0;
            let mut total = 0;
            let mut i = 0;
            while i < 40_000 {
                let t = i as f32 * 0.005;
                if blink(t, rate, 0x7) > 0.5 {
                    shut += 1;
                }
                total += 1;
                i += 1;
            }
            shut as f32 / total as f32
        }
        assert!(blink_fraction(2.0) > blink_fraction(0.5), "higher rate should blink more");
    }

    #[test]
    fn bounded_unit_range() {
        let mut i = 0;
        while i < 20_000 {
            let t = i as f32 * 0.005;
            let v = blink(t, 1.5, 0xDEAD);
            assert!(v >= 0.0 && v <= 1.0, "eyelid {}", v);
            i += 1;
        }
    }

    #[test]
    fn deterministic_per_seed_and_seed_sensitive() {
        // same args → identical
        let mut i = 0;
        while i < 5_000 {
            let t = i as f32 * 0.01;
            assert_eq!(blink(t, 0.7, 0x1234), blink(t, 0.7, 0x1234));
            i += 1;
        }
        // different seeds → differ somewhere
        let mut differ = false;
        let mut k = 0;
        while k < 5_000 {
            let t = k as f32 * 0.01;
            if blink(t, 0.7, 0x1234) != blink(t, 0.7, 0x9999) {
                differ = true;
                break;
            }
            k += 1;
        }
        assert!(differ, "different seeds gave identical blink timing");
    }

    #[test]
    fn rate_zero_never_blinks() {
        let mut i = 0;
        while i < 5_000 {
            let t = i as f32 * 0.02;
            assert_eq!(blink(t, 0.0, 0x1), 0.0);
            assert_eq!(blink(t, -1.0, 0x1), 0.0);
            i += 1;
        }
    }
}
