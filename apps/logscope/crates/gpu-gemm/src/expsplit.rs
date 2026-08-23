//! expsplit.rs — splitting a fused `ffn_gate_up_exps` into gate and up.
//!
//! Reported from a real load. `google.diffusiongemma-26B-A4B-it.Q4_K_M.gguf`
//! ships
//!
//! ```text
//! blk.0.ffn_gate_up_exps.weight  [2816 × 1408 × 128]   Q4_K
//! blk.0.ffn_down_exps.weight     [ 704 × 2816 × 128]   Q8_0
//! ```
//!
//! — gate and up FUSED per expert. This engine decides a layer is MoE by the
//! presence of `ffn_gate_exps` and fills from separate `ffn_gate_exps` +
//! `ffn_up_exps`, so the block reads as DENSE and its 128 experts are never
//! routed to. That happens before any kernel is reached, which is why the model
//! cannot load rather than merely running badly.
//!
//! ## The arithmetic is derivable, not assumed
//!
//! `ffn_down_exps` gives the per-expert intermediate width directly: its first
//! dimension is `ff`, here 704. The fused tensor's middle dimension is 1408,
//! which is exactly `2 × 704`. So the fusion is gate and up concatenated along
//! the intermediate axis, and the split point is `ff`.
//!
//! That check is the point. `qkvsplit` learned the same lesson in the other
//! direction — the MLA `kv_b` fusion was per-head where a blocked split looked
//! right for head 0 and was wrong for every head after — so this REFUSES when
//! the arithmetic does not land rather than slicing something plausible. A
//! wrong split produces a routed FFN over the wrong projections: coherent
//! garbage, not an error.
//!
//! ## What is NOT settled here
//!
//! **Which half is gate.** The arithmetic proves there are two halves of `ff`
//! each; it cannot say whether gate precedes up. Every fused layout this tree
//! has met puts them in the order the name gives — `gate_up` → gate first, as
//! `attn_qkv` is q, then k, then v — and llama.cpp's own `ffn_gate_up` handling
//! in the models that have it does the same. [`Split::GATE_FIRST`] records the
//! assumption in one place so a swap is one line, and the doc says plainly it
//! is an assumption. Wrong here means SwiGLU applies the activation to the
//! wrong half, which is exactly the coherent-garbage failure above.
//!
//! Pure: shapes in, offsets out. No tensor data is touched, so this is
//! testable without a checkpoint and is what the loader will use to place two
//! views over one buffer.

/// Where gate and up live inside the fused expert tensor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Split {
    /// Per-expert intermediate width, from `ffn_down_exps`' first dimension.
    pub ff: u64,
    /// Model dimension.
    pub d: u64,
    pub n_expert: u64,
    /// Element offset, within one expert's slab, where gate starts.
    pub gate_off: u64,
    /// Element offset, within one expert's slab, where up starts.
    pub up_off: u64,
    /// Elements per expert per half.
    pub half_len: u64,
}

impl Split {
    /// Gate occupies the FIRST half of the fused intermediate axis.
    ///
    /// An assumption, named. See the module docs: the arithmetic cannot
    /// distinguish the halves, only prove there are two.
    pub const GATE_FIRST: bool = true;

    /// Derive the split from the two tensors' shapes.
    ///
    /// `fused` is `ffn_gate_up_exps` as `[d, 2*ff, n_expert]`; `down` is
    /// `ffn_down_exps` as `[ff, d, n_expert]`.
    pub fn derive(fused: &[u64], down: &[u64]) -> Result<Split, String> {
        if fused.len() != 3 {
            return Err(format!("ffn_gate_up_exps should be 3-D [d, 2*ff, n_expert], got {fused:?}"));
        }
        if down.len() != 3 {
            return Err(format!("ffn_down_exps should be 3-D [ff, d, n_expert], got {down:?}"));
        }
        let (d, two_ff, ne) = (fused[0], fused[1], fused[2]);
        let (ff, d_down, ne_down) = (down[0], down[1], down[2]);

        if ne != ne_down {
            return Err(format!("expert counts disagree: fused has {ne}, down has {ne_down}"));
        }
        if d != d_down {
            return Err(format!("model dim disagrees: fused has {d}, down has {d_down}"));
        }
        // THE CHECK THAT MATTERS. If the middle axis is not exactly twice the
        // intermediate width, the tensor is not gate+up concatenated and every
        // offset below would be fiction.
        if two_ff != ff.saturating_mul(2) {
            return Err(format!(
                "ffn_gate_up_exps middle dim is {two_ff}, but ffn_down_exps says ff={ff} so a \
gate+up fusion would be {}. This is not the layout assumed — refusing rather than splitting \
something plausible, because a wrong split runs the FFN over the wrong projections and produces \
coherent garbage instead of an error", ff * 2));
        }
        if ff == 0 || d == 0 || ne == 0 {
            return Err(format!("zero dimension: d={d} ff={ff} n_expert={ne}"));
        }

        let half_len = d.saturating_mul(ff);
        Ok(Split {
            ff, d, n_expert: ne,
            gate_off: if Self::GATE_FIRST { 0 } else { half_len },
            up_off:   if Self::GATE_FIRST { half_len } else { 0 },
            half_len,
        })
    }

