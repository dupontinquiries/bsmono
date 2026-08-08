#!/usr/bin/env python3
"""facing.py -- which end of a vehicle is the nose, from its silhouette.

    ./tools/facing.py imported_car_*.iqm

THE REASONING. A road car's roofline is not symmetric: the bonnet rises
GENTLY from a low nose to the windscreen, and the tail falls away SHARPLY
behind the cabin. That asymmetry is a design consequence rather than a
style — the front has to be long and low to get air over the car and to put
a crumple zone somewhere, and the back is cut off because a true teardrop
tail would add a metre nobody wants to park.

So: find the roof peak, measure the mean slope either side, and the GENTLER
side is the nose.

WHAT THE MEASUREMENT ACTUALLY SAID, and why this is a tool rather than a
pass in the exporter: across ten real road cars it agrees seven times. That
is a signal and it is not a rule. Seven-of-ten has two readings and this
cannot tell them apart:

  the rule is 70% accurate, or
  the rule is right and THREE MODELS ARE BACKWARDS IN THE FILE

The second is not a stretch — backwards vehicles have been reported
repeatedly. Resolving it needs eyes on three specific models, which is why
this prints its disagreements by name instead of silently voting.
"""
import json, subprocess, shutil, sys, os
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
BS = os.path.join(HERE, "..", "..", "blackstar")
FV = os.path.join(BS, "rust", "foldview", "target", "release", "foldview")
DATA = os.path.join(BS, "engine_src", "data")


def points(m, t=0.0):
    shutil.rmtree("/tmp/_facing", ignore_errors=True)
    r = subprocess.run([FV, "--data", DATA, "--content", os.path.join(BS, "content"),
                        "--veh", os.path.join(DATA, m), "--repair",
                        "--gltf", "/tmp/_facing", "--t", str(t)],
                       capture_output=True, timeout=240)
    if r.returncode != 0:
        raise RuntimeError(m)
    g = json.load(open("/tmp/_facing/scene.gltf"))
    buf = open("/tmp/_facing/scene.bin", "rb").read()
    o = []
    for n in g["nodes"]:
        a = g["accessors"][g["meshes"][n["mesh"]]["primitives"][0]["attributes"]["POSITION"]]
        bv = g["bufferViews"][a["bufferView"]]
        off = bv.get("byteOffset", 0) + a.get("byteOffset", 0)
        o.append(np.frombuffer(buf, "<f4", count=a["count"] * 3, offset=off).reshape(-1, 3))
    return np.vstack(o).astype(float)


def nose_end(P, slices=14):
    """('low'|'high', front_slope, back_slope, confidence) along the long axis."""
    lo, hi = P.min(0), P.max(0)
    e = hi - lo
    ax = int(np.argmax(e))
    u = np.clip(((P[:, ax] - lo[ax]) / max(e[ax], 1e-9) * slices).astype(int), 0, slices - 1)
    h = []
    for i in range(slices):
        s = P[u == i]
        h.append(float(s[:, 1].max() - lo[1]) if len(s) >= 8 else np.nan)
    h = np.array(h)
    ok = ~np.isnan(h)
    if ok.sum() < 6:
        return None
    idx = np.arange(slices)[ok]
    hv = h[ok] / max(np.nanmax(h), 1e-9)
    pk = idx[int(np.argmax(hv))]
    lo_side, hi_side = hv[idx < pk], hv[idx > pk]
    if len(lo_side) < 2 or len(hi_side) < 2:
        return None
    sf = abs((lo_side[-1] - lo_side[0]) / max(pk - idx[0], 1))
    sb = abs((hi_side[-1] - hi_side[0]) / max(idx[-1] - pk, 1))
    # confidence is how different the two slopes are; a symmetric body has
    # nothing to say and should say so rather than guessing
    conf = abs(sf - sb) / max(sf + sb, 1e-9)
    return ("low" if sf < sb else "high", sf, sb, conf)


def main():
    args = sys.argv[1:]
    if not args:
        args = [f for f in sorted(os.listdir(DATA)) if f.startswith("imported_car_")][:12]
    votes = {"low": [], "high": [], "unsure": []}
    for m in args:
        try:
            r = nose_end(points(m))
        except Exception:
            print(f"  {m[:44]:44s} ERR")
            continue
        if r is None:
            continue
        end, sf, sb, conf = r
        bucket = end if conf > 0.15 else "unsure"
        votes[bucket].append(m)
        print(f"  {m.replace('imported_','')[:40]:40s} nose {end:5s} "
              f"slopes {sf:.3f}/{sb:.3f} confidence {conf:.2f}")
    print(f"\nlow {len(votes['low'])}  high {len(votes['high'])}  unsure {len(votes['unsure'])}")
    minority = "high" if len(votes["low"]) >= len(votes["high"]) else "low"
    if votes[minority]:
        print(f"\nDISAGREE WITH THE MAJORITY — either the rule failed here or these "
              f"models are backwards in the file, and only eyes settle which:")
        for m in votes[minority]:
            print(f"  {m}")


if __name__ == "__main__":
    main()
