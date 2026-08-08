#!/usr/bin/env python3
"""voxel_probe.py -- is a multi-level voxel grouping worth it for this corpus?

The idea under test: instead of storing each model as triangles, store it as
occupancy on a grid, merge coplanar faces into runs (the greedy meshing
unendlich already does in `world/surface.rs`), and keep several levels of
grouping (its `LodLevel::{Near,Mid,Far,VeryFar}`). Voxels are regular, and
regular data compresses in ways triangle soup does not.

This measures BOTH axes, because either one alone is misleading:

  * size   -- occupancy bytes, run-length bytes, and gzip of each, against the
              original mesh
  * error  -- how far the voxelisation is from the original surface, as a
              fraction of the model's own size

A voxel scheme can always win on size by getting coarser. The question is
whether it wins at an error small enough not to see, so both numbers are
reported at every resolution and neither is quoted alone.

Deliberately independent of the engine: this is a probe, and wiring it into
unendlich before knowing the answer would be building the thing the probe
exists to justify.
"""
import sys, os, gzip, math, struct, argparse, json


def load_obj(path):
    verts, tris = [], []
    with open(path) as f:
        for line in f:
            if line.startswith("v "):
                verts.append(tuple(float(x) for x in line.split()[1:4]))
            elif line.startswith("f "):
                idx = [int(p.split("/")[0]) - 1 for p in line.split()[1:]]
                # Fan-triangulate: an n-gon face would otherwise be dropped,
                # and OBJ from other tools is full of quads.
                for i in range(1, len(idx) - 1):
                    tris.append((idx[0], idx[i], idx[i + 1]))
    return verts, tris


def bbox(verts):
    lo = [min(v[i] for v in verts) for i in range(3)]
    hi = [max(v[i] for v in verts) for i in range(3)]
    return lo, hi


def voxelise(verts, tris, n):
    """Occupancy set at resolution n, by rasterising triangle surfaces.

    Surface voxelisation rather than solid: these models are shells, and a
    flood-fill would treat any model with a seam as entirely hollow or
    entirely solid depending on where the fill started.
    """
    lo, hi = bbox(verts)
    span = max(hi[i] - lo[i] for i in range(3)) or 1.0
    step = span / n
    occ = set()

    def put(p):
        occ.add(tuple(min(n - 1, max(0, int((p[i] - lo[i]) / step)))
                      for i in range(3)))

    for a, b, c in tris:
        pa, pb, pc = verts[a], verts[b], verts[c]
        # Sample each triangle densely enough that no voxel is skipped: the
        # longest edge in voxel units sets the count, so a big triangle cannot
        # leave holes in the shell.
        e = max(math.dist(pa, pb), math.dist(pb, pc), math.dist(pc, pa))
        k = max(1, int(e / step) + 1)
        for i in range(k + 1):
            for j in range(k + 1 - i):
                u, v = i / k, j / k
                w = 1.0 - u - v
                put(tuple(pa[d] * w + pb[d] * u + pc[d] * v for d in range(3)))
    return occ, lo, step


def greedy_runs(occ):
    """Merge occupied voxels into x-axis runs -- the 1D core of the greedy
    meshing in surface.rs, which is where most of the win comes from because
    machine parts are full of straight edges."""
    runs = []
    for (x, y, z) in sorted(occ):
        if runs and runs[-1][1] == y and runs[-1][2] == z \
                and runs[-1][0] + runs[-1][3] == x:
            runs[-1][3] += 1
        else:
            runs.append([x, y, z, 1])
    return runs


def fidelity(occ, n, ref_occ, ref_n):
    """Two honest numbers, because the obvious one is a tautology.

    My first attempt measured "distance from each original vertex to the
    nearest occupied voxel" and always got exactly zero — voxelisation marks
    every vertex's own cell by construction, so the test could not fail. It
    would have reported perfect fidelity at every resolution.

    Instead:

      quant_pct -- the cell's half-diagonal as a percentage of the model's
                   longest side. This is the INTRINSIC bound: no detail finer
                   than a cell survives, whatever the rasteriser does.
      iou       -- intersection-over-union of this level's occupancy against a
                   high-resolution reference, downsampled to this grid. This
                   catches what quantisation alone does not: whether the SHAPE
                   is the same shape, not just the same coarseness.
    """
    quant = (math.sqrt(3) / 2) / n * 100.0
    k = ref_n // n
    down = {(x // k, y // k, z // k) for (x, y, z) in ref_occ}
    inter = len(occ & down)
    union = len(occ | down)
    return quant, (inter / union if union else 0.0)


def report(path, levels):
    verts, tris = load_obj(path)
    if not verts or not tris:
        return None
    raw = os.path.getsize(path)
    gz_raw = len(gzip.compress(open(path, "rb").read()))
    out = {"model": os.path.basename(path), "verts": len(verts),
           "tris": len(tris), "obj_bytes": raw, "obj_gz": gz_raw, "levels": []}
    ref_n = max(levels) * 2
    ref_occ, _, _ = voxelise(verts, tris, ref_n)
    for n in levels:
        occ, lo, step = voxelise(verts, tris, n)
        runs = greedy_runs(occ)
        # Occupancy as packed coords (3 x u16), runs as (3 x u16 + u16).
        occ_bytes = len(occ) * 6
        run_bytes = len(runs) * 8
        blob = b"".join(struct.pack("<4H", *r) for r in runs)
        out["levels"].append({
            "n": n, "voxels": len(occ), "runs": len(runs),
            "occ_bytes": occ_bytes, "run_bytes": run_bytes,
            "run_gz": len(gzip.compress(blob)),
            "quant_pct": round(fidelity(occ, n, ref_occ, ref_n)[0], 3),
            "iou": round(fidelity(occ, n, ref_occ, ref_n)[1], 4),
        })
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("paths", nargs="+")
    ap.add_argument("--levels", default="16,32,64,128")
    a = ap.parse_args()
    levels = [int(x) for x in a.levels.split(",")]
    results = []
    for p in a.paths:
        r = report(p, levels)
        if r:
            results.append(r)
    if not results:
        print("no models")
        return 1
    print(json.dumps(results, indent=1) if len(results) == 1 else "")
    # Corpus totals, which is the number that decides this.
    print("%-6s %10s %10s %10s %8s %7s" % ("grid", "run_gz", "vs obj_gz",
                                            "voxels", "quant%", "IoU"))
    base = sum(r["obj_gz"] for r in results)
    for i, n in enumerate(levels):
        gz = sum(r["levels"][i]["run_gz"] for r in results)
        vox = sum(r["levels"][i]["voxels"] for r in results)
        q = sum(r["levels"][i]["quant_pct"] for r in results) / len(results)
        iou = sum(r["levels"][i]["iou"] for r in results) / len(results)
        print("%-6d %10d %9.1f%% %10d %8.3f %7.4f"
              % (n, gz, 100.0 * gz / base, vox, q, iou))
    print("obj gz total: %d across %d models" % (base, len(results)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
