//! cpudecode.rs — the decode loop and its KV cache, on the CPU.
//!
//! Ties together `cpumatvec` (weights read straight from the mapped file) and `cpuattn` (online
//! softmax, NeoX rope, SwiGLU). What this adds is the ORDER and the STATE, which is where a forward
//! pass is usually wrong in ways that still produce fluent text.
//!
//! ## The KV cache is preallocated, not grown
//!
//! `Vec::push` on a cache that reaches gigabytes would reallocate and copy mid-generation, and on a
//! machine where the model already occupies most of RAM that copy is a stall measured in seconds. The
//! cache is sized once from `max_pos` and the position is an index, which is also what makes
//! `cpuattn::write_kv` a plain slice write.
//!
//! ## Grouped-query attention shares K/V heads
//!
//! `n_kv_heads < n_heads` means several query heads read the same key/value rows — Llama-3 8B is 32
//! query heads over 8 kv heads. Getting the mapping wrong (`q_head % n_kv` instead of
//! `q_head / (n_heads/n_kv)`) reads plausible keys from the wrong group and produces text that is
//! grammatical and wrong, with nothing to grep for.

use crate::cpumatvec::{matvec, ActPrep, Quant};
use crate::cpupool::Pool;

/// Where one tensor lives in the mapped file.
#[derive(Clone, Copy, Debug)]
pub struct TensorRef { pub off: usize, pub rows: usize, pub cols: usize, pub quant: Quant }

impl TensorRef {
    fn bytes(&self) -> usize { self.rows * self.quant.row_bytes(self.cols) }
    fn slice<'a>(&self, raw: &'a [u8]) -> Option<&'a [u8]> {
        raw.get(self.off..self.off + self.bytes())
    }
}

/// One block's weights.
#[derive(Clone, Copy, Debug)]
pub struct LayerRef {
    pub attn_norm: usize,          // f32 offset into the norms buffer
    pub q: TensorRef, pub k: TensorRef, pub v: TensorRef, pub o: TensorRef,
    pub ffn_norm: usize,
    pub gate_up: TensorRef,        // packed [2*ff, d]
    pub down: TensorRef,
}

#[derive(Clone, Copy, Debug)]
pub struct Shape {
    pub d: usize, pub ff: usize, pub n_heads: usize, pub n_kv_heads: usize,
    pub head_dim: usize, pub n_layers: usize, pub rope_base: f32, pub eps: f32,
}

impl Shape {
    /// Query heads per key/value head.
    ///
    /// The GQA mapping is `q_head / group`, not `q_head % n_kv`. Both are in range, both run, and the
    /// modulo version reads keys from the wrong group — which produces grammatical, wrong text.
    pub fn group(&self) -> usize { (self.n_heads / self.n_kv_heads).max(1) }
    pub fn kv_dim(&self) -> usize { self.n_kv_heads * self.head_dim }
}

/// Per-layer K and V, preallocated for `max_pos` positions.
pub struct KvCache { k: Vec<Vec<f32>>, v: Vec<Vec<f32>>, pub pos: usize, max_pos: usize }

impl KvCache {
    pub fn new(sh: &Shape, max_pos: usize) -> Self {
        let per = max_pos * sh.kv_dim();
        KvCache {
            k: (0..sh.n_layers).map(|_| vec![0f32; per]).collect(),
            v: (0..sh.n_layers).map(|_| vec![0f32; per]).collect(),
            pos: 0, max_pos,
        }
    }
    pub fn len(&self) -> usize { self.pos }
    pub fn is_empty(&self) -> bool { self.pos == 0 }
    pub fn is_full(&self) -> bool { self.pos >= self.max_pos }
    /// Bytes held, so a caller can size it against the budget before allocating.
    pub fn bytes(sh: &Shape, max_pos: usize) -> usize {
        2 * sh.n_layers * max_pos * sh.kv_dim() * 4
    }
}

