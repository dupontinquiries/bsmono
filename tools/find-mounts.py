#!/usr/bin/env python3
"""find-mounts.py — where does a weapon go on this vehicle?

    ./tools/find-mounts.py [out.json]

Reads each chassis's VEHICLE pose (t=0) unmerged, so foldview's part names
survive, and looks for three kinds of hardpoint. The engine convention is
+Y up and the nose at -Z.

  BARREL     One long thin thing pointing down the nose axis, sitting above
             hull mid-height. Found by aspect ratio — a barrel is the part
             whose longest extent is several times its width — and scored on
             how well its long axis lines up with Z. That is what makes a
             tank a tank, and it is also why a racer will not produce one:
             nothing on a racer is that shape.

  HARDPOINT  Under a wing, in mirrored pairs. Wings are named, so the search
             is for `wing` parts with real span, and the mount goes at the
             bottom of each. Pairs are matched by mirroring in X, because a
             lone hardpoint on one side is a detection error rather than an
             asymmetric aircraft.

  POPOUT     For racers: a spot on the upper hull, forward of centre, with
             nothing above it to foul the doors. Found by taking the top
             surface in the front half and keeping the points whose column
             is clear all the way up. Emitted as a mirrored pair, because a
             single one off the centreline looks like damage.

Every mount is a position plus an aim direction. Nothing here places a
weapon — it says where one COULD go, and the numbers are geometry rather
than taste, so a chassis nobody has looked at still gets sensible answers.
"""
import json, math, os, re, struct, subprocess, sys
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
SKY = os.path.dirname(HERE)
BS = os.path.join(SKY, "..", "blackstar")
FV = os.path.join(BS, "rust", "foldview")
DATA = os.path.join(BS, "engine_src", "data")
WELDED = {"akimbo", "echotron", "reverser", "batwing", "tempest"}


def parts(d):
    """[(name, Nx3 points)] from an unmerged foldview export."""
    g = json.load(open(os.path.join(d, "scene.gltf")))
    buf = open(os.path.join(d, "scene.bin"), "rb").read()
    out = []
    for n in g["nodes"]:
        a = g["accessors"][g["meshes"][n["mesh"]]["primitives"][0]["attributes"]["POSITION"]]
        bv = g["bufferViews"][a["bufferView"]]
        off = bv.get("byteOffset", 0) + a.get("byteOffset", 0)
        p = np.frombuffer(buf, "<f4", count=a["count"] * 3, offset=off).reshape(-1, 3)
        out.append((n.get("name", ""), p.astype(np.float64)))
    return out


# foldview names parts for their ROBOT role, and in vehicle mode most of
# those are folded up inside the shell. A search over every part finds
# struts: the first run returned `guts`, `spine`, `cable` and a `clavr` at
# 42:1 aspect, which is a wire, not a gun. So a barrel has to be OUTSIDE.
INTERNAL = ("guts", "bone", "muscle", "cable", "link", "piston", "vertebra",
            "rib", "coil", "core", "keel", "greeble", "spine", "gyro")


def shell_bounds(ps):
    """The cladding: what a barrel has to protrude from."""
    clad = [p for n, p in ps if n.startswith(("clad", "cap", "wing", "crest"))]
    if not clad:
        clad = [p for _, p in ps]
    a = np.vstack(clad)
    return a.min(0), a.max(0)


