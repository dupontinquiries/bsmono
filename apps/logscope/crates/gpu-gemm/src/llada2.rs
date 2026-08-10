//! llada2 — LLaDA2.0's layer plan, from the llama.cpp reference graph.
//!
//! A masked-diffusion MoE. Like `rnd1` it loads through MoE machinery and runs
//! the DENOISE forward rather than AR decode — but unlike rnd1 it is not simply
//! "qwen3moe with a different forward". Four structural differences that a
//! loader gets wrong silently if it assumes otherwise.
//!
//! ## 1. The last layers are NOT transformer layers
//!
//! `n_transformer_layers = n_layer - nextn_predict_layers`. The NextN block
//! (multi-token prediction: `nextn_eh_proj`, `nextn_embed_tokens`, the shared
//! head and its norms) occupies real layer indices at the end and **the forward
//! loop does not run them**. A loader that iterates `0..n_layer` walks off into
//! tensors that are not attention blocks — and because they are shaped
//! plausibly, the failure is wrong numbers rather than a missing-tensor error.
//!
//! ## 2. The first layers are dense, the rest are MoE
//!
//! `il < n_layer_dense_lead` takes the plain SwiGLU path; everything after
//! takes the router. Getting the boundary wrong routes a dense layer through an
//! expert table that does not exist for it.
//!
//! ## 3. Fused QKV with a GQA-asymmetric layout
//!
//! One `attn_qkv` tensor, sliced at three offsets: Q at 0, K at `n_embd`, V at
//! `n_embd + n_embd_gqa`. The V offset is **not** `2 * n_embd` — under GQA the
//! K block is narrower than the Q block, so an evenly-thirded split silently
//! reads V from inside K.
//!
//! ## 4. Attention is non-causal and uncached
//!
//! `build_attn_inp_no_cache()`. Diffusion attends over the whole block in both
//! directions, so there is no KV cache to accumulate and no causal mask to
//! apply — the same property `attention_mh_full_t` and the denoise path already
//! serve.
//!
//! QK-norm is RMS and applied **before** RoPE, and the MoE carries an expert
//! bias (`ffn_exp_probs_b`) plus a shared expert added to the routed sum.

/// Shapes the loader needs, read from GGUF metadata.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Llada2Shape {
    pub n_layer: u32,
    /// Trailing layers reserved for multi-token prediction.
    pub nextn_predict_layers: u32,
    /// Leading layers that are dense rather than MoE.
    pub n_layer_dense_lead: u32,
    pub n_embd: u32,
    pub n_head: u32,
    pub n_head_kv: u32,
    pub n_embd_head: u32,
}

impl Llada2Shape {
    /// Layers the forward loop actually runs.
    pub fn n_transformer_layers(&self) -> u32 {
        self.n_layer.saturating_sub(self.nextn_predict_layers)
    }

    /// Width of the K (and V) block in a fused QKV tensor.
    pub fn n_embd_gqa(&self) -> u32 { self.n_head_kv * self.n_embd_head }

    /// Byte offsets of Q, K and V within one row of the fused `attn_qkv`.
    ///
    /// The V offset is `n_embd + n_embd_gqa`, NOT `2 * n_embd`: under GQA the K
    /// block is narrower, so thirding the row reads V from inside K. That
    /// produces plausible-looking output, which is the worst failure mode
    /// available.
    pub fn qkv_offsets(&self, elem: u32) -> (u32, u32, u32) {
        (0, self.n_embd * elem, (self.n_embd + self.n_embd_gqa()) * elem)
    }

    /// Total elements in one fused QKV row.
    pub fn qkv_row_elems(&self) -> u32 { self.n_embd + 2 * self.n_embd_gqa() }

    /// Is this layer a transformer layer at all?
    pub fn is_transformer_layer(&self, il: u32) -> bool { il < self.n_transformer_layers() }

    /// Does this layer take the MoE path?
    pub fn is_moe_layer(&self, il: u32) -> bool {
        self.is_transformer_layer(il) && il >= self.n_layer_dense_lead
    }

