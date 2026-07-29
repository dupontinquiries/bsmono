#!/usr/bin/env python3
"""derive_web.py — recover a character's transformation from both of its models.

Brawl exists twice in this corpus: a tank and a robot. That is ground truth. If a
component appears in both, the motion it undergoes is not something to invent —
it is the rigid transform that carries one onto the other, and Kabsch recovers it
exactly.

    match components by rotation-invariant shape  ->  solve each pair  ->  the web

What comes out is a real per-part motion list: pivot, axis, angle. Parts that
appear in only one form are the ones that hide inside the other, and they are
reported rather than guessed at.
"""
import sys, math, os, json, collections
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from transform_collage import load_iqm, weld, components, bbox, centre

def sig(pts, q=None):
    """SCALE-NORMALISED shape signature.

    The first version quantised in absolute units, which silently assumed both
    models were authored at the same scale. Brawl's are not - the robot is
    2.4x denser and differently sized - so identical parts hashed differently
    and nothing matched at all. Dividing the distance profile by its own mean
    makes the key invariant to scale as well as to rotation and translation,
    and the same parts then land in the same bucket."""
    c = centre(*bbox(pts))
    d = sorted(math.dist(p, c) for p in pts)
    n = len(d)
    take = min(12, n)
    prof = [d[i*(n-1)//max(take-1,1)] for i in range(take)]
    m = sum(prof)/len(prof) or 1.0
    return (n, tuple(round(x/m, 2) for x in prof))

def kabsch(A, B):
    """Rigid transform carrying A onto B. Same maths as model-verify's, in the
    language the collage tool already speaks."""
    n = len(A)
    ca = [sum(p[i] for p in A)/n for i in range(3)]
    cb = [sum(p[i] for p in B)/n for i in range(3)]
    H = [[0.0]*3 for _ in range(3)]
    for a, b in zip(A, B):
        for i in range(3):
            for j in range(3):
                H[i][j] += (a[i]-ca[i]) * (b[j]-cb[j])
    R = [[H[j][i] for j in range(3)] for i in range(3)]
    for _ in range(64):
        det = (R[0][0]*(R[1][1]*R[2][2]-R[1][2]*R[2][1])
             - R[0][1]*(R[1][0]*R[2][2]-R[1][2]*R[2][0])
             + R[0][2]*(R[1][0]*R[2][1]-R[1][1]*R[2][0]))
        if abs(det) < 1e-14: return None
        inv = [[0.0]*3 for _ in range(3)]
        inv[0][0]=(R[1][1]*R[2][2]-R[1][2]*R[2][1])/det; inv[0][1]=(R[0][2]*R[2][1]-R[0][1]*R[2][2])/det
        inv[0][2]=(R[0][1]*R[1][2]-R[0][2]*R[1][1])/det; inv[1][0]=(R[1][2]*R[2][0]-R[1][0]*R[2][2])/det
        inv[1][1]=(R[0][0]*R[2][2]-R[0][2]*R[2][0])/det; inv[1][2]=(R[0][2]*R[1][0]-R[0][0]*R[1][2])/det
        inv[2][0]=(R[1][0]*R[2][1]-R[1][1]*R[2][0])/det; inv[2][1]=(R[0][1]*R[2][0]-R[0][0]*R[2][1])/det
        inv[2][2]=(R[0][0]*R[1][1]-R[0][1]*R[1][0])/det
        nxt = [[0.5*(R[i][j] + inv[j][i]) for j in range(3)] for i in range(3)]
        if sum(abs(nxt[i][j]-R[i][j]) for i in range(3) for j in range(3)) < 1e-14:
            R = nxt; break
        R = nxt
    t = [cb[i] - sum(R[i][k]*ca[k] for k in range(3)) for i in range(3)]
    return R, t

def axis_angle_of(R):
    """Rotation as an axis and an angle — what a JOINT is, as opposed to a matrix."""
    tr = R[0][0] + R[1][1] + R[2][2]
    ang = math.acos(max(-1.0, min(1.0, (tr - 1.0) / 2.0)))
    if ang < 1e-7: return [0.0, 0.0, 1.0], 0.0
    s = 2.0 * math.sin(ang)
    ax = [(R[2][1]-R[1][2])/s, (R[0][2]-R[2][0])/s, (R[1][0]-R[0][1])/s]
    l = math.sqrt(sum(x*x for x in ax)) or 1.0
    return [x/l for x in ax], ang

def load_parts(path, min_verts=24):
    v, t = load_iqm(path)
    if not v: return None, []
    span = max(hi-lo for lo, hi in zip(*[bbox(v)[0:1][0:1][0] and bbox(v)[0], bbox(v)[1]])) if v else 1.0
    lo, hi = bbox(v)
    span = max(hi[i]-lo[i] for i in range(3)) or 1.0
    remap = weld(v, span * 1e-5)
    comps = [c for c in components(len(v), t, remap) if len(c) >= min_verts]
    return v, comps

def derive(tank_path, robot_path, out_json):
    tv, tc = load_parts(tank_path)
    rv, rc = load_parts(robot_path)
    if not tv or not rv: return None
    span = max(bbox(tv)[1][i] - bbox(tv)[0][i] for i in range(3)) or 1.0

    # index the robot's components by shape so matching is a lookup, not a scan
    rindex = collections.defaultdict(list)
    for ri, c in enumerate(rc):
        rindex[sig([rv[i] for i in c])].append(ri)

    joints, unmatched_t = [], 0
    used = set()
    for ti, c in enumerate(tc):
        pts = [tv[i] for i in c]
        k = sig(pts)
        cands = [r for r in rindex.get(k, []) if r not in used]
        if not cands:
            unmatched_t += 1
            continue
        ri = cands[0]; used.add(ri)
        B = [rv[i] for i in rc[ri]]
        if len(B) != len(pts):
            unmatched_t += 1; continue
        sol = kabsch(pts, B)
        if not sol: continue
        R, t = sol
        ax, ang = axis_angle_of(R)
        # the two models are at different scales; record the ratio instead of
        # letting it contaminate the recovered rotation
        sa = max(bbox(pts)[1][i]-bbox(pts)[0][i] for i in range(3)) or 1.0
        sb = max(bbox(B)[1][i]-bbox(B)[0][i] for i in range(3)) or 1.0
        joints.append({
            "tank_comp": ti, "robot_comp": ri, "verts": len(pts),
            "pivot": centre(*bbox(pts)), "axis": ax, "angle": ang,
            "translate": t, "scale": sb/sa,
        })
    res = {
        "tank_components": len(tc), "robot_components": len(rc),
        "matched": len(joints), "unmatched_tank": unmatched_t,
        "joints": joints,
    }
    json.dump(res, open(out_json, 'w'), indent=1)
    return res

if __name__ == '__main__':
    tank, robot, out = sys.argv[1], sys.argv[2], (sys.argv[3] if len(sys.argv) > 3 else 'web.json')
    r = derive(tank, robot, out)
    if not r:
        print("could not load"); sys.exit(1)
    print(f"tank {r['tank_components']} comps, robot {r['robot_components']} comps")
    print(f"MATCHED {r['matched']} parts  ({r['unmatched_tank']} tank parts had no twin)")
    angs = sorted((j["angle"] for j in r["joints"]), reverse=True)
    if angs:
        print(f"rotations: max {math.degrees(angs[0]):.0f}deg  median {math.degrees(angs[len(angs)//2]):.0f}deg")
        big = sum(1 for a in angs if a > math.radians(30))
        print(f"  {big} parts swing more than 30deg — those are the visible transformation")
    print(f"-> {out}")
