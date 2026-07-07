//! `sim::batch_body_arc` — batched body-arc offsets, bit-identical to the scalar reference.
//!
//! Mirrors [`crate::sim::body_arc::arc`] in the house `batch_*` style: takes a
//! slice of `(turn, n_segments, size)` inputs and produces the concatenated
//! per-segment yaw values for every entity into a single flat `Vec<f32>`,
//! alongside a parallel `Vec<usize>` of segment counts so callers can slice out
//! each entity's run.
//!
//! # Bit-identity guarantee
//!
//! For every input `(turn, n, size)`, the segment values written by
//! [`arc_batch`] are bit-for-bit identical to calling [`body_arc::arc`]
//! directly. This is guaranteed by the property tests and by the fact that
//! [`arc_batch`] delegates each entity's computation to the same `arc_one`
//! function body (inlined from `body_arc`).
//!
//! # Output layout
//!
//! ```text
//! values:  [seg0_e0, seg1_e0, …, seg0_e1, seg1_e1, …, …]
//! counts:  [n0, n1, n2, …]
//! ```
//!
//! The run for entity `i` starts at `counts[0] + … + counts[i-1]` and has
//! length `counts[i]`. An empty input produces two empty `Vec`s.

use crate::sim::body_arc::arc;

/// Compute body-arc offsets for a batch of entities.
///
/// `inputs` is a slice of `(turn_radians, n_segments, size)` — one entry per
/// entity. Returns `(values, counts)` where `values` is the flat concatenation
/// of each entity's segment yaws (head-first) and `counts[i]` is the number of
/// segments for entity `i` (i.e. its `n_segments`, which may be 0).
///
/// Bit-identical to calling [`crate::sim::body_arc::arc`] for each entity.
pub fn arc_batch(inputs: &[(f32, usize, f32)]) -> (Vec<f32>, Vec<usize>) {
    let total: usize = inputs.iter().map(|&(_, n, _)| n).sum();
    let mut values = Vec::with_capacity(total);
    let mut counts = Vec::with_capacity(inputs.len());

    for &(turn, n, size) in inputs {
        let segs = arc(turn, n, size);
        counts.push(segs.len());
        values.extend_from_slice(&segs);
    }

    (values, counts)
}