    /// Does it take the dense path?
    pub fn is_dense_layer(&self, il: u32) -> bool {
        self.is_transformer_layer(il) && il < self.n_layer_dense_lead
    }

    /// Sanity check, so a bad header is a message rather than wrong numbers.
    pub fn validate(&self) -> Result<(), String> {
        if self.n_head_kv == 0 || self.n_head == 0 {
            return Err("llada2: zero heads".into());
        }
        if self.n_head % self.n_head_kv != 0 {
            return Err(format!("llada2: n_head {} is not a multiple of n_head_kv {}",
                               self.n_head, self.n_head_kv));
        }
        if self.nextn_predict_layers >= self.n_layer {
            return Err(format!("llada2: nextn_predict_layers {} leaves no transformer layers of {}",
                               self.nextn_predict_layers, self.n_layer));
        }
        if self.n_layer_dense_lead > self.n_transformer_layers() {
            return Err(format!("llada2: dense lead {} exceeds {} transformer layers",
                               self.n_layer_dense_lead, self.n_transformer_layers()));
        }
        Ok(())
    }
}

/// What one layer computes, so the forward can be driven from data.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LayerKind {
    Dense,
    /// Routed experts plus a shared expert added to their sum.
    MoeWithShared,
    /// Multi-token prediction block — NOT run by the transformer loop.
    NextN,
}

pub fn layer_kind(shape: &Llada2Shape, il: u32) -> LayerKind {
    if !shape.is_transformer_layer(il) { LayerKind::NextN }
    else if il < shape.n_layer_dense_lead { LayerKind::Dense }
    else { LayerKind::MoeWithShared }
}

/// The order operations apply within an attention block.
///
/// Recorded as data because the reference applies QK-norm BEFORE RoPE, and the
/// opposite order also runs, also produces fluent output, and is wrong. A test
/// that pins the order is cheaper than discovering it from generated text.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AttnStep { SplitQkv, QkNorm, Rope, Attend }

pub const ATTN_ORDER: [AttnStep; 4] =
    [AttnStep::SplitQkv, AttnStep::QkNorm, AttnStep::Rope, AttnStep::Attend];

/// Residual structure of one layer.
///
/// `sa_out = attn + input`, then `out = ffn + sa_out` — the FFN's residual adds
/// `sa_out`, not the layer input. Adding the wrong one drops the attention
/// contribution and is invisible in shapes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResidualSource { LayerInput, PostAttention }

pub const FFN_RESIDUAL: ResidualSource = ResidualSource::PostAttention;

#[cfg(test)]
mod tests {
    use super::*;

    /// LLaDA2.0-mini-preview-ish: 16 layers with a NextN tail and a dense lead.
    fn mini() -> Llada2Shape {
        Llada2Shape { n_layer: 16, nextn_predict_layers: 1, n_layer_dense_lead: 1,
                      n_embd: 2048, n_head: 16, n_head_kv: 4, n_embd_head: 128 }
    }

    #[test]
    fn the_nextn_tail_is_not_a_transformer_layer() {
        // A loader iterating 0..n_layer walks into tensors that are not
        // attention blocks — and they are shaped plausibly, so the failure is
        // WRONG NUMBERS rather than a missing-tensor error.
        let s = mini();
        assert_eq!(s.n_transformer_layers(), 15);
        assert!(s.is_transformer_layer(14));
        assert!(!s.is_transformer_layer(15));
        assert_eq!(layer_kind(&s, 15), LayerKind::NextN);
    }

    #[test]
    fn the_dense_lead_comes_before_the_moe_layers() {
        let s = mini();
        assert_eq!(layer_kind(&s, 0), LayerKind::Dense);
        assert_eq!(layer_kind(&s, 1), LayerKind::MoeWithShared);
        assert!(s.is_dense_layer(0) && !s.is_moe_layer(0));
        assert!(s.is_moe_layer(1) && !s.is_dense_layer(1));
    }

