#!/usr/bin/env python3
"""llada2-to-gguf.py — convert LLaDA2.0 HF safetensors to a Q4_0 GGUF, WITHOUT dropping anything.

WHY THIS EXISTS RATHER THAN convert_hf_to_gguf.py. Mainline llama.cpp knows LLADA (v1) but not
llada2 — `grep LLADA gguf-py/gguf/constants.py` finds the v1 arch and nothing else — so the only
converters for this family are forks. And the fork that produced every circulating
LLaDA2.0-mini GGUF DROPPED THE SHARED EXPERT: the files declare expert_shared_count 1 and ship zero
ffn_*_shexp tensors, so every token's FFN silently loses an always-on contribution. We wrote
gguf-shexp-graft.py to repair that after the fact. This produces a correct file in the first place.

THE DESIGN RULE, and the reason for most of the code below: AN UNMAPPED TENSOR IS A HARD ERROR.
Silently skipping a tensor whose name the converter does not recognise is exactly how the shared
expert went missing, and the result loads, runs, and is quietly worse — the failure mode this whole
project keeps paying for. Every tensor in the index must either map to a GGUF name or be explicitly
listed as intentionally dropped.

Usage:
  llada2-to-gguf.py --hf <dir> --out model-q4_0.gguf [--quant q4_0|f16|f32] [--dry-run]

--dry-run reads only the index and config and prints the full mapping plus anything unmapped, which
is the cheap way to find out whether this will work before touching 32 GB.
"""
import argparse, json, os, struct, sys
import numpy as np

GGML_F32, GGML_F16, GGML_Q4_0 = 0, 1, 2
ALIGN = 32


# ---------------------------------------------------------------- safetensors
def st_open(path):
    """Header + a memoryview of the payload. Lazy: nothing is read until a tensor is asked for."""
    f = open(path, "rb")
    n = struct.unpack("<Q", f.read(8))[0]
    hdr = json.loads(f.read(n))
    return f, hdr, 8 + n

ST_DTYPE = {"F32": np.float32, "F16": np.float16, "BF16": None}  # BF16 handled explicitly

def st_get(f, hdr, base, name):
    """Reads via os.pread: no shared seek pointer, so several threads can pull from one shard at
    once. With a plain seek+read they would silently read each other's tensors."""
    meta = hdr[name]
    dt, shape = meta["dtype"], meta["shape"]
    lo, hi = meta["data_offsets"]
    raw = os.pread(f.fileno(), hi - lo, base + lo)
    if dt == "BF16":
        # bf16 is the top 16 bits of an f32; widen rather than losing the exponent range.
        u16 = np.frombuffer(raw, dtype=np.uint16).astype(np.uint32) << 16
        arr = u16.view(np.float32)
    elif dt in ("F32", "F16"):
        arr = np.frombuffer(raw, dtype=ST_DTYPE[dt]).astype(np.float32)
    else:
        raise SystemExit(f"{name}: unhandled safetensors dtype {dt}")
    return arr.reshape(shape)


# ---------------------------------------------------------------- quantisation
def quantize_q4_0(rows):
    """Bit-identical to ggml quantize_row_q4_0_ref, and measured against the previous formulation
    on three shapes plus zeros, exact +v/-v ties and subnormals.

    ggml takes the element of greatest MAGNITUDE and keeps its SIGN. The obvious spelling is
    abs -> argmax -> gather, which costs a full |x| copy, a reduction and a fancy-index. max and min
    give the same answer in two reductions and no copy: whichever of them is larger in magnitude is
    the one ggml would have picked. Quantising straight to uint8 rather than through int32 removes
    another full-width temporary. 257 -> 332 MB/s, which is 4.2 -> 3.3 minutes on a 16B model before
    any threading.
    """
    x = rows.reshape(-1, 32)
    if x.dtype != np.float32: x = x.astype(np.float32)
    mx = x.max(axis=1); mn = x.min(axis=1)
    vmax = np.where(-mn > mx, mn, mx)
    d = vmax * (-1.0 / 8.0)
    idl = np.divide(1.0, d, out=np.zeros_like(d), where=d != 0)
    t = x * idl[:, None]; t += 8.5
    q = np.clip(t, 0, 15).astype(np.uint8)
    dh = d.astype(np.float16).view(np.uint16)
    out = np.empty((x.shape[0], 18), dtype=np.uint8)
    out[:, 0] = dh & 0xFF
    out[:, 1] = dh >> 8
    np.bitwise_or(q[:, :16], np.left_shift(q[:, 16:], 4), out=out[:, 2:])
    return out.tobytes()