/// Convenience: write results into a caller-provided output buffer.
///
/// Appends segment values for each entity to `out_values` and each count to
/// `out_counts`. Useful when the caller owns a pre-allocated buffer and wants
/// to avoid an extra allocation.
pub fn arc_batch_into(
    inputs: &[(f32, usize, f32)],
    out_values: &mut Vec<f32>,
    out_counts: &mut Vec<usize>,
) {
    out_values.reserve(inputs.iter().map(|&(_, n, _)| n).sum());
    out_counts.reserve(inputs.len());

    for &(turn, n, size) in inputs {
        let segs = arc(turn, n, size);
        out_counts.push(segs.len());
        out_values.extend_from_slice(&segs);
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::body_arc::arc;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    // ── Bit-identity property ──

    #[test]
    fn batch_matches_scalar_bit_for_bit() {
        let mut seed = 0xBA7C_u32;
        for _ in 0..500 {
            let n_entities = (lcg(&mut seed) % 12) as usize;
            let mut inputs: Vec<(f32, usize, f32)> = Vec::new();
            for _ in 0..n_entities {
                let turn = (lcg(&mut seed) % 200_001) as f32 / 100_000.0 - 1.0;
                let n    = (lcg(&mut seed) % 12) as usize;
                let size = (lcg(&mut seed) % 100_001) as f32 / 10_000.0;
                inputs.push((turn, n, size));
            }

            let (values, counts) = arc_batch(&inputs);

            // Reconstruct per-entity and compare bit-for-bit
            let mut offset = 0;
            for (i, &(turn, n, size)) in inputs.iter().enumerate() {
                let expected = arc(turn, n, size);
                let got = &values[offset..offset + counts[i]];
                assert_eq!(got.len(), expected.len(),
                    "entity {i}: length mismatch");
                for (j, (&g, &e)) in got.iter().zip(expected.iter()).enumerate() {
                    assert_eq!(g.to_bits(), e.to_bits(),
                        "entity {i} segment {j}: batch={g} scalar={e}");
                }
                offset += counts[i];
            }
            // All values accounted for
            assert_eq!(offset, values.len());
            assert_eq!(counts.len(), inputs.len());
        }
    }

    // ── Edge cases ──

    #[test]
    fn empty_input_gives_empty_output() {
        let (values, counts) = arc_batch(&[]);
        assert!(values.is_empty());
        assert!(counts.is_empty());
    }

    #[test]
    fn entity_with_zero_segments_contributes_zero_values() {
        let inputs = [(0.5, 0usize, 2.0), (1.0, 3usize, 1.0)];
        let (values, counts) = arc_batch(&inputs);
        assert_eq!(counts[0], 0);
        assert_eq!(counts[1], 3);
        assert_eq!(values.len(), 3);
    }

    #[test]
    fn single_entity_matches_direct_call() {
        let (turn, n, size) = (0.8f32, 5usize, 2.5f32);
        let (values, counts) = arc_batch(&[(turn, n, size)]);
        let expected = arc(turn, n, size);
        assert_eq!(counts, vec![n]);
        assert_eq!(values.len(), expected.len());
        for (&g, &e) in values.iter().zip(expected.iter()) {
            assert_eq!(g.to_bits(), e.to_bits());
        }
    }

    #[test]
    fn single_segment_entity_is_trivial() {
        let (values, counts) = arc_batch(&[(0.7, 1, 5.0)]);
        assert_eq!(counts, vec![1]);
        assert_eq!(values[0].to_bits(), 0.7f32.to_bits());
    }

    // ── arc_batch_into parity ──

    #[test]
    fn into_variant_matches_allocating_variant() {
        let mut seed = 0xC0FF_u32;
        for _ in 0..200 {
            let n_entities = (lcg(&mut seed) % 8) as usize;
            let mut inputs: Vec<(f32, usize, f32)> = Vec::new();
            for _ in 0..n_entities {
                let turn = (lcg(&mut seed) % 200_001) as f32 / 100_000.0 - 1.0;
                let n    = (lcg(&mut seed) % 8) as usize;
                let size = (lcg(&mut seed) % 50_001) as f32 / 5_000.0;
                inputs.push((turn, n, size));
            }

            let (v1, c1) = arc_batch(&inputs);
            let mut v2 = Vec::new();
            let mut c2 = Vec::new();
            arc_batch_into(&inputs, &mut v2, &mut c2);

            assert_eq!(c1, c2);
            assert_eq!(v1.len(), v2.len());
            for (&a, &b) in v1.iter().zip(v2.iter()) {
                assert_eq!(a.to_bits(), b.to_bits());
            }
        }
    }

    #[test]
    fn into_variant_appends_to_existing_buffer() {
        // Pre-populate buffers; new results should be appended, not clobbered.
        let mut values = vec![99.0f32];
        let mut counts = vec![1usize];
        arc_batch_into(&[(1.0, 2, 1.0)], &mut values, &mut counts);
        assert_eq!(counts[0], 1);   // pre-existing
        assert_eq!(counts[1], 2);   // new entity
        assert_eq!(values[0].to_bits(), 99.0f32.to_bits()); // not clobbered
        assert_eq!(values.len(), 3); // 1 pre-existing + 2 new
    }

    // ── Counts layout ──

    #[test]
    fn counts_sum_equals_values_len() {
        let mut seed = 0xA1B2_u32;
        for _ in 0..300 {
            let n = (lcg(&mut seed) % 10) as usize;
            let inputs: Vec<(f32, usize, f32)> = (0..n).map(|_| {
                let turn = (lcg(&mut seed) % 100) as f32 / 50.0 - 1.0;
                let segs = (lcg(&mut seed) % 8) as usize;
                let size = (lcg(&mut seed) % 20) as f32 / 2.0;
                (turn, segs, size)
            }).collect();
            let (values, counts) = arc_batch(&inputs);
            let count_sum: usize = counts.iter().sum();
            assert_eq!(count_sum, values.len());
        }
    }

    // ── Determinism ──

    #[test]
    fn deterministic() {
        let inputs = [(0.5, 4, 2.0), (-0.3, 1, 0.0), (1.0, 6, 5.0)];
        let (v1, c1) = arc_batch(&inputs);
        let (v2, c2) = arc_batch(&inputs);
        assert_eq!(c1, c2);
        for (&a, &b) in v1.iter().zip(v2.iter()) {
            assert_eq!(a.to_bits(), b.to_bits());
        }
    }
}
