//! mla.rs — DeepSeek multi-head latent attention (MLA): the pure dimension & projection planner.
//!
//! MLA's win is the KV cache: instead of storing full per-head K and V, it caches a single low-rank
//! *latent* (plus a small decoupled-RoPE key shared across heads) and up-projects per head at attention
//! time — shrinking the cache by an order of magnitude, which is exactly what lets `kvcache` pack long
//! contexts. The attention/up-projection kernels are the GPU seam; the dims, the cache footprint, and the
//! projection GEMM shapes (which feed `gemmplan`) are pure and unit-tested here.
#![allow(dead_code)]

/// MLA dimensions (read from the GGUF hparams). `q_lora_rank == 0` means Q is projected directly (no
/// low-rank Q compression), as in smaller DeepSeek variants.
#[derive(Clone, Copy, Debug)]
pub struct MlaConfig {
    pub d_model: u32,
    pub n_head: u32,
    pub qk_nope_head_dim: u32, // the non-RoPE part of each head's QK dim
    pub qk_rope_head_dim: u32, // the decoupled RoPE part (carried separately, shared key)
    pub v_head_dim: u32,
    pub kv_lora_rank: u32,     // compressed KV latent width (what actually gets cached)
    pub q_lora_rank: u32,      // compressed Q latent width (0 = none)
}

/// A projection GEMM: `out_features × in_features` weight applied to a token. Feed `(in, out)` to
/// `gemmplan` as the matmul's K and N.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Proj { pub name: &'static str, pub in_features: u32, pub out_features: u32 }

impl MlaConfig {
    /// Full per-head QK dim (non-RoPE + RoPE parts).
    pub fn qk_head_dim(&self) -> u32 { self.qk_nope_head_dim + self.qk_rope_head_dim }

    /// Elements cached per token under MLA: the shared KV latent plus the one decoupled-RoPE key.
    pub fn latent_cache_elems(&self) -> u32 { self.kv_lora_rank + self.qk_rope_head_dim }

    /// Elements a plain MHA cache would store per token: every head's K and V.
    pub fn mha_cache_elems(&self) -> u32 { self.n_head * (self.qk_head_dim() + self.v_head_dim) }

    /// KV-cache shrink factor from MLA (MHA elements ÷ latent elements) — typically ~10–70× on DeepSeek.
    pub fn cache_compression(&self) -> f32 {
        let latent = self.latent_cache_elems().max(1);
        self.mha_cache_elems() as f32 / latent as f32
    }

    /// Bytes cached per token under MLA at `dtype_bytes` per element.
    pub fn latent_cache_bytes(&self, dtype_bytes: u32) -> u64 {
        self.latent_cache_elems() as u64 * dtype_bytes as u64
    }

    /// The projection GEMMs MLA needs, in order. The engine plans each with `gemmplan`.
    pub fn projections(&self) -> Vec<Proj> {
        let qk = self.qk_head_dim();
        let mut v = Vec::new();
        if self.q_lora_rank > 0 {
            v.push(Proj { name: "q_down", in_features: self.d_model, out_features: self.q_lora_rank });
            v.push(Proj { name: "q_up", in_features: self.q_lora_rank, out_features: self.n_head * qk });
        } else {
            v.push(Proj { name: "q", in_features: self.d_model, out_features: self.n_head * qk });
        }
        // KV down-projection produces the cached latent + the decoupled RoPE key
        v.push(Proj { name: "kv_down", in_features: self.d_model, out_features: self.kv_lora_rank + self.qk_rope_head_dim });
        // KV up-projection expands the latent into per-head non-RoPE K and V
        v.push(Proj { name: "kv_up", in_features: self.kv_lora_rank, out_features: self.n_head * (self.qk_nope_head_dim + self.v_head_dim) });
        v.push(Proj { name: "o", in_features: self.n_head * self.v_head_dim, out_features: self.d_model });
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // DeepSeek-V2-shaped dims
    fn v2() -> MlaConfig {
        MlaConfig { d_model: 5120, n_head: 128, qk_nope_head_dim: 128, qk_rope_head_dim: 64,
                    v_head_dim: 128, kv_lora_rank: 512, q_lora_rank: 1536 }
    }

    #[test]
    fn head_dim_and_cache_footprint() {
        let c = v2();
        assert_eq!(c.qk_head_dim(), 192);
        assert_eq!(c.latent_cache_elems(), 576);            // 512 + 64
        assert_eq!(c.mha_cache_elems(), 128 * (192 + 128)); // 40960
    }

    #[test]
    fn mla_compresses_the_kv_cache_by_an_order_of_magnitude() {
        let c = v2();
        assert!((c.cache_compression() - 40960.0 / 576.0).abs() < 1e-3);
        assert!(c.cache_compression() > 60.0, "MLA should shrink the cache dramatically");
        // and in bytes (fp16) it's tiny vs a 40k-element MHA cache
        assert_eq!(c.latent_cache_bytes(2), 576 * 2);
    }

    #[test]
    fn projection_shapes_are_correct() {
        let p = v2().projections();
        let get = |n: &str| *p.iter().find(|x| x.name == n).unwrap();
        assert_eq!(get("q_down"), Proj { name: "q_down", in_features: 5120, out_features: 1536 });
        assert_eq!(get("q_up"), Proj { name: "q_up", in_features: 1536, out_features: 128 * 192 });
        assert_eq!(get("kv_down"), Proj { name: "kv_down", in_features: 5120, out_features: 512 + 64 });
        assert_eq!(get("kv_up"), Proj { name: "kv_up", in_features: 512, out_features: 128 * (128 + 128) });
        assert_eq!(get("o"), Proj { name: "o", in_features: 128 * 128, out_features: 5120 });
    }

    #[test]
    fn no_q_lora_projects_q_directly() {
        let c = MlaConfig { q_lora_rank: 0, ..v2() };
        let p = c.projections();
        assert!(p.iter().any(|x| x.name == "q" && x.out_features == 128 * 192));
        assert!(!p.iter().any(|x| x.name == "q_down"), "no Q compression ⇒ no q_down stage");
    }
}
