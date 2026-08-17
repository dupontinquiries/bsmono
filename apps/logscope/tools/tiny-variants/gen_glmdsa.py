#!/usr/bin/env python3
"""gen_glmdsa.py — a tiny `glm-dsa` (GLM-5.2) checkpoint: MLA + MoE + IndexShare.

WHY THIS EXISTS. GLM-5.2 is 744B-A40B; the smallest real artifact is ~150 GiB, so nothing in this
repo has ever exercised the MLA loader on an actual file — `mla_dims`, the 3-D `attn_k_b`/`attn_v_b`
reconstruction and the MLA cache are all reachable only by a checkpoint nobody can put in CI. This
builds one at 3 layers and d=64.

It reproduces the two things that make GLM-5.2 different from DeepSeek-V3, both of which are LOADER
concerns rather than forward-pass ones:

  * MLA tensor set: attn_q_a / attn_q_a_norm / attn_q_b (low-rank query), attn_kv_a_mqa /
    attn_kv_a_norm, and the 3-D attn_k_b [qk_nope, kv_lora, heads] and attn_v_b [kv_lora, v_head,
    heads] per-head reconstruction matrices.

  * INDEXSHARE: `index_topk_freq` 4 with `index_skip_topk_offset` 3 means only about one layer in
    four CARRIES indexer tensors. Stock llama.cpp demands one per layer and therefore FAILS TO LOAD
    converted GLM-5.2 GGUFs — a third-party conversion needed a custom patch for exactly this. So
    this fixture deliberately puts indexer tensors on SOME layers and not others: a loader that
    assumes they are uniform breaks here, cheaply, instead of on a 150 GiB download.

Usage: gen_glmdsa.py <out.gguf>
"""
import struct, sys

D, HD, NH, L, VOCAB = 64, 16, 4, 3, 256
Q_LORA, KV_LORA, QK_NOPE, QK_ROPE, V_HEAD = 32, 16, 8, 8, 16
FFD, NEXP, USED, FFE = 128, 8, 2, 32
INDEX_HEADS, INDEX_KEY, INDEX_TOPK = 2, 8, 64
TOPK_FREQ, SKIP_OFFSET = 4, 1          # 1 layer in 4, after the first — IndexShare in miniature
P = "glm-dsa"

def s(x):
    b = x.encode(); return struct.pack("<Q", len(b)) + b
def kv_str(k, v): return s(k) + struct.pack("<I", 8) + s(v)
def kv_u32(k, v): return s(k) + struct.pack("<I", 4) + struct.pack("<I", v)
def kv_f32(k, v): return s(k) + struct.pack("<I", 6) + struct.pack("<f", v)
def kv_bool(k, v): return s(k) + struct.pack("<I", 7) + struct.pack("<B", 1 if v else 0)
def kv_arr_str(k, vals):
    return s(k) + struct.pack("<I", 9) + struct.pack("<I", 8) + struct.pack("<Q", len(vals)) \
         + b"".join(s(v) for v in vals)

def weights(n):
    x = 0x2545F4914F6CDD1D
    o = bytearray()
    for _ in range(n):
        x ^= (x << 13) & 0xFFFFFFFFFFFFFFFF; x ^= x >> 7; x ^= (x << 17) & 0xFFFFFFFFFFFFFFFF
        o += struct.pack("<f", ((x >> 40) / (1 << 24) - 0.5) * 0.08)
    return bytes(o)

def has_indexer(layer: int) -> bool:
    """IndexShare: a full indexer layer every TOPK_FREQ layers after SKIP_OFFSET."""
    return layer >= SKIP_OFFSET and (layer - SKIP_OFFSET) % TOPK_FREQ == 0

