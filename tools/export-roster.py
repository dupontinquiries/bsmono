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


def robot_yaw(d):
    """Degrees the robot pose must turn to face -Z, from its own anatomy.

    An earlier version of this read the head/visor part's Z SIGN, which was
    wrong twice over: a folded-away head is not the nose, and a sign test
    cannot tell 90 degrees from 180. Measured properly — left/right limb pair
    gives the lateral axis, cross it with +Y for forward — every chassis in
    the roster already faces -Z to within 7 degrees, so this returns ~0 and
    nothing is rotated. It stays because the measurement is the thing worth
    keeping: if a future import does arrive turned, this catches it.
    """
    g = json.load(open(os.path.join(d, "scene.gltf")))
    buf = open(os.path.join(d, "scene.bin"), "rb").read()
    cent = {}
    for n in g["nodes"]:
        m = re.match(r"[a-z]+", n.get("name", ""))
        if not m:
            continue
        a = g["accessors"][g["meshes"][n["mesh"]]["primitives"][0]["attributes"]["POSITION"]]
        bv = g["bufferViews"][a["bufferView"]]
        off = bv.get("byteOffset", 0) + a.get("byteOffset", 0)
        sx = sz = 0.0
        for i in range(a["count"]):
            x, _, z = struct.unpack_from("<3f", buf, off + i * 12)
            sx += x
            sz += z
        c = a["count"] or 1
        cent.setdefault(m.group(0), []).append((sx / c, sz / c))
    avg = {k: (sum(p[0] for p in v) / len(v), sum(p[1] for p in v) / len(v))
           for k, v in cent.items()}
    for l, r in (("uarml", "uarmr"), ("clavl", "clavr"), ("shinl", "shinr"), ("thighl", "thighr")):
        if l in avg and r in avg:
            lx, lz = avg[l][0] - avg[r][0], avg[l][1] - avg[r][1]
            n = math.hypot(lx, lz)
            if n < 1e-4:
                continue
            lx, lz = lx / n, lz / n
            # forward = lateral x up, with up = +Y
            fx, fz = -lz, lx
            return math.degrees(math.atan2(fx, -fz))
    return None


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


def load_overrides():
    """Human verdicts about which way a chassis faces."""
    p = os.path.join(HERE, "orient-overrides.json")
    try:
        return json.load(open(p)).get("overrides", {})
    except Exception:
        return {}


OVERRIDES = load_overrides()


def main():
    # ROSTER SOURCE. foldview_tuning.json is a list of models someone sat
    # down and tuned, not a list of models that work — and there are over a
    # thousand imports. tools/screen-foldable.py measures which actually
    # stand up (robot height clearly above vehicle height, AND height as its
    # own longest axis, so a body on a diagonal does not pass). Its output
    # is used when present; the tuned list is the fallback.
    screened = os.environ.get("ROSTER", "/tmp/foldable.json")
    if os.path.isfile(screened):
        tuning = json.load(open(screened))
        print(f"roster from {screened}: {len(tuning)} screened chassis")
    else:
        tuning = json.load(open(os.path.join(FV, "foldview_tuning.json")))
        print(f"roster from foldview_tuning.json: {len(tuning)} tuned chassis")
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
        # DECLARED CORRECTIONS FIRST. A vehicle's nose is not reliably
        # measurable (see tools/orient-overrides.json), so a human verdict
        # beats a heuristic and this is where it goes.
        ov = OVERRIDES.get(name, {})
        flat_yaw = float(ov.get("yaw", 0.0))
        turn = float(ov.get("turn", 0.0))
        if turn == 0.0 and flat_yaw == 0.0:
            subprocess.run(
            [fv, "--data", DATA, "--content", os.path.join(BS, "content"),
             "--veh", os.path.join(DATA, iqm), "--gltf", "/tmp/_probe_r", "--t", "1.0"],
            capture_output=True, timeout=180)
            try:
                y = robot_yaw("/tmp/_probe_r")
            except Exception:
                y = None
            # only act on a real misalignment; a few degrees is pose, not
            # framing. Measured across the roster this fires on nothing —
            # every robot already lands within 11 degrees of -Z — but it
            # stays in for an import that does arrive turned.
            if y is not None and abs(y) > 20.0:
                turn = round(y / 90.0) * 90.0

        good = True
        for k in range(POSES):
            t = k / (POSES - 1) if POSES > 1 else 0.0
            # --repair --reattach on EVERY pose: they change the joint tree,
            # so a strip where only one pose got them would fold one way at
            # f04 and another at f00, and the morph blends between poses that
            # no longer describe the same body.
            cmd = [fv, "--data", DATA, "--content", os.path.join(BS, "content"),
                   "--veh", os.path.join(DATA, iqm),
                   "--gltf", os.path.join(d, f"f{k:02d}"), "--gltf-merge",
                   "--repair", "--reattach",
                   "--t", f"{t:.4f}"]
            # THE JOINTS, ONCE PER CHASSIS. They are a property of the rig
            # rather than of a pose, so the robot end writes them and the
            # engine loads one file per bot. --repair refits pivots to their
            # seam first, so what ships is the corrected set.
            if k == POSES - 1:
                cmd += ["--joints-json", os.path.join(d, "joints.json")]
            r = subprocess.run(cmd, capture_output=True, text=True, timeout=180)
            if r.returncode != 0 or not os.path.isfile(os.path.join(d, f"f{k:02d}", "scene.gltf")):
                good = False
                fail.append((name, (r.stderr or "").strip().splitlines()[-1:] or ["?"]))
                break
            # flat_yaw turns the whole strip; turn spends the rotation
            # across the fold. They compose, so a chassis can do both.
            deg = flat_yaw + turn * smoothstep(t)
            if deg:
                yaw_pose(os.path.join(d, f"f{k:02d}"), deg)
        if good:
            ok += 1
        tag = ", ".join(
            [x for x in (f"yaw {flat_yaw:.0f}" if flat_yaw else "",
                         f"turn {turn:.0f}" if turn else "") if x]
        ) or "as exported"
        print(f"  [{i:2d}/{len(todo)}] {name:22s} {'ok' if good else 'FAILED'}  {tag}")
    print(f"\n{ok} exported, {len(fail)} failed, {time.time()-t0:.0f}s")
    for n, msg in fail:
        print(f"  {n}: {msg[0][:100]}")


if __name__ == "__main__":
    main()
