//! batch_pipeline_key.rs — pure batched pipeline-key encoding (microopt of R13).
//!
//! A scalar reference `pipeline_key_one` (mirroring R13 `pipeline_key`'s packing
//! and validity rules) and a batched `pipeline_key_batch` that is **bit-for-bit
//! identical** to the scalar path applied per element — including the `None` for
//! invalid descriptions. The batch amortizes the output allocation across the
//! slice while routing each element through the scalar encoder, so parity holds
//! by construction. Pure, deterministic, panic-free (R1/R11/R21).
//!
//! Perf note: building thousands of pipeline keys per frame, the win is one
//! `Vec::with_capacity` + a tight loop over a contiguous slice (predictable,
//! vectorizable bounds, no per-call Vec growth), not a change to the per-element
//! math — which must stay identical so the keys still match the GPU table.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Desc {
    pub lod: u8,
    pub pass: u8,
    pub material: u8,
    pub double_sided: bool,
}

const MAX_LOD: u8 = 16;
const NUM_PASSES: u8 = 8;

const DS_SHIFT: u32 = 0;
const MAT_SHIFT: u32 = 1;
const PASS_SHIFT: u32 = 9;
const LOD_SHIFT: u32 = 13;

fn is_valid(d: Desc) -> bool {
    d.lod < MAX_LOD && d.pass < NUM_PASSES
}

/// Scalar reference: encode one descriptor, or `None` if invalid.
pub fn pipeline_key_one(d: Desc) -> Option<u64> {
    if !is_valid(d) {
        return None;
    }
    Some(
        ((d.lod as u64) << LOD_SHIFT)
            | ((d.pass as u64) << PASS_SHIFT)
            | ((d.material as u64) << MAT_SHIFT)
            | ((d.double_sided as u64) << DS_SHIFT),
    )
}

/// Batched form: one `Option<u64>` per input, each equal to `pipeline_key_one`
/// of that input (bit-parity, invalids preserved as `None`).
pub fn pipeline_key_batch(descs: &[Desc]) -> Vec<Option<u64>> {
    let mut out = Vec::with_capacity(descs.len());
    for &d in descs {
        out.push(pipeline_key_one(d));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(lod: u8, pass: u8, material: u8, double_sided: bool) -> Desc {
        Desc { lod, pass, material, double_sided }
    }

    fn sample(seed: &mut u32) -> Desc {
        let mut next = || {
            *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (*seed >> 8) as u32
        };
        d((next() % 20) as u8, (next() % 12) as u8, (next() % 256) as u8, next() % 2 == 0)
    }

    #[test]
    fn batch_is_bit_parity_with_scalar() {
        let mut seed = 0xB7C0_0001u32;
        let descs: Vec<Desc> = (0..1000).map(|_| sample(&mut seed)).collect();
        let batched = pipeline_key_batch(&descs);
        assert_eq!(batched.len(), descs.len());
        for (i, &dd) in descs.iter().enumerate() {
            assert_eq!(batched[i], pipeline_key_one(dd), "mismatch at {}", i);
        }
    }

    #[test]
    fn empty_is_empty() {
        assert!(pipeline_key_batch(&[]).is_empty());
    }

    #[test]
    fn single_equals_batch_of_one() {
        let one = pipeline_key_one(d(2, 3, 99, true));
        assert_eq!(pipeline_key_batch(&[d(2, 3, 99, true)])[0], one);
    }

    #[test]
    fn invalid_inputs_handled() {
        let descs = [d(0, 0, 0, false), d(20, 0, 0, false), d(0, 9, 0, true)];
        let out = pipeline_key_batch(&descs);
        assert!(out[0].is_some(), "valid should encode");
        assert!(out[1].is_none(), "lod out of range -> None");
        assert!(out[2].is_none(), "pass out of range -> None");
        // and still bit-parity with the scalar on the invalids
        for (i, &dd) in descs.iter().enumerate() {
            assert_eq!(out[i], pipeline_key_one(dd));
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xB7C0_0002u32;
        let descs: Vec<Desc> = (0..64).map(|_| sample(&mut seed)).collect();
        assert_eq!(pipeline_key_batch(&descs), pipeline_key_batch(&descs));
    }

    #[test]
    fn distinct_valid_descs_distinct_keys() {
        // sanity: the packing is injective for valid inputs
        let a = pipeline_key_one(d(1, 2, 3, false));
        let b = pipeline_key_one(d(1, 2, 3, true));
        assert!(a != b && a.is_some() && b.is_some(), "double_sided bit must matter");
    }
}