def encode(arr, quant):
    """Returns (ggml_type, bytes). 1-D tensors stay f32: norms and biases are tiny and quantising
    them is where accuracy goes for no space saving."""
    a = np.ascontiguousarray(arr, dtype=np.float32)
    if a.ndim == 1 or quant == "f32":
        return GGML_F32, a.tobytes()
    if quant == "f16":
        return GGML_F16, a.astype(np.float16).tobytes()
    if a.shape[-1] % 32 != 0:
        return GGML_F32, a.tobytes()          # can't Q4_0 it; keep it exact rather than reshaping
    return GGML_Q4_0, quantize_q4_0(a.reshape(-1, a.shape[-1]))


# ---------------------------------------------------------------- name mapping
def map_name(hf: str, n_layers: int):
    """HF name -> (gguf name, stack_key or None). Returns None for 'intentionally dropped'.

    Anything this function does not recognise raises, on purpose.
    """
    if hf in ("model.word_embeddings.weight", "model.embed_tokens.weight"):
        return "token_embd.weight", None
    if hf in ("model.final_layernorm.weight", "model.norm.weight"):
        return "output_norm.weight", None
    if hf == "lm_head.weight":
        return "output.weight", None
    if not hf.startswith("model.layers."):
        raise KeyError(hf)
    rest = hf[len("model.layers."):]
    i, rest = rest.split(".", 1)
    p = f"blk.{int(i)}."
    simple = {
        "input_layernorm.weight":            p + "attn_norm.weight",
        "post_attention_layernorm.weight":   p + "ffn_norm.weight",
        "attention.query_key_value.weight":  p + "attn_qkv.weight",
        "attention.dense.weight":            p + "attn_output.weight",
        "attention.query_layernorm.weight":  p + "attn_q_norm.weight",
        "attention.key_layernorm.weight":    p + "attn_k_norm.weight",
        "mlp.gate.weight":                   p + "ffn_gate_inp.weight",
        # THE SAME TENSOR UNDER THREE NAMES. DeepSeek-V3 calls the no-aux selection bias
        # e_score_correction_bias; Ling and LLaDA2 call it expert_bias. The operator's checkpoint
        # uses the latter on all 19 MoE layers, and the refusal caught it — which is the tensor it
        # most needed to catch, because dropping it silently changes which experts are selected on
        # every token (the c4-719 finding: routing with the bias differs from routing without it,
        # by design). A converter that skipped it would produce a file that loads and routes wrong.
        "mlp.gate.expert_bias":              p + "exp_probs_b.bias",
        "mlp.gate.e_score_correction_bias":  p + "exp_probs_b.bias",
        # dense-lead layers
        "mlp.gate_proj.weight":              p + "ffn_gate.weight",
        "mlp.up_proj.weight":                p + "ffn_up.weight",
        "mlp.down_proj.weight":              p + "ffn_down.weight",
        # THE SHARED EXPERT — the tensors the fork converter dropped.
        "mlp.shared_experts.gate_proj.weight": p + "ffn_gate_shexp.weight",
        "mlp.shared_experts.up_proj.weight":   p + "ffn_up_shexp.weight",
        "mlp.shared_experts.down_proj.weight": p + "ffn_down_shexp.weight",
        "mlp.shared_expert.gate_proj.weight":  p + "ffn_gate_shexp.weight",
        "mlp.shared_expert.up_proj.weight":    p + "ffn_up_shexp.weight",
        "mlp.shared_expert.down_proj.weight":  p + "ffn_down_shexp.weight",
    }
    if rest in simple:
        return simple[rest], None
    # routed experts: mlp.experts.<E>.{gate,up,down}_proj.weight  ->  one stacked tensor per layer
    if rest.startswith("mlp.experts."):
        tail = rest[len("mlp.experts."):]
        e, which = tail.split(".", 1)
        stack = {"gate_proj.weight": p + "ffn_gate_exps.weight",
                 "up_proj.weight":   p + "ffn_up_exps.weight",
                 "down_proj.weight": p + "ffn_down_exps.weight"}
        if which in stack:
            return stack[which], (stack[which], int(e))
    raise KeyError(hf)


