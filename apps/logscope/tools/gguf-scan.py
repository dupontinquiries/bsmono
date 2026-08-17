#!/usr/bin/env python3
"""gguf-scan.py — what structure does this file have that the LOADER could exploit?

Answers, with numbers rather than opinion:

  1. TRANSCODE OPPORTUNITY. The loader's `make_lin` takes `&[f32]`, so every weight goes
     GGUF-quant -> dequantize to f32 -> requantize to resident. On an 8.8 GB Q4_0 model that is the
     bulk of the ~12 s between "loading" and "loaded", and most of it is avoidable: when the source
     block layout already matches the resident one in shape (32 values, one scale, 4-bit codes), the
     conversion can skip the f32 detour — though NOT as a "widen and unpack", which was this
     file's first claim and was WRONG. ggml Q4_0 codes mean (c-8)*d, i.e. -8..=+7, sixteen
     asymmetric levels, while the resident format clamps to -7..=+7, fifteen symmetric ones:
     a source code of 0 has no resident equivalent, so the values must be re-derived. What
     makes it cheap anyway is that the source scale CANCELS — with scale = amax/7 and
     amax = |d|*max|c-8|, the resident code is round((c-8)*7/max|c-8|), a function of the
     nibbles alone. Implemented as Gpu::q4_0_transcode_host_bytes, bit-identical to the
     round trip. The expensive parts of requantising —
     the per-block absmax scan and the divide — exist only because the f32 round-trip threw away the
     scale that was already in the file.

  2. TIED TENSORS. Some checkpoints ship token_embd and output as the SAME bytes. At vocab 157184 x
     d 2048 that is ~161 MB of resident memory and one full conversion pass, spent twice.

  3. DEGENERATE BLOCKS. Q4_0 blocks whose scale is exactly zero are constant-zero: they need no
     codes and no arithmetic. Real dense LLM weights rarely have many; a checkpoint that does is
     usually pruned, and worth knowing about before designing around it.

  4. WHAT IS ACTUALLY BIG. Per-tensor-class byte shares, so an optimisation is aimed at the bytes
     that exist rather than the ones that are easy to think about.

Usage: gguf-scan.py <model.gguf> [--sample-blocks N]
Reads only headers plus a bounded sample of tensor data; safe on a 9 GB file.
"""
import struct, sys, os
from collections import defaultdict

# (block_elems, block_bytes) for the types a loader may meet. Same table as gguf-shexp-graft.
TYPES = {0: ("F32", 1, 4), 1: ("F16", 1, 2), 2: ("Q4_0", 32, 18), 3: ("Q4_1", 32, 20),
         6: ("Q5_0", 32, 22), 7: ("Q5_1", 32, 24), 8: ("Q8_0", 32, 34),
         10: ("Q2_K", 256, 84), 11: ("Q3_K", 256, 110), 12: ("Q4_K", 256, 144),
         13: ("Q5_K", 256, 176), 14: ("Q6_K", 256, 210), 15: ("Q8_K", 256, 292),
         30: ("BF16", 1, 2)}

def gstr(b, o):
    n = struct.unpack_from("<Q", b, o)[0]; o += 8
    return b[o:o+n].decode("utf-8", "replace"), o + n

def skip_val(b, o, ty):
    scal = {0:1,1:1,2:2,3:2,4:4,5:4,6:4,7:1,10:8,11:8,12:8}
    if ty in scal: return o + scal[ty]
    if ty == 8: return gstr(b, o)[1]
    if ty == 9:
        ety = struct.unpack_from("<I", b, o)[0]; n = struct.unpack_from("<Q", b, o+4)[0]; o += 12
        for _ in range(n): o = skip_val(b, o, ety)
        return o
    raise SystemExit(f"unhandled kv type {ty}")

def read_header(path):
    with open(path, "rb") as fh:
        head = fh.read(64 << 20)          # headers are far smaller; bound the read
    if head[:4] != b"GGUF": raise SystemExit(f"{path}: not a GGUF")
    n_t = struct.unpack_from("<Q", head, 8)[0]
    n_kv = struct.unpack_from("<Q", head, 16)[0]
    o = 24; align = 32; kvs = {}
    for _ in range(n_kv):
        k, o = gstr(head, o); ty = struct.unpack_from("<I", head, o)[0]; o += 4
        if ty == 4: kvs[k] = struct.unpack_from("<I", head, o)[0]
        elif ty == 8: kvs[k] = gstr(head, o)[0]
        if k == "general.alignment": align = struct.unpack_from("<I", head, o)[0]
        o = skip_val(head, o, ty)
    tensors = []
    for _ in range(n_t):
        name, o = gstr(head, o)
        nd = struct.unpack_from("<I", head, o)[0]; o += 4
        ne = list(struct.unpack_from(f"<{nd}Q", head, o)); o += 8 * nd
        ty = struct.unpack_from("<I", head, o)[0]; o += 4
        off = struct.unpack_from("<Q", head, o)[0]; o += 8
        tensors.append({"name": name, "ne": ne, "type": ty, "offset": off})
    data = (o + align - 1) // align * align
    return kvs, tensors, data

def nbytes(t):
    n = 1
    for d in t["ne"]: n *= d
    if t["type"] not in TYPES: return None, n
    _, blk, bs = TYPES[t["type"]]
    if n % blk: return None, n
    return n // blk * bs, n

