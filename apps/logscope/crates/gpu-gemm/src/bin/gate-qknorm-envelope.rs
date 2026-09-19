//! gate-qknorm-envelope — QK-Norm + RoPE at the head shapes real checkpoints actually have.
//!
//! Every QK-norm fixture in this tree is head_dim 16 with full rotation. Qwen3.5 / Ornith /
//! Qwen3.8 attention layers are head_dim 256 with 64 rotary dims, and they decode through
//! `block_decode_fused`, whose fused QK-norm+rope kernels were written for Qwen3 (head_dim 128,
//! full rotation). `--gate-rope-kv` compares the KV-append kernel against the copy kernel — two
//! kernels with the SAME envelope, so they agree wherever both are wrong.
//!
//! Two independent checks per shape, because agreement between engine paths is not correctness:
//!
//!   1. PATH EQUALITY. The same token stream through (a) fused QKV + KV-append, (b) fused QKV +
//!      copy path, (c) the unfused reference (`LOGSCOPE_FUSED_QKV=0`: rmsnorm per head, then
//!      ROPE_MH, which honours rope.dimension_count). Logits and every K row must match (c).
//!
//!   2. ROTATION ORACLE, engine-independent. Layer 0's K row depends only on the token and the
//!      position (embedding → norm → wk → k_norm → rope). So the same token at positions a and b
//!      must give rows where every pass-through lane [rot, hd) is IDENTICAL and every rotary pair
//!      (i, i + rot/2) of row b equals row a's pair rotated by (b − a)·base^(−2i/rot). That pins
//!      the frequency exponent's denominator (rot, not hd), the NEOX pairing span, and that the
//!      pass-through lanes are written at all — computed here on the CPU from the definition.
//!
//!   cargo run --bin gate-qknorm-envelope          (no fixture file: shapes are built in-process)

use gpu_kernels::*;

const D: usize = 64;
const VOCAB: usize = 32;
const FF: usize = 128;
const BASE: f32 = 10000.0;

