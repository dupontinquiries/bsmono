#!/usr/bin/env python3
"""obj_diagnose.py -- figure out how a multi-file .obj model is really structured.

A folder full of model_0.obj ... model_N.obj can mean two very different things:

  A) TRUE PARTS   - each file holds its own distinct geometry.
                    -> the model is the CONCATENATION of all files.
  B) SHARED BLOCK - every file re-declares the SAME vertex list and differs only
                    in its face (f) lines. A common exporter artifact.
                    -> concatenating duplicates the mesh N times; the vertices
                       must be loaded ONCE and only faces merged.

Whole-file checksums cannot tell these apart (case B still differs per file,
because the face lines differ). This hashes ONLY the vertex block, which does.

READ-ONLY. Changes nothing.

Usage:
  python3 tools/obj_diagnose.py "path/to/model folder"
  python3 tools/obj_diagnose.py . --all        # every multi-obj folder in a tree
"""
import sys, os, argparse, hashlib
from collections import defaultdict

def human(n):
    for u in ("B", "K", "M", "G", "T"):
        if n < 1024 or u == "T":
            return f"{n:.0f}{u}" if u == "B" else f"{n:.1f}{u}"
        n /= 1024.0

def scan_obj(path):
    """Return (vhash, n_v, n_vt, n_f, size, materials)."""
    hv = hashlib.sha256(); nv = nvt = nf = 0; mats = set()
    try:
        with open(path, "r", errors="ignore") as fh:
            for line in fh:
                if line.startswith("v "):
                    hv.update(line.encode("utf-8", "replace")); nv += 1
                elif line.startswith("vt "):
                    nvt += 1
                elif line.startswith("f "):
                    nf += max(0, len(line.split()) - 3)   # triangles after fan-split
                elif line.startswith("usemtl") and len(line.split()) > 1:
                    mats.add(line.split(maxsplit=1)[1].strip())
    except OSError:
        return None
    try: size = os.path.getsize(path)
    except OSError: size = 0
    return hv.hexdigest(), nv, nvt, nf, size, mats

def diagnose(folder, quiet=False):
    if not os.path.isdir(folder):
        print(f"{folder}: not a directory"); return None
    objs = sorted(f for f in os.listdir(folder) if f.lower().endswith(".obj"))
    if len(objs) < 2:
        # the model usually sits one level down; say what is actually there
        sub = []
        for dp, _d, fs in os.walk(folder):
            n_obj = sum(1 for f in fs if f.lower().endswith(".obj"))
            n_gl  = sum(1 for f in fs if f.lower().endswith((".gltf", ".glb")))
            n_fbx = sum(1 for f in fs if f.lower().endswith((".fbx", ".dae", ".stl")))
            if n_obj or n_gl or n_fbx:
                sub.append((os.path.relpath(dp, folder), n_obj, n_gl, n_fbx))
        name = os.path.basename(folder.rstrip("/")) or folder
        if not sub:
            print(f"{name}: no model files found anywhere under this folder")
            return None
        print(f"{name}: {len(objs)} .obj here -- model files live deeper:")
        for rel, n_obj, n_gl, n_fbx in sub[:8]:
            bits = []
            if n_obj: bits.append(f"{n_obj} .obj")
            if n_gl:  bits.append(f"{n_gl} .gltf/.glb")
            if n_fbx: bits.append(f"{n_fbx} .fbx/.dae/.stl")
            print(f"    {rel or '.'}/  -> {', '.join(bits)}")
        if len(sub) > 8: print(f"    ...and {len(sub)-8} more")
        deep = [x for x in sub if x[1] >= 2]
        if deep:
            print(f"  re-run on:  {os.path.join(folder, deep[0][0])}")
        else:
            print("  (single-file model -- nothing to diagnose; harvest it directly)")
        return None
    groups = defaultdict(list); info = {}
    total_size = total_f = 0; all_mats = set()
    for f in objs:
        r = scan_obj(os.path.join(folder, f))
        if not r: continue
        vh, nv, nvt, nf, size, mats = r
        groups[(vh, nv)].append(f); info[f] = (nv, nvt, nf, size)
        total_size += size; total_f += nf; all_mats |= mats

    shared = max((len(v) for v in groups.values()), default=0)
    uniq_blocks = len(groups)
    uniq_verts = sum(k[1] for k in groups)          # vertices counted once per block

    print(f"\n{os.path.basename(folder.rstrip('/'))}")
    print(f"  .obj files          : {len(objs)}")
    print(f"  total size          : {human(total_size)}")
    print(f"  distinct vertex blocks: {uniq_blocks}")
    print(f"  largest shared group : {shared} files share one vertex block")
    ratio = total_f / max(1, uniq_verts)
    print(f"  faces (triangles)   : {total_f:,}")
    print(f"  unique vertices     : {uniq_verts:,}")
    print(f"  faces per vertex    : {ratio:.1f}   (healthy mesh ~2)")
    print(f"  materials           : {len(all_mats)}")

    if shared > 1:
        dup = len(objs) - uniq_blocks
        print(f"\n  VERDICT: SHARED VERTEX BLOCK (case B)")
        print(f"    {dup} of {len(objs)} files re-declare vertices already present elsewhere.")
        if ratio > 6:
            est = int(uniq_verts * 2)
            print(f"    FACES ARE DUPLICATED TOO: {ratio:.0f} faces/vertex means the same")
            print(f"    triangles repeat across files. True size is nearer ~{est:,} triangles,")
            print(f"    not {total_f:,}.")
        print(f"    Correct handling: one vertex block per group, faces de-duplicated.")
        print(f"    -> texture_export v13+ skips duplicate mesh files and dedupes faces.")
    else:
        print(f"\n  VERDICT: TRUE PARTS (case A)")
        print(f"    every file has its own vertex block; the model is all of them.")
        print(f"    -> merging all pieces is correct; expect ~{total_f:,} triangles.")
    return {"files": len(objs), "shared": shared, "blocks": uniq_blocks,
            "tris": total_f, "verts": uniq_verts, "size": total_size}

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("path")
    ap.add_argument("--all", action="store_true", help="scan every multi-obj folder under path")
    a = ap.parse_args()
    if not a.all:
        diagnose(a.path); return
    hits = 0
    for dp, dirs, files in os.walk(a.path):
        dirs[:] = [d for d in dirs if d not in {"parts", "iqm", "build"}]
        if sum(1 for f in files if f.lower().endswith(".obj")) >= 2:
            diagnose(dp); hits += 1
    print(f"\n{hits} multi-obj folder(s) diagnosed")

if __name__ == "__main__":
    main()
