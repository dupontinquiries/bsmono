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
        if good:
            ok += 1
        print(f"  [{i:2d}/{len(todo)}] {name:22s} {'ok' if good else 'FAILED'}")
    print(f"\n{ok} exported, {len(fail)} failed, {time.time()-t0:.0f}s")
    for n, msg in fail:
        print(f"  {n}: {msg[0][:100]}")


if __name__ == "__main__":
    main()
