#!/usr/bin/env python3
"""Does the tile loop actually reorder anything, for THESE inputs?

ASAHI_GPU.md says an exactly-equal GEMM "would be suspicious — it would mean
the tile loop was not actually reordering anything", and quotes 1.669e-6 from an
M2 Ultra. On llvmpipe the same selftest reports 0.000e0.

Two candidate explanations, and they call for opposite reactions:
  A. llvmpipe happens to reproduce the reference's rounding, so exactness is an
     ADAPTER property and says nothing about the kernel.
  B. these particular inputs sum exactly regardless of order, so exactness is an
     INPUT property and the doc's heuristic is wrong as stated.

Decide it in f32 arithmetic, no GPU involved: compute one output cell both ways.
"""
import numpy as np

m, k, n = 37, 53, 41
a = np.array([((i * 37 % 91) - 45) / 45 for i in range(m * k)], dtype=np.float32).reshape(m, k)
b = np.array([((i * 53 % 83) - 41) / 41 for i in range(k * n)], dtype=np.float32).reshape(k, n)

TILE = 16
diffs = []
for r in range(m):
    for c in range(n):
        # sequential, as gemm_ref does
        seq = np.float32(0)
        for t in range(k):
            seq = np.float32(seq + np.float32(a[r, t] * b[t, c]))
        # tiled, as the kernel does: partial sums per TILE-wide chunk
        tiled = np.float32(0)
        for t0 in range(0, k, TILE):
            part = np.float32(0)
            for t in range(t0, min(t0 + TILE, k)):
                part = np.float32(part + np.float32(a[r, t] * b[t, c]))
            tiled = np.float32(tiled + part)
        diffs.append(abs(float(seq) - float(tiled)))

diffs = np.array(diffs)
print(f"cells compared      : {diffs.size}")
print(f"exactly equal       : {(diffs == 0).sum()}")
print(f"worst |seq - tiled| : {diffs.max():.3e}")
print()
if diffs.max() == 0:
    print("=> INPUT property. Reordering these values changes nothing in f32, so")
    print("   0.000e0 is expected on ANY adapter and the doc's 'suspicious' claim")
    print("   does not hold for this fixture.")
else:
    print("=> ADAPTER property. Reordering DOES change the f32 result, so a real")
    print("   tiled kernel must differ from the reference. llvmpipe reporting")
    print("   0.000e0 means it is not accumulating the way the kernel describes —")
    print("   exactness there validates nothing about the tile path.")
