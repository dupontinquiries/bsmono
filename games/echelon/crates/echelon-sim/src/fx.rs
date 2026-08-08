//! Q32.32 fixed point. i64 raw with 32 fractional bits; i128 intermediates.

pub const FRAC: u32 = 32;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Debug)]
pub struct Fx(pub i64);

pub const ZERO: Fx = Fx(0);
pub const ONE: Fx = Fx(1i64 << FRAC);
pub const HALF: Fx = Fx(1i64 << (FRAC - 1));

impl Fx {
    pub fn from_int(v: i32) -> Fx {
        Fx((v as i64) << FRAC)
    }
    /// v is in milli-units: from_milli(2500) == 2.5
    pub fn from_milli(v: i32) -> Fx {
        Fx((((v as i128) << FRAC) / 1000) as i64)
    }
    pub fn to_int_floor(self) -> i32 {
        (self.0 >> FRAC) as i32
    }
    /// display-only helper for the runner (never used inside the sim)
    pub fn to_milli(self) -> i64 {
        ((self.0 as i128 * 1000) >> FRAC) as i64
    }
    pub fn add(self, o: Fx) -> Fx {
        Fx(self.0 + o.0)
    }
    pub fn sub(self, o: Fx) -> Fx {
        Fx(self.0 - o.0)
    }
    pub fn mul(self, o: Fx) -> Fx {
        Fx(((self.0 as i128 * o.0 as i128) >> FRAC) as i64)
    }
    pub fn div(self, o: Fx) -> Fx {
        Fx((((self.0 as i128) << FRAC) / o.0 as i128) as i64)
    }
    pub fn mul_int(self, v: i32) -> Fx {
        Fx(self.0 * v as i64)
    }
    pub fn div_int(self, v: i32) -> Fx {
        Fx(self.0 / v as i64)
    }
    pub fn min(self, o: Fx) -> Fx {
        if self.0 < o.0 {
            self
        } else {
            o
        }
    }
    pub fn max(self, o: Fx) -> Fx {
        if self.0 > o.0 {
            self
        } else {
            o
        }
    }
    pub fn clamp(self, lo: Fx, hi: Fx) -> Fx {
        self.max(lo).min(hi)
    }
    pub fn is_pos(self) -> bool {
        self.0 > 0
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct V2 {
    pub x: Fx,
    pub y: Fx,
}

impl V2 {
    pub fn new(x: Fx, y: Fx) -> V2 {
        V2 { x, y }
    }
    pub fn from_tile(tx: i32, ty: i32) -> V2 {
        V2 {
            x: Fx::from_int(tx).add(HALF),
            y: Fx::from_int(ty).add(HALF),
        }
    }
    pub fn from_milli(mx: i32, my: i32) -> V2 {
        V2 {
            x: Fx::from_milli(mx),
            y: Fx::from_milli(my),
        }
    }
    pub fn tile(self) -> (i32, i32) {
        (self.x.to_int_floor(), self.y.to_int_floor())
    }
}

/// squared distance in raw^2 units (i128 to avoid overflow)
pub fn dist2(a: V2, b: V2) -> i128 {
    let dx = (a.x.0 - b.x.0) as i128;
    let dy = (a.y.0 - b.y.0) as i128;
    dx * dx + dy * dy
}

pub fn range2(r: Fx) -> i128 {
    let r = r.0 as i128;
    r * r
}

/// integer sqrt of a non-negative i128, result fits raw Fx distance
pub fn isqrt(v: i128) -> i64 {
    if v <= 0 {
        return 0;
    }
    let mut x = v as u128;
    let mut r: u128 = 0;
    let mut bit: u128 = 1u128 << 126;
    while bit > x {
        bit >>= 2;
    }
    while bit != 0 {
        if x >= r + bit {
            x -= r + bit;
            r = (r >> 1) + bit;
        } else {
            r >>= 1;
        }
        bit >>= 2;
    }
    r as i64
}

/// step `from` towards `to` by at most `step` (Fx distance); returns new pos
/// and whether the destination was reached.
pub fn step_towards(from: V2, to: V2, step: Fx) -> (V2, bool) {
    let d2 = dist2(from, to);
    let s2 = range2(step);
    if d2 <= s2 {
        return (to, true);
    }
    let len = isqrt(d2); // raw units
    let dx = (to.x.0 - from.x.0) as i128;
    let dy = (to.y.0 - from.y.0) as i128;
    let nx = from.x.0 + ((dx * step.0 as i128) / len as i128) as i64;
    let ny = from.y.0 + ((dy * step.0 as i128) / len as i128) as i64;
    (V2::new(Fx(nx), Fx(ny)), false)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn basics() {
        let a = Fx::from_milli(2500);
        let b = Fx::from_int(2);
        assert_eq!(a.mul(b).to_milli(), 5000);
        assert_eq!(Fx::from_int(9).div(Fx::from_int(2)).to_milli(), 4500);
        assert_eq!(isqrt(range2(Fx::from_int(5))), Fx::from_int(5).0);
    }
    #[test]
    fn stepping() {
        let a = V2::from_tile(0, 0);
        let b = V2::from_tile(3, 4); // dist 5
        let (p, done) = step_towards(a, b, Fx::from_int(1));
        assert!(!done);
        // moved ~1 tile along the 3-4-5 direction (0.5 + 0.6, floor rounding)
        let cm = p.x.to_milli() / 10;
        assert!((108..=110).contains(&cm), "cm = {}", cm);
        let (_, done2) = step_towards(a, b, Fx::from_int(6));
        assert!(done2);
    }
}
