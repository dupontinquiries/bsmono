#!/usr/bin/env python3
"""gguf-to-q4.py — re-encode an f32 GGUF's 2D weights as ggml Q4_0.

WHY: every synthetic tiny in this repo is f32, so the quantised residency paths — q8_0, Q4_K and now
Q4_0 transcode — could only ever be exercised on the operator's real 8.8 GB checkpoint. A Q4_0 tiny
lets the whole gate battery run against quantised weights in seconds, which is where the load-path
fast paths actually live.

The quantiser is the one proven BIT-IDENTICAL to ggml's quantize_row_q4_0_ref (see the c4-731 proof
chain), so a file produced here is a faithful stand-in for one llama.cpp would emit.

Usage: gguf-to-q4.py <in.gguf> <out.gguf>
1D tensors (norms, biases) and any k not divisible by 32 stay f32, matching what real converters do.
"""
import importlib.util, os, struct, sys
import numpy as np

D = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("g", os.path.join(D, "gguf-shexp-graft.py"))
g = importlib.util.module_from_spec(spec); spec.loader.exec_module(g)

def main():
    if len(sys.argv) < 3: raise SystemExit(__doc__)
    src_path, out_path = sys.argv[1], sys.argv[2]
    src, kvb, tens, ds, al, nkv, _ = g.read_gguf(src_path)
    out_t, quantised = [], 0
    for t in tens:
        ne, ty = t["ne"], t["type"]
        raw = src[ds + t["offset"]: ds + t["offset"] + g.tensor_nbytes(t)]
        if ty == 0 and len(ne) >= 2 and ne[0] % 32 == 0:
            w = np.frombuffer(raw, np.float32).reshape(-1, ne[0])
            out_t.append((t["name"], ne, 2, g.quantize_q4_0(w))); quantised += 1
        else:
            out_t.append((t["name"], ne, ty, raw))
    hdr = bytearray(b"GGUF" + struct.pack("<I", 3) + struct.pack("<Q", len(out_t))
                    + struct.pack("<Q", nkv) + kvb)
    body, infos = bytearray(), bytearray()
    for name, ne, ty, blob in out_t:
        body += b"\x00" * ((-len(body)) % al)
        off = len(body); body += blob
        nm = name.encode()
        infos += struct.pack("<Q", len(nm)) + nm + struct.pack("<I", len(ne))
        for d in ne: infos += struct.pack("<Q", d)
        infos += struct.pack("<I", ty) + struct.pack("<Q", off)
    hdr += infos
    open(out_path, "wb").write(bytes(hdr) + b"\x00" * ((-len(hdr)) % al) + bytes(body))
    print(f"{out_path}: {quantised} tensor(s) re-encoded as Q4_0, {len(out_t) - quantised} left f32")

if __name__ == "__main__":
    main()