kv = b"".join([
    kv_str("general.architecture", P),
    kv_u32(f"{P}.block_count", L),
    kv_u32(f"{P}.context_length", 4096),
    kv_u32(f"{P}.embedding_length", D),
    kv_u32(f"{P}.feed_forward_length", FFD),
    kv_u32(f"{P}.attention.head_count", NH),
    kv_u32(f"{P}.attention.head_count_kv", 1),          # MLA: one latent kv "head"
    kv_u32(f"{P}.attention.q_lora_rank", Q_LORA),
    kv_u32(f"{P}.attention.kv_lora_rank", KV_LORA),
    kv_u32(f"{P}.attention.key_length", QK_NOPE + QK_ROPE),
    kv_u32(f"{P}.attention.value_length", V_HEAD),
    kv_f32(f"{P}.attention.layer_norm_rms_epsilon", 1e-6),
    kv_u32(f"{P}.rope.dimension_count", QK_ROPE),
    kv_f32(f"{P}.rope.freq_base", 10000.0),
    kv_u32(f"{P}.expert_count", NEXP),
    kv_u32(f"{P}.expert_used_count", USED),
    kv_u32(f"{P}.expert_shared_count", 1),
    kv_u32(f"{P}.expert_feed_forward_length", FFE),
    kv_u32(f"{P}.leading_dense_block_count", 1),
    kv_u32(f"{P}.expert_gating_func", 2),               # sigmoid
    kv_bool(f"{P}.expert_weights_norm", True),
    kv_f32(f"{P}.expert_weights_scale", 2.5),
    # IndexShare / DSA
    kv_u32(f"{P}.attention.index_topk", INDEX_TOPK),
    kv_u32(f"{P}.attention.index_head_count", INDEX_HEADS),
    kv_u32(f"{P}.attention.index_key_length", INDEX_KEY),
    kv_u32(f"{P}.index_topk_freq", TOPK_FREQ),
    kv_u32(f"{P}.index_skip_topk_offset", SKIP_OFFSET),
    kv_u32(f"{P}.vocab_size", VOCAB),
    kv_str("tokenizer.ggml.model", "gpt2"),
    kv_arr_str("tokenizer.ggml.tokens", [f"t{i}" for i in range(VOCAB)]),
])
N_KV = 30

tensors = [("token_embd.weight", [D, VOCAB]), ("output_norm.weight", [D]), ("output.weight", [D, VOCAB])]
for i in range(L):
    p = f"blk.{i}."
    tensors += [
        (p + "attn_norm.weight", [D]),
        # MLA: low-rank query, latent kv, and the 3-D per-head reconstruction pair.
        (p + "attn_q_a.weight", [D, Q_LORA]),
        (p + "attn_q_a_norm.weight", [Q_LORA]),
        (p + "attn_q_b.weight", [Q_LORA, NH * (QK_NOPE + QK_ROPE)]),
        (p + "attn_kv_a_mqa.weight", [D, KV_LORA + QK_ROPE]),
        (p + "attn_kv_a_norm.weight", [KV_LORA]),
        (p + "attn_k_b.weight", [QK_NOPE, KV_LORA, NH]),
        (p + "attn_v_b.weight", [KV_LORA, V_HEAD, NH]),
        (p + "attn_output.weight", [NH * V_HEAD, D]),
        (p + "ffn_norm.weight", [D]),
    ]
    if has_indexer(i):
        # Only SOME layers carry these — that asymmetry is the point of the fixture.
        tensors += [
            (p + "indexer.attn_k.weight", [D, INDEX_KEY]),
            (p + "indexer.attn_q_b.weight", [Q_LORA, INDEX_HEADS * INDEX_KEY]),
            (p + "indexer.k_norm.weight", [INDEX_KEY]),
            (p + "indexer.proj.weight", [INDEX_HEADS, D]),
        ]
    if i == 0:
        tensors += [(p + "ffn_gate.weight", [D, FFD]), (p + "ffn_up.weight", [D, FFD]),
                    (p + "ffn_down.weight", [FFD, D])]
    else:
        tensors += [
            (p + "ffn_gate_inp.weight", [D, NEXP]),
            (p + "exp_probs_b.bias", [NEXP]),
            (p + "ffn_gate_exps.weight", [D, FFE, NEXP]),
            (p + "ffn_up_exps.weight", [D, FFE, NEXP]),
            (p + "ffn_down_exps.weight", [FFE, D, NEXP]),
            (p + "ffn_gate_shexp.weight", [D, FFE]),
            (p + "ffn_up_shexp.weight", [D, FFE]),
            (p + "ffn_down_shexp.weight", [FFE, D]),
        ]

def write(out, tensors, n_kv):
    ALIGN = 32
    infos, data, off = b"", b"", 0
    for name, dims in tensors:
        n = 1
        for d in dims: n *= d
        pad = (-off) % ALIGN
        data += b"\x00" * pad; off += pad
        nm = name.encode()
        infos += struct.pack("<Q", len(nm)) + nm + struct.pack("<I", len(dims))
        for d in dims: infos += struct.pack("<Q", d)
        infos += struct.pack("<I", 0) + struct.pack("<Q", off)
        data += weights(n); off += n * 4
    hdr = b"GGUF" + struct.pack("<I", 3) + struct.pack("<Q", len(tensors)) + struct.pack("<Q", n_kv) + kv + infos
    pad = (-len(hdr)) % ALIGN
    open(out, "wb").write(hdr + b"\x00" * pad + data)

if __name__ == "__main__":
    out = sys.argv[1] if len(sys.argv) > 1 else "/tmp/tiny-glmdsa.gguf"
    write(out, tensors, N_KV)
    idx = [i for i in range(L) if has_indexer(i)]
    print(f"{out}: {len(tensors)} tensors, {L} layers, indexer on layers {idx} "
          f"(IndexShare freq {TOPK_FREQ} offset {SKIP_OFFSET})")
