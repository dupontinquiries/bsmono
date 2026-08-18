#!/usr/bin/env python3
"""convert-diffusion-gemma — HF DiffusionGemma (google/diffusiongemma-26B-A4B) → GGUF
for the logscope wgpu engine.

Why this exists: no upstream converter knows this arch yet (the vendored llama.cpp master has
gemma4 but not diffusion_gemma), and the engine loads GGUF only. This script pins the tensor and
metadata naming the engine's `gemmadiff::read_shape` + `--gate-gemmadiff` validate — one name map,
written here, read there, gated in between.

Naming: the Gemma 4 base uses llama.cpp's OWN gemma4 conventions (gguf-py constants.py +
src/models/gemma4.cpp), so a future upstream converter lands compatibly:
  token_embd / output_norm / blk.N.{attn_norm, attn_q, attn_q_norm, attn_k, attn_k_norm,
  attn_v (ABSENT on full-attention layers — V comes from K), attn_output, post_attention_norm,
  ffn_norm, ffn_gate, ffn_up, ffn_down, post_ffw_norm, pre_ffw_norm_2, post_ffw_norm_1,
  post_ffw_norm_2, ffn_gate_inp(.weight/.scale), ffn_gate_up_exps, ffn_down_exps(.weight/.scale),
  layer_output_scale}
The diffusion-only pieces have no upstream name, so this script DEFINES them (and the engine
reads exactly these): blk.N.layer_output_scale_enc (the encoder-pass scalar set) and
self_cond.{pre_norm, ffn_gate, ffn_up, ffn_down}.

Known open items, stated rather than papered over:
- rope_freqs ("proportional" rope factor tensors on full-attention layers): the HF drop ships no
  such weight and the modeling code that would define the factor formula is not public here, so
  NO blk.N.rope_freqs is emitted. The engine's archfeat gap list covers the consequences; when
  the formula lands (upstream converter or modeling release), synthesize it here.
- tokenizer: full HF-tokenizer→ggml conversion is the upstream converter's job. This script can
  COPY a compatible tokenizer from a donor GGUF (--tokenizer-gguf, same 262144-token Gemma-4
  vocab), or omit it — the engine's LOGSCOPE_TOKENIZER_GGUF borrows one at load and cross-checks
  the width. tokenizer.ggml.mask_token_id is always written, read from the drop's tokenizer.json
  (`<mask>`), because diffuse_generate refuses without it.
- the vision tower is skipped: this emits a TEXT GGUF.

Quantizing to IQ1_S is downstream of this script (llama-quantize once the arch exists there, or
community UD mixes). Note the geometry: expert gate_up rows are 2816-wide (IQ1_S-alignable) but
expert down rows are 704-wide — NOT a multiple of 256, so ggml cannot hold them as IQ1_S; a real
"IQ1_S" file of this model is necessarily a mix, which the engine's per-tensor pathway handles.

Usage:
  convert.py <hf_dir> --out model-f16.gguf [--dtype f16|f32] [--gguf-py /path/llama.cpp/gguf-py]
             [--tokenizer-gguf donor.gguf]
  convert.py --selfcheck [--gguf-py ...]     # tiny synthetic model, end-to-end, no shards needed
"""

import argparse
import json
import re
import sys
from pathlib import Path

import numpy as np


# ── name map ─────────────────────────────────────────────────────────────────────────────────────

LAYER = re.compile(r"^model\.decoder\.layers\.(\d+)\.(.+?)(?:\.weight)?$")
ENC_SCALAR = re.compile(r"^model\.encoder\.language_model\.layers\.(\d+)\.layer_scalar$")

