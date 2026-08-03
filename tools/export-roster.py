#!/usr/bin/env python3
"""export-roster.py — every foldable blackstar chassis into skydribble.

    ./tools/export-roster.py [poses]

Reads foldview_tuning.json for the hand-tuned models, drops the ones
foldview's own README lists as welded into a single island (they cannot
fold), and exports each as a pose strip under assets/bots/.

Naming: the category prefix decides behaviour in the engine. `flyer` and
`hovertank` chassis get a `jet_` prefix, which physics reads as "never
grounded, lift and nose thrust". Everything else drives.

Scaling note, because "all of them" is the obvious next ask: there are 1012
imported_*.iqm and one pose takes about a second, so the full set is ~85
minutes of export and roughly 4.7 GB of glTF at five poses each — before the
engine tries to load it. Every pose is a model with its own SDF bake at
startup. The tuned set is the one worth shipping; the rest needs a
streaming/on-demand loader first.
"""
import json, os, re, subprocess, sys, time

POSES = int(sys.argv[1]) if len(sys.argv) > 1 else 5
HERE = os.path.dirname(os.path.abspath(__file__))
SKY = os.path.dirname(HERE)
BS = os.path.join(SKY, "..", "blackstar")
FV = os.path.join(BS, "rust", "foldview")
DATA = os.path.join(BS, "engine_src", "data")
OUT = os.path.join(SKY, "assets", "bots")

WELDED = {"akimbo", "echotron", "reverser", "batwing", "tempest"}
FLY = ("flyer", "hovertank")


# --------------------------------------------------------------------------
# FACING: the fold turns the body around, and only sometimes.
#
# Measured across the roster: the robot pose ALWAYS faces -Z, which is the
# engine's convention. The vehicle pose faces +Z on 22 of 26 chassis and -Z
# on the other four. So the two modes point opposite ways on most of the
# fleet, and no single orient.json can fix it — one sidecar per chassis
# cannot rotate f00 and leave f04 alone.
#
# The fix is to spend the 180 degrees ACROSS the strip instead of hiding it
# at one end: pose k is yawed by 180 * smoothstep(k/(n-1)). f00 keeps the
# vehicle's own facing, f04 lands on the robot's, and the poses between turn
# through it. That makes the mismatch into the thing it should have been --
# the body pivoting as it stands up.
#
# Detection reads the head/visor/face part's Z sign, which needs the UNMERGED
# export (merging by colour throws the names away), so each chassis is probed
# once at t=0 and t=1 before its strip is written.
import math, struct


def head_z(d):
    """Mean Z of the head/visor/face parts of an unmerged foldview export."""
    g = json.load(open(os.path.join(d, "scene.gltf")))
    buf = open(os.path.join(d, "scene.bin"), "rb").read()
    best = None
    for key in ("visor", "face", "head"):
        zs = []
        for n in g["nodes"]:
            if not n.get("name", "").startswith(key):
                continue
            a = g["accessors"][g["meshes"][n["mesh"]]["primitives"][0]["attributes"]["POSITION"]]
            bv = g["bufferViews"][a["bufferView"]]
            off = bv.get("byteOffset", 0) + a.get("byteOffset", 0)
            n3 = a["count"]
            tot = 0.0
            for i in range(n3):
                tot += struct.unpack_from("<f", buf, off + i * 12 + 8)[0]
            zs.append(tot / max(n3, 1))
        if zs:
            best = sum(zs) / len(zs)
            break
    return best


