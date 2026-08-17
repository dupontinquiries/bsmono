#!/usr/bin/env python3
"""gguf-shexp-graft — graft missing shared-expert tensors from an HF checkpoint into a GGUF,
and diagnose the fused-qkv layout question from the same files.

WHY THIS EXISTS. LLaDA2.0-mini GGUFs in circulation declare expert_shared_count=1 but carry NO
ffn_*_shexp tensors — the fork converter that produced them dropped the shared expert, so every
token's FFN silently loses an always-on contribution and generation is word salad regardless of
how correct the engine is. llada2 has never landed in mainline llama.cpp (checked against master),
so "reconvert with mainline" is not available. This tool performs the surgical alternative: read
the shared-expert weights from the original HF safetensors, quantize to the file's own format,
and append them under the canonical names the loader already looks for. Memory: the tool
holds input and output in RAM (~2x file size peak) — fine on a workstation for a ~9 GB file.

  graft:       gguf-shexp-graft.py --gguf in.gguf --hf /path/to/hf_dir --out fixed.gguf [--quant q4_0|f32]
  qkv verdict: gguf-shexp-graft.py --gguf in.gguf --hf /path/to/hf_dir --verify-qkv

The --verify-qkv mode settles the OTHER flagged unknown: mainline's Bailing recipe SPLITS
query_key_value and PERMUTES q/k into ggml's interleaved rope layout, while this engine assumes
raw-HF split-half (NEOX) pairing. The verdict dequantizes the GGUF's fused attn_qkv and compares
against the HF tensor both raw and permuted: RAW-MATCH means the engine's assumption is right,
PERMUTED-MATCH means every layer's attention has been rope-scrambled all along, NEITHER means a
third layout and the diff pattern is printed for a human.

HF names tried (first hit wins), per layer N:
  model.layers.N.mlp.shared_experts.{gate,up,down}_proj.weight   (deepseek/Ling style)
  model.layers.N.mlp.shared_expert.{gate,up,down}_proj.weight    (qwen2moe style)
If none match, the actual mlp.* keys of layer 0 are listed instead of guessing.
"""
import argparse, json, os, struct, sys
import numpy as np

ALIGN_DEFAULT = 32
GGML_F32, GGML_F16, GGML_Q4_0, GGML_Q8_0 = 0, 1, 2, 8
# (block_elems, block_bytes) for every type the tool may need to COPY THROUGH from a real file —
# grafting only writes F32/Q4_0, but real Q4_0 conversions routinely keep output/embed as K-quants
# or halves, and an unknown type must not make the tool refuse a file it only needs to copy.
TYPE_SIZES = {GGML_F32: (1, 4), GGML_F16: (1, 2), GGML_Q4_0: (32, 18), GGML_Q8_0: (32, 34),
              3: (32, 20),   # Q4_1
              6: (32, 22), 7: (32, 24),           # Q5_0, Q5_1
              10: (256, 84), 11: (256, 110),       # Q2_K, Q3_K
              12: (256, 144), 13: (256, 176),      # Q4_K, Q5_K
              14: (256, 210), 15: (256, 292),      # Q6_K, Q8_K
              30: (1, 2)}                          # BF16

# ---------- GGUF read ----------
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
    raise SystemExit(f"unknown kv type {ty}")

def read_gguf(path):
    b = open(path, "rb").read()
    if b[:4] != b"GGUF": raise SystemExit(f"{path}: not a GGUF")
    ver = struct.unpack_from("<I", b, 4)[0]
    if ver != 3: raise SystemExit(f"{path}: GGUF v{ver}, this tool speaks v3")
    n_t = struct.unpack_from("<Q", b, 8)[0]; n_kv = struct.unpack_from("<Q", b, 16)[0]
    o = 24; kv_start = o; align = ALIGN_DEFAULT; kvs = {}
    for _ in range(n_kv):
        k, o = gstr(b, o); ty = struct.unpack_from("<I", b, o)[0]; o += 4
        if ty == 4: kvs[k] = struct.unpack_from("<I", b, o)[0]
        elif ty == 8: kvs[k] = gstr(b, o)[0]
        if k == "general.alignment": align = struct.unpack_from("<I", b, o)[0]
        o = skip_val(b, o, ty)
    kv_bytes = b[kv_start:o]
    tensors = []
    for _ in range(n_t):
        name, o = gstr(b, o)
        nd = struct.unpack_from("<I", b, o)[0]; o += 4
        ne = list(struct.unpack_from(f"<{nd}Q", b, o)); o += 8 * nd
        ty = struct.unpack_from("<I", b, o)[0]; o += 4
        off = struct.unpack_from("<Q", b, o)[0]; o += 8
        tensors.append({"name": name, "ne": ne, "type": ty, "offset": off})
    data_start = (o + align - 1) // align * align
    return b, kv_bytes, tensors, data_start, align, n_kv, kvs

