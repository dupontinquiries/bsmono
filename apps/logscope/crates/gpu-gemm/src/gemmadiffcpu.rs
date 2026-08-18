//! gemmadiffcpu.rs — the DiffusionGemma forward on the CPU, f32, composed from gemmadiff's oracles.
//!
//! Two jobs, neither of which is serving:
//!
//! 1. **The whole-model oracle.** gemmadiff.rs pins each novel op in isolation; this composes them
//!    into a runnable stack — per-layer head dims, V-from-K, the twin FFN, both scalar sets, the
//!    block-AR denoise loop — so the eventual GPU port has something to be bit-compared against
//!    that is bigger than one op and smaller than "trust me". Same tier as `dsa.rs`/`mla.rs`,
//!    one level up.
//! 2. **Proof the plan composes.** A layer plan that has never been run end to end is a diagram.
//!    `--gate-gemmadiff` now loads the converter's fixture through this and denoises a canvas, so
//!    the converter's names, `read_shape`'s dims and this forward are held to ONE contract.
//!
//! NOT a serving path: the 26B at f32 on CPU is minutes per forward, and `archcfg` keeps
//! `supported: false` — the archfeat gaps are about the ENGINE forward and stay declared until it
//! implements them. Two omissions here as well, refused loudly rather than guessed:
//!
//! - **Self-conditioning** (`self_cond.*`): the weights load and their PRESENCE is gated, but the
//!   drop ships no modeling code, so where the previous step's state enters is unknown. The
//!   forward does not apply it and `Weights::load` says so once. [archfeat: SelfConditioning]
//! - **Proportional rope factors**: full layers declare `rope_type: "proportional"` and no
//!   `rope_freqs` tensor exists in the drop. Partial-rotary NEOX at the layer's θ is what runs —
//!   correct at short positions where a factor of ~1 is the only sane reading, unverifiable past
//!   that. [named in the diffusion_gemma gap report]
//!
//! Attention scores are UNSCALED (gemma4.cpp: `f_attention_scale = 1.0`; QK-norm covers it) —
//! the converter writes `attention.scale = 1.0` so the engine's generic pre-scaling reaches the
//! same place by a different route.
//!
//! The fused expert stack's row order is HF's `gate_up_proj` copied verbatim: GATE is the first
//! `n_ff_exp` rows of each expert's slice, up the second — the same split `build_moe_ffn` takes
//! (llama-graph.cpp: gate view at offset 0, up at `n_ff * nb[0]`). If HF's fused order were ever
//! up-first, both stacks would be wrong TOGETHER, which is at least a diagnosable state.

use crate::gemmadiff::{self, blockrefine, GemmaDiffShape, Pass};
use crate::gguf::Gguf;

// ── tensor plumbing ──────────────────────────────────────────────────────────────────────────────

/// IEEE 754 half → f32, subnormals and inf/NaN included. Small enough to own; the engine's f16
/// paths live GPU-side and this file must stay runnable with no device at all.
pub fn f16_to_f32(h: u16) -> f32 {
    let (s, e, m) = ((h >> 15) as u32, ((h >> 10) & 0x1f) as u32, (h & 0x3ff) as u32);
    let bits = match e {
        0 => {
            if m == 0 { s << 31 } // ±0
            else {
                // subnormal: renormalize
                let mut e2 = 127 - 15 + 1;
                let mut m2 = m;
                while m2 & 0x400 == 0 { m2 <<= 1; e2 -= 1; }
                (s << 31) | ((e2 as u32) << 23) | ((m2 & 0x3ff) << 13)
            }
        }
        0x1f => (s << 31) | 0x7f80_0000 | (m << 13), // inf / nan
        _ => (s << 31) | ((e + 127 - 15) << 23) | (m << 13),
    };
    f32::from_bits(bits)
}

fn decode_f32_or_f16(ggml_type: u32, raw: &[u8]) -> Option<Vec<f32>> {
    match ggml_type {
        0 => Some(raw.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()),
        1 => Some(raw.chunks_exact(2).map(|c| f16_to_f32(u16::from_le_bytes([c[0], c[1]]))).collect()),
        _ => None,
    }
}

/// Fetch a tensor as f32, refusing on absence, on a dims mismatch (ggml order — ne\[0\] innermost)
/// and on a quantized type, each with the tensor's NAME in the error. A dims check that names the
/// tensor is the difference between "wrong file" and an index panic three layers away.
fn fetch(g: &Gguf, raw: &[u8], name: &str, want_dims: &[u64]) -> Result<Vec<f32>, String> {
    let t = g.tensor(name).ok_or_else(|| format!("gemmadiffcpu: missing tensor {name}"))?;
    if t.dims != want_dims {
        return Err(format!("gemmadiffcpu: {name} dims {:?}, expected {:?} (ggml order)", t.dims, want_dims));
    }
    let count = t.dims.iter().product::<u64>() as usize;
    let bytes = count * if t.ggml_type == 1 { 2 } else { 4 };
    let off = (g.data_offset + t.offset) as usize;
    let raw = raw.get(off..off + bytes)
        .ok_or_else(|| format!("gemmadiffcpu: {name} data out of range"))?;
    decode_f32_or_f16(t.ggml_type, raw)
        .ok_or_else(|| format!("gemmadiffcpu: {name} is ggml type {} — this reference reads f32/f16 \
only; dequantize the file (or extend decode_f32_or_f16) rather than letting a quant slip through \
as garbage floats", t.ggml_type))
}

// ── weights ──────────────────────────────────────────────────────────────────────────────────────