def find_turret(allp, mn, ext):
    """Where a barrel SHOULD go, rather than where one already is.

    Two passes at finding an existing barrel by part name both failed, and
    the reason is structural rather than fixable: foldview names parts for
    their ROBOT role, so a vehicle's gun is not a part called `barrel` — it
    is a slab of `clad`, or it is not modelled separately at all. Pass one
    returned `guts` and `spine`; pass two, after requiring parts to protrude
    past the cladding, returned `thighr` and `shinl`, which are folded LEGS
    poking out of the nose. No tank got a barrel either time.

    So this stops looking for a gun and picks a mount from the body's shape,
    which is the question that was actually asked. The turret is the highest
    broad plateau near the centreline; the barrel comes off its front edge
    at plateau height, on the axis. That is right whether or not the model
    has a gun, and it is the same answer a human would give with a ruler.
    """
    spine = allp[np.abs(allp[:, 0]) < ext[0] * 0.22]
    if len(spine) < 30:
        spine = allp
    # the plateau: the top band of the centreline, ignoring a lone aerial
    top = np.percentile(spine[:, 1], 97)
    band = spine[spine[:, 1] > top - ext[1] * 0.18]
    if len(band) < 8:
        return None
    y = float(np.percentile(band[:, 1], 60))
    # front edge of the plateau, then out to the hull nose
    z = float(np.percentile(band[:, 2], 8))
    return {"pos": [0.0, round(y, 4), round(z, 4)],
            "aim": [0, 0, -1],
            "plateau": round(float(len(band)) / len(spine), 3)}


def find_barrel(ps, hull_mid_y, shell_min, shell_ext):
    """Legacy name-based search. Kept because it is the evidence for why the
    shape-based one exists; it is no longer called."""
    best = None
    for name, p in ps:
        if name.startswith(INTERNAL):
            continue
        if len(p) < 8:
            continue
        ext = p.max(0) - p.min(0)
        order = np.argsort(-ext)
        long_ax, w1, w2 = ext[order[0]], ext[order[1]], ext[order[2]]
        girth = max((w1 + w2) * 0.5, 1e-6)
        aspect = long_ax / girth
        if aspect < 3.0 or long_ax < 0.25:
            continue
        # must point roughly along Z, not across the hull
        if order[0] != 2:
            continue
        c = p.mean(0)
        if c[1] < hull_mid_y:
            continue  # a barrel is not slung under the floor
        score = aspect * long_ax
        if best is None or score > best[0]:
            # the muzzle is the forward tip: -Z is the nose
            tip = p[np.argmin(p[:, 2])]
            # the muzzle must be at or ahead of the cladding's nose: a gun
            # that ends inside the bodywork is a chassis member
            if tip[2] > shell_min[2] + shell_ext[2] * 0.04:
                continue
            best = (score, name, tip, aspect, long_ax)
    return best


def find_hardpoints(ps):
    """Under each wing, matched into mirrored pairs."""
    cands = []
    for name, p in ps:
        if not name.startswith("wing") or len(p) < 8:
            continue
        ext = p.max(0) - p.min(0)
        if ext[0] < 0.15:
            continue  # no span: a fin, not a wing
        c = p.mean(0)
        cands.append((float(c[0]), [float(c[0]), float(p[:, 1].min()), float(c[2])]))
    # A PAIR HAS TO BE A MIRROR, in all three axes. Matching on |x| alone
    # paired racer_grit's spoiler with something 1.2 units further forward
    # and 0.1 higher, which is two unrelated parts that happen to be off
    # centre. Y and Z have to agree too, or it is not the same hardpoint on
    # the other wing.
    out = []
    for x, pos in cands:
        if x == 0:
            continue
        tol = abs(x) * 0.35 + 0.04
        if any(xx * x < 0
               and abs(abs(xx) - abs(x)) < tol
               and abs(o[1] - pos[1]) < tol
               and abs(o[2] - pos[2]) < tol
               for xx, o in cands):
            out.append(pos)
    return out


def find_popouts(all_pts, ext, mn):
    """Upper hull, front half, with a clear column above."""
    z_mid = mn[2] + ext[2] * 0.5
    front = all_pts[all_pts[:, 2] < z_mid]
    if len(front) < 40:
        return []
    top_y = front[:, 1].max()
    band = front[front[:, 1] > top_y - ext[1] * 0.22]
    if len(band) < 10:
        return []
    # off the centreline, so the pair straddles the spine
    side = band[np.abs(band[:, 0]) > ext[0] * 0.12]
    if len(side) < 6:
        side = band
    xs = np.abs(side[:, 0])
    x = float(np.percentile(xs, 70))
    z = float(np.median(side[:, 2]))
    y = float(np.percentile(side[:, 1], 85))
    return [[x, y, z], [-x, y, z]]


