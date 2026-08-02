//! mladims.rs — recover MLA latent dimensions from tensor shapes when the metadata omits them.
//!
//! A real GLM-5.2 preview run fails here:
//!
//! ```text
//! load failed: architecture 'glm-dsa' ships MLA tensors but no kv_lora_rank in its
//! metadata, so the latent dimensions are unknown.
//! ```
//!
//! The refusal is honest — the key genuinely is not in the file's metadata. **But the tensors carry
//! it**, and llama.cpp resolves missing hparams from shapes routinely for exactly this reason: a
//! converted checkpoint that lost a key is common, and shapes cannot disagree with themselves.
//!
//! ## The derivation
//!
//! ```text
//! attn_kv_a_norm.weight  [512]            -> kv_lora_rank = 512
//! attn_q_a_norm.weight   [2048]           -> q_lora_rank  = 2048
//! attn_kv_a_mqa.weight   [6144 x 576]     -> 576 - 512 = 64 = qk_rope_head_dim
//! attn_k_b.weight        [192 x 512 x 64] -> 64 heads over the 512 latent
//! attn_v_b.weight        [512 x 256 x 64] -> v_head_dim = 256
//! ```
//!
//! **The norm vector's length IS the latent rank** — `attn_kv_a_norm` normalizes the compressed KV, so
//! it has exactly `kv_lora_rank` elements. And `kv_a_mqa`'s output width is `kv_lora_rank +
//! qk_rope_head_dim` by the DeepSeek-MLA convention, so the rope dim falls out of a subtraction.
//!
//! ## Why deriving is safe here and was not for the fused-qkv split
//!
//! The shapes **over-determine** the answer: four of them constrain three numbers, so they can be
//! checked against each other. `kv_a_mqa`'s width must equal `kv_a_norm`'s length plus the rope dim;
//! `k_b` and `v_b` must agree on the head count; `k_b`'s latent must equal `kv_a_norm`'s length.
//!
//! A file whose tensors disagree fails loudly rather than loading at a guessed rank — which is the
//! opposite of the fused-qkv case, where the head counts and the tensor were the only two facts and
//! either could have been wrong.

/// The latent dimensions an MLA forward pass needs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MlaDims {
    pub kv_lora_rank: usize,
    pub q_lora_rank: usize,
    pub qk_rope_head_dim: usize,
    pub n_head: usize,
    pub v_head_dim: usize,
}

