#!/usr/bin/env python3
"""check_octree_winding.py — regression guard for the missing-tops fix.

The bug (SESSION_HANDOFF.md s2): the flat +Y top in chunk_mesh_octree.comp was
wound opposite to the sides + bumpy tops, so non-weatherable tops were culled.
The fix reversed the flat +Y emit_quad corner order to q0,q3,q2,q1 (AO following:
ao.x,ao.w,ao.z,ao.y). This asserts that fix is still in place and that the bumpy
top keeps its known-visible winding.

  python3 handoff/check_octree_winding.py [path/to/chunk_mesh_octree.comp]

Exit nonzero if the winding looks regressed.
"""
import os, re, sys

DEFAULT = "crates/unendlich-engine/src/shaders/chunk_mesh_octree.comp"

def main(argv):
    path = argv[1] if len(argv) > 1 else DEFAULT
    if not os.path.exists(path):
        print(f"not found: {path}"); return 2
    s = open(path).read()
    ok = True

    # 1) Flat +Y must use the FIXED corner + AO order.
    fixed = re.search(r"q0,\s*q3,\s*q2,\s*q1,\s*\n\s*color \* ao\.x,\s*color \* ao\.w,\s*color \* ao\.z,\s*color \* ao\.y", s)
    regressed = re.search(r"q0,\s*q1,\s*q2,\s*q3,\s*\n\s*color \* ao\.x,\s*color \* ao\.y,\s*color \* ao\.z,\s*color \* ao\.w", s)
    if fixed and not regressed:
        print("OK   flat +Y winding = q0,q3,q2,q1 (fixed)")
    else:
        ok = False
        print("FAIL flat +Y winding regressed (expected q0,q3,q2,q1 / ao.x,ao.w,ao.z,ao.y)")

    # 2) Bumpy sub-quad keeps its known-visible winding (ib+1 = v10, ib+5 = v01).
    if re.search(r"indices\[ib \+ 1u\]\s*=\s*v10;", s) and re.search(r"indices\[ib \+ 5u\]\s*=\s*v01;", s):
        print("OK   bumpy top winding = v00->v10->v11->v01 (visible)")
    else:
        ok = False
        print("FAIL bumpy top winding changed — it was the VISIBLE reference; do not flip it")

    return 0 if ok else 1

if __name__ == "__main__":
    sys.exit(main(sys.argv))
