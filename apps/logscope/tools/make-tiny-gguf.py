#!/usr/bin/env python3
"""Emit a tiny but LOADABLE qwen2-shaped GGUF (f32 tensors) for end-to-end bench
tests on boxes with no model files — CI, sandboxes, lavapipe. ~1 MB, 2 layers.
Loader-refusal-driven: if load_gguf refuses a missing key, add it here; the
refusals name what they want by design."""
import struct, sys

D, FF, L, VOCAB, NH, NKV = 64, 128, 2, 256, 4, 2
out = sys.argv[1] if len(sys.argv) > 1 else "/tmp/tiny-qwen2.gguf"

def s(x): b = x.encode(); return struct.pack("<Q", len(b)) + b
kv, KV_COUNT = b"", 0
def kv_str(k, v):
    global kv, KV_COUNT; KV_COUNT += 1; kv += s(k) + struct.pack("<I", 8) + s(v)
def kv_u32(k, v):
    global kv, KV_COUNT; KV_COUNT += 1; kv += s(k) + struct.pack("<I", 4) + struct.pack("<I", v)
def kv_arr_str(k, vals):
    global kv, KV_COUNT; KV_COUNT += 1
    kv += s(k) + struct.pack("<I", 9) + struct.pack("<I", 8) + struct.pack("<Q", len(vals))
    for v in vals: kv += s(v)
def kv_bool(k, v):
    global kv, KV_COUNT; KV_COUNT += 1
    kv += s(k) + struct.pack("<I", 7) + struct.pack("<B", 1 if v else 0)
def kv_f32(k, v):
    global kv, KV_COUNT; KV_COUNT += 1; kv += s(k) + struct.pack("<I", 6) + struct.pack("<f", v)

kv_str("general.architecture", "qwen2")
kv_str("general.name", "tiny-qwen2-testmodel")
kv_u32("qwen2.block_count", L)
kv_u32("qwen2.context_length", 512)
kv_u32("qwen2.embedding_length", D)
kv_u32("qwen2.feed_forward_length", FF)
kv_u32("qwen2.attention.head_count", NH)
kv_u32("qwen2.attention.head_count_kv", NKV)
kv_f32("qwen2.attention.layer_norm_rms_epsilon", 1e-6)
kv_f32("qwen2.rope.freq_base", 10000.0)
kv_u32("qwen2.vocab_size", VOCAB)
kv_str("tokenizer.ggml.model", "none")

def weights(n):
    # deterministic small values, no numpy dependency
    x = 0x2545F4914F6CDD1D
    o = bytearray()
    for i in range(n):
        x ^= (x << 13) & 0xFFFFFFFFFFFFFFFF; x ^= x >> 7; x ^= (x << 17) & 0xFFFFFFFFFFFFFFFF
        o += struct.pack("<f", ((x >> 40) / (1 << 24) - 0.5) * 0.08)
    return bytes(o)

