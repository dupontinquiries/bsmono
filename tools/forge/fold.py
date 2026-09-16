#!/usr/bin/env python3
"""fold.py — robot <-> vehicle as parameter interpolation.

`chassis.json` has carried `fold_spin` and `xform_dur` for every chassis
since before I got here and nothing reads them. This is what they are for.

## Why this is possible here and was not before

An imported bot and an imported vehicle are two unrelated meshes; a
transformation between them is a morph target, which means authoring a third
thing and keeping all three in sync. Both sides here are parameters, so a
fold is **the same parts at two poses, interpolated**. The cost of a
transformation is one extra pose per part — six floats — not a second model.

## The structure

`build_bot` baked every part at its robot position as it went, which made a
pose impossible. So assembly is now a two-step:

    layout(chassis)  ->  [Placement]      # what goes where, in BOTH poses
    assemble(placements, t)  ->  Mesh     # bake at fold fraction t

`t=0` is robot, `t=1` is vehicle, and everything between is the animation.
A `Placement` is a part plus two poses; nothing else changes.

## The fold itself, stated as rules rather than keyframes

  * the torso pitches forward to horizontal and becomes the hull
  * the legs fold at hip AND knee and stack underneath, rearward — a leg
    that folds only at the hip sweeps through the torso
  * the arms rotate back along the flanks
  * the head retracts into the collar (scaled down, not hidden — hiding it
    makes the vertex count change mid-animation)
  * wheels grow from zero at the hips and shoulders

`fold_spin` is the yaw each part adds while folding: a juggernaut at 160
barely rotates, an angular at 300 whips round. That one number is most of
what makes two chassis with the same fold read differently.
"""
import math
from dataclasses import dataclass
from typing import Callable

from forge import Mesh, wheel
from bots import Chassis, pchest, phead, phand, plimb, psphere, pbevel

Vec = tuple


@dataclass
class Pose:
    off: Vec = (0.0, 0.0, 0.0)
    rot_x: float = 0.0
    rot_y: float = 0.0
    scale: float = 1.0


@dataclass
class Placement:
    make: Callable[[], Mesh]
    robot: Pose
    car: Pose
    name: str = ""


def lerp(a, b, t):
    return a + (b - a) * t


def lerp_pose(a: Pose, b: Pose, t: float) -> Pose:
    return Pose(
        off=tuple(lerp(a.off[i], b.off[i], t) for i in range(3)),
        rot_x=lerp(a.rot_x, b.rot_x, t),
        rot_y=lerp(a.rot_y, b.rot_y, t),
        scale=lerp(a.scale, b.scale, t),
    )


def ease(t):
    """Smoothstep. A linear fold reads as a rig test; the ease is what makes
    it read as mechanism settling into a detent."""
    t = max(0.0, min(1.0, t))
    return t * t * (3 - 2 * t)