# decoder-layer suffix (after the index, .weight stripped) → gguf suffix under blk.N.
SUFFIX = {
    "input_layernorm":              "attn_norm.weight",
    "self_attn.q_proj":             "attn_q.weight",
    "self_attn.q_norm":             "attn_q_norm.weight",
    "self_attn.k_proj":             "attn_k.weight",
    "self_attn.k_norm":             "attn_k_norm.weight",
    "self_attn.v_proj":             "attn_v.weight",         # absent on full-attention layers
    "self_attn.o_proj":             "attn_output.weight",
    "post_attention_layernorm":     "post_attention_norm.weight",
    "pre_feedforward_layernorm":    "ffn_norm.weight",       # dense (shared-expert) branch pre-norm
    "mlp.gate_proj":                "ffn_gate.weight",
    "mlp.up_proj":                  "ffn_up.weight",
    "mlp.down_proj":                "ffn_down.weight",
    "post_feedforward_layernorm_1": "post_ffw_norm_1.weight",
    "pre_feedforward_layernorm_2":  "pre_ffw_norm_2.weight",  # MoE branch pre-norm
    "post_feedforward_layernorm_2": "post_ffw_norm_2.weight",
    "post_feedforward_layernorm":   "post_ffw_norm.weight",   # the JOINING post-norm
    "router.proj":                  "ffn_gate_inp.weight",
    "router.scale":                 "ffn_gate_inp.scale",
    "router.per_expert_scale":      "ffn_down_exps.scale",
    "experts.gate_up_proj":         "ffn_gate_up_exps.weight",
    "experts.down_proj":            "ffn_down_exps.weight",
    "layer_scalar":                 "layer_output_scale.weight",
}

TOP = {
    "model.decoder.embed_tokens.weight":               "token_embd.weight",
    "model.decoder.norm.weight":                       "output_norm.weight",
    "model.decoder.self_conditioning.pre_norm.weight": "self_cond.pre_norm.weight",
    "model.decoder.self_conditioning.gate_proj.weight": "self_cond.ffn_gate.weight",
    "model.decoder.self_conditioning.up_proj.weight":  "self_cond.ffn_up.weight",
    "model.decoder.self_conditioning.down_proj.weight": "self_cond.ffn_down.weight",
}


def map_name(hf: str):
    """HF tensor name → gguf name, or None to skip (vision tower, projector)."""
    if hf in TOP:
        return TOP[hf]
    m = ENC_SCALAR.match(hf)
    if m:
        return f"blk.{m.group(1)}.layer_output_scale_enc.weight"
    m = LAYER.match(hf)
    if m:
        # try the longest matching suffix first (post_feedforward_layernorm is a prefix of _1/_2)
        rest = m.group(2)
        for k in sorted(SUFFIX, key=len, reverse=True):
            if rest == k:
                return f"blk.{m.group(1)}.{SUFFIX[k]}"
        raise SystemExit(f"unmapped decoder tensor: {hf} — the census changed; extend SUFFIX")
    if hf.startswith(("model.encoder.vision_tower", "model.encoder.embed_vision")):
        return None  # text GGUF; the vision tower is mtmd-class work
    raise SystemExit(f"unmapped tensor: {hf} — refusing to guess")


def orient_experts(name: str, a: np.ndarray, d: int, ff_exp: int) -> np.ndarray:
    """Permute stacked expert tensors to the numpy shape whose REVERSE is the ggml ne
    gemma4.cpp declares: gate_up → ne (d, 2ff, n_exp) i.e. numpy (n_exp, 2ff, d);
    down → ne (ff, d, n_exp) i.e. numpy (n_exp, d, ff). Orientation is DETECTED from the
    distinct dims (2·704=1408 ≠ 2816), never assumed — the shards aren't here to check against,
    so an ambiguous shape is a refusal, not a coin flip."""
    if a.ndim != 3:
        raise SystemExit(f"{name}: expected 3-D expert stack, got {a.shape}")
    want = (a.shape[0], 2 * ff_exp, d) if name.endswith("ffn_gate_up_exps.weight") \
        else (a.shape[0], d, ff_exp)
    if a.shape == want:
        return a
    if a.shape == (want[0], want[2], want[1]):
        return np.ascontiguousarray(a.transpose(0, 2, 1))
    raise SystemExit(f"{name}: shape {a.shape} matches neither {want} nor its transpose")


# ── metadata ─────────────────────────────────────────────────────────────────────────────────────

