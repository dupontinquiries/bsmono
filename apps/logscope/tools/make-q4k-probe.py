#!/usr/bin/env python3
"""make-q4k-probe.py — emit real Q4_K super-block bytes plus the floats they encode.

c4-784 added a Q4_K -> int8-resident slice path (the fix for "0.0% of weights sliced" and a 54.9 s
DeepSeek load) and shipped its LOADER arm verified only by construction, because this repo had no
Q4_K fixture. This makes one.

It is not ggml's optimiser and does not try to be: the test that consumes it checks our dequantiser
against the intended values FIRST, so a bad fixture cannot vouch for a good transcode, and only then
compares the slice path against the f32 round-trip.

Writes /tmp/q4k_probe.bin (144 bytes per 256 weights) and /tmp/q4k_probe.npy (the source floats).
"""
import numpy as np, struct

def quantize_q4_k(row):
    assert row.size % 256 == 0
    out = bytearray()
    for sb in range(row.size // 256):
        groups = row[sb*256:(sb+1)*256].astype(np.float32).reshape(8, 32)
        mins, maxs = groups.min(axis=1), groups.max(axis=1)
        scales = np.where((maxs - mins) / 15.0 > 0, (maxs - mins) / 15.0, 1e-8)
        d = float(scales.max() / 63.0) or 1e-8
        dmin = float((-mins).max() / 63.0) if (-mins).max() > 0 else 1e-8
        sc6 = np.clip(np.round(scales / d), 0, 63).astype(np.int64)
        mn6 = np.clip(np.round(-mins / dmin), 0, 63).astype(np.int64)
        s = bytearray(12)                      # ggml's 6-bit scale/min packing
        for j in range(8):
            if j < 4:
                s[j] = int(sc6[j]) & 63
                s[j+4] = int(mn6[j]) & 63
            else:
                s[j+4] = (int(sc6[j]) & 15) | ((int(mn6[j]) & 15) << 4)
                s[j-4] |= ((int(sc6[j]) >> 4) & 3) << 6
                s[j]   |= ((int(mn6[j]) >> 4) & 3) << 6
        q = np.zeros(256, np.int64)
        for j in range(8):
            ds = max(d * float(sc6[j]), 1e-8); dm = dmin * float(mn6[j])
            q[j*32:(j+1)*32] = np.clip(np.round((groups[j] + dm) / ds), 0, 15)
        packed = bytearray(128)
        for j in range(4):
            lo, hi = q[j*64:j*64+32], q[j*64+32:j*64+64]
            for l in range(32):
                packed[j*32 + l] = int(lo[l]) | (int(hi[l]) << 4)
        out += struct.pack('<e', d) + struct.pack('<e', dmin) + bytes(s) + bytes(packed)
    return bytes(out)

if __name__ == "__main__":
    rng = np.random.default_rng(7)
    probe = rng.standard_normal((4, 256)).astype(np.float32) * 0.1
    np.save('/tmp/q4k_probe.npy', probe)
    open('/tmp/q4k_probe.bin', 'wb').write(b''.join(quantize_q4_k(r) for r in probe))
    print(f"/tmp/q4k_probe.bin: {probe.shape[0]} rows x {probe.shape[1]} weights")