/// One layer's tensors, f32 in memory. Fixture-scale by design: the real 26B wants mmap + lazy
/// reads, and building that here would duplicate the engine loader this file exists to check.
pub struct LayerW {
    pub attn_norm: Vec<f32>,
    pub wq: Vec<f32>, pub wk: Vec<f32>,
    /// Present exactly when the layer is NOT V-from-K (the census rule; load refuses a mismatch).
    pub wv: Option<Vec<f32>>,
    pub wo: Vec<f32>,
    pub q_norm: Vec<f32>, pub k_norm: Vec<f32>,
    pub post_attn_norm: Vec<f32>,
    // shared dense branch
    pub ffn_norm: Vec<f32>, pub gate: Vec<f32>, pub up: Vec<f32>, pub down: Vec<f32>,
    pub post_ffn_1: Vec<f32>,
    // MoE branch + joiner
    pub pre_ffn_2: Vec<f32>, pub post_ffn_2: Vec<f32>, pub post_ffn: Vec<f32>,
    pub router_w: Vec<f32>, pub router_s: Vec<f32>, pub per_expert_s: Vec<f32>,
    pub gate_up_exps: Vec<f32>, pub down_exps: Vec<f32>,
    // per-layer output scalars, one per pass
    pub out_scale_dec: f32, pub out_scale_enc: Option<f32>,
}

pub struct Weights {
    pub shape: GemmaDiffShape,
    pub eps: f32,
    pub vocab: usize,
    pub embed: Vec<f32>, // [vocab, d] row-major, tied output head
    pub out_norm: Vec<f32>,
    pub layers: Vec<LayerW>,
    pub mask_id: u32,
}

impl Weights {
    /// Load from a parsed GGUF + the raw file bytes. f32/f16 tensors only.
    pub fn load(g: &Gguf, raw: &[u8]) -> Result<Weights, String> {
        let shape = gemmadiff::read_shape(g)?;
        shape.validate()?;
        let d = shape.d as u64;
        let eps = g.get_f32("diffusion_gemma.attention.layer_norm_rms_epsilon").unwrap_or(1e-6);
        let mask_id = crate::gguf_mask_token_id(g)
            .ok_or("gemmadiffcpu: no tokenizer.ggml.mask_token_id — a diffusion model without a \
mask token cannot denoise anything")?;
        let embed_t = g.tensor("token_embd.weight")
            .ok_or("gemmadiffcpu: missing token_embd.weight")?;
        let vocab = *embed_t.dims.get(1).ok_or("gemmadiffcpu: token_embd is not 2-D")? as usize;
        let embed = fetch(g, raw, "token_embd.weight", &[d, vocab as u64])?;
        let out_norm = fetch(g, raw, "output_norm.weight", &[d])?;

        let mut layers = Vec::with_capacity(shape.n_layer as usize);
        for il in 0..shape.n_layer {
            let hd = shape.head_dim(il) as u64;
            let kvw = shape.kv_width(il) as u64;
            let qw = shape.n_head as u64 * hd;
            let n = |s: &str| format!("blk.{il}.{s}");
            // The v census IS the rule, enforced both ways: a full layer shipping attn_v means the
            // file disagrees with the architecture, and running it would silently ignore a tensor.
            let v_name = n("attn_v.weight");
            let wv = if shape.v_from_k(il) {
                if g.tensor(&v_name).is_some() {
                    return Err(format!("gemmadiffcpu: layer {il} is full-attention (V-from-K) but \
the file ships {v_name} — census mismatch; refusing rather than ignoring a tensor"));
                }
                None
            } else {
                Some(fetch(g, raw, &v_name, &[d, kvw])?)
            };
            let scalar = |name: &str| -> Result<f32, String> {
                Ok(fetch(g, raw, name, &[1])?[0])
            };
            layers.push(LayerW {
                attn_norm: fetch(g, raw, &n("attn_norm.weight"), &[d])?,
                wq: fetch(g, raw, &n("attn_q.weight"), &[d, qw])?,
                wk: fetch(g, raw, &n("attn_k.weight"), &[d, kvw])?,
                wv,
                wo: fetch(g, raw, &n("attn_output.weight"), &[qw, d])?,
                q_norm: fetch(g, raw, &n("attn_q_norm.weight"), &[hd])?,
                k_norm: fetch(g, raw, &n("attn_k_norm.weight"), &[hd])?,
                post_attn_norm: fetch(g, raw, &n("post_attention_norm.weight"), &[d])?,
                ffn_norm: fetch(g, raw, &n("ffn_norm.weight"), &[d])?,
                gate: fetch(g, raw, &n("ffn_gate.weight"), &[d, shape.n_ff_shared as u64])?,
                up: fetch(g, raw, &n("ffn_up.weight"), &[d, shape.n_ff_shared as u64])?,
                down: fetch(g, raw, &n("ffn_down.weight"), &[shape.n_ff_shared as u64, d])?,
                post_ffn_1: fetch(g, raw, &n("post_ffw_norm_1.weight"), &[d])?,
                pre_ffn_2: fetch(g, raw, &n("pre_ffw_norm_2.weight"), &[d])?,
                post_ffn_2: fetch(g, raw, &n("post_ffw_norm_2.weight"), &[d])?,
                post_ffn: fetch(g, raw, &n("post_ffw_norm.weight"), &[d])?,
                router_w: fetch(g, raw, &n("ffn_gate_inp.weight"), &[d, shape.n_expert as u64])?,
                router_s: fetch(g, raw, &n("ffn_gate_inp.scale"), &[d])?,
                per_expert_s: fetch(g, raw, &n("ffn_down_exps.scale"), &[shape.n_expert as u64])?,
                gate_up_exps: fetch(g, raw, &n("ffn_gate_up_exps.weight"),
                                    &[d, 2 * shape.n_ff_exp as u64, shape.n_expert as u64])?,
                down_exps: fetch(g, raw, &n("ffn_down_exps.weight"),
                                 &[shape.n_ff_exp as u64, d, shape.n_expert as u64])?,
                out_scale_dec: scalar(&n("layer_output_scale.weight"))?,
                out_scale_enc: match g.tensor(&n("layer_output_scale_enc.weight")) {
                    Some(_) => Some(scalar(&n("layer_output_scale_enc.weight"))?),
                    None => None, // layer_out_scale REFUSES an Encode pass without it — loudly, there
                },
            });
        }
        // Presence-only: applying self-conditioning with guessed semantics would run and be wrong.
        if g.tensor("self_cond.ffn_gate.weight").is_some() {
            eprintln!("gemmadiffcpu: self_cond.* present and NOT APPLIED — the drop ships no \
modeling code for where it enters; the forward runs without it and archfeat keeps the gap");
        }
        Ok(Weights { shape, eps, vocab, embed, out_norm, layers, mask_id })
    }
}

