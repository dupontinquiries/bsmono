//! `terrace` — quantize height into terraces with a blended edge.
//!
//! Worker-03 / WORLD pure core, round 16. Snaps a height to `step` multiples to
//! make plateau/terrace terrain (mesas, paddies). `blend` controls how much of
//! the top of each terrace ramps smoothly into the next: `0` gives hard steps,
//! `>0` gives a continuous smoothstep transition so the LOD mesher doesn't crack
//! at the risers.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Terrace `h` to `step` multiples. `blend ∈ [0,1]` is the fraction of each
/// terrace's top that ramps into the next level. `step ≤ 0` returns `h`.
pub fn terrace(h: f32, step: f32, blend: f32) -> f32 {
    if !(step > 0.0) {
        return h;
    }
    let q = (h / step).floor();
    let base = q * step;
    let frac = h - base; // [0, step)
    let b = clamp01(blend);
    if b <= 0.0 {
        return base; // hard steps
    }
    let blend_w = b * step;
    let flat_w = step - blend_w;
    if frac <= flat_w {
        base // flat part of the terrace
    } else {
        // Smoothstep ramp from this level up to the next over the blend zone.
        let t = (frac - flat_w) / blend_w; // [0,1]
        let s = t * t * (3.0 - 2.0 * t);
        base + s * step
    }
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_multiples_preserved() {
        for k in 0..10 {
            let h = k as f32 * 4.0;
            assert!((terrace(h, 4.0, 0.3) - h).abs() < 1e-5);
            assert!((terrace(h, 4.0, 0.0) - h).abs() < 1e-5);
        }
    }

    #[test]
    fn flattens_between_steps() {
        // Two heights inside the same flat zone map to the same terrace level.
        let a = terrace(10.1, 4.0, 0.25); // terrace [8,12), flat up to 8+3=11
        let b = terrace(10.9, 4.0, 0.25);
        assert_eq!(a, 8.0);
        assert_eq!(b, 8.0);
    }

    #[test]
    fn blend_zero_is_hard_steps() {
        assert_eq!(terrace(7.9, 4.0, 0.0), 4.0);
        assert_eq!(terrace(8.0, 4.0, 0.0), 8.0);
        // Discontinuous jump across the boundary.
        let lo = terrace(7.999, 4.0, 0.0);
        let hi = terrace(8.001, 4.0, 0.0);
        assert!((hi - lo).abs() > 3.0);
    }

    #[test]
    fn continuous_for_positive_blend() {
        // Sweep across a step boundary; output never jumps.
        let step = 4.0;
        let blend = 0.5;
        let mut prev = terrace(0.0, step, blend);
        let mut x = 0.0;
        while x <= 16.0 {
            let v = terrace(x, step, blend);
            assert!((v - prev).abs() < 0.2, "terrace jumped at h={x}");
            prev = v;
            x += 0.02;
        }
    }

    #[test]
    fn step_guard() {
        assert_eq!(terrace(5.5, 0.0, 0.5), 5.5);
        assert_eq!(terrace(5.5, -1.0, 0.5), 5.5);
    }

    #[test]
    fn deterministic() {
        assert_eq!(terrace(9.3, 4.0, 0.25), terrace(9.3, 4.0, 0.25));
    }
}
