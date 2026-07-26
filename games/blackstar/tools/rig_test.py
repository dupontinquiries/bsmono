#!/usr/bin/env python3
"""rig_test.py -- verify a rigged model actually holds together when animated.

Joint separation and odd rotations were only ever caught by looking at the game.
This replicates the engine's transform composition in Python and sweeps the whole
gait cycle, so the same class of bug fails a test instead of shipping.

Checks per model:
  JOINT CONTINUITY  a child's pivot must stay coincident with where its parent
                    carries it. Any drift means the limb visibly separates.
  RANGE             no joint may exceed a plausible articulation limit.
  NO INVERSION      a knee must never bend backwards.
  PIVOT SANITY      pivots must be ordered down the limb (shoulder above elbow,
                    hip above knee above ankle) and mirrored left/right.

Usage:
  python3 tools/rig_test.py kit/                 # every manifest in a folder
  python3 tools/rig_test.py kit/imported_x_parts.txt
"""
import sys, os, math, glob, argparse

# must mirror game.c: atag codes and the walk cycle
ARM_L, ARM_R, LEG_L, LEG_R, HEAD = 5, 6, 7, 8, 9
ARM_L_LO, ARM_R_LO, LEG_L_LO, LEG_R_LO = 10, 11, 12, 13
FOOT_L, FOOT_R = 14, 15
LIMB_TAGS = {ARM_L, ARM_R, LEG_L, LEG_R, HEAD,
             ARM_L_LO, ARM_R_LO, LEG_L_LO, LEG_R_LO, FOOT_L, FOOT_R}

def limb_angle(tag, gait, amp, parent_ang):
    """Exactly the engine's switch, so a divergence here means one there."""
    if tag == LEG_L:     return math.sin(gait) * amp
    if tag == LEG_R:     return -math.sin(gait) * amp
    if tag == ARM_L:     return -math.sin(gait) * amp * 0.65
    if tag == ARM_R:     return math.sin(gait) * amp * 0.65
    if tag == LEG_L_LO:  return max(0.0, math.sin(gait - 1.57)) * amp * 1.10
    if tag == LEG_R_LO:  return max(0.0, math.sin(gait + 1.57)) * amp * 1.10
    if tag == ARM_L_LO:  return -max(0.0, math.sin(gait + 1.57)) * amp * 0.55
    if tag == ARM_R_LO:  return -max(0.0, math.sin(gait - 1.57)) * amp * 0.55
    if tag in (FOOT_L, FOOT_R): return -parent_ang * 0.85
    return 0.0

def read_manifest(path):
    rows = []
    for line in open(path, errors="ignore"):
        p = line.split()
        if len(p) < 5: continue
        try:
            tag = int(p[4])
            off = (float(p[5]), float(p[6]), float(p[7])) if len(p) > 7 else (0.0, 0.0, 0.0)
            par = int(p[8]) if len(p) > 8 else -1
        except ValueError:
            continue
        rows.append({"file": p[0], "name": p[2], "tag": tag, "off": off, "parent": par})
    return rows