/// A 2-layer qwen3-arch GGUF (QK-norm, NEOX rope, dense FFN, all f32) with a chosen head shape.
/// q_norm and k_norm weights are non-uniform AND different from each other, so a kernel that
/// norms k with the q weight (the c5b fixture-seeding bug) cannot pass.
fn write_gguf(hd: usize, rot: usize, heads: usize, kvh: usize) -> Vec<u8> {
    let mk = |seed: u32, n: usize, amp: f32| -> Vec<f32> {
        let mut v = Vec::with_capacity(n); let mut x = seed | 1;
        for _ in 0..n { x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            // ZERO-MEAN on purpose. main.rs's fixture generator is `u/2^23 - 1`, which is [-1, 0):
            // every weight negative, so every token embeds to nearly the same vector, every v row
            // is nearly the same, and the attention mix stops mattering — measured here as the
            // history moving the last logits by 1e-5 on a scale of 1.8. A logits comparison on
            // such a fixture cannot see a wrong K cache. `2u - 1` spans [-1, 1).
            v.push(((x >> 9) as f32 / (1u32 << 22) as f32 - 1.0) * amp); }
        v };
    let mat = |rows: usize, cols: usize| vec![cols as u64, rows as u64];
    let v1 = |n: usize| vec![n as u64];
    let mut ts: Vec<(String, Vec<u64>, Vec<u8>)> = Vec::new();
    let mut push = |name: String, dims: Vec<u64>, w: Vec<f32>| {
        ts.push((name, dims, bytemuck::cast_slice::<f32, u8>(&w).to_vec())); };
    push("token_embd.weight".into(), mat(VOCAB, D), mk(11, VOCAB * D, 0.5));
    for b in 0..2u32 {
        push(format!("blk.{b}.attn_norm.weight"), v1(D), vec![1.0; D]);
        push(format!("blk.{b}.attn_q.weight"), mat(heads * hd, D), mk(20 + b, heads * hd * D, 0.05));
        push(format!("blk.{b}.attn_k.weight"), mat(kvh * hd, D), mk(40 + b, kvh * hd * D, 0.05));
        push(format!("blk.{b}.attn_v.weight"), mat(kvh * hd, D), mk(60 + b, kvh * hd * D, 0.05));
        push(format!("blk.{b}.attn_output.weight"), mat(D, heads * hd), mk(80 + b, D * heads * hd, 0.05));
        push(format!("blk.{b}.attn_q_norm.weight"), v1(hd), mk(200 + b, hd, 0.25).iter().map(|v| 1.0 + v).collect());
        push(format!("blk.{b}.attn_k_norm.weight"), v1(hd), mk(300 + b, hd, 0.25).iter().map(|v| 1.0 + v).collect());
        push(format!("blk.{b}.ffn_norm.weight"), v1(D), vec![1.0; D]);
        push(format!("blk.{b}.ffn_gate.weight"), mat(FF, D), mk(100 + b, FF * D, 0.05));
        push(format!("blk.{b}.ffn_up.weight"), mat(FF, D), mk(120 + b, FF * D, 0.05));
        push(format!("blk.{b}.ffn_down.weight"), mat(D, FF), mk(140 + b, D * FF, 0.05));
    }
    push("output_norm.weight".into(), v1(D), vec![1.0; D]);
    push("output.weight".into(), mat(VOCAB, D), mk(160, VOCAB * D, 0.05));

    let align32 = |x: u64| x.div_ceil(32) * 32;
    let mut offs = Vec::with_capacity(ts.len());
    let mut off = 0u64;
    for (_, _, by) in &ts { offs.push(off); off = align32(off + by.len() as u64); }
    let mut b = Vec::new();
    let sput = |b: &mut Vec<u8>, x: &str| { b.extend_from_slice(&(x.len() as u64).to_le_bytes()); b.extend_from_slice(x.as_bytes()); };
    let u32b = |b: &mut Vec<u8>, v: u32| b.extend_from_slice(&v.to_le_bytes());
    let u64b = |b: &mut Vec<u8>, v: u64| b.extend_from_slice(&v.to_le_bytes());
    let f32b = |b: &mut Vec<u8>, v: f32| b.extend_from_slice(&v.to_le_bytes());
    let kvu = |b: &mut Vec<u8>, k: &str, v: u32| { sput(b, k); u32b(b, 4); u32b(b, v); };
    let partial = rot < hd;
    let n_meta = 11 + partial as u64;
    b.extend_from_slice(b"GGUF"); u32b(&mut b, 3); u64b(&mut b, ts.len() as u64); u64b(&mut b, n_meta);
    sput(&mut b, "general.architecture"); u32b(&mut b, 8); sput(&mut b, "qwen3");
    kvu(&mut b, "qwen3.block_count", 2);
    kvu(&mut b, "qwen3.attention.head_count", heads as u32);
    kvu(&mut b, "qwen3.attention.head_count_kv", kvh as u32);
    kvu(&mut b, "qwen3.attention.key_length", hd as u32);
    kvu(&mut b, "qwen3.attention.value_length", hd as u32);
    kvu(&mut b, "qwen3.embedding_length", D as u32);
    kvu(&mut b, "qwen3.feed_forward_length", FF as u32);
    kvu(&mut b, "qwen3.vocab_size", VOCAB as u32);
    sput(&mut b, "qwen3.rope.freq_base"); u32b(&mut b, 6); f32b(&mut b, BASE);
    sput(&mut b, "qwen3.attention.layer_norm_rms_epsilon"); u32b(&mut b, 6); f32b(&mut b, 1e-6);
    if partial { kvu(&mut b, "qwen3.rope.dimension_count", rot as u32); }
    for (i, (name, dims, _)) in ts.iter().enumerate() {
        sput(&mut b, name); u32b(&mut b, dims.len() as u32);
        for &dd in dims { u64b(&mut b, dd); }
        u32b(&mut b, 0); u64b(&mut b, offs[i]);
    }
    while b.len() % 32 != 0 { b.push(0); }
    let data_off = b.len() as u64;
    for (i, (_, _, by)) in ts.iter().enumerate() {
        while (b.len() as u64) < data_off + offs[i] { b.push(0); }
        b.extend_from_slice(by);
    }
    b
}

/// Token stream with the SAME token at positions 1 and 5 (the oracle's pair), distinct elsewhere.
const TOKENS: [u32; 7] = [3, 9, 14, 21, 6, 9, 27];
const POS_A: usize = 1;
const POS_B: usize = 5;

struct Run { logits: Vec<Vec<f32>>, k: Vec<Vec<f32>>, v: Vec<Vec<f32>> }