# ---------------------------------------------------------------- gguf writer
def gs(x): b = x.encode(); return struct.pack("<Q", len(b)) + b
def kv_str(k, v): return gs(k) + struct.pack("<I", 8) + gs(v)
def kv_u32(k, v): return gs(k) + struct.pack("<I", 4) + struct.pack("<I", int(v))
def kv_f32(k, v): return gs(k) + struct.pack("<I", 6) + struct.pack("<f", float(v))
def kv_bool(k, v): return gs(k) + struct.pack("<I", 7) + struct.pack("<B", 1 if v else 0)
def kv_arr_str(k, vals):
    return gs(k) + struct.pack("<I", 9) + struct.pack("<I", 8) + struct.pack("<Q", len(vals)) \
         + b"".join(gs(v) for v in vals)
def kv_arr_i32(k, vals):
    return gs(k) + struct.pack("<I", 9) + struct.pack("<I", 5) + struct.pack("<Q", len(vals)) \
         + b"".join(struct.pack("<i", int(v)) for v in vals)


def first_of(cfg, keys, default):
    """First key that is actually present. Config vocabularies differ across the Ling/DeepSeek
    lineage and a missing key silently becomes a default, which is how a MoE becomes 'dense'."""
    for k in keys:
        if cfg.get(k) is not None: return cfg[k]
    return default


