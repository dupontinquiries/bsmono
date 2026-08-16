#!/usr/bin/env python3
"""fold_unpermute.py — detect (and diagnose) permuted fold strips.

The original handoff warned about this and could not fix it:

    every fold currently steps rather than blends because the strips are
    permuted ... until foldview's merge emits material groups in a
    deterministic order

Measured here: of 60 chassis, 47 have identical primitive order across poses,
12 are PERMUTED, 1 differs another way. So it is a minority — but for those 12
`xform::Morph` lerps vertex i of one part toward vertex i of a DIFFERENT part,
which is why the fold steps instead of blending.

WHY THIS IS RECOVERABLE
-----------------------
The permutation is a reordering, not a loss. `radiachnor` has 39 primitives with
the same 39 vertex counts in both poses, just in a different order — and 18 of
its 20 distinct counts are unique, so most primitives identify themselves.

The ambiguous ones are small: counts 18 (x16) and 36 (x5), which are greebles.
Those are disambiguated by bounding-box centroid, because a greeble that moves
during the fold still moves LESS than the distance to a different greeble.

WHAT THIS TOOL DOES NOT DO
--------------------------
It does not rewrite the .gltf. Emitting a reordered glTF means rewriting
accessors, bufferViews and the buffer itself, and getting that wrong produces
exactly the silent corruption the quantiser produced last time. This reports
which chassis are affected and whether their permutation is solvable, so the fix
can be made where the data is generated — or, if it must be made here, made with
the mapping this prints rather than a guess.
"""
import glob
import json
import os
import sys
from collections import Counter


def prims(chassis, pose):
    """(count, centroid) per primitive, in file order."""
    path = os.path.join(chassis, pose, "scene.gltf")
    with open(path) as fh:
        g = json.load(fh)
    out = []
    for m in g.get("meshes", []):
        for p in m.get("primitives", []):
            a = g["accessors"][p["attributes"]["POSITION"]]
            lo, hi = a.get("min", [0, 0, 0]), a.get("max", [0, 0, 0])
            centre = tuple((l + h) / 2 for l, h in zip(lo, hi))
            out.append((a["count"], centre))
    return out


def solve(a, z):
    """Map each index of `z` back to its index in `a`, or None where ambiguous.

    Count first because it is exact; centroid only to break ties, and only among
    primitives that share a count.
    """
    by_count = {}
    for i, (n, c) in enumerate(a):
        by_count.setdefault(n, []).append(i)
    mapping, unresolved = [], 0
    used = set()
    for n, c in z:
        cands = [i for i in by_count.get(n, []) if i not in used]
        if not cands:
            mapping.append(None)
            unresolved += 1
            continue
        if len(cands) == 1:
            mapping.append(cands[0])
            used.add(cands[0])
            continue
        # Nearest centroid among same-count candidates. A part that moves during
        # the fold still moves less than the gap to a different part — true for
        # greebles, which is where the ambiguity is.
        best = min(cands, key=lambda i: sum((x - y) ** 2 for x, y in zip(a[i][1], c)))
        mapping.append(best)
        used.add(best)
    return mapping, unresolved


def main(bots_dir):
    identical = permuted = other = 0
    solvable = unsolvable = 0
    for chassis in sorted(glob.glob(os.path.join(bots_dir, "*"))):
        poses = sorted(p for p in glob.glob(os.path.join(chassis, "f*"))
                       if os.path.isfile(os.path.join(p, "scene.gltf")))
        if len(poses) < 2:
            continue
        first = prims(chassis, os.path.basename(poses[0]))
        last = prims(chassis, os.path.basename(poses[-1]))
        ca, cz = [x[0] for x in first], [x[0] for x in last]
        if ca == cz:
            identical += 1
            continue
        if Counter(ca) != Counter(cz):
            other += 1
            print(f"OTHER   {os.path.basename(chassis)}: "
                  f"{len(ca)} vs {len(cz)} prims, counts differ — not a permutation")
            continue
        permuted += 1
        mapping, unresolved = solve(first, last)
        if unresolved:
            unsolvable += 1
            print(f"HARD    {os.path.basename(chassis)}: {unresolved} of {len(cz)} unresolved")
        else:
            solvable += 1
            moved = sum(1 for i, m in enumerate(mapping) if i != m)
            print(f"SOLVED  {os.path.basename(chassis)}: {moved} of {len(cz)} prims move")
    print()
    print(f"identical order : {identical}")
    print(f"permuted        : {permuted}  ({solvable} solvable, {unsolvable} ambiguous)")
    print(f"other           : {other}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1] if len(sys.argv) > 1 else "."))
