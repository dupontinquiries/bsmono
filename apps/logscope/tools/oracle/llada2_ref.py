#!/usr/bin/env python3
"""llada2_ref.py — a NumPy transcription of LLaDA2's BIDIRECTIONAL forward, straight from
inclusionAI's modeling_llada2_moe.py, used as an oracle for `denoise_stats`.

WHY THIS EXISTS. The causal path has a cross-engine oracle (llama.cpp, 4 models x 48/48). The
bidirectional path has none, because llada2 has never landed in mainline llama.cpp — so every
diffusion result to date has been compared only against itself. This file closes that hole: it
reads the same f32 GGUF the engine reads and computes logits with no attention mask at all, which
is what a denoise step does.

Reference ordering, verified line by line against modeling_llada2_moe.py:
  - qkv = W_qkv @ x, split [n_head*hd, n_kv*hd, n_kv*hd]  (contiguous, BailingMoe2 layout)
  - QK-NORM per head (RMSNorm over head_dim) BEFORE rope
  - PARTIAL rope on the first rope_dim dims (NEOX half-split within the rotary block); the
    remaining dims pass through untouched
  - GQA: each kv head serves n_head/n_kv query heads; scaling = head_dim ** -0.5
  - MoE: sigmoid(logits) -> +expert_bias for RANKING ONLY -> group-limited top-k (top-2 sum per
    group, topk_group groups) -> weights are the UNBIASED sigmoid at the winners, normalised with
    a +1e-20 guard, times routed_scaling_factor; shared expert always added
  - residual: x + attn, then x + moe, with attn_norm / ffn_norm as pre-norms

Usage:  llada2_ref.py <model.gguf> <n_tokens> [--dump]
Prints one line per position: argmax_id p_top margin entropy — the same four stats
`denoise_stats` returns, so the two can be diffed directly.
"""
import struct, sys
import numpy as np

# ---------- GGUF (f32 only; the oracle runs on f32 tinies by design) ----------
def gstr(b, o):
    n = struct.unpack_from("<Q", b, o)[0]; o += 8
    return b[o:o+n].decode(), o + n

def skip_val(b, o, ty):
    scal = {0:1,1:1,2:2,3:2,4:4,5:4,6:4,7:1,10:8,11:8,12:8}
    if ty in scal: return o + scal[ty]
    if ty == 8: _, o = gstr(b, o); return o
    if ty == 9:
        ety = struct.unpack_from("<I", b, o)[0]; n = struct.unpack_from("<Q", b, o+4)[0]; o += 12
        for _ in range(n): o = skip_val(b, o, ety)
        return o
    raise SystemExit(f"kv type {ty}")

def read_gguf(path):
    b = open(path, "rb").read()
    assert b[:4] == b"GGUF"
    n_t = struct.unpack_from("<Q", b, 8)[0]; n_kv = struct.unpack_from("<Q", b, 16)[0]
    o = 24; align = 32; kvs = {}
    for _ in range(n_kv):
        k, o = gstr(b, o); ty = struct.unpack_from("<I", b, o)[0]; o += 4
        if ty == 4: kvs[k] = struct.unpack_from("<I", b, o)[0]
        elif ty == 6: kvs[k] = struct.unpack_from("<f", b, o)[0]
        elif ty == 7: kvs[k] = bool(b[o])
        elif ty == 8: kvs[k] = gstr(b, o)[0]
        if k == "general.alignment": align = struct.unpack_from("<I", b, o)[0]
        o = skip_val(b, o, ty)
    tens = {}
    order = []
    for _ in range(n_t):
        name, o = gstr(b, o)
        nd = struct.unpack_from("<I", b, o)[0]; o += 4
        ne = list(struct.unpack_from(f"<{nd}Q", b, o)); o += 8 * nd
        ty = struct.unpack_from("<I", b, o)[0]; o += 4
        off = struct.unpack_from("<Q", b, o)[0]; o += 8
        tens[name] = (ne, ty, off); order.append(name)
    data = (o + align - 1) // align * align
    return b, kvs, tens, data

def tensor(b, tens, data, name):
    ne, ty, off = tens[name]
    if ty != 0: raise SystemExit(f"{name}: ggml type {ty}; this oracle is f32-only")
    n = 1
    for d in ne: n *= d
    a = np.frombuffer(b, np.float32, count=n, offset=data + off).astype(np.float64)
    # GGUF ne is fastest-first; a 2D [k, n] tensor is n rows of k.
    if len(ne) == 1: return a
    if len(ne) == 2: return a.reshape(ne[1], ne[0])
    if len(ne) == 3: return a.reshape(ne[2], ne[1], ne[0])
    raise SystemExit(f"{name}: {len(ne)} dims")

