#!/usr/bin/env python3
"""orion_balance.py — mass, centre of mass, and whether the thing falls over.

## Why bother

"The legs need to be thicker or further forward" is a guess until there is a
number. A machine is stable when its centre of mass projects, in plan, inside
the polygon formed by whatever is touching the ground. That is the whole test,
it is cheap, and it turns a judgement into a measurement.

It also decides between the two fixes on offer: if the CoM falls BEHIND the
support polygon, the legs are too far back and moving them forward fixes it.
If it falls outside sideways, the stance is too narrow. If the polygon is
simply tiny, the feet are too small.

## Mass model

Mass proportional to length x radius^2 -- a solid rod. Radii are authored per
bone family rather than measured, because the cladding is what actually weighs
anything and it is not bound yet. The point is the RATIO: a torso outweighs a
forearm by roughly an order of magnitude and that is what moves the CoM.
"""
import math
from orion_rig import BONES, POSES, solve

# bone -> radius. A leg is thick because it carries everything above it; an
# arm is not. These are the numbers to change when the answer is "thicker legs".
RADIUS = {
    'pelvis': 15.0, 'spine': 14.0, 'chest': 17.0, 'neck': 6.0, 'head': 11.0,
    'hip_l': 10.0, 'hip_r': 10.0,
    'thigh_l': 13.0, 'thigh_r': 13.0,
    'shin_l': 11.0, 'shin_r': 11.0,
    'foot_l': 9.0,  'foot_r': 9.0,
    'clav_l': 8.0,  'clav_r': 8.0,
    'uparm_l': 10.0, 'uparm_r': 10.0,
    'forearm_l': 8.5, 'forearm_r': 8.5,
    'hand_l': 7.0,  'hand_r': 7.0,
}
DEFAULT_R = 8.0

# Radii scale with the bone lengths.
#
# `orion_rig.SCALE` multiplied every LENGTH by 1.40 and left the radii alone,
# so every limb got 40% longer at the same thickness. The machine was not just
# small before and correct after -- it went from stocky to spindly, which is
# the "parts read thin" complaint, and it also starved the scale armour: a
# foot 42 long with a radius of 9 gives scales 7 wide and 10 long, taller than
# wide, which is the one shape a scale must not be.
from orion_rig import SCALE as _RIG_SCALE

RADIUS = {k: v * _RIG_SCALE for k, v in RADIUS.items()}
DEFAULT_R = DEFAULT_R * _RIG_SCALE

# A joint is bearing weight when it is within this of the floor. Generous,
# because a foot's tail may be a few units up while its sole is down.
CONTACT_H = 22.0


def masses():
    m = {}
    for name, (parent, length, rest) in BONES.items():
        if length <= 0:
            continue
        r = RADIUS.get(name, DEFAULT_R)
        m[name] = length * r * r
    return m


def com(sol, M=None):
    """Centre of mass. Each bone's mass sits at its midpoint, which is right
    for a uniform rod and close enough for anything shaped like a limb."""
    M = M or masses()
    tot = 0.0
    acc = [0.0, 0.0, 0.0]
    for name, w in M.items():
        h, t, _ = sol[name]
        for a in range(3):
            acc[a] += w * (h[a] + t[a]) * 0.5
        tot += w
    return [acc[a] / tot for a in range(3)], tot


def contacts(sol, h=CONTACT_H):
    """Every joint endpoint near enough the floor to be carrying load."""
    pts = []
    for name, (head, tail, _) in sol.items():
        for p in (head, tail):
            if p[1] <= h:
                pts.append((p[0], p[2]))
    return pts


def hull(points):
    """Convex hull in plan (x, z). Andrew's monotone chain."""
    pts = sorted(set(points))
    if len(pts) <= 2:
        return pts
    def half(seq):
        out = []
        for p in seq:
            while len(out) >= 2:
                (ax, ay), (bx, by) = out[-2], out[-1]
                if (bx-ax)*(p[1]-ay) - (by-ay)*(p[0]-ax) > 0:
                    break
                out.pop()
            out.append(p)
        return out
    return half(pts)[:-1] + half(reversed(pts))[:-1]


def margin(point, poly):
    """Signed distance from `point` to the polygon edge, positive inside.

    This is the number that matters: it is how far the machine can be pushed,
    or how far its own mass can shift, before it tips."""
    if len(poly) < 3:
        return -1e9
    inside = True
    best = 1e30
    n = len(poly)
    for i in range(n):
        ax, ay = poly[i]
        bx, by = poly[(i+1) % n]
        ex, ey = bx-ax, by-ay
        cross = ex*(point[1]-ay) - ey*(point[0]-ax)
        if cross < 0:
            inside = False
        L = math.hypot(ex, ey) or 1e-9
        t = max(0.0, min(1.0, ((point[0]-ax)*ex + (point[1]-ay)*ey) / (L*L)))
        d = math.hypot(point[0]-(ax+ex*t), point[1]-(ay+ey*t))
        best = min(best, d)
    return best if inside else -best


