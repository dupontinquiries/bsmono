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

HOW THE FIX IS APPLIED SAFELY
-----------------------------
`--apply` reorders ONE array: `nodes`. Nothing else is touched.

That is possible because of the shape these files happen to have — checked, not
assumed: one primitive per mesh, nodes are flat `{mesh, name}` with no children,
and the scene lists node indices in order. So the draw order the loader sees is
the node order, and permuting the node array permutes the parts without moving a
single byte of geometry.

Accessors, bufferViews, materials and scene.bin are left exactly as they are.
That is the whole safety argument: a rewrite that renumbers accessors can
produce a file that loads and is silently wrong, which is what the first
quantiser did. This one cannot, because it does not renumber anything the
geometry depends on.

If a chassis does not have that shape — more than one primitive per mesh, or
nodes with children — `--apply` refuses it rather than doing something clever.
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
    # IN NODE ORDER, because that is what the loader walks:
    # model.rs does `for node in scene.nodes()`. Reading mesh order instead
    # measures a sequence nothing renders — which made the first version of
    # --apply report FIXED while the diagnostic still showed a permutation,
    # since the two were looking at different arrays.
    order = g.get("scenes", [{}])[0].get("nodes")
    nodes = g.get("nodes", [])
    if order is None:
        order = range(len(nodes))
    out = []
    for ni in order:
        mi = nodes[ni].get("mesh")
        if mi is None:
            continue
        for p in g["meshes"][mi].get("primitives", []):
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


def reorderable(chassis, pose):
    """Is this file the flat shape --apply relies on?

    Refusing is cheap; a wrong reorder is a corrupt model that still loads.
    """
    with open(os.path.join(chassis, pose, "scene.gltf")) as fh:
        g = json.load(fh)
    if any(len(m.get("primitives", [])) != 1 for m in g.get("meshes", [])):
        return None, "a mesh has more than one primitive"
    if any("children" in n for n in g.get("nodes", [])):
        return None, "nodes have children"
    if len(g.get("nodes", [])) != len(g.get("meshes", [])):
        return None, "node count != mesh count"
    # Every node must point at a distinct mesh, or reordering nodes does not
    # reorder parts one-for-one.
    ms = [n.get("mesh") for n in g["nodes"]]
    if sorted(ms) != list(range(len(ms))):
        return None, "nodes do not map 1:1 onto meshes"
    return g, None


def apply_to_pose(chassis, pose, mapping):
    """Permute the node array so part k of this pose matches part k of pose 0."""
    g, why = reorderable(chassis, pose)
    if g is None:
        return False, why
    old = g["nodes"]
    # mapping[i] = index in POSE 0 that this pose's part i corresponds to.
    # We want the output ordered by that, so part k lines up across poses.
    order = sorted(range(len(old)), key=lambda i: mapping[i])
    g["nodes"] = [old[i] for i in order]
    if "scenes" in g and g["scenes"]:
        g["scenes"][0]["nodes"] = list(range(len(g["nodes"])))
    path = os.path.join(chassis, pose, "scene.gltf")
    with open(path, "w") as fh:
        json.dump(g, fh, separators=(",", ":"))
    return True, None


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
            continue
        solvable += 1
        moved = sum(1 for i, m in enumerate(mapping) if i != m)
        if not APPLY:
            print(f"SOLVED  {os.path.basename(chassis)}: {moved} of {len(cz)} prims move")
            continue
        # Every pose after the first is aligned to the first. Pose 0 is the
        # reference and is never touched, so a re-run is idempotent.
        ok = True
        for pose in poses[1:]:
            pm, _ = solve(first, prims(chassis, os.path.basename(pose)))
            done, why = apply_to_pose(chassis, os.path.basename(pose), pm)
            if not done:
                print(f"REFUSED {os.path.basename(chassis)}/{os.path.basename(pose)}: {why}")
                ok = False
                break
        print(f"{'FIXED  ' if ok else 'PARTIAL'} {os.path.basename(chassis)}: "
              f"{moved} of {len(cz)} prims reordered")
    print()
    print(f"identical order : {identical}")
    print(f"permuted        : {permuted}  ({solvable} solvable, {unsolvable} ambiguous)")
    print(f"other           : {other}")
    return 0


APPLY = False

if __name__ == "__main__":
    args = [a for a in sys.argv[1:] if a != "--apply"]
    APPLY = "--apply" in sys.argv
    sys.exit(main(args[0] if args else "."))
