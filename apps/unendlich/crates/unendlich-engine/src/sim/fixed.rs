//! Deterministic fixed-point arithmetic for the simulation layer.
//!
//! Rollback netcode requires that re-simulating the same inputs from the
//! same state produces a **bit-identical** result — not just on the same
//! machine (where IEEE floats are already deterministic) but across every
//! peer in a multiplayer session. Float determinism across machines is the
//! classic desync footgun: `sqrt`, transcendental functions, and FMA
//! contraction can all differ by a low bit between two builds or two CPUs,
//! and a single low-bit divergence snowballs into a full desync within a
//! few hundred ticks.
//!
//! So the simulation never touches `f32`/`f64`. Every networked quantity
//! is a [`Fixed`]: a signed 48.16 fixed-point number stored in an `i64`.
//! Floats appear only in the *presentation* layer (rendering, the camera,
//! interpolation), which reads sim state but never feeds back into it.
//!
//! Format: `value = raw / 2^FRAC_BITS`. With 16 fractional bits the
//! resolution is `1 / 65536 ≈ 0.0000153` blocks and the integer range is
//! `±2^47 ≈ 1.4e14` blocks — effectively unbounded for a voxel world.

/// Number of fractional bits in a [`Fixed`].
pub const FRAC_BITS: u32 = 16;
/// `1.0` in raw units.
const ONE_RAW: i64 = 1 << FRAC_BITS;

/// A deterministic 48.16 signed fixed-point number.
///
/// All arithmetic is integer-only and identical on any platform. `Add`/`Sub`
/// saturate rather than wrap or panic, so an out-of-range bug clamps
/// deterministically instead of differing between debug (panic) and release
/// (wrap). `Mul`/`Div` use an `i128` intermediate to avoid overflow in the
/// scaling step.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct Fixed(pub i64);

impl Fixed {
    /// `0`.
    pub const ZERO: Fixed = Fixed(0);
    /// `1`.
    pub const ONE: Fixed = Fixed(ONE_RAW);

    /// Construct from a whole number of blocks.
    #[inline]
    pub const fn from_int(i: i64) -> Fixed {
        Fixed(i << FRAC_BITS)
    }

    /// Construct from a rational `num / den`. Intended for compile-time
    /// constants (e.g. `from_frac(1, 60)` for one-sixtieth). Truncates
    /// toward zero, deterministically. Returns `ZERO` if `den == 0`.
    #[inline]
    pub const fn from_frac(num: i64, den: i64) -> Fixed {
        if den == 0 {
            Fixed(0)
        } else {
            Fixed((num << FRAC_BITS) / den)
        }
    }

    /// Construct from a float. **Presentation/test use only** — never call
    /// this from inside a simulation step, since the float input may itself
    /// be non-deterministic. Rounds to nearest.
    #[inline]
    pub fn from_f64(v: f64) -> Fixed {
        Fixed((v * ONE_RAW as f64).round() as i64)
    }

    /// The whole-block part, truncated toward negative infinity.
    #[inline]
    pub const fn floor_int(self) -> i64 {
        self.0 >> FRAC_BITS
    }

    /// Lossy conversion for presentation (rendering/interp).
    #[inline]
    pub fn to_f32(self) -> f32 {
        self.0 as f32 / ONE_RAW as f32
    }

    /// Lossy conversion for presentation.
    #[inline]
    pub fn to_f64(self) -> f64 {
        self.0 as f64 / ONE_RAW as f64
    }

    #[inline]
    pub const fn abs(self) -> Fixed {
        Fixed(self.0.abs())
    }

    #[inline]
    pub const fn min(self, other: Fixed) -> Fixed {
        if self.0 < other.0 { self } else { other }
    }

    #[inline]
    pub const fn max(self, other: Fixed) -> Fixed {
        if self.0 > other.0 { self } else { other }
    }

    #[inline]
    pub const fn clamp(self, lo: Fixed, hi: Fixed) -> Fixed {
        self.max(lo).min(hi)
    }

    /// `-1`, `0`, or `+1` as a `Fixed`.
    #[inline]
    pub const fn signum(self) -> Fixed {
        if self.0 > 0 {
            Fixed::ONE
        } else if self.0 < 0 {
            Fixed(-ONE_RAW)
        } else {
            Fixed::ZERO
        }
    }
}

impl std::ops::Add for Fixed {
    type Output = Fixed;
    #[inline]
    fn add(self, rhs: Fixed) -> Fixed {
        Fixed(self.0.saturating_add(rhs.0))
    }
}

impl std::ops::Sub for Fixed {
    type Output = Fixed;
    #[inline]
    fn sub(self, rhs: Fixed) -> Fixed {
        Fixed(self.0.saturating_sub(rhs.0))
    }
}

impl std::ops::Neg for Fixed {
    type Output = Fixed;
    #[inline]
    fn neg(self) -> Fixed {
        Fixed(self.0.saturating_neg())
    }
}

impl std::ops::Mul for Fixed {
    type Output = Fixed;
    #[inline]
    fn mul(self, rhs: Fixed) -> Fixed {
        // Multiply in i128, then shift the product back down by FRAC_BITS.
        // Arithmetic right shift rounds toward negative infinity, which is
        // deterministic across platforms.
        let p = (self.0 as i128 * rhs.0 as i128) >> FRAC_BITS;
        Fixed(p as i64)
    }
}