def layout(c: Chassis, spine=None, recipe=None):
    """Every part, in both poses. This is the whole model."""
    from recipes import car as _car
    P = []
    hip = 0.88 * c.h * c.leg
    chest_y = hip + 0.06 * c.h
    spin = math.radians(c.fold_spin) * 0.25      # fold_spin is deg/sec-ish;
    #                                              a quarter of it is the yaw
    #                                              a part sweeps end to end.
    HALF = math.pi / 2
    sx = 0.27 * c.bulk + 0.13 * c.shoulder * 0.75
    upper = 0.60 * 0.58 * c.arm
    fore = 0.60 * 0.54 * c.arm
    thigh = 0.60 * 0.72 * c.leg
    shin = 0.60 * 0.70 * c.leg
    geom = {"sx": sx, "upper": upper, "fore": fore, "thigh": thigh, "shin": shin}
    R = (recipe or _car)(c, spin, geom)

    def pose(key, side=None, **fallback):
        """Vehicle pose from the recipe, defaulted to the fallback."""
        v = R.get(key)
        if callable(v):
            v = v(side)
        d = dict(fallback)
        d.update(v or {})
        return Pose(off=d.get("off", fallback.get("off", (0, 0, 0))),
                    rot_x=d.get("rot_x", 0.0), rot_y=d.get("rot_y", 0.0),
                    scale=d.get("scale", 1.0))

    # ── torso: stands up, then lies down as the hull ──────────────────────
    P.append(Placement(
        lambda: pchest(c.bulk, c.shoulder), name="chest",
        robot=Pose(off=(0.0, chest_y, 0.0)),
        car=pose("chest"),
    ))
    P.append(Placement(
        lambda: psphere(0.058 * c.head), name="neck",
        robot=Pose(off=(0.0, chest_y + 0.60 * c.h, 0.0)),
        car=pose("neck"),
    ))
    P.append(Placement(
        lambda: phead(c.head * 0.86), name="head",
        robot=Pose(off=(0.0, chest_y + 0.60 * c.h, 0.02)),
        # retracted, not hidden — hiding changes the vertex count mid-fold
        car=pose("head"),
    ))

    for side in (-1, 1):
        sh = (side * sx, chest_y + 0.50 * c.h, 0.03)
        P.append(Placement(
            lambda s=c.shoulder: psphere(0.082 * s), name="shoulder",
            robot=Pose(off=sh), car=pose("shoulder", side)))
        # upper arm: hangs down, then lies back along the flank
        P.append(Placement(
            lambda: plimb(0.58 * c.arm, 0.95 * c.bulk), name="upperarm",
            robot=Pose(off=sh, scale=1.0, rot_x=math.pi),
            car=pose("upperarm", side)))
        el = (sh[0], sh[1] - upper, sh[2])
        ua = pose("upperarm", side)
        el_car = (ua.off[0], ua.off[1], ua.off[2] + upper)
        P.append(Placement(lambda: psphere(0.062), name="elbow",
                           robot=Pose(off=el), car=Pose(off=el_car)))
        P.append(Placement(
            lambda: plimb(0.54 * c.arm, 0.86 * c.bulk), name="forearm",
            robot=Pose(off=el, rot_x=math.pi),
            car=pose("forearm", side)))
        wr = (el[0], el[1] - fore, el[2])
        fa = pose("forearm", side)
        wr_car = (fa.off[0], fa.off[1], fa.off[2] + fore)
        P.append(Placement(lambda: psphere(0.048), name="wrist",
                           robot=Pose(off=wr), car=Pose(off=wr_car)))
        P.append(Placement(
            lambda: phand(0.62), name="hand",
            robot=Pose(off=(wr[0], wr[1] - 0.055, wr[2])),
            # the hand tucks away: it is the part with no vehicle job
            car=Pose(off=(wr_car[0], wr_car[1] - 0.02, wr_car[2] + 0.05),
                     rot_x=-HALF, scale=0.7)))

    for side in (-1, 1):
        hp = (side * 0.145 * c.bulk, hip, 0.0)
        th = pose("thigh", side)
        hp_car = th.off
        P.append(Placement(lambda b=c.bulk: psphere(0.088 * b), name="hip",
                           robot=Pose(off=hp), car=Pose(off=hp_car)))
        # THIGH folds back to horizontal…
        P.append(Placement(
            lambda: plimb(0.72 * c.leg, 1.12 * c.bulk), name="thigh",
            robot=Pose(off=hp, rot_x=math.pi),
            car=th))
        kn = (hp[0], hp[1] - thigh, hp[2])
        shp = pose("shin", side)
        kn_car = shp.off
        P.append(Placement(lambda: psphere(0.072), name="knee",
                           robot=Pose(off=kn), car=Pose(off=kn_car)))
        # …and the SHIN folds again, back under. A leg that folds only at the
        # hip sweeps its shin straight through the torso.
        P.append(Placement(
            lambda: plimb(0.70 * c.leg, 1.0 * c.bulk), name="shin",
            robot=Pose(off=kn, rot_x=math.pi),
            car=shp))
        ank = (kn[0], kn[1] - shin, kn[2])
        ank_car = (kn_car[0], kn_car[1] - 0.04, kn_car[2] + shin * 0.75)
        P.append(Placement(lambda: psphere(0.056), name="ankle",
                           robot=Pose(off=ank), car=Pose(off=ank_car)))
        P.append(Placement(
            lambda: _foot(c), name="foot",
            robot=Pose(off=(ank[0], ank[1] - 0.052, ank[2] - 0.01)),
            car=Pose(off=ank_car, **{k: v for k, v in
                     (R["foot"](side) if callable(R.get("foot")) else {}).items()})))

        # wheels: absent in robot mode (scale 0), deployed in vehicle mode.
        # Scaling to zero rather than omitting keeps the vertex count fixed
        # across the whole animation, which is what lets one buffer serve it.
        for wz, wr_ in (((-0.62, 0.19), (0.50, 0.19)) if R.get("wheels") else ()):
            P.append(Placement(
                lambda: wheel(0.5, 14), name="wheel",
                robot=Pose(off=(side * 0.20 * c.bulk, 0.20, wz), scale=0.001),
                # wheel() is already UNIT radius — the x6 here made a
                # 1-metre wheel that swallowed the whole vehicle.
                car=Pose(off=(side * 0.30 * c.bulk, wr_, wz), scale=wr_)))

    if spine is not None:
        k = 0.80 + 0.20 * c.bulk
        P.append(Placement(
            spine, name="spine",
            robot=Pose(off=(0.0, chest_y + 0.34 * c.h, 0.17 * c.bulk), scale=k),
            # in vehicle mode the backpiece is the dorsal hardware
            car=Pose(off=(0.0, 0.58 * c.h, -0.20), scale=k * 0.92)))
    return P