def yaw_pose(d, deg):
    """Rotate every position in a written pose about Y, in place."""
    if abs(deg) < 1e-6:
        return
    r = math.radians(deg)
    c, s = math.cos(r), math.sin(r)
    g = json.load(open(os.path.join(d, "scene.gltf")))
    path = os.path.join(d, "scene.bin")
    buf = bytearray(open(path, "rb").read())
    for ai, a in enumerate(g["accessors"]):
        if a.get("type") != "VEC3":
            continue
        bv = g["bufferViews"][a["bufferView"]]
        off = bv.get("byteOffset", 0) + a.get("byteOffset", 0)
        lo = [1e30] * 3
        hi = [-1e30] * 3
        for i in range(a["count"]):
            o = off + i * 12
            x, y, z = struct.unpack_from("<3f", buf, o)
            nx, nz = c * x + s * z, -s * x + c * z
            struct.pack_into("<3f", buf, o, nx, y, nz)
            for k, v in enumerate((nx, y, nz)):
                lo[k] = min(lo[k], v)
                hi[k] = max(hi[k], v)
        a["min"], a["max"] = lo, hi
    open(path, "wb").write(buf)
    json.dump(g, open(os.path.join(d, "scene.gltf"), "w"))


def smoothstep(t):
    return t * t * (3.0 - 2.0 * t)


def label(iqm):
    """imported_racer_grit_aka_rocket_car.iqm -> ('racer', 'grit')"""
    n = re.sub(r"^imported_", "", iqm).replace(".iqm", "")
    n = re.sub(r"_aka_.*$", "", n)
    parts = n.split("_", 1)
    cat = parts[0]
    name = parts[1] if len(parts) > 1 else parts[0]
    return cat, name


def main():
    tuning = json.load(open(os.path.join(FV, "foldview_tuning.json")))
    todo = []
    for iqm in tuning:
        if any(w in iqm for w in WELDED):
            continue
        if not os.path.isfile(os.path.join(DATA, iqm)):
            continue
        cat, name = label(iqm)
        todo.append((("jet_" if cat in FLY else "") + name, iqm))
    todo.sort()

    print(f"{len(todo)} chassis x {POSES} poses")
    fv = os.path.join(FV, "target", "release", "foldview")
    ok, fail = 0, []
    t0 = time.time()
    for i, (name, iqm) in enumerate(todo, 1):
        d = os.path.join(OUT, name)
        subprocess.run(["rm", "-rf", d])

        # probe the facing, unmerged, so part names survive
        turn = 0.0
        probe = {}
        for t, tag in ((0.0, "v"), (1.0, "r")):
            subprocess.run(
                [fv, "--data", DATA, "--content", os.path.join(BS, "content"),
                 "--veh", os.path.join(DATA, iqm), "--gltf", f"/tmp/_probe_{tag}",
                 "--t", f"{t:.4f}"], capture_output=True, timeout=180)
            try:
                probe[tag] = head_z(f"/tmp/_probe_{tag}")
            except Exception:
                probe[tag] = None
        if probe.get("v") is not None and probe.get("r") is not None:
            if probe["v"] * probe["r"] < 0:
                turn = 180.0

        good = True
        for k in range(POSES):
            t = k / (POSES - 1) if POSES > 1 else 0.0
            r = subprocess.run(
                [fv, "--data", DATA, "--content", os.path.join(BS, "content"),
                 "--veh", os.path.join(DATA, iqm),
                 "--gltf", os.path.join(d, f"f{k:02d}"), "--gltf-merge",
                 "--t", f"{t:.4f}"],
                capture_output=True, text=True, timeout=180)
            if r.returncode != 0 or not os.path.isfile(os.path.join(d, f"f{k:02d}", "scene.gltf")):
                good = False
                fail.append((name, (r.stderr or "").strip().splitlines()[-1:] or ["?"]))
                break
            if turn:
                yaw_pose(os.path.join(d, f"f{k:02d}"), turn * smoothstep(t))
        if good:
            ok += 1
        tag = f"turn {turn:.0f}deg" if turn else "aligned"
        print(f"  [{i:2d}/{len(todo)}] {name:22s} {'ok' if good else 'FAILED'}  {tag}")
    print(f"\n{ok} exported, {len(fail)} failed, {time.time()-t0:.0f}s")
    for n, msg in fail:
        print(f"  {n}: {msg[0][:100]}")


if __name__ == "__main__":
    main()