impl std::ops::Div for Fixed {
    type Output = Fixed;
    #[inline]
    fn div(self, rhs: Fixed) -> Fixed {
        if rhs.0 == 0 {
            // Deterministic, non-panicking sentinel for divide-by-zero.
            return Fixed(0);
        }
        let n = (self.0 as i128) << FRAC_BITS;
        Fixed((n / rhs.0 as i128) as i64)
    }
}

impl std::ops::AddAssign for Fixed {
    #[inline]
    fn add_assign(&mut self, rhs: Fixed) {
        *self = *self + rhs;
    }
}

impl std::ops::SubAssign for Fixed {
    #[inline]
    fn sub_assign(&mut self, rhs: Fixed) {
        *self = *self - rhs;
    }
}

/// A deterministic fixed-point 3-vector (world axes: x east, y up, z south).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Vec3 {
    pub x: Fixed,
    pub y: Fixed,
    pub z: Fixed,
}

impl Vec3 {
    pub const ZERO: Vec3 = Vec3 { x: Fixed::ZERO, y: Fixed::ZERO, z: Fixed::ZERO };

    #[inline]
    pub const fn new(x: Fixed, y: Fixed, z: Fixed) -> Vec3 {
        Vec3 { x, y, z }
    }

    /// Multiply every component by a scalar.
    #[inline]
    pub fn scale(self, s: Fixed) -> Vec3 {
        Vec3 { x: self.x * s, y: self.y * s, z: self.z * s }
    }
}

impl std::ops::Add for Vec3 {
    type Output = Vec3;
    #[inline]
    fn add(self, rhs: Vec3) -> Vec3 {
        Vec3 { x: self.x + rhs.x, y: self.y + rhs.y, z: self.z + rhs.z }
    }
}

impl std::ops::Sub for Vec3 {
    type Output = Vec3;
    #[inline]
    fn sub(self, rhs: Vec3) -> Vec3 {
        Vec3 { x: self.x - rhs.x, y: self.y - rhs.y, z: self.z - rhs.z }
    }
}

impl std::ops::AddAssign for Vec3 {
    #[inline]
    fn add_assign(&mut self, rhs: Vec3) {
        *self = *self + rhs;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_round_trip() {
        for i in [-1000i64, -1, 0, 1, 32, 1000, 1 << 30] {
            assert_eq!(Fixed::from_int(i).floor_int(), i);
        }
    }

    #[test]
    fn add_sub_are_exact_on_integers() {
        let a = Fixed::from_int(7);
        let b = Fixed::from_int(5);
        assert_eq!((a + b), Fixed::from_int(12));
        assert_eq!((a - b), Fixed::from_int(2));
        assert_eq!((b - a), Fixed::from_int(-2));
    }

    #[test]
    fn mul_div_round_trip_for_clean_values() {
        let a = Fixed::from_int(6);
        let b = Fixed::from_int(3);
        assert_eq!(a * b, Fixed::from_int(18));
        assert_eq!(a / b, Fixed::from_int(2));
        // Half:
        let half = Fixed::from_frac(1, 2);
        assert_eq!(a * half, Fixed::from_int(3));
    }

    #[test]
    fn from_frac_matches_expected_raw() {
        // 1/60 in raw 16-bit fractional units = 65536/60 = 1092 (trunc).
        assert_eq!(Fixed::from_frac(1, 60).0, 65536 / 60);
        assert_eq!(Fixed::from_frac(5, 60).0, (5 * 65536) / 60);
    }

    #[test]
    fn div_by_zero_is_zero_not_panic() {
        assert_eq!(Fixed::from_int(5) / Fixed::ZERO, Fixed::ZERO);
    }

    #[test]
    fn ordering_and_clamp() {
        let lo = Fixed::from_int(-2);
        let hi = Fixed::from_int(2);
        assert_eq!(Fixed::from_int(5).clamp(lo, hi), hi);
        assert_eq!(Fixed::from_int(-5).clamp(lo, hi), lo);
        assert_eq!(Fixed::from_int(1).clamp(lo, hi), Fixed::from_int(1));
    }

    #[test]
    fn signum_is_three_valued() {
        assert_eq!(Fixed::from_int(9).signum(), Fixed::ONE);
        assert_eq!(Fixed::from_int(-9).signum(), Fixed(-(1 << FRAC_BITS)));
        assert_eq!(Fixed::ZERO.signum(), Fixed::ZERO);
    }

    #[test]
    fn vec3_arithmetic_is_componentwise() {
        let a = Vec3::new(Fixed::from_int(1), Fixed::from_int(2), Fixed::from_int(3));
        let b = Vec3::new(Fixed::from_int(4), Fixed::from_int(5), Fixed::from_int(6));
        assert_eq!(a + b, Vec3::new(Fixed::from_int(5), Fixed::from_int(7), Fixed::from_int(9)));
        assert_eq!(b - a, Vec3::new(Fixed::from_int(3), Fixed::from_int(3), Fixed::from_int(3)));
        assert_eq!(a.scale(Fixed::from_int(2)), Vec3::new(Fixed::from_int(2), Fixed::from_int(4), Fixed::from_int(6)));
    }
}
