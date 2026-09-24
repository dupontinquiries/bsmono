#!/usr/bin/env python3
"""orion_fixpose.py — solve the angles instead of guessing them.

Four rounds of hand-tuning euler angles produced poses whose knees bent
backward in all five and whose hands never reached the floor. The angles are
not the thing being authored -- the INTENT is ("the knee flexes 40 degrees",
"the hand is on the ground") -- so author the intent and search for the angle.

One-dimensional search per joint, in order, re-solving the whole chain each
time. Crude, and it converges in a few dozen evaluations because each target
depends on one angle monotonically over the range that matters.
"""
import math
from orion_rig import BONES, POSES, solve
from orion_balance import knee_flex


def tune(pose, bone, axis, target_fn, lo=-170, hi=170, iters=44):
    """Binary-search one euler component until target_fn(sol) crosses zero."""
    p = dict(pose)
    def at(v):
        a = list(p.get(bone, (0, 0, 0)))
        a[axis] = v
        q = dict(p); q[bone] = tuple(a)
        return target_fn(solve(q)), q
    flo, _ = at(lo)
    fhi, _ = at(hi)
    if flo * fhi > 0:
        # no crossing in range: take whichever end is closer to the target
        best = lo if abs(flo) < abs(fhi) else hi
        _, q = at(best)
        return q
    for _ in range(iters):
        mid = (lo + hi) * 0.5
        fm, q = at(mid)
        if flo * fm <= 0:
            hi, fhi = mid, fm
        else:
            lo, flo = mid, fm
    _, q = at((lo + hi) * 0.5)
    return q


def fix(name, pose):
    """Apply this pose's intent as solved constraints."""
    p = dict(pose)

    # ---- body pitch: BOTH ENDS TOUCH ----------------------------------
    # This has to come first and it has to be at body level. `solve` grounds
    # the pose on its lowest weight-bearing joint, so tuning an arm down just
    # lifts the whole body and the hand stays where it was -- which is why
    # four rounds of adjusting arm angles never got a hand onto the floor.
    # The question is not "where is the hand", it is "is the body lying flat
    # enough that the hands and the feet are at the same height".
    if name in ('prone', 'spread', 'pushup'):
        p = tune(p, '_rot', 2,
                 lambda s: s['hand_l'][1][1] - s['foot_l'][1][1],
                 lo=20, hi=130)

    # ---- knees flex FORWARD, by a per-pose amount ----------------------
    # A knee that bends backward reads as broken whatever the machine's
    # excuse. Targets are the flex a body actually has in each pose.
    knee_target = {'prone': 58, 'spread': 40, 'pushup': 22,
                   'kneel': 84, 'rise': 6}[name]
    for side in ('l', 'r'):
        p = tune(p, 'shin_'+side, 2,
                 lambda s, sd=side: knee_flex(s, sd) - knee_target)

    # ---- then the arms fine-tune to the floor --------------------------
    if name in ('prone', 'spread', 'pushup'):
        for side in ('l', 'r'):
            p = tune(p, 'uparm_'+side, 2,
                     lambda s, sd=side: s['hand_'+sd][1][1] - s['foot_l'][1][1],
                     lo=-150, hi=40)

    # ---- feet flat ------------------------------------------------------
    for side in ('l', 'r'):
        p = tune(p, 'foot_'+side, 2,
                 lambda s, sd=side: s['foot_'+sd][1][1] - s['foot_'+sd][0][1])
    return p


if __name__ == '__main__':
    from orion_balance import com, contacts, hull, margin
    print(f"{'pose':<9}{'knee L':>8}{'knee R':>8}{'hand y':>9}{'contacts':>10}"
          f"{'margin':>9}")
    out = []
    for name, p in POSES:
        q = fix(name, p)
        s = solve(q)
        c, _ = com(s)
        h = hull(contacts(s))
        m = margin((c[0], c[2]), h) if len(h) >= 3 else float('nan')
        print(f"  {name:<7}{knee_flex(s,'l'):>8.0f}{knee_flex(s,'r'):>8.0f}"
              f"{s['hand_l'][1][1]:>9.0f}{len(contacts(s)):>10}"
              f"{m:>9.0f}")
        out.append((name, q))
    import json
    json.dump({n: {k: list(v) if isinstance(v, (list, tuple)) else v
                   for k, v in q.items()} for n, q in out},
              open(__import__('rigpaths').out('orion-poses-fixed.json'), 'w'), indent=1)
    print("\nwrote orion-poses-fixed.json")


# The hips and thighs are part of the body an arm has to miss. The first
# version checked only the spine, chest and pelvis, and the hands passed 0.7
# units from the hip in the prone pose -- clear of everything it was asked
# about and straight through something it was not.
TORSO = ('spine', 'chest', 'pelvis', 'hip_l', 'hip_r', 'thigh_l', 'thigh_r')


def torso_clearance(sol, part, meshes, torso=TORSO):
    """Nearest distance from a limb's mesh to the torso's, in world space.

    Angles alone cannot tell you this -- two bones can be far apart while
    their geometry overlaps, which is exactly what was happening: the
    forearm's nearest approach to the spine measured 1.4 units on a limb 10
    units thick."""
    import math

    def world(n):
        h, t, m = sol[n]
        return [(m[0][0]*x + m[0][1]*y + m[0][2]*z + h[0],
                 m[1][0]*x + m[1][1]*y + m[1][2]*z + h[1],
                 m[2][0]*x + m[2][1]*y + m[2][2]*z + h[2])
                for (x, y, z) in meshes[n].v]
    T = []
    for t in torso:
        T += world(t)
    T = T[::4]
    W = world(part)[::4]
    return min(math.dist(a, b) for a in W for b in T)


def clear_arms(name, pose, meshes, want=11.0):
    """Swing each arm outboard until its forearm clears the torso.

    Solved on the CLAVICLE, not the upper arm: rotating at the shoulder moves
    the whole arm away as a unit, which is what a body does. Rotating the
    upper arm alone splays the elbow while leaving the hand where it was."""
    import orion_rig as R
    p = dict(pose)
    for side, sign in (('l', 1.0), ('r', -1.0)):
        best, bestd = None, -1e9
        base = list(p.get('clav_' + side, (0, 0, 0)))
        for extra in range(0, 46, 3):
            q = dict(p)
            a = list(base)
            a[2] = base[2] + sign * extra
            q['clav_' + side] = tuple(a)
            sol = R.solve(q)
            d = min(torso_clearance(sol, 'forearm_' + side, meshes),
                    torso_clearance(sol, 'hand_' + side, meshes))
            if d > bestd:
                bestd, best = d, q
            if d >= want:
                return_early = q
                p = q
                break
        else:
            p = best
    return p