def tensor_nbytes(t):
    n = 1
    for d in t["ne"]: n *= d
    blk, bs = TYPE_SIZES.get(t["type"], (None, None))
    if blk is None: raise SystemExit(f"tensor {t['name']}: unhandled ggml type {t['type']}")
    if n % blk: raise SystemExit(f"tensor {t['name']}: {n} elements not divisible by block {blk}")
    return n // blk * bs

# ---------- safetensors read (single or sharded) ----------
def st_open_all(hf_dir):
    idx = os.path.join(hf_dir, "model.safetensors.index.json")
    if os.path.exists(idx):
        wm = json.load(open(idx))["weight_map"]
        return {k: os.path.join(hf_dir, v) for k, v in wm.items()}
    single = [f for f in os.listdir(hf_dir) if f.endswith(".safetensors")]
    if not single: raise SystemExit(f"{hf_dir}: no .safetensors or index found")
    out = {}
    for f in single:
        hdr, _ = st_header(os.path.join(hf_dir, f))
        for k in hdr:
            if k != "__metadata__": out[k] = os.path.join(hf_dir, f)
    return out

def st_header(path):
    with open(path, "rb") as fh:
        n = struct.unpack("<Q", fh.read(8))[0]
        return json.loads(fh.read(n)), 8 + n

def st_load(path, key):
    hdr, base = st_header(path)
    e = hdr[key]; a, z = e["data_offsets"]
    with open(path, "rb") as fh:
        fh.seek(base + a); raw = fh.read(z - a)
    dt = e["dtype"]
    if dt == "F32": arr = np.frombuffer(raw, np.float32)
    elif dt == "F16": arr = np.frombuffer(raw, np.float16).astype(np.float32)
    elif dt == "BF16":
        arr = (np.frombuffer(raw, np.uint16).astype(np.uint32) << 16).view(np.float32)
    else: raise SystemExit(f"{key}: dtype {dt} unhandled")
    return arr.reshape(e["shape"]).copy()

# ---------- Q4_0, ported from ggml quantize_row_q4_0_ref ----------
def f32_to_f16_bytes(x):
    return x.astype(np.float16).tobytes()

def quantize_q4_0(rows):  # rows: [r, k] f32
    r, k = rows.shape
    assert k % 32 == 0
    x = rows.reshape(-1, 32)
    absx = np.abs(x)
    imax = absx.argmax(axis=1)
    maxv = x[np.arange(len(x)), imax]          # SIGNED value of the abs-max element
    d = maxv / -8.0
    id_ = np.where(d != 0.0, 1.0 / np.where(d == 0.0, 1.0, d), 0.0)
    q = np.clip((x * id_[:, None]) + 8.5, 0.0, 15.0).astype(np.uint8)  # +8.5: ggml adds 8.5 then truncates
    lo, hi = q[:, :16], q[:, 16:]
    packed = (lo | (hi << 4)).astype(np.uint8)
    out = bytearray()
    dh = np.asarray(d, np.float32).astype(np.float16)
    for i in range(len(x)):
        out += dh[i].tobytes(); out += packed[i].tobytes()
    return bytes(out)