    #[test]
    fn every_layer_index_has_exactly_one_kind() {
        // A layer in no category is skipped; one in two runs twice.
        let s = mini();
        for il in 0..s.n_layer {
            let k = layer_kind(&s, il);
            let flags = [s.is_dense_layer(il), s.is_moe_layer(il), !s.is_transformer_layer(il)];
            assert_eq!(flags.iter().filter(|f| **f).count(), 1, "layer {il} -> {k:?}");
        }
    }

    #[test]
    fn the_v_offset_is_not_a_third_of_the_row() {
        // THE bug this module exists to prevent. Under GQA the K block is
        // narrower than Q, so an evenly-thirded split reads V from inside K —
        // which produces plausible output, the worst failure available.
        let s = mini();
        let (q, k, v) = s.qkv_offsets(1);
        assert_eq!(q, 0);
        assert_eq!(k, 2048);
        assert_eq!(v, 2048 + 512, "n_embd + n_embd_gqa, not 2*n_embd");
        assert_ne!(v, 2 * s.n_embd, "the naive thirding is wrong here");
    }

    #[test]
    fn the_offsets_and_the_row_width_agree() {
        // V must start exactly one K-block before the end, or the last head
        // reads past the row.
        let s = mini();
        let (_, _, v) = s.qkv_offsets(1);
        assert_eq!(v + s.n_embd_gqa(), s.qkv_row_elems());
    }

    #[test]
    fn a_non_gqa_model_does_third_evenly_and_still_works() {
        // The general formula must not special-case GQA — with n_head_kv ==
        // n_head the offsets collapse to the familiar thirds.
        let s = Llada2Shape { n_head_kv: 16, ..mini() };
        let (q, k, v) = s.qkv_offsets(1);
        assert_eq!((q, k, v), (0, 2048, 4096));
        assert_eq!(s.qkv_row_elems(), 3 * s.n_embd);
    }

    #[test]
    fn qk_norm_happens_before_rope() {
        // Both orders run and both produce fluent output; only one is right.
        // Pinning it here is cheaper than discovering it from generated text.
        let n = ATTN_ORDER.iter().position(|s| *s == AttnStep::QkNorm).unwrap();
        let r = ATTN_ORDER.iter().position(|s| *s == AttnStep::Rope).unwrap();
        assert!(n < r, "{ATTN_ORDER:?}");
    }

    #[test]
    fn the_ffn_residual_adds_the_post_attention_value() {
        // out = ffn + sa_out, not ffn + layer_input. Adding the wrong one drops
        // the attention contribution and is invisible in shapes.
        assert_eq!(FFN_RESIDUAL, ResidualSource::PostAttention);
    }

    #[test]
    fn a_header_that_leaves_no_transformer_layers_is_refused() {
        // Rather than looping zero times and emitting the embedding unchanged,
        // which reads as a broken model instead of a broken header.
        let s = Llada2Shape { nextn_predict_layers: 16, ..mini() };
        assert!(s.validate().is_err());
        let s2 = Llada2Shape { nextn_predict_layers: 20, ..mini() };
        assert_eq!(s2.n_transformer_layers(), 0, "saturating, not underflow");
        assert!(s2.validate().is_err());
    }

    #[test]
    fn a_dense_lead_past_the_transformer_layers_is_refused() {
        let s = Llada2Shape { n_layer_dense_lead: 99, ..mini() };
        assert!(s.validate().unwrap_err().contains("dense lead"));
    }

    #[test]
    fn a_gqa_ratio_that_does_not_divide_is_refused() {
        // 16 query heads over 5 kv heads has no expert-to-head mapping, and
        // silently flooring would mis-share the K blocks.
        let s = Llada2Shape { n_head_kv: 5, ..mini() };
        assert!(s.validate().unwrap_err().contains("multiple"));
        assert!(Llada2Shape { n_head_kv: 0, ..mini() }.validate().is_err());
    }

    #[test]
    fn a_valid_shape_passes() {
        assert!(mini().validate().is_ok());
    }
}
