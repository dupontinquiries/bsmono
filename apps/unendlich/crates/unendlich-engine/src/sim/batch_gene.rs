//! `sim::batch_gene` — batched genome→phenotype expression, for spawning mobs
//! fast (a whole brood at once) with a scalar reference for parity.
//!
//! [`express_one`] decodes one genome (a byte slice) into a [`Phenotype`];
//! [`express_batch`] decodes many in a single call. Both route through the same
//! `#[inline]` `kernel`, so the batch is **bit-for-bit identical** to expressing
//! each genome individually — the speedup is purely in the iteration shape.
//!
//! Genomes may be any length: each trait reads a fixed byte index and falls back
//! to `0` if the genome is short (`get(..).unwrap_or(0)`), so a malformed or
//! truncated genome still expresses cleanly without panicking.
//!
//! Perf shape: genomes are read from a contiguous slice of slices and written
//! into a contiguous `Vec<Phenotype>` in order, with no cross-genome
//! dependency — one streaming pass that the spawner can issue once per brood
//! instead of paying call overhead per mob. Pure and deterministic.

/// An expressed creature: a handful of bounded traits decoded from a genome.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Phenotype {
    /// Body size, `0.5 ..= 2.5`.
    pub size: f32,
    /// Move speed, `0.0 ..= 1.0`.
    pub speed: f32,
    /// Armor, `0.0 ..= 1.0`.
    pub armor: f32,
    /// Colour hue, `0.0 ..= 1.0`.
    pub hue: f32,
    /// Aggression, `0.0 ..= 1.0`.
    pub aggression: f32,
}

fn b01(byte: u8) -> f32 {
    byte as f32 / 255.0
}

/// The shared decode. Both the scalar and batch paths call exactly this.
#[inline]
fn kernel(genome: &[u8]) -> Phenotype {
    let g = |i: usize| genome.get(i).copied().unwrap_or(0);
    Phenotype {
        size: 0.5 + b01(g(0)) * 2.0, // 0.5..2.5
        speed: b01(g(1)),
        armor: b01(g(2)),
        hue: b01(g(3)),
        aggression: b01(g(4)),
    }
}

/// Scalar reference: express one genome.
pub fn express_one(genome: &[u8]) -> Phenotype {
    kernel(genome)
}

/// Batched: express many genomes, in order.
pub fn express_batch(genomes: &[&[u8]]) -> Vec<Phenotype> {
    crate::util::batch_kernel::map_collect(genomes, |g| kernel(g))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    fn same_bits(a: &Phenotype, b: &Phenotype) -> bool {
        a.size.to_bits() == b.size.to_bits()
            && a.speed.to_bits() == b.speed.to_bits()
            && a.armor.to_bits() == b.armor.to_bits()
            && a.hue.to_bits() == b.hue.to_bits()
            && a.aggression.to_bits() == b.aggression.to_bits()
    }

    #[test]
    fn batch_matches_scalar_bit_for_bit() {
        let mut seed = 0xBA7E_u32;
        for _ in 0..1500 {
            let n = (lcg(&mut seed) % 12) as usize;
            // build owned genomes of varying length, then borrow as &[u8]
            let mut owned: Vec<Vec<u8>> = Vec::new();
            for _ in 0..n {
                let len = (lcg(&mut seed) % 8) as usize; // includes short/empty
                let mut gbytes = Vec::with_capacity(len);
                for _ in 0..len {
                    gbytes.push((lcg(&mut seed) & 0xFF) as u8);
                }
                owned.push(gbytes);
            }
            let refs: Vec<&[u8]> = owned.iter().map(|v| v.as_slice()).collect();
            let batch = express_batch(&refs);
            assert_eq!(batch.len(), refs.len());
            for i in 0..refs.len() {
                assert!(same_bits(&batch[i], &express_one(refs[i])));
            }
        }
    }

    #[test]
    fn empty_yields_empty() {
        assert!(express_batch(&[]).is_empty());
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
            let one = express_one(&g);
            let batch = express_batch(&[g.as_slice()]);
            assert_eq!(batch.len(), 1);
            assert!(same_bits(&batch[0], &one));
        }
    }

    #[test]
    fn fields_are_bounded() {
        let mut seed = 0xB0DD_u32;
        for _ in 0..4000 {
            let mut g = [0u8; 5];
            let mut i = 0;
            while i < 5 {
                g[i] = (lcg(&mut seed) & 0xFF) as u8;
                i += 1;
            }
            let p = express_one(&g);
            assert!(p.size >= 0.5 && p.size <= 2.5);
            assert!(p.speed >= 0.0 && p.speed <= 1.0);
            assert!(p.armor >= 0.0 && p.armor <= 1.0);
            assert!(p.hue >= 0.0 && p.hue <= 1.0);
            assert!(p.aggression >= 0.0 && p.aggression <= 1.0);
        }
    }

    #[test]
    fn short_genome_does_not_panic_and_zero_fills() {
        // empty genome → all-zero-byte phenotype
        let p = express_one(&[]);
        assert_eq!(p.size, 0.5); // 0.5 + 0
        assert_eq!(p.speed, 0.0);
        assert_eq!(p.aggression, 0.0);
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..2000 {
            let len = (lcg(&mut seed) % 8) as usize;
            let mut g = Vec::new();
            for _ in 0..len {
                g.push((lcg(&mut seed) & 0xFF) as u8);
            }
            assert_eq!(express_one(&g), express_one(&g));
        }
    }
}
