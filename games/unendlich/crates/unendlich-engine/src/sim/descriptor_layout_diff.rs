//! `sim::descriptor_layout_diff` — pure diff between a shader's **reflected**
//! descriptor layout and the **expected** Rust binding table.
//!
//! The render harness adds a gate that reflects each compiled `.spv`'s
//! descriptor-set layout and checks it against the binding table the Rust side
//! declares — so changing a GLSL `layout(set=, binding=)` (or its type/count/
//! stage visibility) and forgetting to update the Rust descriptor-set layout is
//! caught at build time instead of as a validation-layer crash at run time.
//!
//! The coordinator owns the `.spv` parsing and IO; this module is the pure
//! comparison so it is testable in isolation. Both sides arrive as
//! `&[BindingDesc]` (any order); [`diff_layouts`] returns the set of
//! [`Mismatch`]es keyed by `(set, binding)`. The result is **deterministic and
//! independent of input order** (keys are gathered and sorted before
//! comparison), so two equal layouts in different orders diff to empty and the
//! report reads the same every run.
//!
//! Pure: data in, data out. No `crate::render`, no `ash`/`vk`, no IO, no clock,
//! no RNG, no `.unwrap()`/`.expect()` in non-test code.

/// Kind of a descriptor binding (the subset the engine's shaders use). Mirrors
/// the relevant `VkDescriptorType` categories without depending on `ash`/`vk`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingKind {
    UniformBuffer,
    StorageBuffer,
    SampledImage,
    StorageImage,
    Sampler,
}

/// One descriptor binding: where it lives `(set, binding)`, its `kind`,
/// descriptor `count` (array size; 1 for a scalar binding), and the bitmask of
/// shader `stages` it is visible to (e.g. `VK_SHADER_STAGE_*` ORed together).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BindingDesc {
    pub set: u32,
    pub binding: u32,
    pub kind: BindingKind,
    pub count: u32,
    pub stages: u32,
}

/// A single discrepancy between the reflected and expected layouts, keyed by
/// `(set, binding)`. A binding that differs in more than one field produces one
/// variant per differing field (kind, then count, then stages).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mismatch {
    /// Present in `expected` (Rust) but not in `reflected` (shader): the Rust
    /// side declares a binding the shader doesn't have.
    Missing { set: u32, binding: u32 },
    /// Present in `reflected` (shader) but not in `expected` (Rust): the shader
    /// has a binding the Rust side never declared.
    Extra { set: u32, binding: u32 },
    /// Same `(set, binding)` on both sides, different descriptor kind.
    KindMismatch { set: u32, binding: u32, reflected: BindingKind, expected: BindingKind },
    /// Same `(set, binding)` on both sides, different descriptor count.
    CountMismatch { set: u32, binding: u32, reflected: u32, expected: u32 },
    /// Same `(set, binding)` on both sides, different stage-visibility mask.
    StagesMismatch { set: u32, binding: u32, reflected: u32, expected: u32 },
}

/// Find the first binding at `(set, binding)` in `layout`, if any. (A
/// well-formed layout has no duplicate keys; the first wins if it does.)
fn find_at(layout: &[BindingDesc], set: u32, binding: u32) -> Option<&BindingDesc> {
    layout.iter().find(|b| b.set == set && b.binding == binding)
}