def dequantize_q4_0(raw, n):
    blocks = n // 32
    out = np.empty((blocks, 32), np.float32)
    for i in range(blocks):
        o = i * 18
        d = np.frombuffer(raw[o:o+2], np.float16)[0].astype(np.float32)
        qs = np.frombuffer(raw[o+2:o+18], np.uint8)
        lo = (qs & 0x0F).astype(np.int8) - 8
        hi = (qs >> 4).astype(np.int8) - 8
        out[i, :16] = lo * d; out[i, 16:] = hi * d
    return out.reshape(-1)

# ---------- write ----------
def write_grafted(src, out_path, kv_bytes, tensors, data_start, align, n_kv, new_tensors):
    all_t = tensors + [{"name": n, "ne": ne, "type": ty, "offset": 0} for (n, ne, ty, _) in new_tensors]
    hdr = bytearray()
    hdr += b"GGUF" + struct.pack("<I", 3) + struct.pack("<Q", len(all_t)) + struct.pack("<Q", n_kv)
    hdr += kv_bytes
    payload = []
    off = 0
    blobs = {t["name"]: None for t in tensors}
    for t in tensors:
        nb = tensor_nbytes(t)
        payload.append((t, src[data_start + t["offset"]: data_start + t["offset"] + nb]))
    for (n, ne, ty, blob) in new_tensors:
        payload.append(({"name": n, "ne": ne, "type": ty}, blob))
    body = bytearray(); infos = bytearray()
    for t, blob in payload:
        pad = (-len(body)) % align
        body += b"\x00" * pad
        t_off = len(body)
        body += blob
        nm = t["name"].encode()
        infos += struct.pack("<Q", len(nm)) + nm + struct.pack("<I", len(t["ne"]))
        for d in t["ne"]: infos += struct.pack("<Q", d)
        infos += struct.pack("<I", t["type"]) + struct.pack("<Q", t_off)
    hdr += infos
    pad = (-len(hdr)) % align
    with open(out_path, "wb") as fh:
        fh.write(hdr); fh.write(b"\x00" * pad); fh.write(body)

# ---------- main ----------
CANDIDATES = ["model.layers.{n}.mlp.shared_experts.{p}_proj.weight",
              "model.layers.{n}.mlp.shared_expert.{p}_proj.weight"]

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--gguf", required=True); ap.add_argument("--hf", required=True)
    ap.add_argument("--out"); ap.add_argument("--quant", default="q4_0", choices=["q4_0", "f32"])
    ap.add_argument("--verify-qkv", action="store_true")
    a = ap.parse_args()
    src, kv_bytes, tensors, data_start, align, n_kv, kvs = read_gguf(a.gguf)
    names = {t["name"] for t in tensors}
    st = st_open_all(a.hf)
    n_layers = 1 + max(int(t["name"].split(".")[1]) for t in tensors if t["name"].startswith("blk."))

    if a.verify_qkv:
        return verify_qkv(src, tensors, data_start, st, kvs)

    if not a.out: raise SystemExit("--out required for grafting")
    have = [n for n in names if "shexp" in n]
    if have: raise SystemExit(f"{a.gguf} already has shexp tensors ({sorted(have)[:3]}…) — nothing to graft")
    # Probe ACROSS layers: leading dense blocks legitimately carry no shared expert, so layer 0
    # proves nothing either way — the first grafting attempt refused on its own synthetic test
    # exactly because of this, which is the loud-refusal doing its job on the tool's author.
    pattern = None
    for c in CANDIDATES:
        if any(c.format(n=n, p="gate") in st for n in range(n_layers)): pattern = c; break
    if pattern is None:
        actual = sorted(k for k in st if ".mlp." in k)[:12]
        raise SystemExit("no shared-expert tensors found under known names in the HF checkpoint.\n"
                         "sample of mlp keys actually present:\n  " + "\n  ".join(actual))
    new = []; skipped = []
    for n in range(n_layers):
        if pattern.format(n=n, p="gate") not in st:
            skipped.append(n); continue  # dense-lead layer: no shared expert by design
        for p, gname in [("gate", "ffn_gate_shexp"), ("up", "ffn_up_shexp"), ("down", "ffn_down_shexp")]:
            w = st_load(st[pattern.format(n=n, p=p)], pattern.format(n=n, p=p))  # [out, in]
            rows, cols = w.shape
            if a.quant == "q4_0": ty, blob = GGML_Q4_0, quantize_q4_0(w)
            else:                  ty, blob = GGML_F32, w.astype("<f4").tobytes()
            # GGUF ne is fastest-first: [cols, rows]
            new.append((f"blk.{n}.{gname}.weight", [cols, rows], ty, blob))
    if not new:
        raise SystemExit(f"pattern {pattern} matched the probe but produced zero tensors — refusing")
    if skipped:
        print(f"skipped layers with no shared expert (dense-lead by design): {skipped}")
    write_grafted(src, a.out, kv_bytes, tensors, data_start, align, n_kv, new)
    print(f"grafted {len(new)} tensors ({a.quant}) -> {a.out}")