// ── small math ───────────────────────────────────────────────────────────────────────────────────

/// out[r] = Σ_c w[r·k + c] · x[c] — GGUF row-major, ne[0] (k) contiguous.
pub fn matvec(w: &[f32], n: usize, k: usize, x: &[f32]) -> Vec<f32> {
    debug_assert_eq!(w.len(), n * k);
    debug_assert_eq!(x.len(), k);
    (0..n).map(|r| {
        let row = &w[r * k..(r + 1) * k];
        row.iter().zip(x).map(|(a, b)| a * b).sum()
    }).collect()
}

/// Gemma RMS norm: x̂ · (1 + w). The engine folds (1+w) into the weight at load; this reference
/// reads the file's raw weights, so the +1 happens here, visibly.
fn gemma_rms(x: &[f32], w: &[f32], eps: f32) -> Vec<f32> {
    let xn = gemmadiff::rms_weightless(x, eps);
    xn.iter().zip(w).map(|(v, wi)| v * (1.0 + wi)).collect()
}

/// gelu_pytorch_tanh — the config's hidden_activation, exactly the tanh approximation.
fn gelu_tanh(x: f32) -> f32 {
    0.5 * x * (1.0 + ((2.0 / std::f32::consts::PI).sqrt() * (x + 0.044715 * x * x * x)).tanh())
}

/// NEOX-paired rope over the FIRST `n_rot` dims of one head; the rest pass through untouched
/// (partial rotary — full layers rotate head_dim/4 = 128 of 512). Pairs are (i, i + n_rot/2).
fn rope_neox_partial(h: &mut [f32], pos: usize, base: f32, n_rot: usize) {
    let half = n_rot / 2;
    for i in 0..half {
        let theta = (pos as f32) * base.powf(-2.0 * i as f32 / n_rot as f32);
        let (s, c) = theta.sin_cos();
        let (a, b) = (h[i], h[i + half]);
        h[i] = a * c - b * s;
        h[i + half] = a * s + b * c;
    }
}

// ── forward ──────────────────────────────────────────────────────────────────────────────────────

/// Per-layer K/V cache, f32, widths differing per layer (the thing the engine cannot hold yet —
/// archfeat::PerLayerHeadDim). K is cached POST norm+rope, V post weightless-norm: what attention
/// consumes, so a cached token costs no rework.
pub struct KvCache {
    k: Vec<Vec<f32>>,
    v: Vec<Vec<f32>>,
    pub len: usize,
}

impl KvCache {
    pub fn new(sh: &GemmaDiffShape) -> KvCache {
        KvCache {
            k: (0..sh.n_layer).map(|_| Vec::new()).collect(),
            v: (0..sh.n_layer).map(|_| Vec::new()).collect(),
            len: 0,
        }
    }
}

/// Can a query at absolute `qpos` see a key at absolute `kpos`?
///
/// Encode: causal. Denoise: bidirectional — the canvas sees the whole cached prefix and itself in
/// both directions. Sliding layers bound |Δpos| by the window in EITHER pass; full layers see
/// everything. The bidirectional window is a stated generalization: the card is silent on it, and
/// with window 1024 against a 256 canvas the shipped model cannot tell — the fixture can, which is
/// why the rule is a function and not an inline condition.
fn visible(pass: Pass, full: bool, window: u32, qpos: usize, kpos: usize) -> bool {
    let win_ok = full || window == 0 || (qpos.abs_diff(kpos) as u32) < window;
    match pass {
        Pass::Encode => kpos <= qpos && win_ok,
        Pass::Denoise => win_ok,
    }
}