# ---------- pieces ----------
def rmsnorm(x, w, eps=1e-6):
    return x / np.sqrt((x * x).mean(-1, keepdims=True) + eps) * w

def silu(x): return x / (1.0 + np.exp(-x))

def rope_partial(v, rot, base, pos):
    """NEOX half-split WITHIN the rotary block; dims >= rot pass through untouched."""
    out = v.copy()
    hd = v.shape[-1]
    half = rot // 2
    idx = np.arange(half)
    inv = 1.0 / (base ** (2.0 * idx / rot))
    ang = pos[:, None] * inv[None, :]
    c, s = np.cos(ang), np.sin(ang)
    x0 = v[..., :half]
    x1 = v[..., half:rot]
    out[..., :half] = x0 * c[:, None, :] - x1 * s[:, None, :]
    out[..., half:rot] = x0 * s[:, None, :] + x1 * c[:, None, :]
    return out

def group_limited_topk(scores_for_routing, n_group, topk_group, top_k):
    t, n = scores_for_routing.shape
    gsz = n // n_group
    g = scores_for_routing.reshape(t, n_group, gsz)
    gscore = np.sort(g, axis=-1)[:, :, -2:].sum(-1) if gsz >= 2 else g[:, :, 0]
    keep = np.argsort(-gscore, axis=-1)[:, :topk_group]
    mask = np.zeros((t, n_group), bool)
    for i in range(t): mask[i, keep[i]] = True
    smask = np.repeat(mask, gsz, axis=1)
    masked = np.where(smask, scores_for_routing, -np.inf)
    return np.argsort(-masked, axis=-1)[:, :top_k]

def forward(b, kv, tens, data, ids, bcz=0, bco=0):
    """Logits for a token sequence.

    bcz=0: no attention mask at all — one denoise step over a truncated canvas, which is what the
    engine does by default. bcz>0: the REFERENCE's block-causal mask (chatllm.cpp bailing.cpp) —
    blocks anchored at bco, and row block i sees column block j only when i >= j. Prompt rows
    (i < bco) form one committed region: mutually visible, blind to every fill block.

    The distinction matters because truncation and block-causal are NOT the same beyond layer 1:
    under truncation the prompt rows attend forward into the current block, so at layer 2 the
    block's own rows read activations the reference would never produce.
    """
    return _forward(b, kv, tens, data, np.asarray(ids), bcz, bco)

def main():
    path = sys.argv[1] if len(sys.argv) > 1 else "/tmp/tiny-llada2.gguf"
    t = int(sys.argv[2]) if len(sys.argv) > 2 else 16
    b, kv, tens, data = read_gguf(path)
    ids = np.array([1 + (i * 7) % 240 for i in range(t)])
    lg = _forward(b, kv, tens, data, ids)
    emit(lg)

def emit(lg):
    for i in range(lg.shape[0]):
        z = lg[i] - lg[i].max()
        p = np.exp(z); p /= p.sum()
        ent = -(p * np.log(np.maximum(p, 1e-30))).sum()
        zs = np.sort(lg[i])[::-1]
        print(f"{int(np.argmax(p))} {p.max():.9f} {zs[0]-zs[1]:.9f} {ent:.9f}")

