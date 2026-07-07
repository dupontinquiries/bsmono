//! pipeline_key.rs — pure typed pipeline-key packer.
//!
//! The flagship render abstraction: pack a pipeline description
//! `(lod, pass, material, double_sided)` into a single checked `u64` key, with
//! `decode` and `is_valid`, so the LOD×pass×pipeline table is encode-checked
//! rather than stringly/looseley keyed. Pure, deterministic, panic-free
//! (R1/R11/R21).
//!
//! Bit layout (high → low): `lod(4) | pass(4) | material(8) | double_sided(1)`,
//! so sorting raw keys orders lod-major, then pass, then material.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PipelineDesc {
    pub lod: u8,
    pub pass: u8,
    pub material: u8,
    pub double_sided: bool,
}

/// Number of distinct LODs (lod must be `< MAX_LOD`).
pub const MAX_LOD: u8 = 16;
/// Number of render passes (pass must be `< NUM_PASSES`).
pub const NUM_PASSES: u8 = 8;

// field widths / shifts
const DS_BITS: u32 = 1;
const MAT_BITS: u32 = 8;
const PASS_BITS: u32 = 4;
const _LOD_BITS: u32 = 4;

const DS_SHIFT: u32 = 0;
const MAT_SHIFT: u32 = DS_SHIFT + DS_BITS; // 1
const PASS_SHIFT: u32 = MAT_SHIFT + MAT_BITS; // 9
const LOD_SHIFT: u32 = PASS_SHIFT + PASS_BITS; // 13

/// Whether a description is encodable: `lod < MAX_LOD` (4-bit field) and
/// `pass < NUM_PASSES` (a semantic limit, even though the field is 4 bits wide).
pub fn is_valid(d: PipelineDesc) -> bool {
    d.lod < MAX_LOD && d.pass < NUM_PASSES
}

/// Encode to a `u64` key, or `None` if the description is invalid.
pub fn encode(d: PipelineDesc) -> Option<u64> {
    if !is_valid(d) {
        return None;
    }
    let key = ((d.lod as u64) << LOD_SHIFT)
        | ((d.pass as u64) << PASS_SHIFT)
        | ((d.material as u64) << MAT_SHIFT)
        | ((d.double_sided as u64) << DS_SHIFT);
    Some(key)
}

/// Decode a key back into its fields. (Inverse of `encode` for valid inputs.)
pub fn decode(key: u64) -> PipelineDesc {
    let lod = ((key >> LOD_SHIFT) & 0xF) as u8;
    let pass = ((key >> PASS_SHIFT) & 0xF) as u8;
    let material = ((key >> MAT_SHIFT) & 0xFF) as u8;
    let double_sided = ((key >> DS_SHIFT) & 0x1) != 0;
    PipelineDesc { lod, pass, material, double_sided }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(lod: u8, pass: u8, material: u8, double_sided: bool) -> PipelineDesc {
        PipelineDesc { lod, pass, material, double_sided }
    }

    #[test]
    fn encode_decode_round_trip() {
        for lod in 0..MAX_LOD {
            for pass in 0..NUM_PASSES {
                for &mat in &[0u8, 1, 17, 200, 255] {
                    for &ds in &[false, true] {
                        let desc = d(lod, pass, mat, ds);
                        match encode(desc) {
                            Some(k) => assert_eq!(decode(k), desc, "round-trip failed"),
                            None => assert!(false, "valid desc didn't encode"),
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn distinct_inputs_distinct_keys() {
        let mut keys = std::collections::BTreeSet::new();
        for lod in 0..MAX_LOD {
            for pass in 0..NUM_PASSES {
                for mat in 0..=255u8 {
                    for &ds in &[false, true] {
                        if let Some(k) = encode(d(lod, pass, mat, ds)) {
                            assert!(keys.insert(k), "key collision");
                        }
                    }
                }
            }
        }
        // 16 * 8 * 256 * 2 distinct keys
        assert_eq!(keys.len(), 16 * 8 * 256 * 2);
    }

    #[test]
    fn invalid_combos_rejected() {
        assert!(encode(d(0, NUM_PASSES, 0, false)).is_none(), "pass >= NUM_PASSES");
        assert!(encode(d(0, 12, 0, false)).is_none(), "reserved pass");
        assert!(!is_valid(d(0, 9, 0, false)));
    }

    #[test]
    fn field_ranges_enforced() {
        assert!(encode(d(MAX_LOD, 0, 0, false)).is_none(), "lod == MAX_LOD");
        assert!(encode(d(20, 0, 0, false)).is_none(), "lod out of range");
        assert!(!is_valid(d(255, 0, 0, false)));
    }

    fn enc(desc: PipelineDesc) -> u64 {
        match encode(desc) {
            Some(k) => k,
            None => {
                assert!(false, "expected a valid desc");
                0
            }
        }
    }

    #[test]
    fn ordering_is_stable_lod_major() {
        // higher lod -> higher key, regardless of lower fields
        let lo = enc(d(1, 7, 255, true));
        let hi = enc(d(2, 0, 0, false));
        assert!(hi > lo, "lod is not the major sort field");
        // within same lod, pass dominates material
        let a = enc(d(3, 1, 255, true));
        let b = enc(d(3, 2, 0, false));
        assert!(b > a, "pass should dominate material");
    }

    #[test]
    fn deterministic() {
        assert_eq!(encode(d(2, 3, 99, true)), encode(d(2, 3, 99, true)));
    }
}