/// A cache whose every K and V lane starts at a huge sentinel.
///
/// `new_cache` allocates from the buffer pool and does not zero, so run N+1 in this process can be
/// handed run N's buffer — already holding the CORRECT rows, since the token stream is the same
/// and V does not depend on position. The first draft of this gate reported a fallback that never
/// wrote V as passing, for exactly that reason. The engine is right not to zero (it writes a row
/// before reading it); a gate comparing caches across runs is what must not inherit answers.
const POISON: f32 = 1.0e30;
fn poisoned_cache(gpu: &Gpu, m: &Model) -> KvCache {
    let rows = TOKENS.len() + 2;
    let mut c = gpu.new_cache_for(m, rows);
    let fill = vec![POISON; rows * m.kv_dim()];
    for t in c.k.iter_mut().chain(c.v.iter_mut()) { *t = gpu.upload(&fill, rows, m.kv_dim()); }
    c
}

fn run(gpu: &Gpu, m: &Model, fused_qkv: bool, kv_append: bool) -> Run {
    if fused_qkv { std::env::remove_var("LOGSCOPE_FUSED_QKV"); } else { std::env::set_var("LOGSCOPE_FUSED_QKV", "0"); }
    set_rope_kv_append(Some(kv_append));
    let c = poisoned_cache(gpu, m);
    let logits = TOKENS.iter().enumerate().map(|(p, &t)| gpu.decode_step(m, &c, t, p)).collect();
    gpu.end_frame();
    let k = c.k.iter().map(|t| gpu.download(t)).collect();
    let v = c.v.iter().map(|t| gpu.download(t)).collect();
    set_rope_kv_append(None);
    std::env::remove_var("LOGSCOPE_FUSED_QKV");
    Run { logits, k, v }
}

fn max_abs_diff(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| if x.is_finite() && y.is_finite() { (x - y).abs() } else { f32::INFINITY }).fold(0.0, f32::max)
}

/// The engine-independent check on layer 0's K rows. Returns (worst pass-through diff, worst
/// rotary-pair error, smallest rotary-pair magnitude seen) — the last so a vacuous pass on
/// all-zero rows is visible rather than green.
fn rotation_oracle(k0: &[f32], kv: usize, hd: usize, rot: usize, kvh: usize) -> (f32, f32, f32) {
    let (ra, rb) = (&k0[POS_A * kv..(POS_A + 1) * kv], &k0[POS_B * kv..(POS_B + 1) * kv]);
    let (mut pass, mut pair, mut mag) = (0f32, 0f32, f32::INFINITY);
    let rh = rot / 2;
    for h in 0..kvh {
        let o = h * hd;
        for j in rot..hd { pass = pass.max((ra[o + j] - rb[o + j]).abs()); }
        for i in 0..rh {
            // f64 so the oracle's own rounding is not what gets measured
            let theta = (POS_B - POS_A) as f64 * (BASE as f64).powf(-2.0 * i as f64 / rot as f64);
            let (c, s) = (theta.cos(), theta.sin());
            let (a0, a1) = (ra[o + i] as f64, ra[o + i + rh] as f64);
            let (e0, e1) = (a0 * c - a1 * s, a0 * s + a1 * c);
            pair = pair.max((e0 - rb[o + i] as f64).abs().max((e1 - rb[o + i + rh] as f64).abs()) as f32);
            mag = mag.min((a0 * a0 + a1 * a1).sqrt() as f32);
        }
    }
    (pass, pair, mag)
}

