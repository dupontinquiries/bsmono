//! `sim::popdyn_step` — one step of logistic population dynamics, the CPU
//! reference a GPU field-kernel will mirror (see `GPU_COMPUTE_PLAN`).
//!
//! [`step`] advances a population by one tick of the logistic map: the intrinsic
//! growth rate is `birth - death`, modulated by how much headroom is left under
//! the `carrying` capacity. A population below carrying grows, one above it
//! shrinks, and one exactly at carrying stays put (equilibrium). Population can't
//! go negative, and an empty patch stays empty.
//!
//! Pure and deterministic: a single logistic update and a floor at 0 — no clock,
//! no randomness, no cross-imports. Kept deliberately simple so the GPU kernel
//! can match it exactly.

/// One logistic population step.
///
/// `new = pop + (birth - death)·pop·(1 - pop/carrying)`, floored at 0. A
/// non-positive `carrying` collapses the population to 0 (no environment).
pub fn step(pop: f32, birth: f32, death: f32, carrying: f32) -> f32 {
    if !(carrying > 0.0) {
        return 0.0;
    }
    let r = birth - death;
    let new = pop + r * pop * (1.0 - pop / carrying);
    if new > 0.0 {
        new
    } else {
        0.0
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
    fn grows_below_carrying() {
        // birth > death, pop below K → grows
        assert!(step(10.0, 0.5, 0.1, 100.0) > 10.0);
    }

    #[test]
    fn shrinks_above_carrying() {
        // pop above K → logistic term negative → shrinks
        let s = step(150.0, 0.5, 0.1, 100.0);
        assert!(s < 150.0, "should shrink, got {}", s);
    }

    #[test]
    fn equilibrium_at_carrying() {
        // exactly at carrying → no change
        assert_eq!(step(100.0, 0.5, 0.1, 100.0), 100.0);
        assert_eq!(step(50.0, 0.9, 0.2, 50.0), 50.0);
    }

    #[test]
    fn zero_population_stays_zero() {
        let mut seed = 0x0001_u32;
        for _ in 0..1000 {
            let birth = (lcg(&mut seed) % 1000) as f32 / 100.0;
            let death = (lcg(&mut seed) % 1000) as f32 / 100.0;
            let k = (lcg(&mut seed) % 1000 + 1) as f32;
            assert_eq!(step(0.0, birth, death, k), 0.0);
        }
    }

    #[test]
    fn bounded_nonnegative() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..5000 {
            let pop = (lcg(&mut seed) % 100_000) as f32 / 100.0;
            let birth = (lcg(&mut seed) % 1000) as f32 / 100.0;
            let death = (lcg(&mut seed) % 1000) as f32 / 100.0;
            let k = (lcg(&mut seed) % 100_001) as f32 / 100.0 - 100.0; // incl <= 0
            let s = step(pop, birth, death, k);
            assert!(s >= 0.0 && s.is_finite(), "pop {}", s);
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..3000 {
            let pop = (lcg(&mut seed) % 100_000) as f32 / 100.0;
            let birth = (lcg(&mut seed) % 1000) as f32 / 100.0;
            let death = (lcg(&mut seed) % 1000) as f32 / 100.0;
            let k = (lcg(&mut seed) % 1000 + 1) as f32;
            assert_eq!(step(pop, birth, death, k), step(pop, birth, death, k));
        }
    }
}