def profile(ps, allp, mn, ext, cat):
    """What this thing stands on, and whether it is the right way up.

    WHEELS ARE THE HONEST UP-TEST. Every earlier attempt inferred the frame
    from silhouette — extents, taper, head position — and all of them were
    weak or contradictory. A wheel is different: it is a part whose whole
    job is to touch the ground, so if the wheels are at the TOP of the hull
    the vehicle is upside down, and if they are all to one side it is on its
    side. No heuristic needed, just where they are.
    """
    wheels = [p for n, p in ps if n.startswith("wheel")]
    out = {"wheels": len(wheels)}
    if wheels:
        w = np.vstack(wheels)
        wc = w.mean(0)
        # where the wheel mass sits within the hull, 0 = floor, 1 = roof
        fy = float((wc[1] - mn[1]) / max(ext[1], 1e-6))
        fx = float(abs(wc[0] - (mn[0] + ext[0] * 0.5)) / max(ext[0], 1e-6))
        out["wheel_y"] = round(fy, 3)
        out["wheel_x_off"] = round(fx, 3)
        # CONFIDENCE MATTERS MORE THAN THE VERDICT. Only six chassis name
        # any part `wheel` at all and those name two, so a "sideways" call
        # off two wheels may just mean the naming caught one side's pair.
        # Four or more is a real axle set and worth acting on; two is a
        # flag to go and look.
        out["confidence"] = "firm" if len(wheels) >= 4 else "thin"
        if fx > 0.28:
            out["up"] = "SIDEWAYS?" if len(wheels) < 4 else "SIDEWAYS"
        elif fy > 0.62:
            out["up"] = "UPSIDE DOWN?" if len(wheels) < 4 else "UPSIDE DOWN"
        elif fy < 0.45:
            out["up"] = "ok"
        else:
            out["up"] = "unclear"
        # roadwheels in a line under a long hull read as treads; a handful of
        # big ones at the corners read as tyres
        span = float((w[:, 2].max() - w[:, 2].min()) / max(ext[2], 1e-6))
        out["wheel_span"] = round(span, 3)
        out["drive"] = "treads" if (len(wheels) >= 6 and span > 0.55) else "tyres"
    else:
        out["up"] = "no wheels"
        out["drive"] = None

    # no wheels at all: it either hovers, floats or burns something
    if out["drive"] is None:
        if cat in ("hoverer", "hovertank"):
            out["drive"] = "hover"
        elif cat == "tank":
            # treads are not named parts anywhere in the decomposition, so
            # a tank with no `wheel` parts runs on tracks by category. That
            # is an assumption, and it is recorded as one.
            out["drive"] = "treads"
            out["drive_src"] = "category"
        elif cat == "flyer":
            # a long body with real span glides on wings and is driven by
            # turbines; a compact one with no span is on thrust
            span = ext[0] / max(ext[2], 1e-6)
            out["drive"] = "turbine" if span > 0.75 else "thruster"
            out["span_ratio"] = round(float(span), 3)
        else:
            out["drive"] = "unknown"

    # a turret only makes sense on something that can carry one, and only
    # rotates if there is room to swing it
    if cat in ("tank", "hovertank"):
        top = allp[allp[:, 1] > mn[1] + ext[1] * 0.62]
        if len(top) > 20:
            # how round is the upper deck in plan? a square-ish footprint
            # can take a full traverse; a narrow spine cannot
            sx = float(top[:, 0].max() - top[:, 0].min())
            sz = float(top[:, 2].max() - top[:, 2].min())
            ratio = min(sx, sz) / max(max(sx, sz), 1e-6)
            out["turret_traverse"] = "full" if ratio > 0.55 else "limited"
            out["turret_ratio"] = round(ratio, 3)

    # thrust exits the back, low and near the centre — and only for the
    # things that actually have thrust. A hovercar has no nozzle and putting
    # one on it would be wrong in a way nobody could unsee.
    if out["drive"] in ("thruster", "turbine"):
        rear = allp[allp[:, 2] > mn[2] + ext[2] * 0.82]
        if len(rear) > 12:
            x = float(np.percentile(np.abs(rear[:, 0]), 55))
            y = float(np.percentile(rear[:, 1], 45))
            z = float(rear[:, 2].max())
            out["thrusters"] = [[round(x, 4), round(y, 4), round(z, 4)],
                                [round(-x, 4), round(y, 4), round(z, 4)]]
    return out