/// One stack pass over `tokens` at absolute positions `pos0..`. Encode APPENDS this batch's K/V to
/// the cache (block-AR: the committed prefix accumulates); Denoise leaves the cache untouched —
/// canvas rows attend to the cache plus each other as temporaries. Returns one logits row per
/// token (shift_logits=false lineage: row i predicts position i, in place).
pub fn forward(w: &Weights, cache: &mut KvCache, tokens: &[u32], pos0: usize, pass: Pass)
               -> Result<Vec<Vec<f32>>, String> {
    let sh = &w.shape;
    let (d, nh) = (sh.d as usize, sh.n_head as usize);
    let n = tokens.len();
    let scale_embed = (d as f32).sqrt();
    let mut x: Vec<Vec<f32>> = tokens.iter().map(|&t| {
        let t = t as usize;
        if t >= w.vocab { return Err(format!("gemmadiffcpu: token {t} out of vocab {}", w.vocab)); }
        Ok(w.embed[t * d..(t + 1) * d].iter().map(|e| e * scale_embed).collect())
    }).collect::<Result<_, String>>()?;

    for il in 0..sh.n_layer {
        let lw = &w.layers[il as usize];
        let (hd, nkv) = (sh.head_dim(il) as usize, sh.n_kv(il) as usize);
        let kvw = sh.kv_width(il) as usize;
        let (base, n_rot) = sh.rope(il);
        let full = sh.is_full(il);

        // this batch's K/V, processed exactly as the cache stores them
        let mut bk: Vec<Vec<f32>> = Vec::with_capacity(n);
        let mut bv: Vec<Vec<f32>> = Vec::with_capacity(n);
        let mut bq: Vec<Vec<f32>> = Vec::with_capacity(n);
        for (i, xi) in x.iter().enumerate() {
            let pos = pos0 + i;
            let ln = gemma_rms(xi, &lw.attn_norm, w.eps);
            let mut q = matvec(&lw.wq, nh * hd, d, &ln);
            let k_raw = matvec(&lw.wk, kvw, d, &ln);
            // V: from its own projection, or from the RAW K projection where attn_v is absent —
            // either way the weightless rms applies per head (gemma4.cpp norms V unconditionally).
            let v_raw = match &lw.wv {
                Some(wv) => matvec(wv, kvw, d, &ln),
                None => k_raw.clone(),
            };
            let mut k = k_raw;
            let mut v = v_raw;
            for h in 0..nh {
                let qs = &mut q[h * hd..(h + 1) * hd];
                let qn = gemma_rms(qs, &lw.q_norm, w.eps);
                qs.copy_from_slice(&qn);
                rope_neox_partial(qs, pos, base, n_rot as usize);
            }
            for h in 0..nkv {
                let ks = &mut k[h * hd..(h + 1) * hd];
                let kn = gemma_rms(ks, &lw.k_norm, w.eps);
                ks.copy_from_slice(&kn);
                rope_neox_partial(ks, pos, base, n_rot as usize);
                let vs = &mut v[h * hd..(h + 1) * hd];
                let vn = gemmadiff::rms_weightless(vs, w.eps);
                vs.copy_from_slice(&vn);
            }
            bq.push(q); bk.push(k); bv.push(v);
        }

        let ck = &cache.k[il as usize];
        let cv = &cache.v[il as usize];
        let n_cache = cache.len;
        let key_at = |j: usize| -> (&[f32], usize) {
            if j < n_cache { (&ck[j * kvw..(j + 1) * kvw], j) }
            else { (&bk[j - n_cache][..], pos0 + (j - n_cache)) }
        };
        let val_at = |j: usize| -> &[f32] {
            if j < n_cache { &cv[j * kvw..(j + 1) * kvw] } else { &bv[j - n_cache][..] }
        };
        let n_keys = n_cache + n;

        let mut attn_out_rows: Vec<Vec<f32>> = Vec::with_capacity(n);
        for (i, xi) in x.iter().enumerate() {
            let qpos = pos0 + i;
            let mut heads = vec![0.0f32; nh * hd];
            for qh in 0..nh {
                let kvh = crate::cpuops::kv_head_for(qh, nh, nkv);
                let q = &bq[i][qh * hd..(qh + 1) * hd];
                // unscaled scores — gemma4.cpp f_attention_scale = 1.0
                let mut scores: Vec<(usize, f32)> = Vec::new();
                for j in 0..n_keys {
                    let (krow, kpos) = key_at(j);
                    if !visible(pass, full, sh.window, qpos, kpos) { continue; }
                    let ks = &krow[kvh * hd..(kvh + 1) * hd];
                    scores.push((j, q.iter().zip(ks).map(|(a, b)| a * b).sum()));
                }
                if scores.is_empty() {
                    return Err(format!("gemmadiffcpu: layer {il} pos {qpos}: no visible keys — \
window {} cut everything, which is a shape/window inconsistency, not a valid attention", sh.window));
                }
                let mut probs: Vec<f32> = scores.iter().map(|(_, s)| *s).collect();
                crate::cpuops::softmax(&mut probs);
                let out = &mut heads[qh * hd..(qh + 1) * hd];
                for ((j, _), p) in scores.iter().zip(&probs) {
                    let vs = &val_at(*j)[kvh * hd..(kvh + 1) * hd];
                    for (o, v) in out.iter_mut().zip(vs) { *o += p * v; }
                }
            }
            let o = matvec(&lw.wo, d, nh * hd, &heads);
            let post = gemma_rms(&o, &lw.post_attn_norm, w.eps);
            attn_out_rows.push(post.iter().zip(xi).map(|(a, b)| a + b).collect());
        }
        if pass == Pass::Encode {
            for i in 0..n {
                cache.k[il as usize].extend_from_slice(&bk[i]);
                cache.v[il as usize].extend_from_slice(&bv[i]);
            }
        }

        // twin FFN, composed from the gemmadiff oracles
        let (ff, ffe, ne) = (sh.n_ff_shared as usize, sh.n_ff_exp as usize, sh.n_expert as usize);
        let scale = gemmadiff::layer_out_scale(pass, lw.out_scale_dec, lw.out_scale_enc)?;
        let mut next: Vec<Vec<f32>> = Vec::with_capacity(n);
        for attn_out in &attn_out_rows {
            // dense shared branch
            let h1 = gemma_rms(attn_out, &lw.ffn_norm, w.eps);
            let g1 = matvec(&lw.gate, ff, d, &h1);
            let u1 = matvec(&lw.up, ff, d, &h1);
            let a1: Vec<f32> = g1.iter().zip(&u1).map(|(g, u)| gelu_tanh(*g) * u).collect();
            let mlp = gemma_rms(&matvec(&lw.down, d, ff, &a1), &lw.post_ffn_1, w.eps);
            // MoE branch — router on attn_out, not the branch input (the reference's own emphasis)
            let h2 = gemma_rms(attn_out, &lw.pre_ffn_2, w.eps);
            let logits = gemmadiff::router_logits_reference(
                attn_out, &lw.router_s, &lw.router_w, ne, w.eps);
            let mut moe = vec![0.0f32; d];
            for (e, rw) in gemmadiff::route_topk(&logits, sh.n_expert_used as usize) {
                let slab = &lw.gate_up_exps[e * 2 * ffe * d..(e + 1) * 2 * ffe * d];
                let ge = matvec(&slab[..ffe * d], ffe, d, &h2);           // GATE: first ffe rows
                let ue = matvec(&slab[ffe * d..], ffe, d, &h2);           // UP: second ffe rows
                let ae: Vec<f32> = ge.iter().zip(&ue).map(|(g, u)| gelu_tanh(*g) * u).collect();
                let mut out = matvec(&lw.down_exps[e * d * ffe..(e + 1) * d * ffe], d, ffe, &ae);
                gemmadiff::per_expert_scale(&mut out, e, &lw.per_expert_s);
                for (m, o) in moe.iter_mut().zip(&out) { *m += rw * o; }
            }
            let moe = gemma_rms(&moe, &lw.post_ffn_2, w.eps);
            let combined = gemmadiff::twin_ffn_combine(&mlp, &moe, attn_out, &lw.post_ffn, w.eps);
            next.push(combined.iter().map(|v| v * scale).collect());
        }
        x = next;
    }
    if pass == Pass::Encode { cache.len += n; }

    let cap = sh.logit_softcap;
    Ok(x.iter().map(|h| {
        let hn = gemma_rms(h, &w.out_norm, w.eps);
        let mut logits = matvec(&w.embed, w.vocab, d, &hn);
        if let Some(c) = cap {
            for l in &mut logits { *l = gemmadiff::softcap(*l, c); }
        }
        logits
    }).collect())
}