fn main() {
    let gpu = Gpu::new();
    eprintln!("gate-qknorm-envelope: adapter = {}", gpu.adapter_name);
    let mut ok = true;
    let mut ck = |l: String, v: bool| { ok &= v; println!("  [{}] {l}", if v { "\u{2713}" } else { "\u{2717} FAIL" }); };
    // (head_dim, rotary dims, q heads, kv heads). The first is the shape every existing fixture
    // has; the third and fourth are Qwen3.5/Ornith's attention layers; the rest bracket them.
    // The last shape is OUTSIDE the fused kernels' envelope (QKNORM_MAX_HD): it exercises the helpers'
    // unfused fallback, which no real model reaches today and which would otherwise be untested code.
    let shapes = [(16, 16, 4, 2), (128, 128, 2, 1), (256, 256, 2, 1), (256, 64, 2, 1), (64, 32, 4, 2), (128, 64, 2, 1),
                  (512, 128, 1, 1), (2 * QKNORM_MAX_HD, 64, 1, 1)];
    ck(format!("the shape list crosses the envelope in both directions (max fusable head_dim {QKNORM_MAX_HD})"),
       shapes.iter().any(|&(hd, rot, ..)| qknorm_rope_fusable(hd, rot) && hd == QKNORM_MAX_HD)
       && shapes.iter().any(|&(hd, rot, ..)| !qknorm_rope_fusable(hd, rot)));
    for (hd, rot, heads, kvh) in shapes {
        println!("hd={hd} rot={rot} heads={heads}/{kvh} ({})", if qknorm_rope_fusable(hd, rot) { "fused kernel" } else { "unfused fallback" });
        let bytes = write_gguf(hd, rot, heads, kvh);
        // f32-resident: quantization noise would blur a bit-level comparison of rope paths.
        let m = match gpu.load_gguf_opt(&bytes, false) { Ok(m) => m, Err(e) => { ck(format!("loads ({e})"), false); continue } };
        ck(format!("loader read the shape back: head_dim {} rope_dims {}", m.head_dim, m.rope_dims), m.head_dim == hd && m.rope_dims == rot);
        let kv = m.kv_dim();
        let reference = run(&gpu, &m, false, false);
        {
            // How much does the HISTORY move the last token's logits? If this is ~0 the logits
            // comparison below cannot see a wrong K cache, whatever it reports — which is exactly
            // what this gate's first draft did (K off by 2.4, logits "matching" to 1e-5).
            std::env::set_var("LOGSCOPE_FUSED_QKV", "0");
            let c = poisoned_cache(&gpu, &m);
            let mut alt = TOKENS; alt[0] = 17; alt[2] = 4; alt[3] = 30;
            let mut last = Vec::new();
            for (p, &t) in alt.iter().enumerate() { last = gpu.decode_step(&m, &c, t, p); }
            gpu.end_frame();
            std::env::remove_var("LOGSCOPE_FUSED_QKV");
            let base_l = reference.logits.last().unwrap();
            let scale = base_l.iter().fold(0f32, |a, v| a.max(v.abs()));
            let sens = max_abs_diff(&last, base_l);
            ck(format!("logits SEE the history: changing 3 earlier tokens moves the last logits by {sens:.2e} (scale {scale:.2e})"), sens > 0.02 * scale);
        }
        let (pass, pair, mag) = rotation_oracle(&reference.k[0], kv, hd, rot, kvh);
        ck(format!("oracle is not vacuous: smallest rotary pair magnitude {mag:.3e}"), mag > 1e-4 && mag.is_finite());
        ck(format!("UNFUSED reference: pass-through lanes identical across positions (max diff {pass:.2e})"), pass == 0.0);
        ck(format!("UNFUSED reference: rotary pairs follow base^(-2i/rot) (max err {pair:.2e})"), pair < 2e-4);
        for (name, kv_append) in [("fused + copy path (QKNORM_ROPE)", false), ("fused + KV-append (QKNORM_ROPE_KV_APPEND)", true)] {
            let r = run(&gpu, &m, true, kv_append);
            let (pass, pair, mag) = rotation_oracle(&r.k[0], kv, hd, rot, kvh);
            // Per run: an unwritten lane is 0 in both rows, and (0,0) rotated is (0,0) — the
            // oracle passes on a kernel that never wrote the pair. hd=256 did exactly that.
            // Unwritten now means the sentinel, not zero — so count sentinels directly, over K AND V,
            // every layer, every row this run was responsible for.
            let used = TOKENS.len() * kv;
            let left = (0..m.n_layers).map(|l| r.k[l][..used].iter().chain(&r.v[l][..used]).filter(|x| x.abs() > 1e20 || !x.is_finite()).count()).sum::<usize>();
            ck(format!("{name}: every K and V lane of every written row was written ({left} sentinels left; smallest pair magnitude {mag:.3e})"), left == 0 && mag > 1e-4 && mag < 1e6);
            ck(format!("{name}: pass-through lanes identical (max diff {pass:.2e})"), pass == 0.0);
            ck(format!("{name}: rotary pairs follow base^(-2i/rot) (max err {pair:.2e})"), pair < 2e-4);
            let rows = TOKENS.len() * kv;
            if std::env::var_os("GATE_DEBUG").is_some() {
                for l in 0..m.n_layers {
                    let bad: Vec<usize> = (0..rows).filter(|&i| (r.k[l][i] - reference.k[l][i]).abs() > 1e-4).collect();
                    let lanes: std::collections::BTreeSet<usize> = bad.iter().map(|i| (i % kv) % hd).collect();
                    let zeros = (0..rows).filter(|&i| r.k[l][i].abs() > 1e20).count();
                    println!("    debug L{l}: {} of {rows} K values differ; lanes {:?}..{:?} ({} distinct); never written (sentinel): {zeros}",
                        bad.len(), lanes.iter().next(), lanes.iter().last(), lanes.len());
                }
            }
            let dk = (0..m.n_layers).map(|l| max_abs_diff(&r.k[l][..rows], &reference.k[l][..rows])).fold(0.0, f32::max);
            let dl = r.logits.iter().zip(&reference.logits).map(|(a, b)| max_abs_diff(a, b)).fold(0.0, f32::max);
            // Tolerance, not bit-equality: the fused kernel reduces the head's sum of squares in a
            // different order than RMSNORM does, so the last bits legitimately differ.
            ck(format!("{name}: K rows match the unfused reference (max diff {dk:.2e})"), dk < 1e-4);
            // NOT bit-exact, and I asserted it was: the fused block folds the RMSNorm into the wqkv
            // matvec (norm_apply) where the reference norms and then projects, so V differs in the
            // last bits (~2e-7 measured). What V must never be is the sentinel or another row.
            let dv = (0..m.n_layers).map(|l| max_abs_diff(&r.v[l][..rows], &reference.v[l][..rows])).fold(0.0, f32::max);
            ck(format!("{name}: V rows match the unfused reference (max diff {dv:.2e})"), dv < 1e-5);
            ck(format!("{name}: logits match the unfused reference (max diff {dl:.2e})"), dl < 1e-3);
        }
    }
    // THE N-SESSION PATH. decode_batched's body is llama-shaped and applied no QK-norm and no
    // partial rotary to anything, refusing nothing. Two sessions with DIFFERENT streams (so a
    // row mix-up shows) against each stream decoded alone through the unfused reference.
    for (hd, rot, heads, kvh) in [(128, 128, 2, 1), (256, 64, 2, 1)] {
        println!("decode_batched, hd={hd} rot={rot}");
        let m = gpu.load_gguf_opt(&write_gguf(hd, rot, heads, kvh), false).expect("load");
        let streams: [[u32; 5]; 2] = [[3, 9, 14, 21, 6], [27, 4, 9, 30, 11]];
        std::env::set_var("LOGSCOPE_FUSED_QKV", "0");
        let solo: Vec<Vec<f32>> = streams.iter().map(|st| {
            let c = poisoned_cache(&gpu, &m);
            let mut last = Vec::new();
            for (p, &t) in st.iter().enumerate() { last = gpu.decode_step(&m, &c, t, p); }
            gpu.end_frame();
            last
        }).collect();
        std::env::remove_var("LOGSCOPE_FUSED_QKV");
        let mut sessions = vec![gpu.session_new(&m, 8, 1), gpu.session_new(&m, 8, 2)];
        for step in 0..5 { gpu.decode_batched(&m, &mut sessions, &[streams[0][step], streams[1][step]]); }
        ck(format!("the model is outside the batched body's envelope, and says why: {:?}", m.batched_decode_gap(rot)), m.batched_decode_gap(rot).is_some());
        for i in 0..2 {
            let d = max_abs_diff(&sessions[i].logits, &solo[i]);
            ck(format!("session {i}: batched logits match that stream decoded alone (max diff {d:.2e}, pos {})", sessions[i].pos), d < 1e-3 && sessions[i].pos == 5);
        }
        let cross = max_abs_diff(&sessions[0].logits, &solo[1]);
        ck(format!("and the two streams really differ, so a swapped row would show (cross diff {cross:.2e})"), cross > 1e-2);
    }
    println!("{}", if ok { "gate-qknorm-envelope: PASS" } else { "gate-qknorm-envelope: FAIL" });
    std::process::exit(if ok { 0 } else { 1 });
}