def knee_flex(sol, side='l'):
    """Knee angle, and which way it bends.

    Positive = the shin swings BACKWARD relative to the thigh, which is how a
    knee works. Negative is hyperextension: the joint bending the wrong way,
    which reads as broken however efficient it might be on a machine."""
    th = sol['thigh_'+side]
    sh = sol['shin_'+side]
    tv = [th[1][a]-th[0][a] for a in range(3)]
    sv = [sh[1][a]-sh[0][a] for a in range(3)]
    tl = math.sqrt(sum(x*x for x in tv)) or 1
    sl = math.sqrt(sum(x*x for x in sv)) or 1
    dot = sum(tv[a]*sv[a] for a in range(3)) / (tl*sl)
    ang = math.degrees(math.acos(max(-1.0, min(1.0, dot))))
    # Sign from the cross product about the body's lateral axis. The two legs
    # are MIRRORED, so the same cross product reads opposite on each side --
    # the first version reported -26 and +26 for a symmetric pose and made a
    # symmetric rig look broken. Flipping the right side puts both on the same
    # convention: positive = flexes the way a knee flexes.
    cz = tv[2]*sv[0] - tv[0]*sv[2]
    if side == 'r':
        cz = -cz
    return ang if cz >= 0 else -ang


def report(pose_name, pose):
    sol = solve(pose)
    c, tot = com(sol)
    pts = contacts(sol)
    h = hull(pts)
    m = margin((c[0], c[2]), h)
    return {
        'pose': pose_name, 'com': c, 'mass': tot,
        'contacts': len(pts), 'hull': len(h), 'margin': m,
        'knee_l': knee_flex(sol, 'l'), 'knee_r': knee_flex(sol, 'r'),
        'com_y': c[1],
    }


if __name__ == '__main__':
    M = masses()
    tot = sum(M.values())
    print("mass share (a torso should outweigh a forearm by ~10x):")
    for k in sorted(M, key=lambda k: -M[k])[:8]:
        print(f"    {k:<10}{M[k]/tot*100:>6.1f}%")
    print()
    print(f"{'pose':<9}{'CoM x':>8}{'CoM y':>8}{'contacts':>10}{'margin':>9}"
          f"{'knee L':>9}{'knee R':>9}")
    for name, p in POSES:
        r = report(name, p)
        if r['hull'] < 3:
            flag = f"   only {r['hull']} contact pt(s) — no support polygon"
            mg = '  n/a'
        else:
            flag = '' if r['margin'] > 0 else '   TIPS OVER'
            mg = f"{r['margin']:>9.0f}"
        k = ''
        if min(r['knee_l'], r['knee_r']) < -4:
            k = '   KNEE BACKWARD'
        print(f"  {name:<7}{r['com'][0]:>8.0f}{r['com'][1]:>8.0f}"
              f"{r['contacts']:>10}{mg}"
              f"{r['knee_l']:>9.0f}{r['knee_r']:>9.0f}{flag}{k}")
    print()
    print("what is actually near the floor, per pose:")
    for name, p in POSES:
        sol = solve(p)
        low = sorted(((min(v[0][1], v[1][1]), k) for k, v in sol.items()))[:5]
        print(f"  {name:<8}" + "  ".join(f"{k} {y:.0f}" for y, k in low))


# ───────────────────────────────────────────── lever vs strongman

def reach_moment(sol, M=None):
    """How much torque the machine can put through its feet before it tips.

    This is the number behind "lever or strongman". A tall narrow body is a
    long lever: it reaches further, and a small push at the top produces a
    large moment about the edge of its own feet, so it tips easily. A stout
    wide body has a shorter lever and a bigger base -- it reaches less and
    resists more.

    Returned as:
      lever   -- height of the CoM above the floor. The moment arm an outside
                 push acts through.
      base    -- the support polygon's smallest width. What resists it.
      ratio   -- lever / base. Low is stout, high is a pole. A human standing
                 is around 3; a strongman stance is nearer 2.
      push    -- the sideways force, as a fraction of the machine's own weight,
                 that would tip it. base/(2*lever) for a push at CoM height.
    """
    M = M or masses()
    c, tot = com(sol, M)
    pts = contacts(sol)
    h = hull(pts)
    if len(h) < 3:
        return dict(lever=c[1], base=0.0, ratio=float('inf'), push=0.0)
    # smallest width of the polygon: the direction it is easiest to tip
    width = 1e30
    n = len(h)
    for i in range(n):
        ax, ay = h[i]
        bx, by = h[(i+1) % n]
        ex, ey = bx-ax, by-ay
        L = math.hypot(ex, ey) or 1e-9
        far = max(abs(ex*(p[1]-ay) - ey*(p[0]-ax)) / L for p in h)
        width = min(width, far)
    lever = max(1.0, c[1])
    return dict(lever=lever, base=width, ratio=lever/max(1e-6, width),
                push=width / (2.0 * lever), mass=tot)
