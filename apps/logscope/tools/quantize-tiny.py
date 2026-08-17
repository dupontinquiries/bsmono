#!/usr/bin/env python3
"""quantize-tiny.py — re-emit a tiny GGUF with its 2D weights in Q8_0.

Every fixture in this repo is f32, and f32 takes the FALLBACK path everywhere: apply_add has fused
kernels only for q8 and q4, so c4-820/821/831's fusions are invisible on the fixtures that exist.
Three bundles have now shipped dispatch reductions that could not be measured here, and c4-831 found
one of them had silently missed the site that runs.

Q8_0 is the simplest quantisation that reaches the fused paths: per 32 weights, one f16 scale then
32 int8 codes, 34 bytes. Only 2D tensors are converted — norms are 1D and stay f32, as they are in
every real checkpoint.
"""
import struct, sys, importlib.util, numpy as np
from pathlib import Path

spec = importlib.util.spec_from_file_location('gr', str(Path(__file__).parent / 'gguf-shexp-graft.py'))
gr = importlib.util.module_from_spec(spec); spec.loader.exec_module(gr)

def q8_0(row: np.ndarray) -> bytes:
    out = bytearray()
    for b in range(row.size // 32):
        blk = row[b*32:(b+1)*32].astype(np.float32)
        amax = float(np.abs(blk).max())
        d = amax / 127.0 if amax > 0 else 1.0
        q = np.clip(np.rint(blk / d), -127, 127).astype(np.int8)
        out += struct.pack('<e', d) + q.tobytes()
    return bytes(out)

src_path, dst_path = sys.argv[1], sys.argv[2]
src, kvb, tens, ds, al, nkv, kvs = gr.read_gguf(src_path)
body, infos, converted = bytearray(), bytearray(), 0
for t in tens:
    raw = src[ds + t['offset'] : ds + t['offset'] + gr.tensor_nbytes(t)]
    ne, ty = t['ne'], t['type']
    # 2D f32 with a row length divisible by 32 — anything else keeps its bytes.
    if ty == 0 and len(ne) == 2 and ne[0] % 32 == 0:
        a = np.frombuffer(raw, dtype=np.float32).reshape(ne[1], ne[0])
        raw = b''.join(q8_0(r) for r in a); ty = 8; converted += 1
    body += b'\x00' * ((-len(body)) % al); off = len(body); body += raw
    nm = t['name'].encode()
    infos += struct.pack('<Q', len(nm)) + nm + struct.pack('<I', len(ne))
    for d_ in ne: infos += struct.pack('<Q', d_)
    infos += struct.pack('<I', ty) + struct.pack('<Q', off)

hdr = bytearray(b'GGUF' + struct.pack('<I', 3) + struct.pack('<Q', len(tens)) + struct.pack('<Q', nkv) + kvb)
hdr += infos
open(dst_path, 'wb').write(bytes(hdr) + b'\x00' * ((-len(hdr)) % al) + bytes(body))
print(f"{dst_path}: {converted} tensor(s) converted to Q8_0, {len(tens)-converted} left as-is")
