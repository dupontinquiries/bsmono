//! complex.rs — complex f64, shared.
//!
//! `sim-power` carries its own copy for the admittance algebra; that predates this module and
//! should migrate. Two spellings of complex multiplication in one workspace is exactly the kind of
//! duplication that lets a conjugate go wrong in one place and not the other.

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct C64 {
    pub re: f64,
    pub im: f64,
}

pub const I: C64 = C64 { re: 0.0, im: 1.0 };

#[inline]
pub fn c64(re: f64, im: f64) -> C64 {
    C64 { re, im }
}

impl C64 {
    #[inline]
    pub fn conj(self) -> C64 {
        C64 { re: self.re, im: -self.im }
    }
    #[inline]
    pub fn norm_sq(self) -> f64 {
        f64::mul_add(self.re, self.re, self.im * self.im)
    }
    #[inline]
    pub fn abs(self) -> f64 {
        self.re.hypot(self.im)
    }
    /// e^{i·θ}
    #[inline]
    pub fn expi(theta: f64) -> C64 {
        let (s, c) = theta.sin_cos();
        C64 { re: c, im: s }
    }
    #[inline]
    pub fn scale(self, s: f64) -> C64 {
        C64 { re: self.re * s, im: self.im * s }
    }
}

impl std::ops::Add for C64 {
    type Output = C64;
    #[inline]
    fn add(self, o: C64) -> C64 {
        C64 { re: self.re + o.re, im: self.im + o.im }
    }
}
impl std::ops::Sub for C64 {
    type Output = C64;
    #[inline]
    fn sub(self, o: C64) -> C64 {
        C64 { re: self.re - o.re, im: self.im - o.im }
    }
}
impl std::ops::Mul for C64 {
    type Output = C64;
    #[inline]
    fn mul(self, o: C64) -> C64 {
        C64 {
            re: f64::mul_add(self.re, o.re, -(self.im * o.im)),
            im: f64::mul_add(self.re, o.im, self.im * o.re),
        }
    }
}
impl std::ops::AddAssign for C64 {
    #[inline]
    fn add_assign(&mut self, o: C64) {
        *self = *self + o;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiplication_and_conjugation_are_standard() {
        assert_eq!(c64(3.0, 4.0) * c64(1.0, -2.0), c64(11.0, -2.0));
        assert_eq!(c64(3.0, 4.0).abs(), 5.0);
        assert_eq!(c64(3.0, 4.0).norm_sq(), 25.0);
        let z = C64::expi(std::f64::consts::PI);
        assert!((z.re + 1.0).abs() < 1e-15 && z.im.abs() < 1e-15);
    }
}