// ── the block-AR denoise loop ────────────────────────────────────────────────────────────────────

/// One block's refinement, seamed on a closure: `probs_of(canvas, temperature) -> probs per row`.
/// The loop logic — selection, renoise, adaptive stop, the step cap — is here; the model is not,
/// which is what lets the schedule be tested against rigged distributions and the real forward
/// against the schedule, separately, then together.
pub fn denoise_block<F>(sched: &blockrefine::Schedule, block_len: usize, mask_id: u32, mut probs_of: F)
                        -> Result<(Vec<u32>, u32), String>
where F: FnMut(&[u32], f32) -> Result<Vec<Vec<f32>>, String> {
    let mut canvas = vec![mask_id; block_len];
    let mut masked: Vec<bool> = vec![true; block_len];
    let mut tracker = blockrefine::StopTracker::default();
    let mut steps = 0u32;
    while masked.iter().any(|m| *m) {
        if steps >= sched.max_steps {
            // the cap is a CEILING, not a schedule: whatever is still masked commits its argmax,
            // because emitting mask tokens into a reply is never the right reading of "out of steps"
            let probs = probs_of(&canvas, blockrefine::temperature(sched, steps.saturating_sub(1)))?;
            for (i, m) in masked.iter_mut().enumerate() {
                if *m { canvas[i] = argmax(&probs[i]); *m = false; }
            }
            break;
        }
        let t = blockrefine::temperature(sched, steps);
        let probs = probs_of(&canvas, t)?;
        if probs.len() != block_len {
            return Err(format!("gemmadiffcpu: probs rows {} for a {}-block", probs.len(), block_len));
        }
        let positions: Vec<blockrefine::Position> = (0..block_len).filter(|i| masked[*i]).map(|i| {
            let p = &probs[i];
            let p_top = p.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
            blockrefine::Position { idx: i, entropy: blockrefine::entropy(p), p_top }
        }).collect();
        let plan = blockrefine::plan_step(sched, &positions);
        for &i in &plan.commit { canvas[i] = argmax(&probs[i]); masked[i] = false; }
        for &i in &plan.renoise { canvas[i] = mask_id; } // explicit: uncommitted drafts do not linger
        steps += 1;
        // adaptive stop reads the WHOLE canvas (committed rows included), per the card
        let all_argmax: Vec<u32> = (0..block_len).map(|i| argmax(&probs[i])).collect();
        let mean_h = probs.iter().map(|p| blockrefine::entropy(p)).sum::<f32>() / block_len as f32;
        if tracker.observe(sched, &all_argmax, mean_h) {
            for (i, m) in masked.iter_mut().enumerate() {
                if *m { canvas[i] = all_argmax[i]; *m = false; }
            }
            break;
        }
    }
    Ok((canvas, steps))
}

fn argmax(p: &[f32]) -> u32 {
    p.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(i, _)| i as u32).unwrap_or(0)
}

