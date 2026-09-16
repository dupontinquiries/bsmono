//! `util::mathx` — the two helpers the extracted sim files use.
//!
//! Carried VERBATIM from `unendlich-engine/src/util/mathx.rs` rather than
//! reimplemented, and `saturate` in particular is not the obvious
//! `v.clamp(0.0, 1.0)`.
//!
//! The engine's own comment explains why, and it is load-bearing: `clamp`'s
//! NaN and signed-zero behaviour is not guaranteed identical across compiler
//! versions on one x86_64 target, let alone across the ARM and Apple Silicon
//! targets this fleet ships to, and a rollback-netcode sim needs bit-identical
//! `f32` results across machines. The ladder below is direct IEEE-754
//! comparison with no tie-break, deterministic by construction rather than by
//! measurement.
//!
//! Only the two functions the extracted files actually call are here. Copying
//! the whole module would be a bigger fork to keep in step for no benefit.

/// Linear interpolation: `a + (b - a) * t`. Exact IEEE form. `t` is *not*
/// clamped, so it extrapolates outside `[0,1]`.
#[inline]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Clamp to `[0, 1]`, NaN to 0. See the module header for why this is not
/// `clamp`.
#[inline]
pub fn saturate(v: f32) -> f32 {
    if v.is_nan() {
        0.0
    } else if v <= 0.0 {
        0.0
    } else if v >= 1.0 {
        1.0
    } else {
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The determinism ladder, including the cases `clamp` is not guaranteed
    /// to agree on across targets.
    #[test]
    fn saturate_pins_the_edges_and_nan() {
        assert_eq!(saturate(f32::NAN), 0.0);
        assert_eq!(saturate(-0.0), 0.0);
        assert_eq!(saturate(-1.0), 0.0);
        assert_eq!(saturate(0.0), 0.0);
        assert_eq!(saturate(0.5), 0.5);
        assert_eq!(saturate(1.0), 1.0);
        assert_eq!(saturate(2.0), 1.0);
        assert_eq!(saturate(f32::INFINITY), 1.0);
        assert_eq!(saturate(f32::NEG_INFINITY), 0.0);
    }

    /// Not clamped: terrain code relies on extrapolation.
    #[test]
    fn lerp_extrapolates_outside_the_unit_range() {
        assert_eq!(lerp(0.0, 10.0, 0.5), 5.0);
        assert_eq!(lerp(0.0, 10.0, 2.0), 20.0);
        assert_eq!(lerp(0.0, 10.0, -1.0), -10.0);
    }
}
