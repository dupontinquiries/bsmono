//! Perf: batched engage standoff distance, bit-identical to the scalar reference.
//! Mirrors `engage_distance::standoff` (more distance when vitals are exposed or
//! the threat is high; floored at melee reach — handoff/COMBAT_STANCE.md), run
//! over a slice of mobs in one call. Same shape as `batch_guard_side`.
//!
//! `standoff_one` is a verbatim copy of the scalar core, so each element is
//! computed by identical arithmetic and the batch is bit-for-bit equal to the
//! per-mob scalar path. Pure: logic in, data out — no RNG/IO/clock, no
//! `.unwrap()`/`.expect()`.

/// Verbatim mirror of `sim::engage_distance::standoff`.
pub fn standoff_one(vital_exposure: f32, threat: f32, base: f32, melee: f32) -> f32 {
    let e = vital_exposure.clamp(0.0, 1.0);
    let t = threat.clamp(0.0, 1.0);
    let d = base.max(0.0) * (1.0 + e + 0.5 * t);
    d.max(melee.max(0.0))
}

/// Batched standoff over `(vital_exposure, threat, base, melee)` tuples.
/// Bit-identical to calling `standoff_one` per element.
pub fn standoff_batch(items: &[(f32, f32, f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(e, t, b, m)| standoff_one(e, t, b, m)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (*seed >> 8) as f32 / ((1u32 << 24) as f32)
    }

    #[test]
    fn parity_random() {
        let mut s = 0x0B2E_0031u32; // documented seed (R20): "batch engage", round 31
        let mut items = Vec::new();
        for _ in 0..400 {
            // mix in/out of range so clamps + floor are exercised
            items.push((
                lcg(&mut s) * 1.4 - 0.2, // vital_exposure (some out of [0,1])
                lcg(&mut s) * 1.4 - 0.2, // threat
                lcg(&mut s) * 30.0 - 5.0, // base (some negative)
                lcg(&mut s) * 12.0 - 2.0, // melee (some negative)
            ));
        }
        let b = standoff_batch(&items);
        assert_eq!(b.len(), items.len());
        for (i, &(e, t, ba, m)) in items.iter().enumerate() {
            assert_eq!(b[i].to_bits(), standoff_one(e, t, ba, m).to_bits(), "bit drift at {i}");
        }
    }

    #[test]
    fn empty_is_empty() {
        assert!(standoff_batch(&[]).is_empty());
    }

    #[test]
    fn single_eq_batch1() {
        let one = standoff_one(0.4, 0.6, 8.0, 2.0);
        assert_eq!(standoff_batch(&[(0.4, 0.6, 8.0, 2.0)])[0].to_bits(), one.to_bits());
    }

    #[test]
    fn floored_at_melee() {
        // tiny base ⇒ result floored to melee reach, element-for-element.
        let items = [(0.0f32, 0.0, 1.0, 9.0), (0.0, 0.0, 0.5, 4.0)];
        let b = standoff_batch(&items);
        assert_eq!(b, vec![9.0, 4.0]);
    }

    #[test]
    fn values_finite_and_nonneg() {
        let b = standoff_batch(&[(0.5, 0.5, 10.0, 2.0), (1.5, -1.0, -5.0, -3.0)]);
        for v in b {
            assert!(v.is_finite() && v >= 0.0);
        }
    }

    #[test]
    fn deterministic() {
        let items = [(0.3f32, 0.7, 12.0, 1.0), (0.9, 0.1, 5.0, 6.0)];
        assert_eq!(standoff_batch(&items), standoff_batch(&items));
    }
}
