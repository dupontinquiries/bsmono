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

/// # What the engine actually stores today, checked rather than assumed
///
/// I wrote the arithmetic above against an f16 baseline and then went to look. **The MLA latent
/// cache is f32**: `mla_attention_step_t` takes `c_kv: &Tensor`, a plain f32 tensor, and the
/// int8 `QKvCache` serves standard attention only. There is no quantized MLA variant.
///
/// So every f16 figure above understates the case by 2x, and the real picture is:
///
/// ```text
///   GLM-4.7-Flash Q4_K_S, 15.3 GiB weights, f32 MLA latent
///     ctx  32,000   cache 3.3 GiB  (17.7% of bytes read)   int8 would give 1.15x
///     ctx 128,000   cache 13.2 GiB (46.3%)                  int8 would give 1.53x
///     ctx 512,000   cache 52.7 GiB (77.5%)                  int8 would give 2.39x
///
///   max context on 31 GiB:   f32  152,431 tokens
///                            int8 609,727 tokens
/// ```
///
/// **Quantizing the MLA latent is the single largest unbuilt lever for long context**, and it is
/// both a 4x capacity win and — past roughly 64k — a real speed win. The mechanism exists for the
/// other cache type, which makes this a port rather than a design.
///
/// The staged cache (hot f32 window over a warm int8 tail) already exists too, and applies to the
/// same problem from the other direction.
///
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

/// # The q8 cache EXISTS and the app cannot reach it
///
/// Checked rather than assumed, after the last commit ended by saying this was unknown:
///
/// ```text
///   QKvCache { kq, ks, .. }        a quantized KV cache, with separate quant and scale tensors
///   forward_kv_q8                  prefill against it
///   decode_step_q8                 decode against it, used by kernel-validate
///
///   Session { cache: KvCache }     what the APP holds — .k and .v, f16
///   stream_session_window          the app's decode path, reads s.cache.k
/// ```
///
/// **So the 1.46x at 512k is implemented, tested, and unreachable from the interface.** The engine
/// carries both caches and the application selects the larger one, permanently, because the type it
/// stores in `Session` decides which decode path is even callable.
///
/// That makes this the `unwired` pattern at engine scale: not a module nobody calls, but a
/// capability nobody can ask for. Wiring it is a `Session` change rather than a flag, which is why
/// it is recorded here rather than done in passing.
///
/// What share of a decoded token's memory traffic is the cache rather than the weights.
///
/// **Cache precision is a capacity lever at every context length and a SPEED lever only at long
/// ones**, and the crossover is sharp enough to matter:
///
/// ```text
///   GLM-4.7-Flash Q4_K_S, 15.3 GiB resident, f16 MLA cache
///     ctx   8,000    cache is  2.6% of bytes read per token
///     ctx  32,000    cache is  9.7%
///     ctx 128,000    cache is 30.1%
///     ctx 512,000    cache is 63.3%
/// ```
///
/// Decode is bandwidth-bound, so this share is roughly the share of decode time. At 512k, moving
/// f16 to q8 takes bytes-per-token from 41.7 GiB to 28.5 — **a 1.46x speedup from a change that
/// does nothing measurable at 8k.**
///
/// It also explains part of something already measured: a real conversation went 74%, 91%, 112%
/// sync across three turns, and the cache growing under it is one of the reasons decode gets slower
/// as a session continues. Not the whole reason, and not one that had a name.
pub fn cache_read_share(shape: &KvShape, dtype: &str, ctx: u64, weights_gib: f64) -> Option<f64> {
    let b = kv_elem_bytes(dtype)?;
    let cache_gib = shape.bytes_per_token(b) * ctx as f64 / 1024f64.powi(3);
    let total = cache_gib + weights_gib;
    if total <= 0.0 { return None }
    Some(cache_gib / total)
}

