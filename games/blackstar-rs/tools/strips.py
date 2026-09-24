#!/usr/bin/env python3
"""strips.py — write forge chassis as the fold strips blackstar-rs discovers.

The loader wants, per chassis:

    assets/bots/<name>/chassis.json
    assets/bots/<name>/f00/{scene.gltf,scene.bin,orient.json}   vehicle
    assets/bots/<name>/f01 .. f0N                               fold frames
    assets/bots/<name>/f0N/...                                  robot

`poses.sort()` puts f00 first and f0N last, and main.rs names pose 0 as the
label, the last as "<LABEL> ROBOT" and the middle ones "<LABEL> FOLD k". So
the strip must run VEHICLE -> ROBOT, which is fold.py's t=1 -> t=0.

Five frames, not more: main.rs says "Every pose is a real model and gets a
real SDF bake below, so poses cost startup time: keep strips short (5-7)
rather than smooth", and SDF_BUDGET is CSDF_MAXM = 126 poses for the whole
roster.
"""
import json, os, sys, math
from forge import Mesh, loft, wheel, sphere, tube, torus, build, _hw_at, arch, detail_car
from fleet import ALL
from bots import CHASSIS
from fold import build_pair, PAIRS
from glb import Glb, wheel_name

FRAMES = 5


def _mesh_to_glb(mesh, name):
    g = Glb()
    g.add_node(name, g.add_mesh(name, mesh, name))
    return g


def write_chassis(c, root, frames=FRAMES):
    """One chassis, vehicle -> robot, as f00..f0(N-1)."""
    bdir = os.path.join(root, f"forge_{c.name}")
    os.makedirs(bdir, exist_ok=True)
    total = 0
    for k in range(frames):
        # f00 is the VEHICLE end. fold.py's t=1 is vehicle, t=0 is robot, so
        # the strip walks t from 1 down to 0.
        t = 1.0 - k / (frames - 1)
        m = build_pair(c, t)
        g = Glb()
        # One node per strip frame: the fold is baked per pose, because that is
        # what the loader consumes. The per-JOINT named nodes in export.py are
        # the other product -- useful to skeletonize, wrong shape for a strip.
        g.add_node("body", g.add_mesh(f"{c.name}_f{k}", m, "body"))
        total += g.write_gltf(os.path.join(bdir, f"f{k:02d}"), f"{c.name}_f{k}")
    vehicle = PAIRS[c.name][0]
    json.dump({"scale": 1.0, "boss": False,
               "source": f"forge:{c.name} <-> {vehicle}",
               "family": "FORGE",
               "why": PAIRS[c.name][1]},
              open(os.path.join(bdir, "chassis.json"), "w"))
    return bdir, total


def write_vehicle(spec, root):
    """A vehicle with no robot form still needs 2+ poses to be discovered, so
    it gets a two-frame strip that does not fold: both frames are the vehicle.
    Cheap (the second is the same geometry) and it keeps the loader's
    invariant instead of special-casing it."""
    bdir = os.path.join(root, f"forge_{spec.name}")
    os.makedirs(bdir, exist_ok=True)
    m = build(spec)
    if spec.cls == "car":
        for _n, d in detail_car(spec):
            m.add(d)
    total = 0
    for k in range(2):
        g = Glb()
        g.add_node("body", g.add_mesh(f"{spec.name}", m, "body"))
        total += g.write_gltf(os.path.join(bdir, f"f{k:02d}"), spec.name)
    json.dump({"scale": 1.0, "boss": False,
               "source": f"forge:{spec.name}", "family": "FORGE",
               "why": spec.role[:120]},
              open(os.path.join(bdir, "chassis.json"), "w"))
    return bdir, total


if __name__ == "__main__":
    root = sys.argv[1] if len(sys.argv) > 1 else "assets/bots"
    os.makedirs(root, exist_ok=True)
    n = 0
    for c in CHASSIS:
        d, b = write_chassis(c, root)
        print(f"  {os.path.basename(d):<22} {FRAMES} poses  {b/1024:>7.0f} KB")
        n += FRAMES
    paired = {PAIRS[c.name][0] for c in CHASSIS}
    for s in ALL:
        if s.name in paired:
            continue          # already reachable as a chassis' vehicle pose
        d, b = write_vehicle(s, root)
        print(f"  {os.path.basename(d):<22} 2 poses  {b/1024:>7.0f} KB")
        n += 2
    print(f"\n{n} poses written to {root}")
    print(f"SDF_BUDGET is 126 for the WHOLE roster — this adds {n}")