def _forward(b, kv, tens, data, ids, bcz=0, bco=0):
    P = "llada2"
    d = kv[f"{P}.embedding_length"]; L = kv[f"{P}.block_count"]
    nh = kv[f"{P}.attention.head_count"]; nkv = kv[f"{P}.attention.head_count_kv"]
    hd = d // nh
    rot = kv.get(f"{P}.rope.dimension_count", hd // 2)
    base = kv.get(f"{P}.rope.freq_base", 10000.0)
    eps = kv.get(f"{P}.attention.layer_norm_rms_epsilon", 1e-6)
    n_exp = kv.get(f"{P}.expert_count", 0); n_used = kv.get(f"{P}.expert_used_count", 0)
    n_shared = kv.get(f"{P}.expert_shared_count", 0)
    scale = kv.get(f"{P}.expert_weights_scale", 1.0)
    n_group = kv.get(f"{P}.expert_group_count", 0) or 8
    topk_group = kv.get(f"{P}.expert_group_used_count", 0) or 4
    if n_exp and n_exp % n_group: n_group, topk_group = 1, 1
    t = len(ids)
    x = tensor(b, tens, data, "token_embd.weight")[ids]
    pos = np.arange(t).astype(np.float64)

    for li in range(L):
        pre = rmsnorm(x, tensor(b, tens, data, f"blk.{li}.attn_norm.weight"), eps)
        if f"blk.{li}.attn_qkv.weight" in tens:
            qkv = pre @ tensor(b, tens, data, f"blk.{li}.attn_qkv.weight").T
            q = qkv[:, :nh*hd].reshape(t, nh, hd)
            k = qkv[:, nh*hd:nh*hd+nkv*hd].reshape(t, nkv, hd)
            v = qkv[:, nh*hd+nkv*hd:].reshape(t, nkv, hd)
        else:   # separate projections (the sepqkv variant, and whatever a converter chooses to ship)
            q = (pre @ tensor(b, tens, data, f"blk.{li}.attn_q.weight").T).reshape(t, nh, hd)
            k = (pre @ tensor(b, tens, data, f"blk.{li}.attn_k.weight").T).reshape(t, nkv, hd)
            v = (pre @ tensor(b, tens, data, f"blk.{li}.attn_v.weight").T).reshape(t, nkv, hd)
        if f"blk.{li}.attn_q_norm.weight" in tens:
            q = rmsnorm(q, tensor(b, tens, data, f"blk.{li}.attn_q_norm.weight"), eps)
            k = rmsnorm(k, tensor(b, tens, data, f"blk.{li}.attn_k_norm.weight"), eps)
        q = rope_partial(q, rot, base, pos)
        k = rope_partial(k, rot, base, pos)
        rep = nh // nkv
        ctx = np.zeros((t, nh, hd))
        # Block-causal key limit per row, matching the kernel: rows before bco see exactly [0,bco);
        # rows at or past it see through the end of their own block.
        if bcz > 0:
            lim = np.array([bco if i < bco else bco + ((i - bco) // bcz + 1) * bcz for i in range(t)])
            lim = np.minimum(lim, t)
        for h in range(nh):
            kh, vh = k[:, h // rep], v[:, h // rep]
            s = (q[:, h] @ kh.T) * (hd ** -0.5)          # bidirectional unless masked below
            if bcz > 0:
                cols = np.arange(t)[None, :]
                s = np.where(cols < lim[:, None], s, -np.inf)
            s -= s.max(-1, keepdims=True)
            p = np.exp(s); p /= p.sum(-1, keepdims=True)
            ctx[:, h] = p @ vh
        x = x + ctx.reshape(t, nh*hd) @ tensor(b, tens, data, f"blk.{li}.attn_output.weight").T

        pre = rmsnorm(x, tensor(b, tens, data, f"blk.{li}.ffn_norm.weight"), eps)
        # Decide dense-vs-MoE by TENSOR PRESENCE, not by the metadata index: leading_dense_block_count
        # is advisory and the all-dense variants carry no expert tensors at any layer.
        if f"blk.{li}.ffn_gate_exps.weight" not in tens or n_exp == 0:
            g = pre @ tensor(b, tens, data, f"blk.{li}.ffn_gate.weight").T
            u = pre @ tensor(b, tens, data, f"blk.{li}.ffn_up.weight").T
            x = x + (silu(g) * u) @ tensor(b, tens, data, f"blk.{li}.ffn_down.weight").T
            continue

        logits = pre @ tensor(b, tens, data, f"blk.{li}.ffn_gate_inp.weight").T
        sc = 1.0 / (1.0 + np.exp(-logits))
        bias = tensor(b, tens, data, f"blk.{li}.exp_probs_b.bias") if f"blk.{li}.exp_probs_b.bias" in tens else 0.0
        idx = group_limited_topk(sc + bias, n_group, topk_group, n_used)
        w = np.take_along_axis(sc, idx, axis=1)
        w = w / (w.sum(-1, keepdims=True) + 1e-20) * scale
        ge = tensor(b, tens, data, f"blk.{li}.ffn_gate_exps.weight")
        ue = tensor(b, tens, data, f"blk.{li}.ffn_up_exps.weight")
        de = tensor(b, tens, data, f"blk.{li}.ffn_down_exps.weight")
        out = np.zeros_like(x)
        for i in range(t):
            for j in range(n_used):
                e = idx[i, j]
                h = silu(pre[i] @ ge[e].T) * (pre[i] @ ue[e].T)
                out[i] += w[i, j] * (h @ de[e].T)
        if n_shared and f"blk.{li}.ffn_gate_shexp.weight" in tens:
            hs = silu(pre @ tensor(b, tens, data, f"blk.{li}.ffn_gate_shexp.weight").T) \
                 * (pre @ tensor(b, tens, data, f"blk.{li}.ffn_up_shexp.weight").T)
            out += hs @ tensor(b, tens, data, f"blk.{li}.ffn_down_shexp.weight").T
        x = x + out

    x = rmsnorm(x, tensor(b, tens, data, "output_norm.weight"), eps)
    return x @ tensor(b, tens, data, "output.weight").T

if __name__ == "__main__":
    main()