    /// Element offset of expert `e`'s gate block in the whole fused tensor.
    pub fn gate_at(&self, e: u64) -> u64 { e * self.half_len * 2 + self.gate_off }
    /// Element offset of expert `e`'s up block.
    pub fn up_at(&self, e: u64) -> u64 { e * self.half_len * 2 + self.up_off }

    /// Shape each half presents as, matching what `ffn_gate_exps` would be.
    pub fn half_dims(&self) -> [u64; 3] { [self.d, self.ff, self.n_expert] }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real checkpoint's numbers.
    #[test]
    fn the_diffusion_gemma_shapes_derive() {
        let s = Split::derive(&[2816, 1408, 128], &[704, 2816, 128]).expect("derives");
        assert_eq!(s.ff, 704, "ff comes from ffn_down_exps, not from halving the fused dim");
        assert_eq!(s.d, 2816);
        assert_eq!(s.n_expert, 128);
        assert_eq!(s.half_len, 2816 * 704);
        assert_eq!(s.half_dims(), [2816, 704, 128], "each half must look like ffn_gate_exps");
    }

    /// Every expert's two blocks are disjoint and in order.
    #[test]
    fn expert_offsets_tile_the_tensor_without_overlap() {
        let s = Split::derive(&[2816, 1408, 128], &[704, 2816, 128]).unwrap();
        let total = 2816u64 * 1408 * 128;
        let mut prev_end = 0;
        for e in 0..s.n_expert {
            let (g, u) = (s.gate_at(e), s.up_at(e));
            assert!(g >= prev_end, "expert {e} gate overlaps the previous expert");
            assert_eq!(u, g + s.half_len, "up must follow gate within the expert");
            prev_end = u + s.half_len;
        }
        assert_eq!(prev_end, total, "the halves must exactly cover the tensor");
    }

    /// The refusal, which is the whole reason this is a function and not a
    /// slice expression.
    #[test]
    fn a_middle_dim_that_is_not_twice_ff_is_refused() {
        // 1408 vs an ff of 512 → 1024 expected. Not this layout.
        let e = Split::derive(&[2816, 1408, 128], &[512, 2816, 128]).unwrap_err();
        assert!(e.contains("1408") && e.contains("1024"), "{e}");
        assert!(e.contains("coherent garbage"),
                "the refusal must say WHY slicing anyway is worse than stopping: {e}");
    }

    #[test]
    fn disagreeing_expert_counts_and_dims_are_refused() {
        assert!(Split::derive(&[2816, 1408, 128], &[704, 2816, 64]).is_err(), "expert count");
        assert!(Split::derive(&[2816, 1408, 128], &[704, 2048, 128]).is_err(), "model dim");
        assert!(Split::derive(&[2816, 1408], &[704, 2816, 128]).is_err(), "not 3-D");
        assert!(Split::derive(&[2816, 0, 128], &[0, 2816, 128]).is_err(), "zero dim");
    }

    /// Swapping the assumption must move both offsets, not one.
    ///
    /// The constant exists so a wrong guess is one line to fix; a test that
    /// only checked `gate_off` would pass with `up_off` left stale.
    #[test]
    fn the_gate_first_assumption_is_recorded_in_one_place() {
        let s = Split::derive(&[2816, 1408, 128], &[704, 2816, 128]).unwrap();
        if Split::GATE_FIRST {
            assert_eq!(s.gate_off, 0);
            assert_eq!(s.up_off, s.half_len);
        } else {
            assert_eq!(s.gate_off, s.half_len);
            assert_eq!(s.up_off, 0);
        }
        assert_ne!(s.gate_off, s.up_off, "the halves must not alias whichever way round they are");
    }
}
