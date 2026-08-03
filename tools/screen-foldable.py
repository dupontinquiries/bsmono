#!/usr/bin/env python3
"""screen-foldable.py — which models actually fold, measured rather than assumed.

The roster shipped 28 chassis: the hand-tuned entries in
foldview_tuning.json. But `engine_src/data` holds over a thousand imports,
and the tuning file is a list of models someone sat down with, not a list of
models that work. Plenty of untuned ones fold perfectly well.

The gate is functional, not cosmetic. Export the vehicle pose and the robot
pose, and ask whether the thing actually STOOD UP:

  * the robot's height must exceed the vehicle's by a clear margin, and
  * the robot's height must be its own longest axis (a body that got taller
    but is still longest front-to-back is lying on a diagonal, not standing)

A model welded into one island cannot fold — foldview's segmentation has
nothing to separate — and it fails both tests by staying the shape it was.
That is exactly what we want to filter, and it costs one export pair to find
out instead of a judgement call.

    ./tools/screen-foldable.py [category ...]     # default: the vehicle-ish ones
"""
import json, os, re, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
SKY = os.path.dirname(HERE)
BS = os.path.join(SKY, "..", "blackstar")
FV = os.path.join(BS, "rust", "foldview", "target", "release", "foldview")
DATA = os.path.join(BS, "engine_src", "data")
CONTENT = os.path.join(BS, "content")

CATS = sys.argv[1:] or [
    "flyer", "car", "tank", "spaceship", "hoverer", "hovertank",
    "racer", "truck", "mech", "boat",
]
# foldview's README lists these as welded into a single island
WELDED = ("akimbo", "echotron", "reverser", "batwing", "tempest")


def extents(d):
    g = json.load(open(os.path.join(d, "scene.gltf")))
    lo = [1e30] * 3
    hi = [-1e30] * 3
    for a in g["accessors"]:
        if "min" in a and len(a["min"]) == 3:
            for k in range(3):
                lo[k] = min(lo[k], a["min"][k])
                hi[k] = max(hi[k], a["max"][k])
    return [hi[k] - lo[k] for k in range(3)]


def pose(iqm, t, out):
    r = subprocess.run(
        [FV, "--data", DATA, "--content", CONTENT, "--veh", os.path.join(DATA, iqm),
         "--gltf", out, "--gltf-merge", "--t", f"{t:.2f}"],
        capture_output=True, text=True, timeout=240)
    if r.returncode != 0 or not os.path.isfile(os.path.join(out, "scene.gltf")):
        return None
    return extents(out)


def main():
    cands = []
    for c in CATS:
        for f in sorted(os.listdir(DATA)):
            if f.startswith(f"imported_{c}_") and f.endswith(".iqm"):
                if not any(w in f for w in WELDED):
                    cands.append(f)
    print(f"screening {len(cands)} candidates across {len(CATS)} categories\n")

    good, bad, err = [], [], []
    t0 = time.time()
    for i, iqm in enumerate(cands, 1):
        v = pose(iqm, 0.0, "/tmp/_s0")
        r = pose(iqm, 1.0, "/tmp/_s1")
        name = re.sub(r"^imported_", "", iqm).replace(".iqm", "")
        if v is None or r is None:
            err.append(iqm)
            verdict = "ERROR"
        else:
            grew = r[1] > v[1] * 1.6
            upright = r[1] >= max(r[0], r[2]) * 0.95
            if grew and upright:
                good.append(iqm)
                verdict = "folds"
            else:
                bad.append(iqm)
                verdict = f"no ({'flat' if not grew else 'not upright'})"
        if i % 20 == 0 or verdict.startswith("folds"):
            print(f"  [{i:3d}/{len(cands)}] {name[:44]:44s} {verdict}")
    dt = time.time() - t0
    print(f"\n{len(good)} fold, {len(bad)} do not, {len(err)} errored, {dt:.0f}s")
    json.dump(good, open("/tmp/foldable.json", "w"), indent=1)
    print("kept list -> /tmp/foldable.json")


if __name__ == "__main__":
    main()
