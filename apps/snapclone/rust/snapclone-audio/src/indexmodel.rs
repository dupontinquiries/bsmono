//! Index-stream model — the "prediction angle," done right and losslessly.
//!
//! Once frames/utterances are codebook indices (see `codebook`), the index
//! STREAM has its own statistics: some indices are far more common than others,
//! and some transitions are far more likely than others (a given sound tends to
//! be followed by particular sounds). This models that and turns it into bits:
//! a likely index, or a likely transition, costs a fraction of a bit; a rare one
//! costs more. That is real, and it's where "prediction" legitimately helps.
//!
//! What it is NOT: predicting *content* so you can skip sending it. You still
//! emit one index per frame — the information about what was actually said is
//! irreducible. This compresses the index STREAM, it doesn't conjure it. The
//! floor is the entropy of the speech given the corpus: low if the corpus fits
//! the speaker well, never zero.
//!
//! Why it's safe to share only indices: the model is adaptive and updated from
//! the index stream itself, identically on both ends (encoder updates after
//! coding each symbol; decoder updates after decoding it). So both sides always
//! hold the same probabilities — the arithmetic/range coder that turns those
//! probabilities into actual bits is then exact and lossless. That coder is a
//! standard, mechanical step; what's modelled here is the probability assignment
//! and the bits/symbol it achieves, which is the substance and the thing worth
//! measuring before building the bit-level coder.
//!
//! The model is order-1 with add-1 smoothing, backing off to order-0 for unseen
//! contexts — deliberately simple (not PPM/CM). Enough to show the win and to
//! drive a coder; swap in a stronger context model later without changing the
//! interface.

use std::collections::HashMap;

/// Adaptive index model. `n` is the alphabet size (codebook capacity).
pub struct IndexModel {
    n: usize,
    order0: Vec<u32>,
    order0_total: u32,
    order1: HashMap<u32, HashMap<u32, u32>>, // ctx -> (idx -> count)
    order1_total: HashMap<u32, u32>,         // ctx -> total
    last: Option<u32>,
}

impl IndexModel {
    pub fn new(n: usize) -> Self {
        let n = n.max(1);
        Self {
            n,
            order0: vec![0; n],
            order0_total: 0,
            order1: HashMap::new(),
            order1_total: HashMap::new(),
            last: None,
        }
    }

    /// Probability the model assigns to `idx` given the current context.
    /// Order-1 (add-1 smoothed) when the context has been seen, else order-0.
    pub fn prob(&self, idx: u32) -> f64 {
        let i = idx as usize;
        if let Some(ctx) = self.last {
            if let Some(total) = self.order1_total.get(&ctx) {
                if *total > 0 {
                    let c = self
                        .order1
                        .get(&ctx)
                        .and_then(|m| m.get(&idx))
                        .copied()
                        .unwrap_or(0);
                    return (c as f64 + 1.0) / (*total as f64 + self.n as f64);
                }
            }
        }
        let c = if i < self.n { self.order0[i] } else { 0 };
        (c as f64 + 1.0) / (self.order0_total as f64 + self.n as f64)
    }

    /// Bits to code `idx` under the current model: -log2(p).
    pub fn cost_bits(&self, idx: u32) -> f64 {
        -self.prob(idx).log2()
    }

    /// Fold an observed index into the model (call after coding/decoding it).
    pub fn update(&mut self, idx: u32) {
        let i = idx as usize;
        if i < self.n {
            self.order0[i] += 1;
            self.order0_total += 1;
        }
        if let Some(ctx) = self.last {
            *self.order1.entry(ctx).or_default().entry(idx).or_insert(0) += 1;
            *self.order1_total.entry(ctx).or_insert(0) += 1;
        }
        self.last = Some(idx);
    }

    /// Total adaptive cost (bits) to code a whole stream, updating as it goes.
    pub fn stream_bits(&mut self, stream: &[u32]) -> f64 {
        let mut bits = 0.0;
        for &idx in stream {
            bits += self.cost_bits(idx);
            self.update(idx);
        }
        bits
    }

    /// Flat cost of the same stream with no model: `len * log2(n)` bits.
    pub fn raw_bits(&self, len: usize) -> f64 {
        len as f64 * (self.n as f64).log2()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skewed_stream_beats_flat_indices() {
        // 16-entry alphabet, but index 3 dominates → adaptive cost well under
        // the flat 4 bits/symbol.
        let mut s = Vec::new();
        for k in 0..1000 {
            s.push(if k % 10 == 0 { (k % 16) as u32 } else { 3u32 });
        }
        let mut m = IndexModel::new(16);
        let bits = m.stream_bits(&s);
        let avg = bits / s.len() as f64;
        assert!(avg < 2.0, "skewed stream should compress: {avg:.2} bits/sym");
        assert!(bits < m.raw_bits(s.len()));
    }

    #[test]
    fn order1_beats_order0_on_correlated_stream() {
        // a deterministic cycle 0→1→2→3→0… : order-1 nails the successor,
        // order-0 sees ~uniform-over-4. Compare adaptive cost with vs without
        // a context (simulate order-0 by resetting `last` each step).
        let s: Vec<u32> = (0..1000).map(|k| (k % 4) as u32).collect();

        let mut m1 = IndexModel::new(4);
        let o1 = m1.stream_bits(&s) / s.len() as f64;

        // order-0 reference: never let a context form
        let mut m0 = IndexModel::new(4);
        let mut bits0 = 0.0;
        for &idx in &s {
            bits0 += m0.cost_bits(idx);
            m0.update(idx);
            m0.last = None; // child module can touch private fields; forces order-0
        }
        let o0 = bits0 / s.len() as f64;

        assert!(o1 < o0 * 0.6, "order-1 should crush order-0: o1={o1:.2} o0={o0:.2}");
        assert!(o1 < 0.5, "predictable transitions → near-free: {o1:.2}");
    }

    #[test]
    fn random_stream_does_not_compress() {
        // LCG pseudo-uniform indices: ~incompressible, adaptive ≈ flat (honest:
        // no free lunch on high-entropy streams).
        let n = 8usize;
        let mut st: u32 = 0xC0FFEE;
        let s: Vec<u32> = (0..2000)
            .map(|_| {
                st = st.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                ((st >> 16) % n as u32) as u32
            })
            .collect();
        let mut m = IndexModel::new(n);
        let avg = m.stream_bits(&s) / s.len() as f64;
        let flat = (n as f64).log2();
        assert!(avg > flat - 0.6 && avg < flat + 0.6, "≈flat for random: {avg:.2} vs {flat:.2}");
    }

    #[test]
    fn both_ends_stay_in_lockstep() {
        // The encoder records the prob it used for each symbol; a fresh decoder,
        // fed the same stream, must assign the identical prob at each step before
        // updating. Equal probabilities at every step ⇒ exact, lossless coding.
        let s: Vec<u32> = [1u32, 2, 2, 3, 1, 2, 3, 3, 1, 2]
            .iter()
            .cycle()
            .take(200)
            .copied()
            .collect();

        let mut enc = IndexModel::new(8);
        let mut probs = Vec::with_capacity(s.len());
        for &idx in &s {
            probs.push(enc.prob(idx));
            enc.update(idx);
        }

        let mut dec = IndexModel::new(8);
        for (k, &idx) in s.iter().enumerate() {
            let p = dec.prob(idx);
            assert!((p - probs[k]).abs() < 1e-12, "desync at {k}: {p} vs {}", probs[k]);
            dec.update(idx);
        }
    }
}