def main():
    out_path = sys.argv[1] if len(sys.argv) > 1 else os.path.join(HERE, "mounts.json")
    tuning = json.load(open(os.path.join(FV, "foldview_tuning.json")))
    fv = os.path.join(FV, "target", "release", "foldview")
    result = {}
    print(f"{'chassis':26s} {'drive':9s} {'up':>12s} {'whl':>4s} "
          f"{'turret':>7s} {'pyl':>4s} {'pop':>4s} {'thr':>4s}")
    for iqm in tuning:
        if any(w in iqm for w in WELDED) or not os.path.isfile(os.path.join(DATA, iqm)):
            continue
        nm = re.sub(r"_aka_.*$", "", re.sub(r"^imported_", "", iqm).replace(".iqm", ""))
        cat = nm.split("_")[0]
        subprocess.run([fv, "--data", DATA, "--content", os.path.join(BS, "content"),
                        "--veh", os.path.join(DATA, iqm), "--gltf", "/tmp/_mnt", "--t", "0.0"],
                       capture_output=True)
        try:
            ps = parts("/tmp/_mnt")
        except Exception:
            print(f"  {nm:26s} ERR")
            continue
        allp = np.vstack([p for _, p in ps])
        mn, mx = allp.min(0), allp.max(0)
        ext = mx - mn
        hull_mid_y = mn[1] + ext[1] * 0.5

        entry = {"mounts": []}
        entry["profile"] = profile(ps, allp, mn, ext, cat)
        for t in entry["profile"].get("thrusters", []):
            entry["mounts"].append({"kind": "thruster", "pos": t, "aim": [0, 0, 1]})
        # a gun platform for anything that fights from a hull
        b = None
        if cat in ("tank", "hovertank"):
            t = find_turret(allp, mn, ext)
            if t:
                b = True
                entry["mounts"].append({"kind": "barrel", **t})
        hps = find_hardpoints(ps)
        if not hps and cat == "flyer":
            # no named wings: hang pylons off the widest point of the hull,
            # which on a delta or a lifting body is where a wing would be
            wide = allp[allp[:, 1] < mn[1] + ext[1] * 0.55]
            if len(wide) > 20:
                x = float(np.percentile(np.abs(wide[:, 0]), 88))
                z = float(np.median(wide[:, 2]))
                y = float(np.percentile(wide[:, 1], 20))
                hps = [[x, y, z], [-x, y, z]]
        for hp in hps:
            entry["mounts"].append({"kind": "hardpoint",
                                    "pos": [round(v, 4) for v in hp], "aim": [0, 0, -1]})
        # popouts are a racer answer; a tank already has a gun
        if cat in ("racer", "hoverer", "flyer"):
            for po in find_popouts(allp, ext, mn):
                entry["mounts"].append({"kind": "popout",
                                        "pos": [round(v, 4) for v in po], "aim": [0, 0, -1]})
        result[nm] = entry
        pr = entry["profile"]
        nh = sum(1 for m in entry["mounts"] if m["kind"] == "hardpoint")
        np_ = sum(1 for m in entry["mounts"] if m["kind"] == "popout")
        nt = sum(1 for m in entry["mounts"] if m["kind"] == "thruster")
        print(f"  {nm:26s} {str(pr['drive']):9s} {pr['up']:>12s} {pr['wheels']:>4d} "
              f"{('yes' if b else '-'):>7s} {nh:>4d} {np_:>4d} {nt:>4d}")

    json.dump({"_doc": "generated by tools/find-mounts.py; positions are in "
                       "the vehicle pose's own space, +Y up, nose at -Z",
               "chassis": result}, open(out_path, "w"), indent=1)
    tot = sum(len(v["mounts"]) for v in result.values())
    print(f"\n{len(result)} chassis, {tot} mounts -> {out_path}")


if __name__ == "__main__":
    main()