/// Why the shapes could not be resolved.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DeriveError {
    /// A tensor the derivation needs is absent.
    Missing(&'static str),
    /// A tensor is not the rank the derivation expects.
    BadRank { tensor: &'static str, dims: usize },
    /// Two shapes disagree about the same quantity.
    Inconsistent(String),
}

impl DeriveError {
    pub fn explain(&self) -> String {
        match self {
            DeriveError::Missing(t) => format!(
                "{t} is absent, and it is one of the tensors the latent dimensions are read from. \
Without it the rank cannot be recovered from shapes and must come from metadata"),
            DeriveError::BadRank { tensor, dims } => format!(
                "{tensor} has {dims} dimensions, which is not the shape this derivation understands — \
refusing rather than indexing whatever is there"),
            DeriveError::Inconsistent(w) => format!(
                "the tensors disagree: {w}. The shapes over-determine these dimensions, so a \
disagreement means the file is not the layout assumed — and a guess here produces attention over the \
wrong projections, which loads and runs and writes nonsense"),
        }
    }
}

/// Look up a tensor's dims. Returns None when the tensor is absent.
pub type Dims<'a> = &'a dyn Fn(&str) -> Option<Vec<u64>>;

/// Recover the MLA dimensions from tensor shapes.
///
/// `prefix` is the block prefix, e.g. `"blk.0."`.
pub fn derive(prefix: &str, dims: Dims) -> Result<MlaDims, DeriveError> {
    let get = |suffix: &str| dims(&format!("{prefix}{suffix}"));

    let kv_norm = get("attn_kv_a_norm.weight").ok_or(DeriveError::Missing("attn_kv_a_norm.weight"))?;
    if kv_norm.len() != 1 {
        return Err(DeriveError::BadRank { tensor: "attn_kv_a_norm.weight", dims: kv_norm.len() });
    }
    let kv_lora_rank = kv_norm[0] as usize;

    let q_norm = get("attn_q_a_norm.weight").ok_or(DeriveError::Missing("attn_q_a_norm.weight"))?;
    if q_norm.len() != 1 {
        return Err(DeriveError::BadRank { tensor: "attn_q_a_norm.weight", dims: q_norm.len() });
    }
    let q_lora_rank = q_norm[0] as usize;

    // kv_a_mqa is [in, kv_lora_rank + qk_rope_head_dim]: GGUF dims are fastest-varying first, so the
    // OUTPUT width is dims[1].
    let mqa = get("attn_kv_a_mqa.weight").ok_or(DeriveError::Missing("attn_kv_a_mqa.weight"))?;
    if mqa.len() != 2 {
        return Err(DeriveError::BadRank { tensor: "attn_kv_a_mqa.weight", dims: mqa.len() });
    }
    let mqa_out = mqa[1] as usize;
    if mqa_out <= kv_lora_rank {
        return Err(DeriveError::Inconsistent(format!(
            "attn_kv_a_mqa's output width is {mqa_out}, which is not larger than the latent rank \
{kv_lora_rank} taken from attn_kv_a_norm — the rope dimension would be zero or negative")));
    }
    let qk_rope_head_dim = mqa_out - kv_lora_rank;

    // k_b is [qk_nope+rope, kv_lora_rank, n_head]; v_b is [kv_lora_rank, v_head_dim, n_head].
    let k_b = get("attn_k_b.weight").ok_or(DeriveError::Missing("attn_k_b.weight"))?;
    let v_b = get("attn_v_b.weight").ok_or(DeriveError::Missing("attn_v_b.weight"))?;
    if k_b.len() != 3 { return Err(DeriveError::BadRank { tensor: "attn_k_b.weight", dims: k_b.len() }); }
    if v_b.len() != 3 { return Err(DeriveError::BadRank { tensor: "attn_v_b.weight", dims: v_b.len() }); }

    let n_head = k_b[2] as usize;
    if v_b[2] as usize != n_head {
        return Err(DeriveError::Inconsistent(format!(
            "attn_k_b has {n_head} heads and attn_v_b has {}", v_b[2])));
    }
    if k_b[1] as usize != kv_lora_rank {
        return Err(DeriveError::Inconsistent(format!(
            "attn_k_b's latent axis is {} and attn_kv_a_norm says the rank is {kv_lora_rank}", k_b[1])));
    }
    if v_b[0] as usize != kv_lora_rank {
        return Err(DeriveError::Inconsistent(format!(
            "attn_v_b's latent axis is {} and attn_kv_a_norm says the rank is {kv_lora_rank}", v_b[0])));
    }
    let v_head_dim = v_b[1] as usize;

    if kv_lora_rank == 0 || q_lora_rank == 0 || n_head == 0 || v_head_dim == 0 {
        return Err(DeriveError::Inconsistent(
            "a derived dimension is zero, which no forward pass can use".into()));
    }
    Ok(MlaDims { kv_lora_rank, q_lora_rank, qk_rope_head_dim, n_head, v_head_dim })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real GLM-5.2 preview shapes, transcribed from a load attempt's tensor dump.
    fn glm52(name: &str) -> Option<Vec<u64>> {
        Some(match name {
            "blk.0.attn_kv_a_norm.weight" => vec![512],
            "blk.0.attn_q_a_norm.weight"  => vec![2048],
            "blk.0.attn_kv_a_mqa.weight"  => vec![6144, 576],
            "blk.0.attn_k_b.weight"       => vec![192, 512, 64],
            "blk.0.attn_v_b.weight"       => vec![512, 256, 64],
            _ => return None,
        })
    }

    #[test]
    fn the_REAL_glm52_shapes_resolve() {
        // Not invented dimensions: these are the shapes from an actual failed load.
        let d = derive("blk.0.", &glm52).expect("the shapes over-determine this");
        assert_eq!(d.kv_lora_rank, 512, "the norm vector's LENGTH is the latent rank");
        assert_eq!(d.q_lora_rank, 2048);
        assert_eq!(d.qk_rope_head_dim, 64, "576 - 512, the DeepSeek-MLA convention");
        assert_eq!(d.n_head, 64);
        assert_eq!(d.v_head_dim, 256);
    }

    #[test]
    fn a_HEAD_COUNT_disagreement_is_refused() {
        // The check the over-determination buys. k_b and v_b both carry the head count, so a file
        // where they differ is not the layout assumed.
        let f = |n: &str| if n == "blk.0.attn_v_b.weight" { Some(vec![512, 256, 32]) } else { glm52(n) };
        let e = derive("blk.0.", &f).unwrap_err();
        assert!(matches!(e, DeriveError::Inconsistent(_)));
        assert!(e.explain().contains("writes nonsense"));
    }

    #[test]
    fn a_LATENT_disagreement_is_refused() {
        // k_b's middle axis must equal the norm's length. If they differ, one of them is not what the
        // derivation thinks it is — and slicing at the wrong rank is silent.
        let f = |n: &str| if n == "blk.0.attn_k_b.weight" { Some(vec![192, 256, 64]) } else { glm52(n) };
        assert!(matches!(derive("blk.0.", &f), Err(DeriveError::Inconsistent(_))));
    }

    #[test]
    fn an_mqa_width_at_or_below_the_rank_is_refused() {
        // qk_rope_head_dim comes from a subtraction, and a subtraction that underflows would wrap to
        // an enormous dimension rather than failing.
        let f = |n: &str| if n == "blk.0.attn_kv_a_mqa.weight" { Some(vec![6144, 512]) } else { glm52(n) };
        let e = derive("blk.0.", &f).unwrap_err();
        assert!(e.explain().contains("zero or negative"));
    }

    #[test]
    fn a_missing_tensor_NAMES_itself() {
        // So the message says which tensor to look for rather than "the latent dimensions are unknown".
        let f = |n: &str| if n == "blk.0.attn_kv_a_norm.weight" { None } else { glm52(n) };
        let e = derive("blk.0.", &f).unwrap_err();
        assert_eq!(e, DeriveError::Missing("attn_kv_a_norm.weight"));
        assert!(e.explain().contains("attn_kv_a_norm.weight"));
    }

    #[test]
    fn a_dense_model_is_MISSING_rather_than_inconsistent() {
        // A model with no MLA tensors at all must report absence, so a caller can treat it as "not
        // applicable" instead of "this file is broken".
        let none = |_: &str| None;
        assert!(matches!(derive("blk.0.", &none), Err(DeriveError::Missing(_))));
    }

    #[test]
    fn a_wrong_RANK_tensor_is_refused_rather_than_indexed() {
        // attn_k_b as a 2-D tensor would still index [0] and [1] and produce numbers.
        let f = |n: &str| if n == "blk.0.attn_k_b.weight" { Some(vec![192, 512]) } else { glm52(n) };
        assert!(matches!(derive("blk.0.", &f), Err(DeriveError::BadRank { .. })));
    }
}