def write_metadata(w, cfg: dict, mask_id: int):
    t = cfg["text_config"]
    p = "diffusion_gemma"
    layers = t["num_hidden_layers"]
    # the layer_types array is the ground truth; the engine reads a PERIOD (full layer last per
    # group, swa::swa_layers convention) — verify the array IS that period before flattening it
    lt = t["layer_types"]
    fulls = [i for i, k in enumerate(lt) if k == "full_attention"]
    period = fulls[0] + 1 if fulls else 0
    if fulls != [i for i in range(period - 1, layers, period)]:
        raise SystemExit(f"layer_types is not a clean every-{period}th-full pattern: {fulls}")

    u32 = w.add_uint32
    f32 = w.add_float32
    u32(f"{p}.block_count", layers)
    u32(f"{p}.embedding_length", t["hidden_size"])
    u32(f"{p}.feed_forward_length", t["intermediate_size"])
    u32(f"{p}.expert_feed_forward_length", t["moe_intermediate_size"])
    u32(f"{p}.expert_count", t["num_experts"])
    u32(f"{p}.expert_used_count", t["top_k_experts"])
    u32(f"{p}.expert_shared_count", 1)  # the dense FFN, per the model card
    u32(f"{p}.attention.head_count", t["num_attention_heads"])
    u32(f"{p}.attention.head_count_kv", t["num_key_value_heads"])            # sliding layers
    u32(f"{p}.attention.head_count_kv_full", t["num_global_key_value_heads"])  # full layers
    u32(f"{p}.attention.key_length", t["global_head_dim"])                   # full layers
    u32(f"{p}.attention.key_length_swa", t["head_dim"])                      # sliding layers
    u32(f"{p}.attention.sliding_window", t["sliding_window"])
    u32(f"{p}.attention.sliding_window_pattern", period)
    f32(f"{p}.attention.layer_norm_rms_epsilon", t["rms_norm_eps"])
    rp = t["rope_parameters"]
    f32(f"{p}.rope.freq_base", rp["full_attention"]["rope_theta"])
    f32(f"{p}.rope.freq_base_swa", rp["sliding_attention"]["rope_theta"])
    u32(f"{p}.rope.dimension_count",
        int(t["global_head_dim"] * rp["full_attention"]["partial_rotary_factor"]))
    f32(f"{p}.final_logit_softcapping", t["final_logit_softcapping"])
    u32(f"{p}.context_length", t["max_position_embeddings"])
    u32(f"{p}.vocab_size", t["vocab_size"])
    # diffusion block — shared namespace with the llada/rnd1 loaders. shift_logits is a STRING
    # because the engine reads it with get_str (the llada fork-GGUF lesson: the kv must be
    # PRESENT and explicit, or a lineage default decides).
    w.add_string("diffusion.shift_logits", "false")
    # Schedule keys come from the checkpoint's own scheduler/ and generation_config.json when the
    # drop ships them (merged into cfg by main); the literals are the 26B-A4B values and exist so a
    # weights-only directory still converts. A canvas that does not tile into blocks is refused HERE
    # with both numbers named — the engine's read_shape enforces the same invariant, and failing at
    # convert time beats failing at load time by the length of a 51 GB write.
    sched = cfg.get("_scheduler", {})
    gen = cfg.get("_generation", {})
    samp = gen.get("sampler_config", {})
    canvas = cfg.get("canvas_length", 256)
    block = sched.get("block_length", 32)
    if block <= 0 or canvas % block != 0:
        raise SystemExit(f"canvas_length {canvas} does not tile into block_length {block} — "
                         f"fix config.json/scheduler_config.json before converting")
    u32("diffusion.canvas_length", canvas)
    u32("diffusion.block_length", block)
    f32("diffusion.entropy_bound", samp.get("entropy_bound", 0.1))
    u32("diffusion.max_denoising_steps", gen.get("max_denoising_steps", 48))
    f32("diffusion.t_max", gen.get("t_max", 0.8))
    f32("diffusion.t_min", gen.get("t_min", 0.4))
    f32("diffusion.confidence_threshold", gen.get("confidence_threshold", 0.005))
    u32("diffusion.stability_threshold", gen.get("stability_threshold", 1))
    if (ct := sched.get("threshold")) is not None:
        f32("diffusion.commit_threshold", ct)   # BlockRefinementScheduler's 0.95 fast-commit
    u32("tokenizer.ggml.mask_token_id", mask_id)