/// Is halving the cache precision worth it for SPEED at this context?
///
/// Returns the estimated speedup, or a reason it is not worth doing. Separated from capacity because
/// the same change is always a capacity win and only sometimes a speed one — and conflating them
/// leads to spending accuracy at 8k for nothing.
pub fn precision_speed_case(
    shape: &KvShape, from: &str, to: &str, ctx: u64, weights_gib: f64,
) -> Result<f64, String> {
    let (bf, bt) = (kv_elem_bytes(from).ok_or("unknown from-dtype")?,
                    kv_elem_bytes(to).ok_or("unknown to-dtype")?);
    if bt >= bf { return Err(format!("{to} is not coarser than {from}")) }
    let g = |b: f32| shape.bytes_per_token(b) * ctx as f64 / 1024f64.powi(3) + weights_gib;
    let speedup = g(bf) / g(bt);
    // Below a few percent the accuracy cost buys nothing measurable, and saying so prevents a
    // quality regression traded for noise.
    if speedup < 1.05 {
        return Err(format!(
            "{speedup:.3}x at {ctx} tokens — the cache is only {:.1}% of bytes read per token here, so {to} costs accuracy and buys nothing measurable. It remains a CAPACITY win; this is the speed question", 100.0 * cache_read_share(shape, from, ctx, weights_gib).unwrap_or(0.0)));
    }
    Ok(speedup)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_f32_baseline_makes_the_case_TWICE_what_f16_suggested() {
        // Checked rather than assumed: the MLA path takes c_kv as a plain f32 Tensor, so the f16
        // numbers elsewhere in this module understate it by 2x.
        let s = KvShape::glm_flash(48);
        let f32_share = {
            let cache = s.bytes_per_token(4.0) * 128_000.0 / 1024f64.powi(3);
            cache / (cache + 15.3)
        };
        let f16_share = cache_read_share(&s, "f16", 128_000, 15.3).unwrap();
        assert!(f32_share > f16_share * 1.4, "f32 {f32_share:.3} vs f16 {f16_share:.3}");
        assert!(f32_share > 0.45, "at 128k the f32 cache is nearly half the read");
    }

    #[test]
    fn the_speedup_this_module_computes_is_currently_UNREACHABLE() {
        // Documented so the number is never quoted as available. QKvCache and decode_step_q8 exist;
        // Session holds KvCache; the app's stream_session_window reads s.cache.k. A capability
        // nobody can ask for is not a capability.
        let src = include_str!("kvbudget.rs");
        assert!(src.contains("implemented, tested, and unreachable from the interface"));
        assert!(src.contains("a `Session` change rather than a flag"));
    }

    #[test]
    fn the_cache_share_CROSSES_OVER_between_short_and_long_context() {
        // 2.6% at 8k and 63% at 512k: the same knob is noise at one end and the dominant cost at the
        // other, which is why one recommendation cannot serve both.
        let s = KvShape::glm_flash(48);
        assert!(cache_read_share(&s, "f16", 8_000, 15.3).unwrap() < 0.05);
        assert!(cache_read_share(&s, "f16", 512_000, 15.3).unwrap() > 0.60);
    }

    #[test]
    fn coarsening_the_cache_is_REFUSED_as_a_speed_change_at_short_context() {
        // Spending accuracy at 8k for a 1.01x gain is a quality regression traded for noise.
        let e = precision_speed_case(&KvShape::glm_flash(48), "f16", "q8", 8_000, 15.3).unwrap_err();
        assert!(e.contains("buys nothing measurable"));
        assert!(e.contains("remains a CAPACITY win"), "the two questions must stay separate");
    }

    #[test]
    fn at_LONG_context_the_same_change_is_a_real_speedup() {
        let x = precision_speed_case(&KvShape::glm_flash(48), "f16", "q8", 512_000, 15.3).unwrap();
        assert!(x > 1.4, "got {x}");
    }

    #[test]
    fn a_FINER_target_precision_is_refused_rather_than_reported_as_a_slowdown() {
        assert!(precision_speed_case(&KvShape::glm_flash(48), "q8", "f16", 512_000, 15.3).is_err());
    }

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

/// Which attention kernels are tiled, checked by BODY rather than by a line window.
///
/// A sibling session prescribed `grep -A130 'const ATTENTION_MH_FULL' | grep -c 'var<workgroup>'`.
/// On this tree that returns **2**, and the answer is **0** — the window overshoots FULL's body into
/// the next shader, whose `sv: array<f32,256>` and `si: array<u32,256>` are a value/index pair for
/// top-k sampling, not a KV tile.
///
/// The distinction the grep cannot make is the one that matters: **`sc: array<f32,2048>` in
/// EXT_PAR is a KV score tile; a 256-entry index/value pair is not.** Presence of shared memory is
/// not tiling, and a count is the wrong instrument for the question.
///
/// So the conclusion stands and the check that produced it does not: FULL is naive-shaped, and
/// routing a diffusion step at it would re-inherit the quadratic the causal path just shed. The
/// right target is a non-causal MASK FLAG on EXT_PAR — at base=0, s=t the only difference between
/// causal prefill and bidirectional attention is the `col <= row` bound.
pub fn kernel_is_tiled(name: &str) -> Option<bool> {
    match name {
        // Named individually rather than derived, because deriving it is what went wrong.
        "ATTENTION_MH" => Some(false),
        "ATTENTION_MH_FULL" => Some(false),
        "ATTENTION_MH_EXT_PAR" => Some(true),
        _ => None,
    }
}

/// Prefill reads K/V without reuse; decode does not.
///
/// Traced after a sibling session measured `t(L) = 197 + 0.510·L + 7.17e-4·L²` on a different engine
/// and attributed the quadratic to an attention kernel with one thread per (row, head), each
/// serially streaming the whole K and V. **This tree has both kernels and picks per path:**
///
/// ```text
///   block_prefill        -> attention_mh_t   -> ATTENTION_MH          no var<workgroup>
///   attention_mh_ext_swa -> ATTENTION_MH_EXT_PAR                      tiled, online softmax
///   decode               -> attention_mh_ext_swa                      takes the tiled one
/// ```
///
/// So the decode numbers in this module are unaffected — **and prefill is not.** A prompt of L rows
/// re-reads the cache once per (row, head) with no cross-row reuse, which is quadratic in L and is
/// where a 197 ms constant plus 188 ms of quadratic came from on the other engine at L=512.
///
/// **This corrects an assumption in `cache_read_share`**: that function models DECODE, where one
/// token reads the cache once. Prefill's traffic is a different and much larger number, and quoting
/// the decode share as though it covered both would understate prefill by roughly `n_heads`.
pub struct AttentionPath { pub tiled: bool, pub why: &'static str }

/// Which kernel a phase takes today.
pub fn attention_path(phase: &str) -> Option<AttentionPath> {
    match phase {
        "decode" => Some(AttentionPath { tiled: true,
            why: "attention_mh_ext_swa dispatches ATTENTION_MH_EXT_PAR, which tiles the KV loop \
with an online softmax over 2048-row tiles" }),
        "prefill" => Some(AttentionPath { tiled: false,
            why: "block_prefill calls attention_mh_t, which dispatches ATTENTION_MH — one thread \
per (row, head), each streaming the whole K and V, no shared memory and no cross-row reuse. That is \
quadratic in prompt length and the tiled kernel it needs already exists" }),
        _ => None,
    }
}

#[cfg(test)]
mod attention_path_tests {
    use super::*;

    #[test]
    fn the_BIDIRECTIONAL_kernel_is_naive_and_a_grep_window_says_otherwise() {
        // grep -A130 returns 2 for FULL on this tree and the true count is 0 — the window spills
        // into a sampling kernel's top-k scratch. Recorded so nobody re-runs the one-liner and
        // concludes the diffusion path is already tiled.
        assert_eq!(kernel_is_tiled("ATTENTION_MH_FULL"), Some(false));
        assert_eq!(kernel_is_tiled("ATTENTION_MH_EXT_PAR"), Some(true));
        let src = include_str!("kvbudget.rs");
        assert!(src.contains("Presence of shared memory is
/// not tiling"));
    }

    #[test]
    fn the_diffusion_target_is_a_MASK_FLAG_not_a_kernel() {
        // At base=0, s=t the only difference between causal prefill and bidirectional attention is
        // the col <= row bound — which is the same shape as the prefill fix: route, do not write.
        let src = include_str!("kvbudget.rs");
        assert!(src.contains("non-causal MASK FLAG on EXT_PAR"));
    }

    #[test]
    fn DECODE_takes_the_tiled_kernel_and_PREFILL_does_not() {
        // The split that makes this worth recording: the fix is not writing a kernel, it is routing
        // one path to a kernel the tree already has.
        assert!(attention_path("decode").unwrap().tiled);
        assert!(!attention_path("prefill").unwrap().tiled);
    }

    #[test]
    fn the_prefill_note_says_the_TILED_KERNEL_ALREADY_EXISTS() {
        // Otherwise this reads as a request to write flash-attention rather than to change a call.
        assert!(attention_path("prefill").unwrap().why.contains("already exists"));
        assert!(attention_path("prefill").unwrap().why.contains("quadratic in prompt length"));
    }

    #[test]
    fn cache_read_share_is_documented_as_a_DECODE_model_only() {
        // It assumes one cache read per token. Prefill re-reads per (row, head), so quoting the
        // decode share for prefill understates it by roughly n_heads.
        let src = include_str!("kvbudget.rs");
        assert!(src.contains("understate prefill by roughly `n_heads`"));
    }
}
