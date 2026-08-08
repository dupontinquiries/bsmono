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
            # THE 1.6 GATE WAS ARBITRARY AND WAS REJECTING GOOD MODELS.
            # Sampling 20 grimorph rejects: they rose 1.10-1.66x and stood
            # upright, and fifteen of twenty were thrown out purely for not
            # growing enough. A fold that adds a quarter to a body's height
            # and leaves it taller than it is long HAS stood up.
            # THE UPRIGHT GATE WAS A BIPED ASSUMPTION, and it made a whole
            # category invisible. The library's own metadata lists spider 7,
            # beast 2, avian 2, dragon 1 — body plans that fold into
            # something LOW AND LONG rather than tall. Requiring the robot to
            # be taller than it is wide rejects every one of them, which is
            # why measuring ground contacts never found a quadruped: they
            # were filtered out before any measurement ran.
            #
            # A fold is a fold if the body CHANGED SHAPE. Growing upward is
            # one way; a spider unfolding legs outward is another, and it
            # shows as a large change in the extent ratios rather than in
            # height alone.
            grew = r[1] > v[1] * 1.25
            upright = r[1] >= max(r[0], r[2]) * 0.90
            # how much the proportions moved, regardless of direction
            def shape(e):
                m = max(e[0], e[1], e[2]) or 1.0
                return (e[0] / m, e[1] / m, e[2] / m)
            sv, sr = shape(v), shape(r)
            reshaped = sum(abs(a - b) for a, b in zip(sv, sr)) > 0.35
            # sprawling: wider or longer than it was, and it opened up
            sprawled = (max(r[0], r[2]) > max(v[0], v[2]) * 1.25) and reshaped
            if (grew and upright) or sprawled:
                good.append(iqm)
                verdict = "folds"
            else:
                bad.append(iqm)
                verdict = f"no ({'flat' if not grew else 'not upright'}, no sprawl)"
        if i % 20 == 0 or verdict.startswith("folds"):
            print(f"  [{i:3d}/{len(cands)}] {name[:44]:44s} {verdict}")
    dt = time.time() - t0
    print(f"\n{len(good)} fold, {len(bad)} do not, {len(err)} errored, {dt:.0f}s")
    json.dump(good, open("/tmp/foldable.json", "w"), indent=1)
    print("kept list -> /tmp/foldable.json")


if __name__ == "__main__":
    main()