/// Diff a reflected shader layout against the expected Rust binding table.
///
/// Returns every [`Mismatch`], in a deterministic order sorted by
/// `(set, binding)` (and, within a key, kind → count → stages). An empty result
/// means the layouts agree. Independent of the order of either input slice.
pub fn diff_layouts(reflected: &[BindingDesc], expected: &[BindingDesc]) -> Vec<Mismatch> {
    // Gather the union of (set, binding) keys from both sides, deduped, then
    // sort — this is what makes the output order-independent + deterministic.
    let mut keys: Vec<(u32, u32)> = Vec::new();
    for b in reflected.iter().chain(expected.iter()) {
        let k = (b.set, b.binding);
        if !keys.contains(&k) {
            keys.push(k);
        }
    }
    keys.sort_unstable();

    let mut out = Vec::new();
    for (set, binding) in keys {
        match (find_at(reflected, set, binding), find_at(expected, set, binding)) {
            (Some(r), Some(e)) => {
                if r.kind != e.kind {
                    out.push(Mismatch::KindMismatch {
                        set,
                        binding,
                        reflected: r.kind,
                        expected: e.kind,
                    });
                }
                if r.count != e.count {
                    out.push(Mismatch::CountMismatch {
                        set,
                        binding,
                        reflected: r.count,
                        expected: e.count,
                    });
                }
                if r.stages != e.stages {
                    out.push(Mismatch::StagesMismatch {
                        set,
                        binding,
                        reflected: r.stages,
                        expected: e.stages,
                    });
                }
            }
            // in shader, not declared in Rust
            (Some(_), None) => out.push(Mismatch::Extra { set, binding }),
            // declared in Rust, missing from shader
            (None, Some(_)) => out.push(Mismatch::Missing { set, binding }),
            (None, None) => {} // unreachable: every key came from one side
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ubo(set: u32, binding: u32, stages: u32) -> BindingDesc {
        BindingDesc { set, binding, kind: BindingKind::UniformBuffer, count: 1, stages }
    }
    fn ssbo(set: u32, binding: u32, stages: u32) -> BindingDesc {
        BindingDesc { set, binding, kind: BindingKind::StorageBuffer, count: 1, stages }
    }

    // Seeded deterministic shuffle (R20), seed documented at call sites.
    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn shuffled(v: &[BindingDesc], seed: &mut u32) -> Vec<BindingDesc> {
        let mut out = v.to_vec();
        // Fisher–Yates
        let n = out.len();
        let mut i = n;
        while i > 1 {
            i -= 1;
            let j = (lcg(seed) as usize) % (i + 1);
            out.swap(i, j);
        }
        out
    }

    fn sample_layout() -> Vec<BindingDesc> {
        vec![
            ubo(0, 0, 0x1),
            ssbo(0, 1, 0x20),
            BindingDesc { set: 1, binding: 0, kind: BindingKind::SampledImage, count: 4, stages: 0x10 },
            BindingDesc { set: 2, binding: 3, kind: BindingKind::StorageImage, count: 1, stages: 0x20 },
        ]
    }

    #[test]
    fn identical_layouts_yield_no_mismatches() {
        let a = sample_layout();
        // same set, different order
        let mut s = 0xDED1_0000u32;
        let b = shuffled(&a, &mut s);
        assert!(diff_layouts(&a, &b).is_empty(), "identical layouts must diff empty");
        assert!(diff_layouts(&b, &a).is_empty());
    }

    #[test]
    fn changed_kind_at_one_binding_is_one_mismatch() {
        let expected = sample_layout();
        let mut reflected = sample_layout();
        // flip the kind at (0,1): StorageBuffer -> UniformBuffer
        reflected[1].kind = BindingKind::UniformBuffer;
        let m = diff_layouts(&reflected, &expected);
        assert_eq!(m.len(), 1, "exactly one mismatch, got {:?}", m);
        assert_eq!(
            m[0],
            Mismatch::KindMismatch {
                set: 0,
                binding: 1,
                reflected: BindingKind::UniformBuffer,
                expected: BindingKind::StorageBuffer,
            }
        );
    }

    #[test]
    fn missing_binding_reported() {
        let expected = sample_layout();
        // reflected is missing (2,3)
        let reflected: Vec<_> = expected.iter().filter(|b| !(b.set == 2 && b.binding == 3)).cloned().collect();
        let m = diff_layouts(&reflected, &expected);
        assert_eq!(m, vec![Mismatch::Missing { set: 2, binding: 3 }]);
    }

    #[test]
    fn extra_binding_reported() {
        let expected = sample_layout();
        let mut reflected = sample_layout();
        reflected.push(ssbo(3, 7, 0x20)); // shader has a binding Rust never declared
        let m = diff_layouts(&reflected, &expected);
        assert_eq!(m, vec![Mismatch::Extra { set: 3, binding: 7 }]);
    }

    #[test]
    fn count_and_stages_mismatch_at_one_binding() {
        let expected = sample_layout();
        let mut reflected = sample_layout();
        // (1,0): change count 4->2 and stages 0x10->0x11
        reflected[2].count = 2;
        reflected[2].stages = 0x11;
        let m = diff_layouts(&reflected, &expected);
        assert_eq!(
            m,
            vec![
                Mismatch::CountMismatch { set: 1, binding: 0, reflected: 2, expected: 4 },
                Mismatch::StagesMismatch { set: 1, binding: 0, reflected: 0x11, expected: 0x10 },
            ]
        );
    }

    #[test]
    fn output_is_sorted_by_set_binding() {
        // expected has (0,0),(0,1),(1,0),(2,3); reflected drops (0,1) [missing],
        // adds (3,9) [extra], and changes kind at (2,3).
        let expected = sample_layout();
        let mut reflected: Vec<_> =
            expected.iter().filter(|b| !(b.set == 0 && b.binding == 1)).cloned().collect();
        reflected.push(ubo(3, 9, 0x1));
        for b in reflected.iter_mut() {
            if b.set == 2 && b.binding == 3 {
                b.kind = BindingKind::SampledImage;
            }
        }
        let m = diff_layouts(&reflected, &expected);
        assert_eq!(
            m,
            vec![
                Mismatch::Missing { set: 0, binding: 1 },
                Mismatch::KindMismatch {
                    set: 2,
                    binding: 3,
                    reflected: BindingKind::SampledImage,
                    expected: BindingKind::StorageImage,
                },
                Mismatch::Extra { set: 3, binding: 9 },
            ]
        );
    }

    #[test]
    fn deterministic_and_order_independent() {
        // Build a layout pair with a spread of mismatches, then diff it under
        // many random input permutations — the output must be byte-identical.
        let expected = sample_layout();
        let mut reflected = sample_layout();
        reflected[0].stages = 0x2; // (0,0) stages differ
        reflected.retain(|b| !(b.set == 0 && b.binding == 1)); // (0,1) missing
        reflected.push(ssbo(5, 5, 0x20)); // (5,5) extra

        let baseline = diff_layouts(&reflected, &expected);
        let mut s = 0xDED1_0001u32;
        for _ in 0..500 {
            let r = shuffled(&reflected, &mut s);
            let e = shuffled(&expected, &mut s);
            assert_eq!(diff_layouts(&r, &e), baseline, "diff must be order-independent");
        }
        // and stable across repeated identical calls
        assert_eq!(diff_layouts(&reflected, &expected), baseline);
    }

    #[test]
    fn empty_inputs() {
        assert!(diff_layouts(&[], &[]).is_empty());
        let one = vec![ubo(0, 0, 0x1)];
        assert_eq!(diff_layouts(&one, &[]), vec![Mismatch::Extra { set: 0, binding: 0 }]);
        assert_eq!(diff_layouts(&[], &one), vec![Mismatch::Missing { set: 0, binding: 0 }]);
    }
}
