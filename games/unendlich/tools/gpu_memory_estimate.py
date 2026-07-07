#!/usr/bin/env python3
"""
gpu_memory_estimate.py — back-of-the-envelope device-memory + indirect-slot
pressure by render distance, grounded in the engine's real constants.

WHY THIS EXISTS
---------------
A run log showed, in a contiguous band of chunks (x -5..-15, z -13..-21, y 2-3):

    overflowed GPU mesher (ERROR_OUT_OF_DEVICE_MEMORY); CPU-fallback meshing
    Indirect draw slot pool exhausted (in_use=4096/4096)

Two distinct failures from one cause — too many resident chunks for the device:
  1. The indirect-draw slot pool (MAX_CHUNK_DRAWS = 4096) runs out  -> some
     chunks get no draw slot -> HOLES.
  2. The GPU mesher's scratch/output allocation hits ERROR_OUT_OF_DEVICE_MEMORY
     -> those chunks fall back to the CPU greedy mesher, which is called as
     mesh_chunk_greedy(chunk, None, None, tick) -- i.e. WITHOUT the GPU path's
     climate tint -> a band of flat, untinted, neutrally-lit terrain against
     tinted GPU neighbours -> the visible STRIPE (and the "missing texture" look).

This is NOT a worldgen-divergence bug (the density/noise parity lint proves CPU
and GPU generate identical voxels). It is purely device-memory pressure.

GROUNDED CONSTANTS (read from the source this lives beside)
-----------------------------------------------------------
  CHUNK_SIZE        = 32          # constants.rs
  RENDER_DISTANCE   = 6 (default) # constants.rs (UNENDLICH_RENDER_DISTANCE overrides, clamp 1..256)
  MAX_CHUNK_DRAWS   = 4096        # chunk_indirect.rs  (the slot pool)
  PackedVertex      = 8 bytes     # mesher.rs  (vertices are ALREADY packed small)
  ~925 drawn chunks @ RD 6        # chunk_indirect.rs comment -> ~5.5 vertical layers

This is an ESTIMATE for reasoning about fixes, not an allocator. avg_verts and
the budget are inputs; tune them to your scene. Run with no args for the table.
"""

import argparse

CHUNK_SIZE = 32
MAX_CHUNK_DRAWS = 4096
PACKED_VERTEX_BYTES = 8
INDEX_BYTES = 4
# Anchor: the engine comment cites ~925 *drawn* chunks at render distance 6.
# horizontal chunks at RD 6 = (2*6+1)^2 = 169, so effective vertical layers:
VERTICAL_LAYERS = 925 / ((2 * 6 + 1) ** 2)  # ~= 5.47


def horizontal_chunks(rd: int) -> int:
    return (2 * rd + 1) ** 2


def drawn_chunks(rd: int) -> int:
    return round(horizontal_chunks(rd) * VERTICAL_LAYERS)


def per_chunk_mesh_bytes(avg_verts: int) -> int:
    # vertices (packed 8B) + indices (~1.5 indices per vertex, 4B each).
    return avg_verts * PACKED_VERTEX_BYTES + round(avg_verts * 1.5) * INDEX_BYTES


def report(avg_verts: int, budget_mb: int, rds):
    print("=" * 78)
    print("GPU memory + indirect-slot pressure by render distance")
    print(f"  avg_verts/chunk = {avg_verts}   per-chunk mesh ~= "
          f"{per_chunk_mesh_bytes(avg_verts)/1024:.1f} KiB   "
          f"device budget = {budget_mb} MiB")
    print(f"  slot pool MAX_CHUNK_DRAWS = {MAX_CHUNK_DRAWS}   "
          f"vertical layers ~= {VERTICAL_LAYERS:.2f}")
    print("=" * 78)
    hdr = f"{'RD':>4} {'horiz':>7} {'drawn':>8} {'slots':>14} {'mesh mem':>12} {'verdict':>10}"
    print(hdr)
    print("-" * len(hdr))
    for rd in rds:
        drawn = drawn_chunks(rd)
        mem_mb = drawn * per_chunk_mesh_bytes(avg_verts) / (1024 * 1024)
        slot_over = drawn - MAX_CHUNK_DRAWS
        slot_str = (f"{drawn}/{MAX_CHUNK_DRAWS}" if drawn <= MAX_CHUNK_DRAWS
                    else f"{drawn}/{MAX_CHUNK_DRAWS} +{slot_over}!")
        bad = drawn > MAX_CHUNK_DRAWS or mem_mb > budget_mb
        verdict = "OOM/HOLES" if bad else "ok"
        print(f"{rd:>4} {horizontal_chunks(rd):>7} {drawn:>8} {slot_str:>14} "
              f"{mem_mb:>9.0f} MiB {verdict:>10}")
    print("-" * len(hdr))
    # Largest RD that fits both limits.
    safe = None
    for rd in range(1, 64):
        drawn = drawn_chunks(rd)
        mem_mb = drawn * per_chunk_mesh_bytes(avg_verts) / (1024 * 1024)
        if drawn <= MAX_CHUNK_DRAWS and mem_mb <= budget_mb:
            safe = rd
        else:
            break
    print(f"Largest render distance that fits BOTH the 4096-slot pool and "
          f"{budget_mb} MiB: RD = {safe}")
    if safe is not None:
        print(f"  -> immediate, zero-code mitigation: "
              f"UNENDLICH_RENDER_DISTANCE={safe}")
    print()
    print("Ranked fixes (cheapest first):")
    print("  1. Lower render distance (above) — no code, fits the existing pool.")
    print("  2. Raise MAX_CHUNK_DRAWS — costs more indirect-buffer memory; only")
    print("     helps the HOLES, not the mesher OOM.")
    print("  3. Tighter LOD/eviction — coarse-mesh or drop far chunks so 'drawn'")
    print("     grows sub-quadratically with RD.")
    print("  4. Thread climate tint into the CPU fallback — kills the STRIPE even")
    print("     under OOM (symptom fix; needs world_model in the dispatcher scope).")
    print("  5. Further vertex/index compression — vertices are already 8B packed,")
    print("     so the win here is smaller than (3); indices are the bigger target.")


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--avg-verts", type=int, default=4000,
                    help="average vertices per drawn chunk (default 4000)")
    ap.add_argument("--budget-mb", type=int, default=4096,
                    help="device-memory budget in MiB for mesh buffers (default 4096)")
    ap.add_argument("--rd", type=int, nargs="*",
                    default=[6, 8, 10, 12, 16, 20, 24],
                    help="render distances to tabulate")
    args = ap.parse_args()
    report(args.avg_verts, args.budget_mb, args.rd)


if __name__ == "__main__":
    main()
