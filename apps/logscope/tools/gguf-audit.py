#!/usr/bin/env python3
"""Pre-flight audit for a GGUF before any load: parse the header only (ms, not the
90 s quantize+load) and report what a bad conversion would make you discover the
slow way. Born from the LLaDA2.0-mini preview: metadata claimed 1 shared expert,
the file shipped no ffn_*_shexp tensors, and the always-on FFN missing from every
MoE layer produced fluent degenerate output. This check is that round-trip, in
milliseconds."""
import struct, sys, collections

def read(b, off, n): return b[off:off+n], off+n
def u32(b,o): v,o2=read(b,o,4); return struct.unpack("<I",v)[0],o2
def u64(b,o): v,o2=read(b,o,8); return struct.unpack("<Q",v)[0],o2
def gstr(b,o):
    n,o=u64(b,o); v,o=read(b,o,n); return v.decode('utf-8','replace'),o
def skip_val(b,o,ty):
    if ty in (0,1,7): return o+1
    if ty in (2,3): return o+2
    if ty in (4,5,6): return o+4
    if ty in (10,11,12): return o+8
    if ty==8: n,o=u64(b,o); return o+n
    if ty==9:
        et,o=u32(b,o); n,o=u64(b,o)
        for _ in range(n): o=skip_val(b,o,et)
        return o
    raise SystemExit(f"unknown kv type {ty}")
def get_val(b,o,ty):
    if ty==4: return u32(b,o)
    if ty==8: return gstr(b,o)
    if ty==6: v,o2=read(b,o,4); return struct.unpack("<f",v)[0],o2
    return None, skip_val(b,o,ty)

path = sys.argv[1]
b = open(path,'rb').read(64<<20)  # header only; tensors not needed
o=0
magic,o=u32(b,o); assert magic==0x46554747, "not a GGUF"
ver,o=u32(b,o); n_t,o=u64(b,o); n_kv,o=u64(b,o)
meta={}
for _ in range(n_kv):
    k,o=gstr(b,o); ty,o=u32(b,o)
    v,o=get_val(b,o,ty)
    if v is not None: meta[k]=v
names=set()
for _ in range(n_t):
    nm,o=gstr(b,o); nd,o=u32(b,o)
    o+=8*nd; o+=4+8
    names.add(nm)
arch=meta.get("general.architecture","?")
P=lambda k: meta.get(f"{arch}.{k}")
L=P("block_count") or 0
print(f"{path}\n  arch={arch} blocks={L} d={P('embedding_length')} experts={P('expert_count')} used={P('expert_used_count')} shared={P('expert_shared_count')} dense_lead={P('leading_dense_block_count')} nextn={P('nextn_predict_layers')}")
bias_blocks = sum(1 for i in range(L) if f"blk.{i}.exp_probs_b.bias" in names or f"blk.{i}.exp_probs_b.weight" in names or f"blk.{i}.exp_probs_b" in names)
print(f"  exp_probs_b (no-aux selection bias): {'present on ' + str(bias_blocks) + ' block(s) — 719+ loaders HONOR it (routing differs from pre-719 runs BY DESIGN)' if bias_blocks else 'absent'}")
print(f"  diffusion.shift_logits={meta.get('diffusion.shift_logits','ABSENT (defaults TRUE downstream — wrong for LLaDA lineage)')}  mask_token_id={meta.get('tokenizer.ggml.mask_token_id','ABSENT')}")
issues=0
has=lambda i,t: f"blk.{i}.{t}.weight" in names
for i in range(L):
    moe = has(i,"ffn_gate_exps")
    if moe:
        for t in ("ffn_up_exps","ffn_down_exps","ffn_gate_inp"):
            if not has(i,t): print(f"  [!] blk.{i}: MoE but missing {t}"); issues+=1
        sh = P("expert_shared_count") or 0
        shexp_present = all(has(i,f"ffn_{x}_shexp") for x in ("gate","up","down"))
        if sh and not shexp_present:
            print(f"  [!] blk.{i}: metadata claims {sh} shared expert(s), NO ffn_*_shexp tensors — the LLaDA2-preview defect"); issues+=1
        if not sh and shexp_present:
            print(f"  [!] blk.{i}: shexp tensors present but expert_shared_count=0 — weights nobody reads"); issues+=1
        if has(i,"exp_probs_b") if False else (f"blk.{i}.exp_probs_b" in names or has(i,"exp_probs_b")):
            pass
    fused = has(i,"attn_qkv"); sep = has(i,"attn_q")
    if not fused and not sep: print(f"  [!] blk.{i}: neither attn_qkv nor attn_q"); issues+=1
print(f"  audit: {'CLEAN' if issues==0 else f'{issues} issue(s) — do NOT spend a load on this file'}")
# LLaDA2 completeness checklist (ground truth: inclusionAI config.json + modeling_llada2_moe.py):
# one run on a real file enumerates every converter-dropped item that silently damages output.
if arch == "llada2":
    rows = [
        ("rope.dimension_count kv (partial RoPE — ref rotates 64 of 128; absent => engine defaults to head_dim/2)",
         P("rope.dimension_count") is not None),
        ("attn_q_norm/attn_k_norm tensors (per-head QK-norm; ref has query/key_layernorm every layer)",
         any("attn_q_norm" in n for n in names)),
        ("ffn_*_shexp tensors (always-on shared expert)", any("shexp" in n for n in names)),
        ("exp_probs_b tensors (routing selection bias)", any("exp_probs_b" in n for n in names)),
        ("expert_group kvs (group-limited routing — ref n_group 8, topk_group 4; HONORED since c4-750, \
CPU and GPU: per-group top-2 sum, keep the best topk_group groups, top-k within them)",
         any(k.startswith(f"{arch}.expert_group") for k in meta)),
    ]
    print("  llada2 completeness vs the reference checkpoint:")
    for label, ok in rows:
        print(f"    [{'ok' if ok else 'MISSING'}] {label}")

sys.exit(0 if issues==0 else 1)
