//! kvbudget.rs — how much context fits, once the weights have taken their share.
//!
//! "1M context" is a claim about the KV cache and it is answered by the wrong arithmetic almost
//! every time. **The cache does not get the whole device.** The weights are resident first, and what
//! is left decides the context — so a model that fits comfortably and a model that fits exactly give
//! very different context ceilings on the same hardware.
//!
//! ## MLA is what makes the question interesting
//!
//! A standard GQA cache stores K and V per head. **MLA stores a compressed latent and reconstructs
//! them**, which for GLM-4.7-Flash's shapes is 54 KiB/token against GQA's 192 — a 3.6x difference
//! that moves 1M context from absurd to arguable:
//!
//! ```text
//! 1,000,000 tokens        f16        q8
//!   MLA latent          51.5 GiB   25.7 GiB
//!   GQA K+V            183.1 GiB   91.6 GiB
//! ```
//!
//! ## And 25.7 GiB still is not the answer
//!
//! It fits under a 31 GiB ceiling with 5.3 GiB spare, and **no useful model fits in 5.3 GiB**. The
//! honest figure is context AFTER weights, which is what this computes.

/// The shapes that decide cache size.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct KvShape {
    pub layers: u32,
    /// MLA: the compressed latent width. GQA: `kv_heads * head_dim * 2` for K and V together.
    pub per_layer_elems: u32,
    /// True when this is a compressed-latent cache rather than stored K and V.
    pub is_mla: bool,
}

impl KvShape {
    /// GLM-4.7-Flash, read from its own tensor dump: `attn_kv_a_mqa [2048x576]` and
    /// `attn_kv_a_norm [512]` give a 512-wide latent plus 64 rope dims.
    pub fn glm_flash(layers: u32) -> Self {
        KvShape { layers, per_layer_elems: 512 + 64, is_mla: true }
    }

    /// A comparable GQA model: 8 KV heads at 128 dims, K and V both stored.
    pub fn gqa(layers: u32, kv_heads: u32, head_dim: u32) -> Self {
        KvShape { layers, per_layer_elems: 2 * kv_heads * head_dim, is_mla: false }
    }

    pub fn bytes_per_token(&self, bytes_per_elem: f32) -> f64 {
        self.layers as f64 * self.per_layer_elems as f64 * bytes_per_elem as f64
    }
}

/// Bytes per element for a cache precision.
pub fn kv_elem_bytes(dtype: &str) -> Option<f32> {
    match dtype { "f16" => Some(2.0), "q8" => Some(1.0), "q4" => Some(0.5), _ => None }
}

/// The most context that fits, given the device ceiling and what the weights already take.
///
/// Returns an error rather than zero when the weights alone exceed the ceiling, because "0 tokens"
/// invites tuning the cache when the problem is the model.
pub fn max_context(
    shape: &KvShape, dtype: &str, ceiling_gib: f64, weights_gib: f64,
) -> Result<u64, String> {
    let Some(b) = kv_elem_bytes(dtype) else {
        return Err(format!("unknown cache dtype '{dtype}'"));
    };
    if weights_gib >= ceiling_gib {
        return Err(format!(
            "the weights alone are {weights_gib:.1} GiB against a {ceiling_gib:.1} GiB ceiling, so \
there is no context budget to compute — the model is the problem rather than the cache"));
    }
    let left = (ceiling_gib - weights_gib) * (1024f64.powi(3));
    Ok((left / shape.bytes_per_token(b)) as u64)
}

/// What it would take to reach a target context.
///
/// **Reports the shortfall in the terms the operator can act on** — a smaller quant or a coarser
/// cache — rather than a bare "does not fit".
pub fn plan_for(
    shape: &KvShape, target_ctx: u64, ceiling_gib: f64, weights_gib: f64,
) -> String {
    let mut lines = vec![format!("{target_ctx} tokens on a {ceiling_gib:.0} GiB device with \
{weights_gib:.1} GiB of weights:")];
    for dt in ["f16", "q8", "q4"] {
        let b = kv_elem_bytes(dt).unwrap();
        let need = shape.bytes_per_token(b) * target_ctx as f64 / 1024f64.powi(3);
        let total = need + weights_gib;
        lines.push(format!("  KV {dt:>3}: {need:6.1} GiB cache + {weights_gib:.1} weights = \
{total:6.1} GiB  {}", if total <= ceiling_gib { "FITS" } else { "over" }));
    }
    if !shape.is_mla {
        lines.push("  (this is a GQA cache; an MLA model of similar size stores a compressed latent \
and needs roughly a third of this)".into());
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn MLA_is_roughly_a_third_of_GQA_for_the_same_model_size() {
        // The difference that moves 1M context from absurd to arguable, and the reason GLM's
        // architecture matters more than its parameter count here.
        let mla = KvShape::glm_flash(48).bytes_per_token(2.0);
        let gqa = KvShape::gqa(48, 8, 128).bytes_per_token(2.0);
        assert!(gqa / mla > 3.0 && gqa / mla < 4.0, "ratio {}", gqa / mla);
    }

    #[test]
    fn a_1M_MLA_cache_FITS_a_31_GiB_device_and_leaves_no_room_for_a_model() {
        // The arithmetic that looks like a win and is not: 25.7 GiB of cache under a 31 GiB ceiling,
        // and no useful model fits in the 5.3 GiB left.
        let alone = max_context(&KvShape::glm_flash(48), "q8", 31.0, 0.0).unwrap();
        assert!(alone > 1_000_000, "1M fits with no weights: {alone}");
        let real = max_context(&KvShape::glm_flash(48), "q8", 31.0, 15.3).unwrap();
        assert!(real < 700_000, "and does not once the model is resident: {real}");
    }

    #[test]
    fn a_COARSER_cache_buys_context_linearly() {
        // q4 doubles q8 doubles f16 — the one lever that is purely arithmetic, and the reason the
        // plan reports all three rather than recommending one.
        let s = KvShape::glm_flash(48);
        let f16 = max_context(&s, "f16", 31.0, 15.3).unwrap();
        let q8 = max_context(&s, "q8", 31.0, 15.3).unwrap();
        assert!((q8 as f64 / f16 as f64 - 2.0).abs() < 0.01);
    }

    #[test]
    fn WEIGHTS_OVER_THE_CEILING_is_an_error_rather_than_zero_tokens() {
        // "0 tokens" invites tuning the cache when the problem is the model.
        let e = max_context(&KvShape::glm_flash(48), "q8", 31.0, 40.0).unwrap_err();
        assert!(e.contains("the model is the problem rather than the cache"));
    }

    #[test]
    fn the_plan_reports_every_precision_rather_than_choosing() {
        // Cache precision is a quality decision the measurement does not own.
        let p = plan_for(&KvShape::glm_flash(48), 1_000_000, 31.0, 15.3);
        for dt in ["f16", "q8", "q4"] { assert!(p.contains(&format!("KV {dt:>3}"))); }
        assert!(p.contains("FITS"), "q4 should reach 1M with these weights");
    }

    #[test]
    fn a_GQA_plan_says_an_MLA_model_would_need_a_third() {
        // So a reader comparing two models does not conclude the cache is simply large.
        let p = plan_for(&KvShape::gqa(48, 8, 128), 128_000, 31.0, 15.3);
        assert!(p.contains("compressed latent and needs roughly a third"));
    }

    #[test]
    fn an_unknown_dtype_is_refused_rather_than_defaulted() {
        assert!(max_context(&KvShape::glm_flash(48), "fp8", 31.0, 0.0).is_err());
    }
}