def mask_id_from_tokenizer(hf_dir: Path) -> int:
    tc = json.loads((hf_dir / "tokenizer_config.json").read_text())
    mask = tc.get("mask_token") or "<mask>"
    tj = json.loads((hf_dir / "tokenizer.json").read_text())
    for tok in tj.get("added_tokens", []):
        if tok.get("content") == mask:
            return int(tok["id"])
    raise SystemExit(f"mask token {mask!r} not in tokenizer.json added_tokens — "
                     "diffuse_generate refuses without an id, so this converter does too")


def copy_tokenizer(w, donor: Path, gguf):
    """Copy every tokenizer.* kv from a donor GGUF (same-vocab Gemma 4), except the mask id we
    write ourselves from the model's own tokenizer."""
    r = gguf.GGUFReader(str(donor))
    n = 0
    for field in r.fields.values():
        if not field.name.startswith("tokenizer.") or field.name == "tokenizer.ggml.mask_token_id":
            continue
        val = field.contents()
        if isinstance(val, str):
            w.add_string(field.name, val)
        elif isinstance(val, bool):
            w.add_bool(field.name, val)
        elif isinstance(val, int):
            w.add_uint32(field.name, val)
        elif isinstance(val, float):
            w.add_float32(field.name, val)
        elif isinstance(val, list):
            w.add_array(field.name, val)
        else:
            raise SystemExit(f"donor kv {field.name}: unhandled type {type(val)}")
        n += 1
    print(f"tokenizer: copied {n} kvs from {donor.name}")


# ── conversion core (dict in → gguf out, so --selfcheck needs no shards) ────────────────────────

def convert(tensors: dict, cfg: dict, mask_id: int, out: Path, gguf, dtype: str,
            tokenizer_donor=None):
    t = cfg["text_config"]
    d, ff_exp = t["hidden_size"], t["moe_intermediate_size"]
    w = gguf.GGUFWriter(str(out), "diffusion_gemma")
    write_metadata(w, cfg, mask_id)
    if tokenizer_donor:
        copy_tokenizer(w, tokenizer_donor, gguf)

    emitted = set()
    for hf_name, arr in tensors.items():
        name = map_name(hf_name)
        if name is None:
            continue
        a = np.asarray(arr)
        if name.endswith(("ffn_gate_up_exps.weight", "ffn_down_exps.weight")):
            a = orient_experts(name, a, d, ff_exp)
        if name.endswith("layer_output_scale.weight") or name.endswith("layer_output_scale_enc.weight"):
            a = a.reshape(1)
        # norms, scales and scalars stay f32 (the engine folds/reads them as f32); projections
        # take the requested dtype
        keep_f32 = a.ndim == 1 or ".scale" in name or "norm" in name
        a = a.astype(np.float32 if (keep_f32 or dtype == "f32") else np.float16)
        w.add_tensor(name, np.ascontiguousarray(a))
        emitted.add(name)

    # the census invariants the engine's gate re-checks — fail HERE, near the cause
    layers = t["num_hidden_layers"]
    lt = t["layer_types"]
    for il in range(layers):
        v = f"blk.{il}.attn_v.weight"
        if (lt[il] == "full_attention") == (v in emitted):
            raise SystemExit(f"layer {il} ({lt[il]}): attn_v {'present' if v in emitted else 'absent'} "
                             "— contradicts the V-from-K census")
        if f"blk.{il}.layer_output_scale_enc.weight" not in emitted:
            raise SystemExit(f"layer {il}: encoder scalar missing — "
                             "gemmadiff::layer_out_scale will refuse every encode pass")

    w.write_header_to_file()
    w.write_kv_data_to_file()
    w.write_tensors_to_file()
    w.close()
    print(f"wrote {out} ({len(emitted)} tensors, {dtype} projections)")


