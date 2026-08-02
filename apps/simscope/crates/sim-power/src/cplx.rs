//! cplx.rs — just enough complex arithmetic, written rather than imported.
//!
//! A dependency would be fine here; writing it is a deliberate 60 lines because the admittance
//! algebra is where power flow bugs live (a conjugate in the wrong place is a working program that
//! converges to the wrong answer), and having the operations in front of us makes the Ybus code
//! readable against a textbook.

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct C {
    pub re: f64,
    pub im: f64,
}

pub const ZERO: C = C { re: 0.0, im: 0.0 };

#[inline]
pub fn c(re: f64, im: f64) -> C {
    C { re, im }
}

impl C {
    #[inline]
    pub fn conj(self) -> C {
        C { re: self.re, im: -self.im }
    }
    #[inline]
    pub fn norm_sq(self) -> f64 {
        f64::mul_add(self.re, self.re, self.im * self.im)
    }
    #[inline]
    pub fn abs(self) -> f64 {
        self.re.hypot(self.im)
    }
    #[inline]
    pub fn inv(self) -> C {
        let d = self.norm_sq();
        C { re: self.re / d, im: -self.im / d }
    }
    /// e^{jθ}
    #[inline]
    pub fn from_polar(mag: f64, angle: f64) -> C {
        let (s, co) = angle.sin_cos();
        C { re: mag * co, im: mag * s }
    }
}

impl std::ops::Add for C {
    type Output = C;
    #[inline]
    fn add(self, o: C) -> C {
        C { re: self.re + o.re, im: self.im + o.im }
    }
}
impl std::ops::Sub for C {
    type Output = C;
    #[inline]
    fn sub(self, o: C) -> C {
        C { re: self.re - o.re, im: self.im - o.im }
    }
}
impl std::ops::Mul for C {
    type Output = C;
    #[inline]
    fn mul(self, o: C) -> C {
        // FMA on the real part keeps the cancellation in (ac - bd) one rounding cheaper.
        C {
            re: f64::mul_add(self.re, o.re, -(self.im * o.im)),
            im: f64::mul_add(self.re, o.im, self.im * o.re),
        }
    }
}
impl std::ops::Mul<f64> for C {
    type Output = C;
    #[inline]
    fn mul(self, s: f64) -> C {
        C { re: self.re * s, im: self.im * s }
    }
}
impl std::ops::Div for C {
    type Output = C;
    #[inline]
    fn div(self, o: C) -> C {
        self * o.inv()
    }
}
impl std::ops::AddAssign for C {
    #[inline]
    fn add_assign(&mut self, o: C) {
        *self = *self + o;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_matches_hand_computation() {
        let a = c(3.0, 4.0);
        let b = c(1.0, -2.0);
        assert_eq!(a + b, c(4.0, 2.0));
        assert_eq!(a - b, c(2.0, 6.0));
        // (3+4j)(1-2j) = 3 - 6j + 4j + 8 = 11 - 2j
        assert_eq!(a * b, c(11.0, -2.0));
        assert_eq!(a.conj(), c(3.0, -4.0));
        assert_eq!(a.abs(), 5.0);
        assert_eq!(a.norm_sq(), 25.0);
    }

    #[test]
    fn division_and_inverse_round_trip() {
        let a = c(0.01938, 0.05917); // a real line impedance from case14
        let y = a.inv();
        let back = y.inv();
        assert!((back.re - a.re).abs() < 1e-15 && (back.im - a.im).abs() < 1e-15);
        let q = (a * c(2.0, -3.0)) / a;
        assert!((q.re - 2.0).abs() < 1e-12 && (q.im + 3.0).abs() < 1e-12);
    }

    #[test]
    fn from_polar_is_the_unit_circle() {
        let t = std::f64::consts::FRAC_PI_3;
        let z = C::from_polar(1.0, t);
        assert!((z.abs() - 1.0).abs() < 1e-15);
        assert!((z.re - 0.5).abs() < 1e-15);
    }
}
