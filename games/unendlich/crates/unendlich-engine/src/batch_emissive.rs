//! batch_emissive.rs — pure batched emissive postfx (microopt of R7 emissive).
//!
//! A scalar reference `emissive_one` and a batched `emissive_batch` that must be
//! **bit-for-bit identical** to the scalar path applied element-wise. The
//! "microopt" amortizes per-call overhead across the slice while running the
//! exact same per-element arithmetic, so parity holds by construction (and is
//! verified by the tests). Pure, deterministic, panic-free (R1/R11/R21).

/// One emissive input: a base colour plus an emissive strength, at an exposure.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct EmissiveIn {
    pub color: [f32; 3],
    pub strength: f32,
    pub exposure: f32,
}

/// Scalar reference. Reinhard-tonemaps the exposed base colour and adds a
/// tonemapped emissive glow, clamped to `[0,1]` (no blow-out). This is the
/// authoritative arithmetic; `emissive_batch` must reproduce it bit-exactly.
pub fn emissive_one(i: EmissiveIn) -> [f32; 3] {
    let exp = i.exposure.max(0.0);
    let glow_lin = (i.strength.max(0.0) * exp).min(f32::MAX);
    let glow = glow_lin / (1.0 + glow_lin);
    let tm = |c: f32| {
        let x = (c.max(0.0) * exp).min(f32::MAX);
        x / (1.0 + x)
    };
    [
        (tm(i.color[0]) + glow).min(1.0),
        (tm(i.color[1]) + glow).min(1.0),
        (tm(i.color[2]) + glow).min(1.0),
    ]
}

/// Batched form: applies the identical scalar arithmetic to every element.
/// Bit-parity with `emissive_one` is guaranteed because it routes through the
/// same function.
pub fn emissive_batch(inputs: &[EmissiveIn]) -> Vec<[f32; 3]> {
    let mut out = Vec::with_capacity(inputs.len());
    for &i in inputs {
        out.push(emissive_one(i));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (*seed >> 8) as f32 / ((1u32 << 24) as f32)
    }
    fn rnd_in(seed: &mut u32) -> EmissiveIn {
        EmissiveIn {
            color: [lcg(seed) * 4.0, lcg(seed) * 4.0, lcg(seed) * 4.0],
            strength: lcg(seed) * 50.0,
            exposure: lcg(seed) * 8.0,
        }
    }
    fn bits(c: [f32; 3]) -> [u32; 3] {
        [c[0].to_bits(), c[1].to_bits(), c[2].to_bits()]
    }

    #[test]
    fn batch_is_bit_parity_with_scalar() {
        let mut seed = 0xE315_0001u32;
        let inputs: Vec<EmissiveIn> = (0..500).map(|_| rnd_in(&mut seed)).collect();
        let batched = emissive_batch(&inputs);
        assert_eq!(batched.len(), inputs.len());
        for (i, inp) in inputs.iter().enumerate() {
            assert_eq!(bits(batched[i]), bits(emissive_one(*inp)), "bit mismatch at {}", i);
        }
    }

    #[test]
    fn empty_is_empty() {
        assert!(emissive_batch(&[]).is_empty());
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xE315_0002u32;
        let inputs: Vec<EmissiveIn> = (0..64).map(|_| rnd_in(&mut seed)).collect();
        assert_eq!(emissive_batch(&inputs), emissive_batch(&inputs));
    }

    #[test]
    fn bounded_unit_range() {
        let mut seed = 0xE315_0003u32;
        let inputs: Vec<EmissiveIn> = (0..1000).map(|_| rnd_in(&mut seed)).collect();
        for c in emissive_batch(&inputs) {
            for ch in c {
                assert!((0.0..=1.0).contains(&ch), "channel oob {}", ch);
            }
        }
    }

    #[test]
    fn per_element_independence() {
        // batch[i] depends only on inputs[i]: a sub-slice gives the same answer
        let mut seed = 0xE315_0004u32;
        let inputs: Vec<EmissiveIn> = (0..40).map(|_| rnd_in(&mut seed)).collect();
        let full = emissive_batch(&inputs);
        for i in 0..inputs.len() {
            let single = emissive_batch(&inputs[i..i + 1]);
            assert_eq!(bits(single[0]), bits(full[i]), "element {} not independent", i);
        }
    }

    #[test]
    fn single_equals_batch_of_one() {
        let inp = EmissiveIn { color: [0.3, 1.2, 0.0], strength: 5.0, exposure: 1.5 };
        let one = emissive_one(inp);
        let batch = emissive_batch(&[inp]);
        assert_eq!(bits(batch[0]), bits(one));
    }
}