tensors = [("token_embd.weight", [D, VOCAB]), ("output_norm.weight", [D]), ("output.weight", [D, VOCAB])]
for i in range(L):
    p = f"blk.{i}."
    tensors += [(p+"attn_norm.weight", [D]), (p+"attn_q.weight", [D, D]), (p+"attn_k.weight", [D, D*NKV//NH]),
                (p+"attn_v.weight", [D, D*NKV//NH]), (p+"attn_output.weight", [D, D]), (p+"ffn_norm.weight", [D]),
                (p+"ffn_gate.weight", [D, FF]), (p+"ffn_up.weight", [D, FF]), (p+"ffn_down.weight", [FF, D])]

def write(out, tensors, n_kv):
    ALIGN = 32
    infos, data, off = b"", b"", 0
    for name, dims in tensors:
        n = 1
        for d in dims: n *= d
        pad = (-off) % ALIGN
        data += b"\0" * pad; off += pad
        infos += s(name) + struct.pack("<I", len(dims)) + b"".join(struct.pack("<Q", d) for d in dims)
        infos += struct.pack("<I", 0) + struct.pack("<Q", off)   # type 0 = f32
        if name.endswith("exp_probs_b.bias"):
            # CONTROLLED values, not noise: +3 beats every sigmoid(<1), so expert 2 must
            # appear in every selection — the flip the bias gate observes. Others 0.
            w = b"".join(struct.pack("<f", v) for v in ([0.0, 0.0, 3.0, 0.0] * (n // 4 + 1))[:n])
        else:
            w = weights(n)
        data += w; off += len(w)

    hdr = struct.pack("<IIQQ", 0x46554747, 3, len(tensors), n_kv) + kv + infos
    pad = (-len(hdr)) % ALIGN
    open(out, "wb").write(hdr + b"\0" * pad + data)
    print(f"{out}: {len(tensors)} tensors, {len(hdr)+pad+len(data)} bytes")


def emit_llada2(out, nomaskkey=False, noshexp=False, alldense=False, sepqkv=False):
    global kv, KV_COUNT
    kv, KV_COUNT = b"", 0
    D, HD, NH, NKV, L, VOCAB = 64, 16, 4, 2, 3, 256
    KVW, FFD, NEXP, USED, FFE, FFS = HD*NKV, 128, 4, 2, 64, 64
    kv_str("general.architecture", "llada2")
    kv_str("general.name", "tiny-llada2-testmodel")
    kv_u32("llada2.block_count", L)
    kv_u32("llada2.context_length", 512)
    kv_u32("llada2.embedding_length", D)
    kv_u32("llada2.feed_forward_length", FFD)
    kv_u32("llada2.expert_feed_forward_length", FFE)
    kv_u32("llada2.expert_shared_feed_forward_length", FFS)
    kv_u32("llada2.attention.head_count", NH)
    kv_u32("llada2.rope.dimension_count", HD // 2)
    kv_u32("llada2.attention.head_count_kv", NKV)
    kv_f32("llada2.attention.layer_norm_rms_epsilon", 1e-6)
    kv_f32("llada2.rope.freq_base", 10000.0)
    kv_u32("llada2.vocab_size", VOCAB)
    kv_u32("llada2.expert_count", NEXP)
    kv_u32("llada2.expert_used_count", USED)
    kv_u32("llada2.expert_shared_count", 1)
    kv_u32("llada2.expert_gating_func", 2)          # sigmoid — the hazard-1 key, present
    kv_u32("llada2.leading_dense_block_count", 3 if alldense else 1)
    kv_u32("llada2.nextn_predict_layers", 0)
    if not nomaskkey:
        kv_u32("tokenizer.ggml.mask_token_id", 250)
    toks = [f"tok{i}" for i in range(256)]
    toks[250] = "<|mask|>"; toks[251] = "[MASK-ish]"
    kv_arr_str("tokenizer.ggml.tokens", toks)
    kv_str("diffusion.shift_logits", "false")
    tensors = [("token_embd.weight", [D, VOCAB]), ("output_norm.weight", [D]), ("output.weight", [D, VOCAB])]
    for i in range(L):
        p = f"blk.{i}."
        tensors += [(p+"attn_norm.weight", [D])]
        if sepqkv:
            tensors += [(p+"attn_q.weight", [D, D]), (p+"attn_k.weight", [D, KVW]), (p+"attn_v.weight", [D, KVW])]
        else:
            tensors += [(p+"attn_qkv.weight", [D, D + 2*KVW])]
        tensors += [   # fused: rows = d + 2*kv, gguf dims fastest-first
                    (p+"attn_q_norm.weight", [HD]), (p+"attn_k_norm.weight", [HD]),
                    (p+"attn_output.weight", [D, D]), (p+"ffn_norm.weight", [D])]
        if i == 0:   # dense lead
            tensors += [(p+"ffn_gate.weight", [D, FFD]), (p+"ffn_up.weight", [D, FFD]), (p+"ffn_down.weight", [FFD, D])]
        else:        # MoE + shared expert
            tensors += [(p+"ffn_gate_inp.weight", [D, NEXP]),
                        (p+"exp_probs_b.bias", [NEXP]),
                        (p+"ffn_gate_exps.weight", [D, FFE, NEXP]), (p+"ffn_up_exps.weight", [D, FFE, NEXP]),
                        (p+"ffn_down_exps.weight", [FFE, D, NEXP]),
                        ] + ([] if noshexp else [
                        (p+"ffn_gate_shexp.weight", [D, FFS]), (p+"ffn_up_shexp.weight", [D, FFS]),
                        (p+"ffn_down_shexp.weight", [FFS, D])])
    write(out, tensors, KV_COUNT)

def emit_qwen3(out):
    global kv, KV_COUNT
    kv, KV_COUNT = b"", 0
    D, HD, NH, NKV, L, VOCAB, FF = 64, 16, 4, 2, 2, 256, 128
    kv_str("general.architecture", "qwen3")
    kv_str("general.name", "tiny-qwen3-testmodel")
    kv_u32("qwen3.block_count", L)
    kv_u32("qwen3.context_length", 512)
    kv_u32("qwen3.embedding_length", D)
    kv_u32("qwen3.feed_forward_length", FF)
    kv_u32("qwen3.attention.head_count", NH)
    kv_u32("qwen3.attention.head_count_kv", NKV)
    kv_u32("qwen3.attention.key_length", HD)
    kv_u32("qwen3.attention.value_length", HD)
    kv_f32("qwen3.attention.layer_norm_rms_epsilon", 1e-6)
    kv_f32("qwen3.rope.freq_base", 10000.0)
    kv_u32("qwen3.vocab_size", VOCAB)
    kv_str("tokenizer.ggml.model", "none")
    tensors = [("token_embd.weight", [D, VOCAB]), ("output_norm.weight", [D]), ("output.weight", [D, VOCAB])]
    for i in range(L):
        p = f"blk.{i}."
        tensors += [(p+"attn_norm.weight", [D]),
                    (p+"attn_q.weight", [D, NH*HD]), (p+"attn_k.weight", [D, NKV*HD]), (p+"attn_v.weight", [D, NKV*HD]),
                    (p+"attn_q_norm.weight", [HD]), (p+"attn_k_norm.weight", [HD]),
                    (p+"attn_output.weight", [NH*HD, D]), (p+"ffn_norm.weight", [D]),
                    (p+"ffn_gate.weight", [D, FF]), (p+"ffn_up.weight", [D, FF]), (p+"ffn_down.weight", [FF, D])]
    write(out, tensors, KV_COUNT)

def emit_qwen3moe(out):
    global kv, KV_COUNT
    kv, KV_COUNT = b"", 0
    D, HD, NH, NKV, L, VOCAB, NEXP, USED, FFE = 64, 16, 4, 2, 2, 256, 4, 2, 64
    kv_str("general.architecture", "qwen3moe")
    kv_str("general.name", "tiny-qwen3moe-testmodel")
    kv_u32("qwen3moe.block_count", L)
    kv_u32("qwen3moe.context_length", 512)
    kv_u32("qwen3moe.embedding_length", D)
    kv_u32("qwen3moe.feed_forward_length", 4*FFE)
    kv_u32("qwen3moe.expert_feed_forward_length", FFE)
    kv_u32("qwen3moe.attention.head_count", NH)
    kv_u32("qwen3moe.attention.head_count_kv", NKV)
    kv_u32("qwen3moe.attention.key_length", HD)
    kv_u32("qwen3moe.attention.value_length", HD)
    kv_f32("qwen3moe.attention.layer_norm_rms_epsilon", 1e-6)
    kv_f32("qwen3moe.rope.freq_base", 10000.0)
    kv_u32("qwen3moe.vocab_size", VOCAB)
    kv_u32("qwen3moe.expert_count", NEXP)
    kv_u32("qwen3moe.expert_used_count", USED)
    kv_str("tokenizer.ggml.model", "none")
    tensors = [("token_embd.weight", [D, VOCAB]), ("output_norm.weight", [D]), ("output.weight", [D, VOCAB])]
    for i in range(L):
        p = f"blk.{i}."
        tensors += [(p+"attn_norm.weight", [D]),
                    (p+"attn_q.weight", [D, NH*HD]), (p+"attn_k.weight", [D, NKV*HD]), (p+"attn_v.weight", [D, NKV*HD]),
                    (p+"attn_q_norm.weight", [HD]), (p+"attn_k_norm.weight", [HD]),
                    (p+"attn_output.weight", [NH*HD, D]), (p+"ffn_norm.weight", [D]),
                    (p+"ffn_gate_inp.weight", [D, NEXP]),
                    (p+"ffn_gate_exps.weight", [D, FFE, NEXP]), (p+"ffn_up_exps.weight", [D, FFE, NEXP]),
                    (p+"ffn_down_exps.weight", [FFE, D, NEXP])]
    write(out, tensors, KV_COUNT)

def emit_glm4moe(out):
    global kv, KV_COUNT
    kv, KV_COUNT = b"", 0
    D, HD, NH, NKV, L, VOCAB = 64, 16, 4, 2, 2, 256
    NEXP, USED, FFE, FFD, KVW = 4, 2, 64, 128, 32
    kv_str("general.architecture", "glm4moe")
    kv_str("general.name", "tiny-glm4moe-testmodel")
    kv_u32("glm4moe.block_count", L)
    kv_u32("glm4moe.context_length", 512)
    kv_u32("glm4moe.embedding_length", D)
    kv_u32("glm4moe.feed_forward_length", FFD)
    kv_u32("glm4moe.expert_feed_forward_length", FFE)
    kv_u32("glm4moe.attention.head_count", NH)
    kv_u32("glm4moe.attention.head_count_kv", NKV)
    kv_u32("glm4moe.attention.key_length", HD)
    kv_u32("glm4moe.attention.value_length", HD)
    kv_u32("glm4moe.rope.dimension_count", HD)
    kv_f32("glm4moe.attention.layer_norm_rms_epsilon", 1e-6)
    kv_f32("glm4moe.rope.freq_base", 10000.0)
    kv_u32("glm4moe.vocab_size", VOCAB)
    kv_u32("glm4moe.expert_count", NEXP)
    kv_u32("glm4moe.expert_used_count", USED)
    kv_u32("glm4moe.expert_shared_count", 1)
    kv_u32("glm4moe.expert_gating_func", 2)
    kv_f32("glm4moe.expert_weights_scale", 2.5)
    kv_bool("glm4moe.expert_weights_norm", True)
    kv_u32("glm4moe.leading_dense_block_count", 1)
    kv_u32("glm4moe.nextn_predict_layers", 0)
    kv_str("tokenizer.ggml.model", "none")
    tensors = [("token_embd.weight", [D, VOCAB]), ("output_norm.weight", [D]), ("output.weight", [D, VOCAB])]
    for i in range(L):
        p = f"blk.{i}."
        tensors += [(p+"attn_norm.weight", [D]),
                    (p+"attn_q.weight", [D, NH*HD]), (p+"attn_k.weight", [D, KVW]), (p+"attn_v.weight", [D, KVW]),
                    (p+"attn_q_norm.weight", [HD]), (p+"attn_k_norm.weight", [HD]),
                    (p+"attn_output.weight", [NH*HD, D]),
                    (p+"post_attention_norm.weight", [D])]
        if i == 0 or alldense:
            tensors += [(p+"ffn_gate.weight", [D, FFD]), (p+"ffn_up.weight", [D, FFD]), (p+"ffn_down.weight", [FFD, D])]
        else:
            tensors += [(p+"ffn_gate_inp.weight", [D, NEXP]),
                        (p+"exp_probs_b.bias", [NEXP]),
                        (p+"ffn_gate_exps.weight", [D, FFE, NEXP]), (p+"ffn_up_exps.weight", [D, FFE, NEXP]),
                        (p+"ffn_down_exps.weight", [FFE, D, NEXP]),
                        (p+"ffn_gate_shexp.weight", [D, FFE]), (p+"ffn_up_shexp.weight", [D, FFE]),
                        (p+"ffn_down_shexp.weight", [FFE, D])]
    write(out, tensors, KV_COUNT)

if len(sys.argv) > 2 and sys.argv[2] == "glm4moe":
    emit_glm4moe(out)
    sys.exit(0)
if len(sys.argv) > 2 and sys.argv[2] == "qwen3moe":
    emit_qwen3moe(out)
    sys.exit(0)
if len(sys.argv) > 2 and sys.argv[2] == "qwen3":
    emit_qwen3(out)
    sys.exit(0)
if len(sys.argv) > 2 and sys.argv[2] == "llada2":
    emit_llada2(out, nomaskkey=("nomaskkey" in sys.argv[3:]), noshexp=("noshexp" in sys.argv[3:]), alldense=("alldense" in sys.argv[3:]), sepqkv=("sepqkv" in sys.argv[3:]))
    sys.exit(0)

write(out, tensors, KV_COUNT)