def _foot(c: Chassis):
    from forge import loft
    m = Mesh()
    f = loft([(-0.09, 0.052, 0.0, 0.075, 0.30),
              ( 0.04, 0.060, 0.0, 0.055, 0.28),
              ( 0.15, 0.045, 0.0, 0.030, 0.30)], 8)
    m.add(f, scale=(c.bulk, 1.0, 1.0))
    return m


def assemble(placements, t=0.0):
    """Bake every placement at fold fraction t. 0 = robot, 1 = vehicle."""
    e = ease(t)
    m = Mesh()
    for p in placements:
        po = lerp_pose(p.robot, p.car, e)
        s = po.scale
        # plimb/phand/foot are built +y from their pivot; rot_x=pi points the
        # segment DOWN, which is the robot rest pose. Interpolating rot_x is
        # therefore the joint angle, exactly as the animation contract wants.
        m.add(p.make(), scale=(s, s, s), rot_x=po.rot_x, rot_y=po.rot_y, off=po.off)
    return m


def build_at(c: Chassis, t=0.0, spine=None):
    return assemble(layout(c, spine), t)


# ──────────────────────────────────────────────────────────── pairing

# Which vehicle each chassis folds INTO. Matched on proportion, not taste:
# a wide chassis cannot credibly become a narrow vehicle, because the fold
# preserves volume — the parts are the same parts.
#
# This is also what `model_tuning.json`'s `pair` field wants, and what
# test_tuning.py checks: pairs must be symmetric and must cross kinds
# (vehicle <-> robot). Generating both sides from one table is how they stay
# symmetric — an authored pair list drifts the first time someone renames a
# mesh, which is the failure PIPELINE.md records for truncated names.
PAIRS = {
    # chassis      vehicle   why
    "standard":   ("ANVIL",  "the reference both ways: 1.61 l/w against bulk 1.00"),
    "angular":    ("LANCE",  "lean and fast-folding becomes the flat blade; "
                             "spin 300 is what lets it lie down that far"),
    "juggernaut": ("DRUM",   "bulk 1.28 has nowhere narrow to go — DRUM is the "
                             "only car wide enough to absorb those shoulders"),
    "strider":    ("SPLIT",  "tall and narrow both ways: l/w 3.33 is the only "
                             "car that stays thin"),
    "raptor":     ("DART",   "legs 1.32 fold into the longest thing in the set; "
                             "the pouncer becomes the interceptor"),
    "bruiser":    ("CAIRN",  "widest chassis, widest hull. It does not fold "
                             "small, so it folds into a cushion instead"),
    "sentinel":   ("SPAN",   "h 1.34 and bulk 0.76 is a glider's proportions "
                             "already; the watchtower becomes the wing"),
}


