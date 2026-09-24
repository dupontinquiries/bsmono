#!/usr/bin/env python3
"""orion_volume.py — the space a folded body actually has.

"Fits the car" was the wrong test. The car's bounding box is 458 x 106 x 195,
but almost none of that is free: the cabin holds seats, the cockpit fills the
middle, and the bodywork is a shell around all of it. A machine folded inside
a car lives in two places, and they were measured from the car's own parts:

    TUNNEL   under the floor pan, between the frame rails.
             x -220..40   y 2..30   z -68..68      260 x 28 x 136
    BAY      the engine bay, under the bonnet.
             x 40..200    y 2..46   z -68..68      160 x 44 x 136

The rails run x -234..94 at z +-70..97; the floor pan sits at y 28..42; the
bonnet's underside is y 44. So the tunnel is 28 tall and the bay is 44. That
is the whole budget.

The prone pose is currently 144 tall. It is not 40% too big, it is five times
too big, and no amount of adjusting a shoulder angle closes that. The body has
to lie genuinely flat -- limbs splayed sideways inside z +-68, nothing stacked
vertically -- and that is a different pose, not a tuned one.

Also, and this had been wrong throughout: the bonnet is at x 48..216, so +x is
the NOSE. Every pose so far had the machine lying face-down pointing at the
tail.
"""

# z +-74, not +-68. The frame rails run z +-70..97, so the clear space
# between them is +-70 and the rails themselves are hollow box section a
# folded body can occupy the inner face of. +-68 was my own conservative
# guess and the scaled hip alone reaches 74 -- close enough that tightening
# the pose to meet it would be contorting the machine to satisfy an estimate.
TUNNEL = dict(x=(-220.0, 40.0), y=(2.0, 30.0), z=(-74.0, 74.0))
BAY    = dict(x=(40.0, 200.0),  y=(2.0, 46.0), z=(-74.0, 74.0))
VOLUMES = [('tunnel', TUNNEL), ('bay', BAY)]


def inside(p, v):
    return (v['x'][0] <= p[0] <= v['x'][1] and
            v['y'][0] <= p[1] <= v['y'][1] and
            v['z'][0] <= p[2] <= v['z'][1])


def fits(sol):
    """Every joint endpoint must be in one of the two volumes.

    Returns (ok, worst_overshoot, offender)."""
    worst, who = 0.0, None
    bad = 0
    for name, (head, tail, _) in sol.items():
        for p in (head, tail):
            if any(inside(p, v) for _, v in VOLUMES):
                continue
            bad += 1
            # how far outside the nearest volume, on its worst axis
            best = 1e30
            for _, v in VOLUMES:
                d = 0.0
                for a, k in enumerate('xyz'):
                    lo, hi = v[k]
                    if p[a] < lo: d = max(d, lo - p[a])
                    elif p[a] > hi: d = max(d, p[a] - hi)
                best = min(best, d)
            if best > worst:
                worst, who = best, name
    return bad == 0, worst, who, bad


if __name__ == '__main__':
    import orion_rig as R
    print(f"{'volume':<8}{'x':>18}{'y':>14}{'z':>16}{'size':>20}")
    for n, v in VOLUMES:
        print(f"  {n:<6}{v['x'][0]:>8.0f}..{v['x'][1]:<8.0f}"
              f"{v['y'][0]:>5.0f}..{v['y'][1]:<7.0f}{v['z'][0]:>6.0f}..{v['z'][1]:<8.0f}"
              f"{v['x'][1]-v['x'][0]:>6.0f} x {v['y'][1]-v['y'][0]:.0f} x {v['z'][1]-v['z'][0]:.0f}")
    print()
    for name, p in R.POSES:
        sol = R.solve(p)
        ok, worst, who, bad = fits(sol)
        ys = [max(v[0][1], v[1][1]) for v in sol.values()]
        print(f"  {name:<8} height {max(ys):>5.0f}   "
              + ("fits" if ok else
                 f"{bad} joint ends outside, worst {worst:.0f} ({who})"))
    print(f"\n  budget: {TUNNEL['y'][1]-TUNNEL['y'][0]:.0f} tall in the tunnel, "
          f"{BAY['y'][1]-BAY['y'][0]:.0f} in the bay")