/// The whole block-AR loop against the real forward: encode the prompt (Encode pass, causal, enc
/// scalars), then per block — denoise it bidirectionally against the cached prefix, commit, ENCODE
/// the finished block into the cache, continue. Greedy commits (argmax): the reference is pinned
/// deterministic; sampling is the app loop's lever, not the oracle's.
pub fn denoise_canvas(w: &Weights, sched: &blockrefine::Schedule, prompt: &[u32], n_fill: usize)
                      -> Result<Vec<u32>, String> {
    let sh = &w.shape;
    let block = sh.block_len as usize;
    let mut cache = KvCache::new(sh);
    if !prompt.is_empty() {
        forward(w, &mut cache, prompt, 0, Pass::Encode)?;
    }
    let mut out: Vec<u32> = Vec::with_capacity(n_fill);
    while out.len() < n_fill {
        let this = block.min(n_fill - out.len());
        let base = cache.len;
        let (tokens, _steps) = denoise_block(sched, this, w.mask_id, |canvas, t| {
            let rows = forward(w, &mut cache, canvas, base, Pass::Denoise)?;
            Ok(rows.into_iter().map(|logits| {
                let mut p: Vec<f32> = logits.iter().map(|l| l / t.max(1e-6)).collect();
                crate::cpuops::softmax(&mut p);
                p
            }).collect())
        })?;
        forward(w, &mut cache, &tokens, base, Pass::Encode)?;
        out.extend_from_slice(&tokens);
    }
    Ok(out)
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic small weights for a given shape — every tensor filled from SplitMix so two
    /// builds are bit-identical, scaled ~1/sqrt(k) so activations stay sane through 2 layers.
    fn tiny(sh: GemmaDiffShape, seed: u64) -> Weights {
        struct Rng(u64);
        impl Rng {
            fn f(&mut self, scale: f32) -> f32 {
                self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                (((self.0 >> 33) as u32 as f32 / u32::MAX as f32) - 0.5) * 2.0 * scale
            }
            fn v(&mut self, n: usize, scale: f32) -> Vec<f32> {
                (0..n).map(|_| self.f(scale)).collect()
            }
        }
        let mut r = Rng(seed);
        let d = sh.d as usize;
        let vocab = 32usize;
        let layers = (0..sh.n_layer).map(|il| {
            let (hd, kvw) = (sh.head_dim(il) as usize, sh.kv_width(il) as usize);
            let qw = sh.n_head as usize * hd;
            let (ff, ffe, ne) = (sh.n_ff_shared as usize, sh.n_ff_exp as usize, sh.n_expert as usize);
            let ws = 1.0 / (d as f32).sqrt();
            LayerW {
                attn_norm: r.v(d, 0.1), wq: r.v(qw * d, ws), wk: r.v(kvw * d, ws),
                wv: if sh.v_from_k(il) { None } else { Some(r.v(kvw * d, ws)) },
                wo: r.v(d * qw, 1.0 / (qw as f32).sqrt()),
                q_norm: r.v(hd, 0.1), k_norm: r.v(hd, 0.1), post_attn_norm: r.v(d, 0.1),
                ffn_norm: r.v(d, 0.1), gate: r.v(ff * d, ws), up: r.v(ff * d, ws),
                down: r.v(d * ff, 1.0 / (ff as f32).sqrt()), post_ffn_1: r.v(d, 0.1),
                pre_ffn_2: r.v(d, 0.1), post_ffn_2: r.v(d, 0.1), post_ffn: r.v(d, 0.1),
                router_w: r.v(ne * d, ws), router_s: r.v(d, 0.5).iter().map(|x| 1.0 + x).collect(),
                per_expert_s: r.v(ne, 0.2).iter().map(|x| 1.0 + x).collect(),
                gate_up_exps: r.v(ne * 2 * ffe * d, ws),
                down_exps: r.v(ne * d * ffe, 1.0 / (ffe as f32).sqrt()),
                out_scale_dec: 1.0 + r.f(0.1), out_scale_enc: Some(1.0 + r.f(0.1)),
            }
        }).collect();
        Weights {
            shape: sh, eps: 1e-6, vocab,
            embed: r.v(vocab * d, 0.5), out_norm: r.v(d, 0.1), layers, mask_id: 4,
        }
    }

    fn two_layer() -> GemmaDiffShape {
        GemmaDiffShape {
            n_layer: 2, d: 8, n_head: 2,
            head_dim_full: 8, n_kv_full: 1, head_dim_swa: 4, n_kv_swa: 2,
            swa_period: 2, window: 64,
            rope_base_full: 1e6, rope_base_swa: 1e4, n_rot_full: 2,
            n_expert: 4, n_expert_used: 2, n_ff_shared: 8, n_ff_exp: 4,
            logit_softcap: Some(30.0), canvas_len: 8, block_len: 4,
        }
    }

    fn all_swa(window: u32) -> GemmaDiffShape {
        GemmaDiffShape { swa_period: 0, window, ..two_layer() }
    }

    #[test]
    fn encode_is_causal_and_denoise_is_bidirectional() {
        let w = tiny(two_layer(), 7);
        let run = |toks: &[u32], pass: Pass| {
            let mut c = KvCache::new(&w.shape);
            forward(&w, &mut c, toks, 0, pass).unwrap()
        };
        let (a, b) = ([1u32, 2, 3], [1u32, 2, 9]);
        let (ea, eb) = (run(&a, Pass::Encode), run(&b, Pass::Encode));
        assert_eq!(ea[0], eb[0], "encode row 0 must not see a change at position 2 (causal)");
        assert_ne!(ea[2], eb[2], "sanity: the changed position's own row differs");
        let (da, db) = (run(&a, Pass::Denoise), run(&b, Pass::Denoise));
        assert_ne!(da[0], db[0], "denoise row 0 MUST see position 2 — bidirectional is the point");
    }

    #[test]
    fn sliding_window_bounds_influence_exactly() {
        // All-sliding, window 2, two layers: row r's receptive field over TOKENS is [r-2, r]
        // (window-1 per layer, compounded). Changing token 0 must leave row 4 bit-identical and
        // must change row 2 — both directions, so the window can be neither ignored nor over-cut.
        let w = tiny(all_swa(2), 11);
        let run = |toks: &[u32]| {
            let mut c = KvCache::new(&w.shape);
            forward(&w, &mut c, toks, 0, Pass::Encode).unwrap()
        };
        let ea = run(&[1, 2, 3, 5, 6]);
        let eb = run(&[9, 2, 3, 5, 6]);
        assert_eq!(ea[4], eb[4], "token 0 is outside row 4's compounded window — no path may leak");
        assert_ne!(ea[2], eb[2], "token 0 is inside row 2's field — the window must not over-cut");
    }

    #[test]
    fn v_comes_from_k_on_full_layers_and_not_on_sliding_ones() {
        // Zero the K projection. On a V-from-K (full) layer that zeroes V too, so attention
        // contributes nothing — logits must equal a run with attention FORCED null (wq/wk/wo all
        // zero). On a sliding layer V has its own projection: scores collapse to uniform but the
        // values are real, so the same comparison must FAIL. The pair pins where V is sourced.
        let full_1 = GemmaDiffShape { n_layer: 1, swa_period: 1, ..two_layer() };
        let sw_1 = GemmaDiffShape { n_layer: 1, swa_period: 0, ..two_layer() };
        for (sh, expect_equal) in [(full_1, true), (sw_1, false)] {
            let zero = |q: bool, k: bool, o: bool| {
                let mut w2 = tiny(sh, 13);
                if q { w2.layers[0].wq.iter_mut().for_each(|x| *x = 0.0); }
                if k { w2.layers[0].wk.iter_mut().for_each(|x| *x = 0.0); }
                if o { w2.layers[0].wo.iter_mut().for_each(|x| *x = 0.0); }
                w2
            };
            let wk0 = zero(false, true, false);
            let null = zero(true, true, true);
            let run = |w: &Weights| {
                let mut c = KvCache::new(&w.shape);
                forward(w, &mut c, &[1, 2, 3], 0, Pass::Encode).unwrap()
            };
            let (a, b) = (run(&wk0), run(&null));
            if expect_equal {
                assert_eq!(a, b, "full layer: zero K ⇒ zero V ⇒ attention null — V must come from K");
            } else {
                assert_ne!(a, b, "sliding layer: V is its own projection; zero K must NOT null it");
            }
        }
    }

    #[test]
    fn the_encode_pass_uses_the_encoder_scalars() {
        // Zero every enc scalar: the whole residual stream is multiplied to zero at each layer
        // output, so ENCODE logits are exactly softcap(0) = 0 everywhere — while the SAME weights
        // on a Denoise pass (decoder scalars intact) stay nonzero. Pins that the pass argument
        // reaches the scalar selection and that the scalar multiplies the post-residual output.
        let mut w = tiny(two_layer(), 17);
        for l in &mut w.layers { l.out_scale_enc = Some(0.0); }
        let mut c = KvCache::new(&w.shape);
        let enc = forward(&w, &mut c, &[1, 2], 0, Pass::Encode).unwrap();
        assert!(enc.iter().flatten().all(|l| *l == 0.0), "zeroed enc scalars ⇒ all-zero encode logits");
        let mut c2 = KvCache::new(&w.shape);
        let den = forward(&w, &mut c2, &[1, 2], 0, Pass::Denoise).unwrap();
        assert!(den.iter().flatten().any(|l| *l != 0.0), "denoise uses the DEC scalars — unaffected");
    }

    #[test]
    fn an_encode_pass_without_enc_scalars_refuses() {
        let mut w = tiny(two_layer(), 19);
        w.layers[1].out_scale_enc = None;
        let mut c = KvCache::new(&w.shape);
        let e = forward(&w, &mut c, &[1], 0, Pass::Encode).unwrap_err();
        assert!(e.contains("enc"), "the refusal names the missing scalar set: {e}");
        assert!(forward(&w, &mut c, &[1], 0, Pass::Denoise).is_ok(), "denoise needs only dec");
    }

    #[test]
    fn the_ffn_block_matches_the_oracles_composed_by_hand() {
        // Null the attention (wo = 0) so attn_out is exactly the scaled embedding — then the whole
        // layer output is computable OUTSIDE the forward from gemmadiff's own oracle functions.
        // This is the composition pin: expert-slab slicing (gate first!), router-on-attn_out, the
        // per-expert scale, the twin join and the out scalar all have to line up or the two paths
        // diverge. One layer, one token, exact f32 equality.
        let sh = GemmaDiffShape { n_layer: 1, swa_period: 0, ..two_layer() };
        let mut w = tiny(sh, 23);
        w.layers[0].wo.iter_mut().for_each(|x| *x = 0.0);
        let lw = &w.layers[0];
        let d = sh.d as usize;
        let tok = 3usize;
        let attn_out: Vec<f32> = {
            // wo = 0 ⇒ o = 0 ⇒ post_attn_norm(0) = 0 ⇒ attn_out = x = embed·sqrt(d)
            let s = (d as f32).sqrt();
            w.embed[tok * d..(tok + 1) * d].iter().map(|e| e * s).collect()
        };
        let (ff, ffe, ne) = (sh.n_ff_shared as usize, sh.n_ff_exp as usize, sh.n_expert as usize);
        let h1 = gemma_rms(&attn_out, &lw.ffn_norm, w.eps);
        let a1: Vec<f32> = matvec(&lw.gate, ff, d, &h1).iter()
            .zip(&matvec(&lw.up, ff, d, &h1)).map(|(g, u)| gelu_tanh(*g) * u).collect();
        let mlp = gemma_rms(&matvec(&lw.down, d, ff, &a1), &lw.post_ffn_1, w.eps);
        let h2 = gemma_rms(&attn_out, &lw.pre_ffn_2, w.eps);
        let logits = gemmadiff::router_logits_reference(&attn_out, &lw.router_s, &lw.router_w, ne, w.eps);
        let mut moe = vec![0.0f32; d];
        for (e, rw) in gemmadiff::route_topk(&logits, sh.n_expert_used as usize) {
            let slab = &lw.gate_up_exps[e * 2 * ffe * d..(e + 1) * 2 * ffe * d];
            let ae: Vec<f32> = matvec(&slab[..ffe * d], ffe, d, &h2).iter()
                .zip(&matvec(&slab[ffe * d..], ffe, d, &h2)).map(|(g, u)| gelu_tanh(*g) * u).collect();
            let mut out = matvec(&lw.down_exps[e * d * ffe..(e + 1) * d * ffe], d, ffe, &ae);
            gemmadiff::per_expert_scale(&mut out, e, &lw.per_expert_s);
            for (m, o) in moe.iter_mut().zip(&out) { *m += rw * o; }
        }
        let moe = gemma_rms(&moe, &lw.post_ffn_2, w.eps);
        let combined = gemmadiff::twin_ffn_combine(&mlp, &moe, &attn_out, &lw.post_ffn, w.eps);
        let scale = gemmadiff::layer_out_scale(Pass::Denoise, lw.out_scale_dec, lw.out_scale_enc).unwrap();
        let hn = gemma_rms(&combined.iter().map(|v| v * scale).collect::<Vec<_>>(), &w.out_norm, w.eps);
        let mut want = matvec(&w.embed, w.vocab, d, &hn);
        for l in &mut want { *l = gemmadiff::softcap(*l, 30.0); }

        let mut c = KvCache::new(&w.shape);
        let got = forward(&w, &mut c, &[tok as u32], 0, Pass::Denoise).unwrap();
        assert_eq!(got[0], want, "forward must equal the oracles composed by hand, exactly");
    }

    #[test]
    fn softcap_bounds_every_logit() {
        let w = tiny(two_layer(), 29);
        let mut c = KvCache::new(&w.shape);
        let rows = forward(&w, &mut c, &[0, 1, 2, 3], 0, Pass::Encode).unwrap();
        assert!(rows.iter().flatten().all(|l| l.abs() <= 30.0));
    }

    #[test]
    fn denoise_canvas_fills_deterministically_and_terminates() {
        let w = tiny(two_layer(), 31);
        let sched = blockrefine::Schedule { max_steps: 8, ..blockrefine::Schedule::shipped() };
        let a = denoise_canvas(&w, &sched, &[1, 2, 3], 8).unwrap();
        let b = denoise_canvas(&w, &sched, &[1, 2, 3], 8).unwrap();
        assert_eq!(a.len(), 8);
        assert_eq!(a, b, "greedy reference must be deterministic");
        assert!(a.iter().all(|t| *t != w.mask_id), "no mask token may survive into the output");
        assert!(a.iter().all(|t| (*t as usize) < w.vocab));
    }

    // ── loop-seam tests: the schedule against rigged distributions ──────────────────────────────

    fn rigged(p_top: f32, top: u32, vocab: usize) -> Vec<f32> {
        let rest = (1.0 - p_top) / (vocab - 1) as f32;
        (0..vocab).map(|i| if i as u32 == top { p_top } else { rest }).collect()
    }

    #[test]
    fn uniform_probs_pace_at_minimal_topk() {
        // Nothing clears the threshold, every entropy exceeds the bound ⇒ the floor commits
        // exactly one position per step; a 4-block takes exactly 4 steps.
        let sched = blockrefine::Schedule { commit_threshold: Some(0.95), ..blockrefine::Schedule::shipped() };
        let (tokens, steps) = denoise_block(&sched, 4, 99, |canvas, _t| {
            Ok(canvas.iter().map(|_| rigged(1.0 / 16.0, 0, 16)).collect())
        }).unwrap();
        assert_eq!(steps, 4);
        assert!(tokens.iter().all(|t| *t != 99));
    }

    #[test]
    fn confident_probs_commit_the_whole_block_in_one_step() {
        let sched = blockrefine::Schedule::shipped();
        let (tokens, steps) = denoise_block(&sched, 4, 99, |canvas, _t| {
            Ok(canvas.iter().map(|_| rigged(0.99, 7, 16)).collect())
        }).unwrap();
        assert_eq!(steps, 1, "p_top 0.99 ≥ threshold 0.95 at every position");
        assert_eq!(tokens, vec![7, 7, 7, 7]);
    }

    #[test]
    fn adaptive_stop_needs_both_conditions_and_then_flushes() {
        // No commit threshold, zero entropy budget ⇒ the floor paces 1/step. Distributions are
        // extremely confident (mean entropy < 0.005) and the argmax never changes, so the tracker
        // fires at step 2 (stability 1 = two equal reads) and the remaining masked rows flush as
        // their argmax. Steps observed: 2, not 4.
        let sched = blockrefine::Schedule {
            commit_threshold: None, entropy_bound: 0.0, stability: 1,
            ..blockrefine::Schedule::shipped()
        };
        let (tokens, steps) = denoise_block(&sched, 4, 99, |canvas, _t| {
            Ok(canvas.iter().map(|_| rigged(0.99999, 5, 16)).collect())
        }).unwrap();
        assert_eq!(steps, 2, "one step to seed prev_argmax, one to confirm — then the stop");
        assert_eq!(tokens, vec![5, 5, 5, 5]);
        // and confidence WITHOUT stability does not stop: argmax alternates, so the loop paces
        // the floor all the way even though every step's mean entropy is under the threshold
        let mut flip = false;
        let (_tokens, steps) = denoise_block(&sched, 4, 99, move |canvas, _t| {
            flip = !flip;
            let top = if flip { 5 } else { 6 };
            Ok(canvas.iter().map(|_| rigged(0.99999, top, 16)).collect())
        }).unwrap();
        assert_eq!(steps, 4, "unstable argmax must hold the stop back — both conditions, not one");
    }

    #[test]
    fn the_step_cap_flushes_rather_than_emitting_masks() {
        let sched = blockrefine::Schedule {
            max_steps: 2, commit_threshold: None, entropy_bound: 0.0, minimal_topk: 1,
            stop_entropy: 0.0, ..blockrefine::Schedule::shipped()
        };
        let (tokens, steps) = denoise_block(&sched, 4, 99, |canvas, _t| {
            Ok(canvas.iter().map(|_| rigged(0.5, 3, 16)).collect())
        }).unwrap();
        assert_eq!(steps, 2, "capped");
        assert!(tokens.iter().all(|t| *t == 3), "the two uncommitted rows flush as argmax, not mask");
    }

    #[test]
    fn f16_decode_covers_the_edges() {
        assert_eq!(f16_to_f32(0x0000), 0.0);
        assert_eq!(f16_to_f32(0x8000), -0.0);
        assert_eq!(f16_to_f32(0x3c00), 1.0);
        assert_eq!(f16_to_f32(0xc000), -2.0);
        assert_eq!(f16_to_f32(0x7c00), f32::INFINITY);
        assert!(f16_to_f32(0x7e00).is_nan());
        assert!((f16_to_f32(0x0001) - 5.960464e-8).abs() < 1e-12, "smallest subnormal");
        assert_eq!(f16_to_f32(0x7bff), 65504.0, "largest finite half");
    }
}
