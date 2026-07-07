//! `sim::ability_aura` — area-of-effect intensity for a cast ability (pairs with
//! `ability_cast`, round 21).
//!
//! [`aura`] returns how strongly an ability's aura is felt at a given distance:
//! `1` at the centre, falling to `0` at the edge of `radius` and staying `0`
//! beyond it. The `falloff` selector chooses the shape of that fade — linear,
//! quadratic, smoothstep, or a hard cutoff — so different abilities can feel
//! sharp or soft. Output is always in `0..1` and never increases with distance.
//!
//! Pure and deterministic: a normalized distance and one curve — no clock, no
//! randomness, no cross-imports. A GPU kernel evaluating per-cell aura mirrors
//! this exactly.

/// Linear falloff.
pub const FALLOFF_LINEAR: u8 = 0;
/// Quadratic (ease-out) falloff.
pub const FALLOFF_QUADRATIC: u8 = 1;
/// Smoothstep falloff.
pub const FALLOFF_SMOOTH: u8 = 2;
/// Hard cutoff (full inside the radius, nothing outside).
pub const FALLOFF_HARD: u8 = 3;

fn clamp01(x: f32) -> f32 {
    let x = if x > 0.0 { x } else { 0.0 };
    if x < 1.0 {
        x
    } else {
        1.0
    }
}

/// Aura intensity (`0..1`) at `dist` for an aura of `radius`, shaped by
/// `falloff`.
///
/// `1` at the centre, `0` at/beyond `radius`. Unknown falloff selectors fall back
/// to linear. A non-positive radius is a point source (1 only at `dist <= 0`).
pub fn aura(dist: f32, radius: f32, falloff: u8) -> f32 {
    if !(radius > 0.0) {
        return if dist <= 0.0 { 1.0 } else { 0.0 };
    }
    let t = clamp01(dist / radius); // 0 at center → 1 at edge
    let x = 1.0 - t; // 1 at center → 0 at edge
    match falloff {
        FALLOFF_QUADRATIC => x * x,
        FALLOFF_SMOOTH => x * x * (3.0 - 2.0 * x),
        FALLOFF_HARD => {
            if t < 1.0 {
                1.0
            } else {
                0.0
            }
        }
        _ => x, // linear (and fallback)
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
    fn center_is_one() {
        for f in [FALLOFF_LINEAR, FALLOFF_QUADRATIC, FALLOFF_SMOOTH, FALLOFF_HARD] {
            assert!((aura(0.0, 10.0, f) - 1.0).abs() < 1e-6, "mode {} center", f);
        }
    }

    #[test]
    fn past_radius_is_zero() {
        for f in [FALLOFF_LINEAR, FALLOFF_QUADRATIC, FALLOFF_SMOOTH, FALLOFF_HARD] {
            assert_eq!(aura(10.0, 10.0, f), 0.0, "mode {} edge", f);
            assert_eq!(aura(50.0, 10.0, f), 0.0, "mode {} beyond", f);
        }
    }

    #[test]
    fn falloff_modes_differ() {
        // at half radius the modes give different intensities
        let lin = aura(5.0, 10.0, FALLOFF_LINEAR);
        let quad = aura(5.0, 10.0, FALLOFF_QUADRATIC);
        let hard = aura(5.0, 10.0, FALLOFF_HARD);
        assert!(lin != quad, "linear {} == quad {}", lin, quad);
        assert!(hard != lin, "hard {} == linear {}", hard, lin);
    }

    #[test]
    fn bounded_unit_range() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..5000 {
            let dist = (lcg(&mut seed) % 200_001) as f32 / 1000.0 - 100.0;
            let radius = (lcg(&mut seed) % 200_001) as f32 / 1000.0 - 50.0; // incl <=0
            let f = (lcg(&mut seed) & 0xFF) as u8;
            let v = aura(dist, radius, f);
            assert!(v >= 0.0 && v <= 1.0, "aura {}", v);
        }
    }

    #[test]
    fn monotone_non_increasing_in_distance() {
        for f in [FALLOFF_LINEAR, FALLOFF_QUADRATIC, FALLOFF_SMOOTH, FALLOFF_HARD] {
            let mut prev = aura(0.0, 10.0, f);
            let mut i = 1;
            while i <= 200 {
                let d = i as f32 / 10.0; // 0 .. 20
                let cur = aura(d, 10.0, f);
                assert!(cur <= prev + 1e-6, "mode {} rose at d {}", f, d);
                prev = cur;
                i += 1;
            }
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..3000 {
            let dist = (lcg(&mut seed) % 100_001) as f32 / 1000.0;
            let radius = (lcg(&mut seed) % 100_001) as f32 / 1000.0 + 0.1;
            let f = (lcg(&mut seed) & 0xFF) as u8;
            assert_eq!(aura(dist, radius, f), aura(dist, radius, f));
        }
    }
}