def fmt(b):
    return f"{b/1e9:.2f} GB" if b >= 1e9 else (f"{b/1e6:.1f} MB" if b >= 1e6 else f"{b/1e3:.1f} KB")

def klass(name):
    if "exps" in name: return "moe experts"
    if "shexp" in name: return "shared expert"
    if name.startswith("token_embd"): return "token embedding"
    if name.startswith("output"): return "output projection"
    if "attn" in name: return "attention"
    if "ffn" in name: return "ffn (dense)"
    return "other"

def main():
    if len(sys.argv) < 2: raise SystemExit(__doc__)
    path = sys.argv[1]
    sample = 4096
    if "--sample-blocks" in sys.argv:
        sample = int(sys.argv[sys.argv.index("--sample-blocks") + 1])
    kvs, tensors, data = read_header(path)
    size = os.path.getsize(path)
    arch = kvs.get("general.architecture", "?")
    print(f"{path}\n  arch={arch}  tensors={len(tensors)}  file={size/1e9:.2f} GB")

    # ---- 1. type histogram + transcode opportunity -------------------------------------------
    by_type = defaultdict(lambda: [0, 0])     # type -> [count, bytes]
    by_class = defaultdict(int)
    total = 0
    for t in tensors:
        nb, _ = nbytes(t)
        if nb is None: continue
        tn = TYPES.get(t["type"], (f"type{t['type']}",))[0]
        by_type[tn][0] += 1; by_type[tn][1] += nb
        by_class[klass(t["name"])] += nb
        total += nb
    print("\n  source quantisation:")
    for tn, (c, b) in sorted(by_type.items(), key=lambda kv: -kv[1][1]):
        print(f"    {tn:<6} {c:>5} tensors  {fmt(b):>10}  {100*b/max(total,1):5.1f}%")

    # A 32-value/one-scale/4-bit source reaches the resident layout without an f32 buffer: the
    # unpacking nibbles — no absmax scan, no divide, no f32 buffer.
    transcodable = sum(b for tn, (_, b) in by_type.items() if tn in ("Q4_0", "Q4_1"))
    f32_roundtrip_bytes = total * 8      # q4 -> f32 is 8x the bytes materialised per tensor
    print(f"\n  TRANSCODE: {transcodable/1e9:.2f} GB of {total/1e9:.2f} GB is 32-value/4-bit source,")
    print(f"    transcodable without the f32 detour (Gpu::q4_0_transcode_host_bytes).")
    print(f"    The f32 round-trip currently materialises ~{f32_roundtrip_bytes/1e9:.1f} GB of")
    print(f"    intermediate (per tensor, not at once) and runs an absmax scan per 32-block.")

    # ---- 2. tied tensors ----------------------------------------------------------------------
    by_off = defaultdict(list)
    for t in tensors: by_off[(t["offset"], t["type"], tuple(t["ne"]))].append(t["name"])
    tied = [names for names in by_off.values() if len(names) > 1]
    if tied:
        saved = 0
        for names in tied:
            t = next(x for x in tensors if x["name"] == names[0])
            nb, _ = nbytes(t)
            if nb: saved += nb * (len(names) - 1)
        print(f"\n  TIED TENSORS: {len(tied)} group(s) share bytes — {saved/1e6:.0f} MB of resident")
        print(f"    memory and one conversion pass each, currently spent twice:")
        for names in tied[:4]: print(f"      {' == '.join(names)}")
    else:
        print("\n  TIED TENSORS: none (every tensor has its own bytes)")

    # ---- 3. degenerate Q4_0 blocks -------------------------------------------------------------
    q4 = [t for t in tensors if t["type"] == 2]
    if q4:
        # Sample EVERY q4 tensor, not just the biggest. The first cut looked only at the largest and
        # reported "0%" on a fixture whose zero blocks were deliberately placed in a smaller tensor —
        # structure does not arrange itself to be convenient, and a scanner that can only see the
        # big rows will confidently miss it.
        per = max(1, sample // len(q4))
        seen = zeros = 0
        worst = None
        with open(path, "rb") as fh:
            for t in q4:
                nb, n_el = nbytes(t)
                if not nb: continue
                nblocks = n_el // 32
                take = min(per, nblocks)
                fh.seek(data + t["offset"])
                raw = fh.read(take * 18)
                z = sum(1 for i in range(take)
                        if struct.unpack_from("<e", raw, i * 18)[0] == 0.0)
                seen += take; zeros += z
                if z and (worst is None or z / max(take, 1) > worst[1]):
                    worst = (t["name"], z / max(take, 1))
        pct = 100 * zeros / max(seen, 1)
        print(f"\n  DEGENERATE BLOCKS: sampled {seen} blocks across {len(q4)} Q4_0 tensor(s) —")
        print(f"    {zeros} ({pct:.2f}%) have scale 0 (constant-zero: no codes, no arithmetic).")
        if worst:
            print(f"    concentrated in {worst[0]} ({100*worst[1]:.1f}% of its sampled blocks).")
        if pct < 1.0:
            print("    Under 1%: not worth a branch in every kernel. Dense weights, as expected.")
        else:
            print("    Above 1%: worth skipping at conversion time — those blocks need no codes.")

    # ---- 4. where the bytes actually are -------------------------------------------------------
    print("\n  bytes by class (aim optimisations at the big rows):")
    for c, b in sorted(by_class.items(), key=lambda kv: -kv[1]):
        print(f"    {c:<18} {fmt(b):>10}  {100*b/max(total,1):5.1f}%")

if __name__ == "__main__":
    main()
