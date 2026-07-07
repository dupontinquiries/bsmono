//! spirv_reflect_check.rs — pure SPIR-V descriptor-binding reflection + diff.
//!
//! Generalizes the R16 "Rust↔shader parity" idea: reflect the `(set, binding)`
//! decorations out of a compiled SPIR-V module and diff them against the
//! Rust-side binding table, so changing a GLSL binding without updating Rust is
//! caught by a test. Pure, deterministic, panic-free (R1/R11/R21).
//!
//! SPIR-V layout: word 0 is the magic `0x07230203`, words 1..5 are the rest of
//! the header, then instructions. Each instruction's word 0 is
//! `(word_count << 16) | opcode`. `OpDecorate` is opcode 71 with operands
//! `target-id, decoration, [operand...]`; decoration `Binding` = 33 (operand =
//! binding #) and `DescriptorSet` = 34 (operand = set #).

use std::collections::BTreeMap;

const SPIRV_MAGIC: u32 = 0x0723_0203;
const OP_DECORATE: u32 = 71;
const DEC_BINDING: u32 = 33;
const DEC_DESCRIPTOR_SET: u32 = 34;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Binding {
    pub set: u32,
    pub binding: u32,
}

/// Reflect all `(set, binding)` pairs declared by `OpDecorate` in `words`.
/// Returns them sorted by `(set, binding)` (deterministic). A resource with a
/// `Binding` but no explicit `DescriptorSet` is treated as set 0 (the GLSL
/// default). Returns empty on a bad magic or a malformed/empty stream.
pub fn reflect(words: &[u32]) -> Vec<Binding> {
    if words.first().copied() != Some(SPIRV_MAGIC) || words.len() < 5 {
        return Vec::new();
    }
    // target-id -> (set, binding), accumulated across its decorations.
    let mut acc: BTreeMap<u32, (Option<u32>, Option<u32>)> = BTreeMap::new();

    let mut i = 5; // skip the 5-word header
    while i < words.len() {
        let word_count = (words[i] >> 16) as usize;
        let opcode = words[i] & 0xFFFF;
        if word_count == 0 || i + word_count > words.len() {
            break; // malformed / truncated -> stop cleanly
        }
        if opcode == OP_DECORATE && word_count >= 4 {
            let target = words[i + 1];
            let decoration = words[i + 2];
            let operand = words[i + 3];
            let entry = acc.entry(target).or_insert((None, None));
            if decoration == DEC_DESCRIPTOR_SET {
                entry.0 = Some(operand);
            } else if decoration == DEC_BINDING {
                entry.1 = Some(operand);
            }
        }
        i += word_count;
    }

    let mut out: Vec<Binding> = acc
        .values()
        .filter_map(|&(set, binding)| binding.map(|b| Binding { set: set.unwrap_or(0), binding: b }))
        .collect();
    out.sort();
    out
}

/// Diff `reflected` (from SPIR-V) against `expected` (the Rust table). Returns a
/// sorted, human-readable list of mismatches; an empty vec means they match.
pub fn check(reflected: &[Binding], expected: &[Binding]) -> Vec<String> {
    let mut diffs = Vec::new();
    let mut want = expected.to_vec();
    want.sort();
    let mut got = reflected.to_vec();
    got.sort();

    for e in &want {
        if !got.contains(e) {
            diffs.push(format!("missing in shader: set {} binding {}", e.set, e.binding));
        }
    }
    for g in &got {
        if !want.contains(g) {
            diffs.push(format!("extra in shader: set {} binding {}", g.set, g.binding));
        }
    }
    diffs.sort();
    diffs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decorate(target: u32, decoration: u32, operand: u32) -> [u32; 4] {
        [(4u32 << 16) | OP_DECORATE, target, decoration, operand]
    }
    fn module(decos: &[[u32; 4]]) -> Vec<u32> {
        // magic, version, generator, id-bound, schema, then instructions
        let mut w = vec![SPIRV_MAGIC, 0x0001_0000, 0, 100, 0];
        for d in decos {
            w.extend_from_slice(d);
        }
        w
    }

    fn b(set: u32, binding: u32) -> Binding {
        Binding { set, binding }
    }

    #[test]
    fn reflects_hand_built_bindings() {
        let words = module(&[
            decorate(10, DEC_DESCRIPTOR_SET, 0),
            decorate(10, DEC_BINDING, 1),
            decorate(20, DEC_DESCRIPTOR_SET, 1),
            decorate(20, DEC_BINDING, 4),
        ]);
        assert_eq!(reflect(&words), vec![b(0, 1), b(1, 4)]);
    }

    #[test]
    fn binding_without_set_defaults_to_zero() {
        let words = module(&[decorate(7, DEC_BINDING, 3)]);
        assert_eq!(reflect(&words), vec![b(0, 3)]);
    }

    #[test]
    fn bad_magic_yields_empty() {
        let mut words = module(&[decorate(1, DEC_BINDING, 0)]);
        words[0] = 0xDEAD_BEEF;
        assert!(reflect(&words).is_empty());
        assert!(reflect(&[]).is_empty());
    }

    #[test]
    fn matching_tables_have_no_diffs() {
        let words = module(&[
            decorate(10, DEC_DESCRIPTOR_SET, 0),
            decorate(10, DEC_BINDING, 0),
            decorate(11, DEC_DESCRIPTOR_SET, 0),
            decorate(11, DEC_BINDING, 2),
        ]);
        let reflected = reflect(&words);
        let expected = vec![b(0, 0), b(0, 2)];
        assert!(check(&reflected, &expected).is_empty());
    }

    #[test]
    fn missing_binding_is_reported() {
        let reflected = vec![b(0, 0)];
        let expected = vec![b(0, 0), b(0, 4)]; // shader is missing set0/binding4
        let diffs = check(&reflected, &expected);
        assert_eq!(diffs.len(), 1);
        assert!(diffs[0].contains("missing"), "{}", diffs[0]);
    }

    #[test]
    fn extra_binding_is_reported() {
        let reflected = vec![b(0, 0), b(1, 7)]; // shader has an extra set1/binding7
        let expected = vec![b(0, 0)];
        let diffs = check(&reflected, &expected);
        assert_eq!(diffs.len(), 1);
        assert!(diffs[0].contains("extra"), "{}", diffs[0]);
    }

    #[test]
    fn truncated_stream_stops_cleanly() {
        // an OpDecorate claiming 4 words but only 2 present must not panic.
        let mut w = vec![SPIRV_MAGIC, 0x0001_0000, 0, 100, 0];
        w.push((4u32 << 16) | OP_DECORATE);
        w.push(10); // target only; operands missing
        assert!(reflect(&w).is_empty());
    }

    #[test]
    fn prop_deterministic() {
        let words = module(&[
            decorate(20, DEC_BINDING, 4),
            decorate(20, DEC_DESCRIPTOR_SET, 1),
            decorate(10, DEC_BINDING, 1),
            decorate(10, DEC_DESCRIPTOR_SET, 0),
        ]);
        assert_eq!(reflect(&words), reflect(&words));
        let expected = vec![b(1, 4), b(0, 1)];
        assert_eq!(check(&reflect(&words), &expected), check(&reflect(&words), &expected));
    }
}