def load_hf(hf_dir: Path):
    """Stream every shard named by the safetensors index. Needs `pip install safetensors`."""
    from safetensors import safe_open
    idx = json.loads((hf_dir / "model.safetensors.index.json").read_text())
    by_shard = {}
    for name, shard in idx["weight_map"].items():
        by_shard.setdefault(shard, []).append(name)
    tensors = {}
    for shard, names in sorted(by_shard.items()):
        with safe_open(hf_dir / shard, framework="numpy") as f:
            for n in names:
                if map_name(n) is not None:   # don't materialize the vision tower at all
                    tensors[n] = f.get_tensor(n)
        print(f"read {shard}: {len(names)} tensors")
    return tensors


# ── selfcheck: a tiny synthetic model through the whole pipe ─────────────────────────────────────

def selfcheck(gguf, out: Path):
    """2 layers (one sliding, one full), every tensor family present, then re-read and assert.
    Green means the NAME MAP and METADATA round-trip — the two things the 51 GB shards aren't
    needed to validate."""
    layers, d, ff, ffe, nexp = 2, 8, 16, 8, 4
    cfg = {"canvas_length": 8, "_scheduler": {"block_length": 4, "threshold": 0.95},
           "text_config": {
        "num_hidden_layers": layers, "hidden_size": d, "intermediate_size": ff,
        "moe_intermediate_size": ffe, "num_experts": nexp, "top_k_experts": 2,
        "num_attention_heads": 2, "num_key_value_heads": 2, "num_global_key_value_heads": 1,
        "head_dim": 4, "global_head_dim": 8, "sliding_window": 4,
        "layer_types": ["sliding_attention", "full_attention"],
        "rms_norm_eps": 1e-6, "final_logit_softcapping": 30.0,
        "max_position_embeddings": 64, "vocab_size": 32,
        "rope_parameters": {"full_attention": {"rope_theta": 1e6, "partial_rotary_factor": 0.25},
                            "sliding_attention": {"rope_theta": 1e4}},
    }}
    t = cfg["text_config"]
    rng = np.random.default_rng(7)
    ten = {"model.decoder.embed_tokens.weight": rng.standard_normal((t["vocab_size"], d)),
           "model.decoder.norm.weight": rng.standard_normal(d)}
    for part in ("pre_norm", "gate_proj", "up_proj", "down_proj"):
        shape = (d,) if part == "pre_norm" else ((ff, d) if part != "down_proj" else (d, ff))
        ten[f"model.decoder.self_conditioning.{part}.weight"] = rng.standard_normal(shape)
    for il in range(layers):
        full = t["layer_types"][il] == "full_attention"
        hd = t["global_head_dim"] if full else t["head_dim"]
        nkv = t["num_global_key_value_heads"] if full else t["num_key_value_heads"]
        L = f"model.decoder.layers.{il}"
        ten[f"{L}.input_layernorm.weight"] = rng.standard_normal(d)
        ten[f"{L}.self_attn.q_proj.weight"] = rng.standard_normal((t["num_attention_heads"] * hd, d))
        ten[f"{L}.self_attn.q_norm.weight"] = rng.standard_normal(hd)
        ten[f"{L}.self_attn.k_proj.weight"] = rng.standard_normal((nkv * hd, d))
        ten[f"{L}.self_attn.k_norm.weight"] = rng.standard_normal(hd)
        if not full:
            ten[f"{L}.self_attn.v_proj.weight"] = rng.standard_normal((nkv * hd, d))
        ten[f"{L}.self_attn.o_proj.weight"] = rng.standard_normal((d, t["num_attention_heads"] * hd))
        for n in ("post_attention_layernorm", "pre_feedforward_layernorm",
                  "post_feedforward_layernorm", "post_feedforward_layernorm_1",
                  "pre_feedforward_layernorm_2", "post_feedforward_layernorm_2"):
            ten[f"{L}.{n}.weight"] = rng.standard_normal(d)
        ten[f"{L}.mlp.gate_proj.weight"] = rng.standard_normal((ff, d))
        ten[f"{L}.mlp.up_proj.weight"] = rng.standard_normal((ff, d))
        ten[f"{L}.mlp.down_proj.weight"] = rng.standard_normal((d, ff))
        ten[f"{L}.router.proj.weight"] = rng.standard_normal((nexp, d))
        ten[f"{L}.router.scale"] = rng.standard_normal(d)
        ten[f"{L}.router.per_expert_scale"] = rng.standard_normal(nexp)
        # deliberately emit gate_up in the TRANSPOSED orientation to exercise the detector
        ten[f"{L}.experts.gate_up_proj"] = rng.standard_normal((nexp, d, 2 * ffe))
        ten[f"{L}.experts.down_proj"] = rng.standard_normal((nexp, d, ffe))
        ten[f"{L}.layer_scalar"] = np.array(0.5 + il)
        ten[f"model.encoder.language_model.layers.{il}.layer_scalar"] = np.array(1.5 + il)
    # a vision tensor that must be SKIPPED, not refused
    ten["model.encoder.vision_tower.std_bias"] = rng.standard_normal(3)

    convert(ten, cfg, mask_id=4, out=out, gguf=gguf, dtype="f32")

    r = gguf.GGUFReader(str(out))
    kv = {f.name: f.contents() for f in r.fields.values()}
    names = {t.name for t in r.tensors}
    assert kv["general.architecture"] == "diffusion_gemma"
    assert kv["diffusion_gemma.attention.sliding_window_pattern"] == 2
    assert kv["diffusion_gemma.attention.key_length"] == 8 and kv["diffusion_gemma.attention.key_length_swa"] == 4
    assert kv["diffusion_gemma.rope.dimension_count"] == 2, "0.25 × global_head_dim"
    assert kv["diffusion.shift_logits"] == "false" and kv["tokenizer.ggml.mask_token_id"] == 4
    assert "blk.0.attn_v.weight" in names and "blk.1.attn_v.weight" not in names, "V-from-K census"
    assert {"self_cond.pre_norm.weight", "blk.1.layer_output_scale_enc.weight",
            "blk.0.ffn_gate_inp.scale", "blk.0.ffn_down_exps.scale"} <= names
    gu = next(t for t in r.tensors if t.name == "blk.0.ffn_gate_up_exps.weight")
    # GGUFReader reports numpy-order shape; target numpy (n_exp, 2ff, d) — the transposed input
    # must have been detected and permuted
    assert tuple(int(x) for x in gu.shape) in {(nexp, 2 * ffe, d), (d, 2 * ffe, nexp)}, gu.shape
    assert not any("vision" in n for n in names)
    print(f"selfcheck \u2713 — {len(names)} tensors, name map + metadata round-trip")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("hf_dir", nargs="?", help="HF model dir (config.json + safetensors shards)")
    ap.add_argument("--out", type=Path, help="output .gguf path")
    ap.add_argument("--dtype", choices=("f16", "f32"), default="f16")
    ap.add_argument("--gguf-py", type=Path, help="path to llama.cpp's gguf-py (else: pip gguf)")
    ap.add_argument("--tokenizer-gguf", type=Path, help="donor GGUF to copy tokenizer.* kvs from")
    ap.add_argument("--selfcheck", action="store_true", help="synthetic end-to-end check, no shards")
    args = ap.parse_args()

    if args.gguf_py:
        sys.path.insert(0, str(args.gguf_py))
    import gguf  # noqa: E402

    if args.selfcheck:
        out = args.out or Path("/tmp/gemmadiff-selfcheck.gguf")
        selfcheck(gguf, out)
        return
    if not args.hf_dir or not args.out:
        ap.error("hf_dir and --out are required (or --selfcheck)")
    hf = Path(args.hf_dir)
    cfg = json.loads((hf / "config.json").read_text())
    # The scheduler and generation configs are separate files in the HF drop. Merge the keys the
    # GGUF carries, so the file states what THIS checkpoint shipped rather than what the 26B did.
    # Missing files fall back to the 26B constants pinned in write_metadata — stated there, once.
    sched = hf / "scheduler" / "scheduler_config.json"
    if sched.exists():
        cfg["_scheduler"] = json.loads(sched.read_text())
    gen = hf / "generation_config.json"
    if gen.exists():
        cfg["_generation"] = json.loads(gen.read_text())
    convert(load_hf(hf), cfg, mask_id_from_tokenizer(hf), args.out, gguf, args.dtype,
            tokenizer_donor=args.tokenizer_gguf)


if __name__ == "__main__":
    main()