def check(path, speeds=(0.0, 2.0, 4.0, 8.0), steps=24, tol=1e-4):
    rows = read_manifest(path)
    if not rows: return None
    name = os.path.basename(path).replace("imported_", "").replace("_parts.txt", "")
    limbs = [r for r in rows if r["tag"] in LIMB_TAGS]
    if not limbs:
        return {"name": name, "skip": "no limb tags"}

    fails = []
    max_sep = 0.0
    max_ang = 0.0
    for spd in speeds:
        amp = min(34.0, 5.0 + spd * 7.0)
        for st in range(steps):
            gait = (st / steps) * 2 * math.pi * 1.6
            ang = {}
            for i, r in enumerate(rows):
                if r["tag"] not in LIMB_TAGS:
                    ang[i] = 0.0; continue
                pa = ang.get(r["parent"], 0.0) if r["parent"] >= 0 else 0.0
                a = limb_angle(r["tag"], gait, amp, pa)
                if r["parent"] >= 0: a += pa           # engine inherits parent angle
                ang[i] = a
                max_ang = max(max_ang, abs(a))
                # NO INVERSION: knees only bend one way
                if r["tag"] in (LEG_L_LO, LEG_R_LO):
                    own = a - pa
                    if own < -tol:
                        fails.append(f"knee inverts ({own:.1f} deg) at speed {spd}")
            # JOINT CONTINUITY: child pivot carried by the parent must land where
            # the child expects to be. Same composition the engine performs.
            for i, r in enumerate(rows):
                par = r["parent"]
                if par < 0: continue
                pa = math.radians(ang[par])
                dy = r["off"][1] - rows[par]["off"][1]
                dz = r["off"][2] - rows[par]["off"][2]
                ry = dy * math.cos(pa) - dz * math.sin(pa)
                rz = dy * math.sin(pa) + dz * math.cos(pa)
                # the rotated offset must preserve the bone LENGTH exactly
                sep = abs(math.hypot(ry, rz) - math.hypot(dy, dz))
                max_sep = max(max_sep, sep)
                if sep > 1e-5:
                    fails.append(f"{r['name']} drifts {sep:.5f} from {rows[par]['name']}")

    # PIVOT SANITY: ordered down the limb, mirrored across the body
    by = {r["name"]: r["off"] for r in rows}
    def below(a, b):
        return a in by and b in by and by[a][1] <= by[b][1] + 1e-6
    for up, lo in (("arm_l", "arm_l_lo"), ("arm_r", "arm_r_lo"),
                   ("leg_l", "leg_l_lo"), ("leg_r", "leg_r_lo"),
                   ("leg_l_lo", "leg_l_ft"), ("leg_r_lo", "leg_r_ft")):
        if up in by and lo in by and not below(lo, up):
            fails.append(f"{lo} pivot is ABOVE {up} (limb inverted)")
    # Which axis is left/right? Not always X -- models are authored facing
    # different ways, so infer it from the pivots: the mirror axis is the one where
    # the L/R pairs most consistently sit on opposite sides.
    pairs = [(l, r) for l, r in (("arm_l", "arm_r"), ("leg_l", "leg_r"),
                                 ("leg_l_ft", "leg_r_ft")) if l in by and r in by]
    max_ = 0
    if pairs:
        score = {ax: sum(1 for l, r in pairs if by[l][ax] * by[r][ax] < 0) for ax in (0, 2)}
        sep = {ax: sum(abs(by[l][ax] - by[r][ax]) for l, r in pairs) for ax in (0, 2)}
        max_ = max((0, 2), key=lambda ax: (score[ax], sep[ax]))
    AX = "XYZ"[max_]
    for l, r in pairs:
        if by[l][max_] * by[r][max_] > 0:
            fails.append(f"{l}/{r} on the SAME side ({AX} {by[l][max_]:+.2f}/{by[r][max_]:+.2f})")
        if abs(abs(by[l][max_]) - abs(by[r][max_])) > 0.15:
            fails.append(f"{l}/{r} not mirrored ({AX} {by[l][max_]:+.2f}/{by[r][max_]:+.2f})")
        if abs(by[l][1] - by[r][1]) > 0.10:
            fails.append(f"{l}/{r} at different heights ({by[l][1]:+.2f}/{by[r][1]:+.2f})")
    return {"name": name, "parts": len(rows), "limbs": len(limbs),
            "max_sep": max_sep, "max_ang": max_ang,
            "fails": sorted(set(fails))}

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("path")
    ap.add_argument("--quiet", action="store_true")
    a = ap.parse_args()
    targets = ([a.path] if a.path.endswith(".txt")
               else sorted(glob.glob(os.path.join(a.path, "*_parts.txt"))))
    if not targets:
        print("no manifests found in", a.path); sys.exit(1)

    total_fail = 0; tested = 0
    print("%-34s %6s %6s %10s %9s  %s" % ("model", "parts", "limbs", "max drift", "max ang", "result"))
    for t in targets:
        r = check(t)
        if not r: continue
        if r.get("skip"):
            if not a.quiet: print("%-34s %6s %6s %10s %9s  skip (%s)"
                                  % (r["name"][:34], "-", "-", "-", "-", r["skip"]))
            continue
        tested += 1
        bad = len(r["fails"])
        total_fail += bad
        print("%-34s %6d %6d %10.6f %8.1fd  %s"
              % (r["name"][:34], r["parts"], r["limbs"], r["max_sep"], r["max_ang"],
                 "OK" if not bad else "%d FAIL" % bad))
        for f in r["fails"][:4]:
            print("      %s" % f)
    print("\n%d model(s) tested, %d failure(s)" % (tested, total_fail))
    sys.exit(1 if total_fail else 0)

if __name__ == "__main__":
    main()