def build_kv(cfg, tok, n_layers, stm=None):
    stm = stm or {}
    P = "llada2"
    # head_dim and rotary_dim are both stated in this config; prefer them over inference. The
    # derived head_dim would be 2048/16 = 128, which agrees here — but a model with a head_dim that
    # is not hidden/heads would be silently mis-shaped by the division.
    hd = cfg.get("head_dim") or (cfg["hidden_size"] // cfg["num_attention_heads"])
    rot = cfg.get("rotary_dim") or int(hd * cfg.get("partial_rotary_factor", 1.0))
    items = [
        kv_str("general.architecture", P),
        kv_str("general.name", "LLaDA2.0-mini"),
        kv_u32(f"{P}.block_count", n_layers),
        kv_u32(f"{P}.context_length", cfg.get("max_position_embeddings", 32768)),
        kv_u32(f"{P}.embedding_length", cfg["hidden_size"]),
        kv_u32(f"{P}.feed_forward_length", cfg["intermediate_size"]),
        kv_u32(f"{P}.attention.head_count", cfg["num_attention_heads"]),
        kv_u32(f"{P}.attention.head_count_kv", cfg.get("num_key_value_heads", cfg["num_attention_heads"])),
        kv_f32(f"{P}.attention.layer_norm_rms_epsilon", cfg.get("rms_norm_eps", 1e-6)),
        kv_f32(f"{P}.rope.freq_base", cfg.get("rope_theta", 10000.0)),
        kv_u32(f"{P}.rope.dimension_count", rot),
        kv_u32(f"{P}.vocab_size", cfg["vocab_size"]),
        # MoE. expert_shared_count is written as 1 AND the tensors are emitted — the pair that the
        # fork converter got half right, declaring the shared expert without shipping it.
        # LLaDA2 SPELLS THESE DIFFERENTLY FROM DEEPSEEK, and getting it wrong is silent: absent
        # keys default to 0, which writes expert_count 0 and produces a file the loader treats as
        # DENSE — a 16B MoE running one FFN. The real config says num_experts / num_shared_experts;
        # the DeepSeek names (n_routed_experts / n_shared_experts) are kept as fallbacks because
        # other Ling-family exports do use them.
        kv_u32(f"{P}.expert_count", first_of(cfg, ["num_experts", "n_routed_experts"], 0)),
        kv_u32(f"{P}.expert_used_count", cfg.get("num_experts_per_tok", 0)),
        kv_u32(f"{P}.expert_shared_count", first_of(cfg, ["num_shared_experts", "n_shared_experts"], 0)),
        kv_u32(f"{P}.expert_feed_forward_length", cfg.get("moe_intermediate_size", 0)),
        kv_u32(f"{P}.leading_dense_block_count", cfg.get("first_k_dense_replace", 0)),
        kv_f32(f"{P}.expert_weights_scale", cfg.get("routed_scaling_factor", 1.0)),
        kv_bool(f"{P}.expert_weights_norm", bool(cfg.get("norm_topk_prob", True))),
        # From the config rather than hardcoded: 2 = sigmoid, 1 = softmax. This checkpoint says
        # score_function sigmoid, which matches, but a hardcoded 2 would silently mis-route a
        # softmax sibling.
        kv_u32(f"{P}.expert_gating_func",
               2 if str(cfg.get("score_function", "sigmoid")).lower() == "sigmoid" else 1),
        # Group-limited routing, if the config declares it.
        kv_u32(f"{P}.expert_group_count", cfg.get("n_group", 0)),
        kv_u32(f"{P}.expert_group_used_count", cfg.get("topk_group", 0)),
        # DIFFUSION. shift_logits false for this lineage — mainline's converter writes false and the
        # engine's absent-defaults-true rule would otherwise read every prediction off by one.
        kv_str("diffusion.shift_logits", "false"),
    ]
    n = len(items)
    # tokenizer
    vocab = [None] * cfg["vocab_size"]
    for t, i in tok["model"]["vocab"].items():
        if i < len(vocab): vocab[i] = t
    for a in tok.get("added_tokens", []):
        if a["id"] < len(vocab): vocab[a["id"]] = a["content"]
    for i, v in enumerate(vocab):
        if v is None: vocab[i] = f"<unused{i}>"
    types = [1] * len(vocab)
    for a in tok.get("added_tokens", []):
        if a["id"] < len(types): types[a["id"]] = 4      # CONTROL
    merges = [" ".join(m) if isinstance(m, list) else m
              for m in tok["model"].get("merges", [])]
    # SPECIAL TOKEN IDS. Without these the file loads and is unusable in two separate ways.
    #
    # mask_token_id is read ONLY from metadata — gguf_mask_token_id checks
    # tokenizer.ggml.mask_token_id and any *.mask_token_id key, and never scans the vocabulary. A
    # diffusion checkpoint without it cannot run the denoise path at all: c4-744 gates skipping the
    # KV prefill on its presence and c4-764 refuses rather than guessing. The token is right there
    # in the vocab as <|mask|> and would have been dropped on the floor.
    #
    # eos_token_id is what builds the stop set. Absent, generation runs to the token cap every time.
    ids = {}
    for name, tokstr in [("bos", stm.get("bos_token")), ("eos", stm.get("eos_token")),
                         ("padding", stm.get("pad_token")), ("mask", stm.get("mask_token"))]:
        if isinstance(tokstr, dict): tokstr = tokstr.get("content")
        if not tokstr: continue
        tid = next((a["id"] for a in tok.get("added_tokens", []) if a["content"] == tokstr), None)
        if tid is None: tid = tok["model"]["vocab"].get(tokstr)
        if tid is not None: ids[name] = int(tid)
    if "padding" not in ids and cfg.get("pad_token_id") is not None:
        ids["padding"] = int(cfg["pad_token_id"])
    for name, tid in ids.items():
        items.append(kv_u32(f"tokenizer.ggml.{name}_token_id", tid))
    n += len(ids)
    missing_special = [k for k in ("eos", "mask") if k not in ids]
    if missing_special:
        print(f"WARNING: could not resolve {missing_special} token id(s). Without eos the stop set "
              f"is empty; without mask the diffusion path refuses to run.", file=sys.stderr)

    items += [kv_str("tokenizer.ggml.model", "gpt2"),
              kv_arr_str("tokenizer.ggml.tokens", vocab),
              kv_arr_i32("tokenizer.ggml.token_type", types),
              kv_arr_str("tokenizer.ggml.merges", merges)]
    n += 4
    return b"".join(items), n


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--hf", required=True)
    ap.add_argument("--out", default="llada2-q4_0.gguf")
    ap.add_argument("--quant", default="q4_0", choices=["q4_0", "f16", "f32"])
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--threads", type=int, default=(os.cpu_count() or 4),
                    help="quantisation workers (default: all cores)")
    a = ap.parse_args()

    cfg = json.load(open(os.path.join(a.hf, "config.json")))
    idx = json.load(open(os.path.join(a.hf, "model.safetensors.index.json")))
    tok = json.load(open(os.path.join(a.hf, "tokenizer.json")))
    wmap = idx["weight_map"]
    n_layers = cfg["num_hidden_layers"]

    # REFUSE A CHECKPOINT THAT IS NOT THIS FAMILY, before looking at a single tensor.
    #
    # The unmapped-tensor guard is not enough on its own. Run this against a Qwen3-MoE checkpoint and
    # it flags only the twelve self_attn.* names — everything else (embed_tokens, lm_head, the norms,
    # mlp.gate, the stacked experts) maps cleanly, because those names are shared across the whole
    # HF ecosystem. Add six aliases to silence it and the tool would write general.architecture =
    # "llada2", diffusion.shift_logits and a mask token id onto a causal Qwen model, producing a file
    # that loads as the wrong architecture. The tensors are an observation; the config says what the
    # file IS, and it is free to check.
    mt = str(cfg.get("model_type", "")).lower()
    archs = " ".join(cfg.get("architectures", [])).lower()
    if "llada2" not in mt and "llada2" not in archs:
        print(f"REFUSING: this is not a LLaDA2 checkpoint — model_type={cfg.get('model_type')!r}, "
              f"architectures={cfg.get('architectures')}. This converter writes llada2 metadata "
              f"(diffusion.shift_logits, a mask token, the Bailing2 tensor names) and exists only "
              f"because mainline llama.cpp cannot convert that family. For anything mainline knows "
              f"— Qwen, Llama, Mistral — use convert_hf_to_gguf.py, which is maintained and tested.",
              file=sys.stderr)
        sys.exit(2)

    # Map every name FIRST, so an unknown tensor stops the run before 32 GB is read.
    mapped, stacks, unmapped = {}, {}, []
    for hf in sorted(wmap):
        try:
            g, stack = map_name(hf, n_layers)
        except KeyError:
            unmapped.append(hf); continue
        if stack is None:
            mapped[hf] = g
        else:
            stacks.setdefault(stack[0], {})[stack[1]] = hf
    if unmapped:
        print(f"REFUSING: {len(unmapped)} tensor(s) have no mapping. Silently skipping these is "
              f"exactly how the shared expert went missing from every circulating GGUF — the file "
              f"loads and is quietly worse. Add them to map_name or list them as dropped.",
              file=sys.stderr)
        for u in unmapped[:20]: print(f"  {u}", file=sys.stderr)
        sys.exit(2)

    # THE INDEX IS A CLAIM, THE SHARD IS THE FACT. weight_map can name a tensor the shard does not
    # contain — a rename, a partial re-export, a hand-edited index. Trusting it meant discovering
    # the mismatch mid-write, gigabytes in. Check every name against its shard header first: the
    # headers are small and reading all seven costs a moment against a 32 GB write.
    missing = []
    hdr_cache = {}
    for hf, shard in sorted(wmap.items()):
        if shard not in hdr_cache:
            path = os.path.join(a.hf, shard)
            if not os.path.exists(path):
                # --dry-run exists to be runnable on metadata alone, before 32 GB is downloaded or
                # touched. Refusing there because the shards are absent defeats its purpose, so the
                # cross-check is skipped and SAID to be skipped rather than quietly passing.
                if a.dry_run:
                    hdr_cache[shard] = None
                    continue
                print(f"REFUSING: {shard} is named in the index but not present", file=sys.stderr)
                sys.exit(2)
            with open(path, "rb") as fh:
                n_hdr = struct.unpack("<Q", fh.read(8))[0]
                hdr_cache[shard] = set(json.loads(fh.read(n_hdr)).keys())
        if hdr_cache[shard] is None:
            continue
        if hf not in hdr_cache[shard]:
            missing.append((hf, shard))
    if missing:
        print(f"REFUSING: {len(missing)} tensor(s) are in the index but NOT in the shard named for "
              f"them. The index is a claim; the shard is the fact. Failing here beats failing "
              f"gigabytes into the write.", file=sys.stderr)
        for hf, sh in missing[:10]: print(f"  {hf}  ->  {sh}", file=sys.stderr)
        sys.exit(2)

    absent = [sh for sh, v in hdr_cache.items() if v is None]
    if absent:
        print(f"note: {len(absent)} shard(s) not present — mapping checked, index-vs-shard NOT "
              f"checked. Re-run without --dry-run where the weights are.")

    shexp = sum(1 for g in mapped.values() if "shexp" in g)
    print(f"{len(mapped)} direct + {len(stacks)} stacked expert tensor(s); "
          f"{shexp} shared-expert tensor(s) "
          f"({'PRESENT — this is the fix' if shexp else 'ABSENT — check the HF names'})")
    print(f"declared: {first_of(cfg, ['num_experts','n_routed_experts'], 0)} routed, "
          f"{first_of(cfg, ['num_shared_experts','n_shared_experts'], 0)} shared, "
          f"{cfg.get('num_experts_per_tok')} active, groups {cfg.get('n_group')}/{cfg.get('topk_group')}, "
          f"scale {cfg.get('routed_scaling_factor')}, dense-lead {cfg.get('first_k_dense_replace')}, "
          f"rot {cfg.get('rotary_dim')} of head_dim {cfg.get('head_dim')}, {cfg.get('score_function')} gating")
    if a.dry_run:
        return

    stm_path = os.path.join(a.hf, "special_tokens_map.json")
    stm = json.load(open(stm_path)) if os.path.exists(stm_path) else {}
    kv, n_kv = build_kv(cfg, tok, n_layers, stm)

    # PLAN, THEN STREAM. The first version appended every quantised tensor to a list and wrote at
    # the end, so peak RSS held the whole ~9 GB output. Q4_0's size is exactly computable before
    # quantising (18 bytes per 32 weights), so the offsets can be laid out first, the header written,
    # and each tensor appended as it is produced. Memory is then bounded by the worker pool, not by
    # the model.
    files, flock = {}, __import__("threading").Lock()
    def reader(shard):
        with flock:
            if shard not in files: files[shard] = st_open(os.path.join(a.hf, shard))
        return files[shard]

    def shape_of(hf):
        f, hdr, _ = reader(wmap[hf])
        return hdr[hf]["shape"]

    def nbytes_for(shape):
        n = 1
        for d in shape: n *= d
        if len(shape) == 1 or a.quant == "f32": return n * 4, GGML_F32
        if a.quant == "f16": return n * 2, GGML_F16
        if shape[-1] % 32 != 0: return n * 4, GGML_F32
        return n // 32 * 18, GGML_Q4_0

    plan = []          # (gguf_name, shape, ggml_type, nbytes, producer)
    for hf, g in sorted(mapped.items(), key=lambda kv2: kv2[1]):
        sh = shape_of(hf)
        nb, ty = nbytes_for(sh)
        plan.append((g, sh, ty, nb, ("one", hf)))
    for g, per_expert in sorted(stacks.items()):
        n_e = max(per_expert) + 1
        sh = [n_e] + shape_of(per_expert[0])
        nb, ty = nbytes_for(sh)
        plan.append((g, sh, ty, nb, ("stack", per_expert, n_e)))

    infos, off = b"", 0
    for g, sh, ty, nb, _ in plan:
        pad = (-off) % ALIGN
        off += pad
        ne = list(sh)[::-1]
        nm = g.encode()
        infos += struct.pack("<Q", len(nm)) + nm + struct.pack("<I", len(ne))
        for d in ne: infos += struct.pack("<Q", d)
        infos += struct.pack("<I", ty) + struct.pack("<Q", off)
        off += nb

    def produce(entry):
        kind = entry[0]
        if kind == "one":
            hf = entry[1]
            f, hdr, base = reader(wmap[hf])
            return encode(st_get(f, hdr, base, hf), a.quant)[1]
        # STACKED EXPERTS, quantised PER EXPERT and concatenated. Sound because Q4_0 blocks run
        # along the last dimension and stacking is row concatenation — the commutation proved in
        # c4-756 — so this is bit-identical to building the [n_e, rows, cols] f32 tensor first, and
        # it never materialises that 1 GB buffer.
        _, per_expert, n_e = entry
        parts = []
        for e in range(n_e):
            hf = per_expert[e]
            f, hdr, base = reader(wmap[hf])
            parts.append(encode(st_get(f, hdr, base, hf), a.quant)[1])
        return b"".join(parts)

    hdr_bytes = b"GGUF" + struct.pack("<I", 3) + struct.pack("<Q", len(plan)) \
              + struct.pack("<Q", n_kv) + kv + infos
    workers = max(1, min(a.threads, len(plan)))
    from concurrent.futures import ThreadPoolExecutor
    written = 0
    with open(a.out, "wb") as o:
        o.write(hdr_bytes)
        o.write(b"\x00" * ((-len(hdr_bytes)) % ALIGN))
        with ThreadPoolExecutor(max_workers=workers) as ex:
            # map keeps order, so tensors land at the offsets planned above while several are
            # quantised at once. numpy releases the GIL across the reductions that dominate here.
            for (g, sh, ty, nb, _), data in zip(plan, ex.map(produce, [p2 for *_, p2 in plan])):
                assert len(data) == nb, f"{g}: planned {nb} bytes, produced {len(data)}"
                o.write(b"\x00" * ((-o.tell() + len(hdr_bytes) % ALIGN) % ALIGN if False else 0))
                pos = o.tell()
                pad = (-pos) % ALIGN
                if pad: o.write(b"\x00" * pad)
                o.write(data)
                written += 1
                if written % 25 == 0 or written == len(plan):
                    print(f"  {written}/{len(plan)} tensors", end="\r", flush=True)
    print()
    print(f"wrote {a.out} ({os.path.getsize(a.out)/1e9:.2f} GB, {a.quant})")
    print("verify with: tools/gguf-audit.py " + a.out)


if __name__ == "__main__":
    main()