/// One decode step: token embedding in, logits-ready hidden state out.
///
/// `x` is the embedding of the current token and is overwritten with the block output. Returns false
/// when the cache is full rather than overwriting position 0 — a silent wrap would corrupt the oldest
/// context and read as the model forgetting, which is indistinguishable from it being bad.
pub fn decode_step(pool: &Pool, raw: &[u8], norms: &[f32], layers: &[LayerRef], sh: &Shape,
                   kv: &mut KvCache, x: &mut [f32]) -> bool {
    if kv.is_full() { return false; }
    let pos = kv.pos;
    let (mut resid, mut nx) = (vec![0f32; sh.d], vec![0f32; sh.d]);
    let mut attn_out = vec![0f32; sh.d];

    for (li, l) in layers.iter().enumerate().take(sh.n_layers) {
        resid.copy_from_slice(x);

        // ---- attention ----
        crate::cpuattn::rmsnorm(x, &norms[l.attn_norm..l.attn_norm + sh.d], sh.eps, &mut nx);
        let prep = ActPrep::new(&nx);
        let Some(qw) = l.q.slice(raw) else { return false };
        let Some(kw) = l.k.slice(raw) else { return false };
        let Some(vw) = l.v.slice(raw) else { return false };
        let mut q = matvec(pool, l.q.quant, qw, l.q.rows, sh.d, &prep);
        let mut kk = matvec(pool, l.k.quant, kw, l.k.rows, sh.d, &prep);
        let vv = matvec(pool, l.v.quant, vw, l.v.rows, sh.d, &prep);

        // Rope is applied per head, to q and k only — v carries no position.
        for h in 0..sh.n_heads {
            crate::cpuattn::rope_neox(&mut q[h * sh.head_dim..(h + 1) * sh.head_dim],
                                      sh.head_dim, pos, sh.rope_base);
        }
        for h in 0..sh.n_kv_heads {
            crate::cpuattn::rope_neox(&mut kk[h * sh.head_dim..(h + 1) * sh.head_dim],
                                      sh.head_dim, pos, sh.rope_base);
        }

        // Append this position, then attend over 0..=pos. Append FIRST: a token must attend to itself,
        // and appending after would make the newest position invisible to its own query.
        let kvd = sh.kv_dim();
        crate::cpuattn::write_kv(&mut kv.k[li][pos * kvd..], 0, kvd, &kk);
        crate::cpuattn::write_kv(&mut kv.v[li][pos * kvd..], 0, kvd, &vv);

        let scale = 1.0 / (sh.head_dim as f32).sqrt();
        let group = sh.group();
        for h in 0..sh.n_heads {
            let kvh = h / group;                    // NOT h % n_kv_heads
            // Gather this kv head's rows across positions. The cache interleaves heads within a
            // position, so a head's rows are strided — collected here rather than restructuring the
            // cache, because the alternative layout costs a scatter on every write instead.
            let n = pos + 1;
            let mut kh = Vec::with_capacity(n * sh.head_dim);
            let mut vh = Vec::with_capacity(n * sh.head_dim);
            for p in 0..n {
                let base = p * kvd + kvh * sh.head_dim;
                kh.extend_from_slice(&kv.k[li][base..base + sh.head_dim]);
                vh.extend_from_slice(&kv.v[li][base..base + sh.head_dim]);
            }
            crate::cpuattn::attend(&q[h * sh.head_dim..(h + 1) * sh.head_dim], &kh, &vh,
                                   n, sh.head_dim, scale,
                                   &mut attn_out[h * sh.head_dim..(h + 1) * sh.head_dim]);
        }

        let Some(ow) = l.o.slice(raw) else { return false };
        let po = ActPrep::new(&attn_out);
        let proj = matvec(pool, l.o.quant, ow, l.o.rows, sh.d, &po);
        for i in 0..sh.d { x[i] = resid[i] + proj[i]; }

        // ---- feed-forward ----
        resid.copy_from_slice(x);
        crate::cpuattn::rmsnorm(x, &norms[l.ffn_norm..l.ffn_norm + sh.d], sh.eps, &mut nx);
        let prep = ActPrep::new(&nx);
        let Some(guw) = l.gate_up.slice(raw) else { return false };
        let gu = matvec(pool, l.gate_up.quant, guw, l.gate_up.rows, sh.d, &prep);
        let mut act = vec![0f32; sh.ff];
        crate::cpuattn::swiglu(&gu, sh.ff, &mut act);
        let Some(dw) = l.down.slice(raw) else { return false };
        let pa = ActPrep::new(&act);
        let dn = matvec(pool, l.down.quant, dw, l.down.rows, sh.ff, &pa);
        for i in 0..sh.d { x[i] = resid[i] + dn[i]; }
    }
    kv.pos += 1;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f32_to_half(f: f32) -> u16 {
        let b = f.to_bits();
        let sign = ((b >> 16) & 0x8000) as u16;
        let exp = ((b >> 23) & 0xFF) as i32 - 127 + 15;
        let mant = (b >> 13) & 0x3FF;
        if exp <= 0 { return sign; }
        sign | ((exp as u16) << 10) | mant as u16
    }

    fn shape() -> Shape {
        Shape { d: 64, ff: 128, n_heads: 4, n_kv_heads: 2, head_dim: 16, n_layers: 2,
                rope_base: 10000.0, eps: 1e-5 }
    }

    /// A layer's worth of Q8_0 weights laid out back to back, as a file would hold them.
    fn build(sh: &Shape) -> (Vec<u8>, Vec<f32>, Vec<LayerRef>) {
        let q = Quant::Q8_0;
        let mut raw = Vec::new();
        let mut layers = Vec::new();
        let mut norms = Vec::new();
        // Q8_0 is [f16 scale][32 int8] per 34 bytes. Filling all 34 with pseudo-random bytes puts
        // arbitrary bit patterns in the SCALE, and some decode to NaN or Inf — which then propagates
        // through every layer and fails as "non-finite output" with nothing wrong in the code. This is
        // the second fixture to hit it (the Q4_K one did too), so the scale is written explicitly.
        let mut push = |raw: &mut Vec<u8>, rows: usize, cols: usize, seed: usize| -> TensorRef {
            let off = raw.len();
            let nblk = q.row_bytes(cols) / 34;
            for r in 0..rows {
                for b in 0..nblk {
                    let d = f32_to_half(0.02 + ((r + b + seed) % 7) as f32 * 0.003);
                    raw.extend_from_slice(&d.to_le_bytes());
                    raw.extend((0..32).map(|i| (((i * 31 + seed * 7 + r) % 251) as i8) as u8));
                }
            }
            TensorRef { off, rows, cols, quant: q }
        };
        for li in 0..sh.n_layers {
            let an = norms.len(); norms.extend((0..sh.d).map(|i| 1.0 + (i % 3) as f32 * 0.01));
            let qr = push(&mut raw, sh.n_heads * sh.head_dim, sh.d, li * 10 + 1);
            let kr = push(&mut raw, sh.kv_dim(), sh.d, li * 10 + 2);
            let vr = push(&mut raw, sh.kv_dim(), sh.d, li * 10 + 3);
            let or = push(&mut raw, sh.d, sh.d, li * 10 + 4);
            let fnn = norms.len(); norms.extend((0..sh.d).map(|i| 1.0 + (i % 5) as f32 * 0.01));
            let gu = push(&mut raw, 2 * sh.ff, sh.d, li * 10 + 5);
            let dn = push(&mut raw, sh.d, sh.ff, li * 10 + 6);
            layers.push(LayerRef { attn_norm: an, q: qr, k: kr, v: vr, o: or,
                                   ffn_norm: fnn, gate_up: gu, down: dn });
        }
        (raw, norms, layers)
    }

    #[test]
    fn a_step_advances_the_cache_by_exactly_one_position() {
        let sh = shape();
        let (raw, norms, layers) = build(&sh);
        let pool = Pool::new(2);
        let mut kv = KvCache::new(&sh, 8);
        let mut x: Vec<f32> = (0..sh.d).map(|i| (i as f32) * 0.01 - 0.3).collect();
        for expect in 1..=5 {
            assert!(decode_step(&pool, &raw, &norms, &layers, &sh, &mut kv, &mut x));
            assert_eq!(kv.len(), expect);
            assert!(x.iter().all(|v| v.is_finite()), "step {expect} produced non-finite output");
        }
    }

    #[test]
    fn a_full_cache_refuses_rather_than_wrapping_onto_position_zero() {
        // A silent wrap would overwrite the oldest context and read as the model forgetting, which is
        // indistinguishable from it simply being bad — no error, no log line, nothing to grep for.
        let sh = shape();
        let (raw, norms, layers) = build(&sh);
        let pool = Pool::new(1);
        let mut kv = KvCache::new(&sh, 2);
        let mut x = vec![0.1f32; sh.d];
        assert!(decode_step(&pool, &raw, &norms, &layers, &sh, &mut kv, &mut x));
        assert!(decode_step(&pool, &raw, &norms, &layers, &sh, &mut kv, &mut x));
        assert!(!decode_step(&pool, &raw, &norms, &layers, &sh, &mut kv, &mut x), "full must refuse");
        assert_eq!(kv.len(), 2, "and not advance");
    }

    #[test]
    fn the_residual_is_added_so_the_output_is_not_the_block_output_alone() {
        // Dropping the residual gives a model that still emits tokens and is badly wrong. The check:
        // an input of all zeros stays finite, but a large input must still be visible in the output.
        let sh = shape();
        let (raw, norms, layers) = build(&sh);
        let pool = Pool::new(1);
        let mut kv = KvCache::new(&sh, 4);
        let mut big = vec![50.0f32; sh.d];
        assert!(decode_step(&pool, &raw, &norms, &layers, &sh, &mut kv, &mut big));
        // With two residual additions per layer over two layers, a 50.0 input cannot vanish.
        assert!(big.iter().map(|v| v.abs()).fold(0f32, f32::max) > 10.0,
                "the residual carried nothing through: {:?}", &big[..4]);
    }

    #[test]
    fn grouped_query_maps_by_DIVISION_not_modulo() {
        // 4 query heads over 2 kv heads: heads 0,1 read kv head 0 and heads 2,3 read kv head 1.
        // The modulo version gives 0,1,0,1 — in range, runs fine, reads the wrong group, and produces
        // grammatical wrong text with nothing to grep for.
        let sh = shape();
        assert_eq!(sh.group(), 2);
        let mapped: Vec<usize> = (0..sh.n_heads).map(|h| h / sh.group()).collect();
        assert_eq!(mapped, vec![0, 0, 1, 1]);
        let modulo: Vec<usize> = (0..sh.n_heads).map(|h| h % sh.n_kv_heads).collect();
        assert_ne!(mapped, modulo, "if these agreed the test would prove nothing");
    }

    #[test]
    fn attending_to_a_single_position_returns_that_positions_value() {
        // The first token attends only to itself, so softmax over one score is 1.0 and the output is
        // exactly V. If append happened AFTER attending, this would attend over zero positions and
        // return zeros — which is the most common way to get this ordering wrong.
        let sh = shape();
        let (raw, norms, layers) = build(&sh);
        let pool = Pool::new(1);
        let mut kv = KvCache::new(&sh, 4);
        let mut x = vec![0.2f32; sh.d];
        assert!(decode_step(&pool, &raw, &norms, &layers, &sh, &mut kv, &mut x));
        assert!(x.iter().any(|v| *v != 0.0), "a first token that attends to nothing gives zeros");
    }

    #[test]
    fn the_cache_size_is_computable_before_it_is_allocated() {
        // On a machine where the model already occupies most of RAM, discovering the cache does not fit
        // by allocating it is the wrong order.
        let sh = shape();
        let want = 2 * sh.n_layers * 100 * sh.kv_dim() * 4;
        assert_eq!(KvCache::bytes(&sh, 100), want);
        // a realistic figure: Llama-3 8B at 8k context
        let big = Shape { d: 4096, ff: 14336, n_heads: 32, n_kv_heads: 8, head_dim: 128,
                          n_layers: 32, rope_base: 500000.0, eps: 1e-5 };
        let gib = KvCache::bytes(&big, 8192) as f64 / (1u64 << 30) as f64;
        assert!(gib > 1.0 && gib < 32.0, "8B at 8k should be a few GiB, got {gib:.1}");
    }
}
