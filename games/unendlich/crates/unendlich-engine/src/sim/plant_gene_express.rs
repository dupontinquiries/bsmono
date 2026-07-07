//! `sim::plant_gene_express` — express a plant sub-part's genome into a
//! phenotype, with a batched path for whole plants (a CPU reference a GPU kernel
//! will mirror; see `GPU_COMPUTE_PLAN`).
//!
//! [`express_one`] decodes one genome byte-string into a [`Phenotype`];
//! [`express_batch`] decodes many sub-parts at once. Both route through the same
//! `#[inline]` `kernel`, so the batch is **bit-for-bit identical** to expressing
//! each sub-part on its own. Short genomes are zero-filled, so any length is safe.
//!
//! Perf shape: sub-part genomes are decoded in one streaming pass with no
//! cross-part dependency — the per-plant expression a GPU dispatches one thread
//! per sub-part for. Pure and deterministic: byte reads and clamped scaling, no
//! clock, no randomness, no cross-imports.

/// A decoded plant sub-part phenotype.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Phenotype {
    /// Relative height, `0..1`.
    pub height: f32,
    /// Vigor / growth strength, `0..1`.
    pub vigor: f32,
    /// Hue byte (raw, `0..=255`).
    pub hue: u8,
    /// Number of repeated parts, `1..=8`.
    pub parts: u8,
}

/// Read genome byte `i`, zero if absent.
fn gene(g: &[u8], i: usize) -> u8 {
    match g.get(i) {
        Some(b) => *b,
        None => 0,
    }
}

/// The shared decode kernel. Both `express_one` and `express_batch` call this.
#[inline]
fn kernel(g: &[u8]) -> Phenotype {
    Phenotype {
        height: gene(g, 0) as f32 / 255.0,
        vigor: gene(g, 1) as f32 / 255.0,
        hue: gene(g, 2),
        parts: (gene(g, 3) % 8) + 1,
    }
}

/// Express one sub-part genome into a phenotype.
pub fn express_one(g: &[u8]) -> Phenotype {
    kernel(g)
}

/// Express many sub-part genomes, in order.
pub fn express_batch(genomes: &[&[u8]]) -> Vec<Phenotype> {
    let mut out = Vec::with_capacity(genomes.len());
    for g in genomes {
        out.push(kernel(g));
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
    fn same(a: Phenotype, b: Phenotype) -> bool {
        a.height.to_bits() == b.height.to_bits()
            && a.vigor.to_bits() == b.vigor.to_bits()
            && a.hue == b.hue
            && a.parts == b.parts
    }

    #[test]
    fn batch_matches_scalar_bit_for_bit() {
        let mut seed = 0xB17C_u32;
        for _ in 0..1500 {
            let n = (lcg(&mut seed) % 16) as usize;
            let mut owned: Vec<Vec<u8>> = Vec::new();
            for _ in 0..n {
                let len = (lcg(&mut seed) % 8) as usize; // incl shorter-than-4
                let mut g = Vec::new();
                for _ in 0..len {
                    g.push((lcg(&mut seed) & 0xFF) as u8);
                }
                owned.push(g);
            }
            let refs: Vec<&[u8]> = owned.iter().map(|v| v.as_slice()).collect();
            let batch = express_batch(&refs);
            assert_eq!(batch.len(), refs.len());
            for i in 0..refs.len() {
                assert!(same(batch[i], express_one(refs[i])));
            }
        }
    }

    #[test]
    fn empty_yields_empty() {
        let g: Vec<&[u8]> = Vec::new();
        assert!(express_batch(&g).is_empty());
    }

    #[test]
    fn single_equals_batch_of_one() {
        let mut seed = 0x0001_u32;
        for _ in 0..2000 {
            let len = (lcg(&mut seed) % 8) as usize;
            let mut g = Vec::new();
            for _ in 0..len {
                g.push((lcg(&mut seed) & 0xFF) as u8);
            }
            let b = express_batch(&[g.as_slice()]);
            assert_eq!(b.len(), 1);
            assert!(same(b[0], express_one(g.as_slice())));
        }
    }

    #[test]
    fn bounded() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..3000 {
            let len = (lcg(&mut seed) % 8) as usize;
            let mut g = Vec::new();
            for _ in 0..len {
                g.push((lcg(&mut seed) & 0xFF) as u8);
            }
            let p = express_one(g.as_slice());
            assert!(p.height >= 0.0 && p.height <= 1.0);
            assert!(p.vigor >= 0.0 && p.vigor <= 1.0);
            assert!(p.parts >= 1 && p.parts <= 8);
        }
    }

    #[test]
    fn deterministic() {
        let g: &[u8] = &[200, 50, 123, 9];
        assert!(same(express_one(g), express_one(g)));
        // short genome zero-fills predictably
        let short: &[u8] = &[255];
        let p = express_one(short);
        assert_eq!(p.vigor, 0.0);
        assert_eq!(p.hue, 0);
        assert_eq!(p.parts, 1);
    }

    #[test]
    fn empty_genome_is_zero_phenotype() {
        let p = express_one(&[]);
        assert_eq!(p, Phenotype { height: 0.0, vigor: 0.0, hue: 0, parts: 1 });
    }
}
