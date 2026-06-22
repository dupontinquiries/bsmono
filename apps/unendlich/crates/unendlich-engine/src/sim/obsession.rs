//! `sim::obsession` — a creature's obsession gene biases what it spends its life
//! doing.
//!
//! [`drives`] maps an `obsession` gene (`0..1`) and a context vector to a
//! normalized weighting over five drives — hoard, hunt, build, territory, song.
//! The gene picks a dominant drive (its band of the `0..1` range), which gets a
//! large base weight; context can nudge any drive up; then everything is
//! normalized to sum to 1. So different obsessions yield different dominant
//! behaviours, context shifts the mix without breaking the distribution, and the
//! result is always a valid set of non-negative weights summing to ~1.
//!
//! Pure and deterministic: a band pick, additive context, and one
//! normalization — no clock, no randomness, no cross-imports.

/// The five drive slots (indices into the returned weights).
pub const DRIVE_COUNT: usize = 5;
const BASE: f32 = 0.1;
const DOMINANT_BUMP: f32 = 1.0;

fn clamp01(x: f32) -> f32 {
    let x = if x > 0.0 { x } else { 0.0 };
    if x < 1.0 {
        x
    } else {
        1.0
    }
}

/// Drive weights `[hoard, hunt, build, territory, song]` summing to ~1.
///
/// The dominant drive is `floor(obsession·5)`; `ctx[i]` (if present, floored at
/// 0) adds to drive `i` before normalization.
pub fn drives(obsession: f32, ctx: &[f32]) -> [f32; DRIVE_COUNT] {
    let band = ((clamp01(obsession) * DRIVE_COUNT as f32) as usize).min(DRIVE_COUNT - 1);

    let mut raw = [0.0f32; DRIVE_COUNT];
    let mut sum = 0.0f32;
    let mut i = 0;
    while i < DRIVE_COUNT {
        let bump = if i == band { DOMINANT_BUMP } else { 0.0 };
        let c = ctx.get(i).copied().unwrap_or(0.0);
        let cc = if c > 0.0 { c } else { 0.0 };
        raw[i] = BASE + bump + cc;
        sum += raw[i];
        i += 1;
    }

    let mut out = [0.0f32; DRIVE_COUNT];
    let mut j = 0;
    while j < DRIVE_COUNT {
        out[j] = raw[j] / sum; // sum >= DRIVE_COUNT*BASE > 0
        j += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn argmax(w: &[f32; DRIVE_COUNT]) -> usize {
        let mut best = 0;
        let mut i = 1;
        while i < DRIVE_COUNT {
            if w[i] > w[best] {
                best = i;
            }
            i += 1;
        }
        best
    }
    fn sum(w: &[f32; DRIVE_COUNT]) -> f32 {
        let mut s = 0.0;
        let mut i = 0;
        while i < DRIVE_COUNT {
            s += w[i];
            i += 1;
        }
        s
    }

    #[test]
    fn distinct_obsessions_distinct_dominant() {
        // one obsession per band → each band's drive dominates
        let d0 = argmax(&drives(0.1, &[]));
        let d1 = argmax(&drives(0.3, &[]));
        let d2 = argmax(&drives(0.5, &[]));
        let d3 = argmax(&drives(0.7, &[]));
        let d4 = argmax(&drives(0.9, &[]));
        assert_eq!([d0, d1, d2, d3, d4], [0, 1, 2, 3, 4]);
    }

    #[test]
    fn weights_sum_to_one() {
        let mut seed = 0x5111_u32;
        for _ in 0..3000 {
            let ob = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            let n = (lcg(&mut seed) % 7) as usize;
            let mut ctx = Vec::new();
            for _ in 0..n {
                ctx.push((lcg(&mut seed) % 100_001) as f32 / 50_000.0);
            }
            let w = drives(ob, &ctx);
            assert!((sum(&w) - 1.0).abs() < 1e-5, "sum {}", sum(&w));
        }
    }

    #[test]
    fn bounded_nonnegative() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..3000 {
            let ob = (lcg(&mut seed) % 300_001) as f32 / 100_000.0 - 1.0;
            let w = drives(ob, &[0.5, 0.2, 0.8, 0.1, 0.3]);
            let mut i = 0;
            while i < DRIVE_COUNT {
                assert!(w[i] >= 0.0 && w[i] <= 1.0, "weight {}", w[i]);
                i += 1;
            }
        }
    }

    #[test]
    fn ctx_shifts_within_bounds() {
        let no_ctx = drives(0.1, &[]); // hoard dominant
        let with_ctx = drives(0.1, &[0.0, 5.0, 0.0, 0.0, 0.0]); // big push on hunt
        assert_ne!(no_ctx, with_ctx, "ctx should change the mix");
        // still a valid distribution
        assert!((sum(&with_ctx) - 1.0).abs() < 1e-5);
        // and a strong enough context can flip the dominant drive
        assert_eq!(argmax(&with_ctx), 1);
    }

    #[test]
    fn stable_within_a_band() {
        // nearby obsessions in the same band keep the same dominant drive
        assert_eq!(argmax(&drives(0.12, &[])), argmax(&drives(0.14, &[])));
        assert_eq!(argmax(&drives(0.62, &[])), argmax(&drives(0.78, &[])));
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..3000 {
            let ob = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            let ctx = [
                (lcg(&mut seed) % 100) as f32 / 100.0,
                (lcg(&mut seed) % 100) as f32 / 100.0,
            ];
            assert_eq!(drives(ob, &ctx), drives(ob, &ctx));
        }
    }
}