def target_box(spec):
    """(length, width, height) of the vehicle AS BUILT.

    Not of its hull control points. A flyer's wings, a hoverer's skirt and a
    ship's pods are most of the silhouette and none of them are in `hull` —
    measuring the control polygon told me DART was l/w 5.0 when the actual
    winged outline is nothing like that, and the fit then stretched the fold
    into a sliver chasing a ratio that does not exist."""
    from forge import build as _build
    lo, hi = _build(spec).bounds()
    return (hi[2] - lo[2], hi[0] - lo[0], hi[1] - lo[1])


def fit_to(placements, c: Chassis, spec, iters=3):
    """Shape the VEHICLE pose to the paired vehicle's PROPORTIONS.

    Not its absolute size. A bot has a fixed amount of material, so a folded
    juggernaut cannot become a 3.4 m DRUM — and scaling the mesh to make it
    one would mean the parts change size mid-fold, which is the one thing a
    transformation must not do. What can match is the ratio: DRUM is l/w
    1.49 and h/w 0.32, and a folded juggernaut can be asked to hit that.

    Solved by measuring rather than by formula, because the parts have their
    own extents and the layout only partly controls the result. Three passes
    converge to within a few percent; the robot pose is never touched.
    """
    L, W, H = target_box(spec)
    want_lw, want_hw = L / W, H / W

    # A chassis cannot span wider than its arms reach. SPAN is a 6.62 m wing
    # and a sentinel reaches 2.07 m — chasing that ratio stretches the fold
    # into a sliver instead of admitting the pairing is out of range. Clamp,
    # and say so, rather than produce a confidently wrong shape.
    reach = 2 * (0.27 * c.bulk + 0.13 * c.shoulder * 0.75
                 + 0.60 * 0.58 * c.arm + 0.60 * 0.54 * c.arm)
    if W > reach * 1.6:
        fit_to.last_note = (f"{spec.name} spans {W:.2f} m; {c.name} reaches "
                            f"{reach:.2f} m. Ratio clamped — pick a narrower "
                            f"target or give the chassis longer arms.")
        want_lw = max(want_lw, L / (reach * 1.6))
    else:
        fit_to.last_note = None
    for _ in range(iters):
        m = assemble(placements, 1.0)
        (lo, hi) = m.bounds()
        w = max(1e-6, hi[0] - lo[0])
        got_lw = (hi[2] - lo[2]) / w
        got_hw = (hi[1] - lo[1]) / w
        kz = (want_lw / got_lw) ** 0.7        # damped: overshoot oscillates
        ky = (want_hw / got_hw) ** 0.7
        for p in placements:
            o = p.car.off
            p.car.off = (o[0], o[1] * ky, o[2] * kz)
    return placements


def build_pair(c: Chassis, t=0.0, spine=None):
    """A chassis folded toward the vehicle it is paired with."""
    from fleet import ALL
    from recipes import BY_CLASS
    name = PAIRS[c.name][0]
    spec = next(s for s in ALL if s.name == name)
    rec = BY_CLASS[spec.cls]
    return assemble(fit_to(layout(c, spine, rec), c, spec), t)