def verify_qkv(src, tensors, data_start, st, kvs):
    arch = kvs.get("general.architecture", "llada2")
    n_head = kvs.get(f"{arch}.attention.head_count")
    n_kv_head = kvs.get(f"{arch}.attention.head_count_kv", n_head)
    if n_head is None: raise SystemExit(f"{arch}.attention.head_count missing from kv — cannot infer the permute grouping")
    t = next((t for t in tensors if t["name"] == "blk.0.attn_qkv.weight"), None)
    if t is None:
        print("GGUF has no fused blk.0.attn_qkv.weight — nothing to verify (separate q/k/v path)."); return
    cols, rows = t["ne"][0], t["ne"][1]
    nb = tensor_nbytes(t)
    raw = src[data_start + t["offset"]: data_start + t["offset"] + nb]
    if t["type"] == GGML_Q4_0: g = dequantize_q4_0(raw, rows * cols).reshape(rows, cols)
    elif t["type"] == GGML_F32: g = np.frombuffer(raw, "<f4").reshape(rows, cols).astype(np.float32)
    else: raise SystemExit(f"attn_qkv type {t['type']} unhandled in verify")
    key = next((k for k in st if k.endswith("layers.0.attention.query_key_value.weight")
                or k.endswith("layers.0.self_attn.query_key_value.weight")), None)
    if key is None:
        cand = sorted(k for k in st if "layers.0" in k and ("attn" in k or "attention" in k))
        raise SystemExit("no fused query_key_value in HF layer 0 — attention keys present:\n  " + "\n  ".join(cand))
    h = st_load(st[key], key)  # [rows, cols]
    if h.shape != (rows, cols): raise SystemExit(f"shape mismatch: gguf {rows}x{cols} vs hf {h.shape}")
    # roundtrip HF through the SAME quantizer so quantization error cancels
    def rt(x):
        if t["type"] == GGML_Q4_0: return dequantize_q4_0(quantize_q4_0(x), rows * cols).reshape(rows, cols)
        return x
    def permute(w, n_head):  # llama.cpp convert-time rope permute, rows = n_head*hd
        hd = w.shape[0] // n_head
        return w.reshape(n_head, 2, hd // 2, w.shape[1]).swapaxes(1, 2).reshape(w.shape)
    d_model = cols
    # infer heads from ne: q rows = d_model when head grouping standard; use metadata-free heuristic
    # q block = first d_model rows; k,v split the remainder evenly
    q_rows = d_model; kv_rows = (rows - d_model) // 2
    for label, cand in [
        ("RAW  (engine's assumption: unpermuted HF, NEOX rope)", rt(h)),
        ("PERMUTED (llama.cpp bailing recipe on q,k)", rt(np.concatenate([
            permute(h[:q_rows], n_head), permute(h[q_rows:q_rows+kv_rows], n_kv_head), h[q_rows+kv_rows:]]))),
    ]:
        err = np.abs(g - cand).max()
        rel = err / (np.abs(g).max() + 1e-30)
        print(f"{label}: max abs diff {err:.3e} (rel {rel:.3e}) -> {'MATCH' if rel < 1e-5 else 'no'}")
    print("verdict: whichever line says MATCH is the layout this GGUF actually carries. "
          "If neither matches, the fork used a third layout; file an issue with this output.")

if __name__ == "__main__":
    main()
