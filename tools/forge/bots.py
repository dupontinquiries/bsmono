#!/usr/bin/env python3
"""bots.py — the six parts every blackstar bot is made of, plus assembly.

`content/part_groups.json` says a bot is not a model. It is:

    torso  -> pchest.iqm      head -> phead.iqm     hand  -> phand.iqm
    limb   -> plimb.iqm       joint -> psphere.iqm  bevel -> pbevel.iqm

assembled by the seven chassis in `content/chassis.json`, each of which is
six multipliers (h, bulk, arm, leg, head, shoulder) plus fold_spin and
xform_dur. So the leverage is not in drawing bots — it is in these six
primitives, because every chassis inherits them.

Two things the existing parts get wrong, per PIPELINE.md's own findings:

  * **"Unwelded meshes yield zero parts."** A skeleton reference produced
    115,386 one-triangle components. Everything here is generated welded by
    construction — shared ring indices, no duplicated vertices — so
    part_harvest never sees an island storm.
  * **"Limbs are rigid segments. Thigh/shin/foot with single-axis pivots."**
    That is the animation contract, so limbs here are built AS segments with
    their pivot recorded at the joint, not as one mesh to be cut later.

The joint sphere matters more than it looks: it is what lets a rigid segment
rotate without opening a gap at the elbow, and it is the cheapest part in the
set. Every limb here gets one at each end.
"""
import math
from dataclasses import dataclass, field
from forge import Mesh, loft, sphere, tube, _quad


# ────────────────────────────────────────────────────────── the six parts

def pchest(bulk=1.0, shoulder=1.0, taper=0.62):
    """Torso. A lofted box that narrows to the waist, with shoulder pads as
    part of the same surface rather than bolted on — a separate pad reads as
    armour, an integral one reads as build."""
    # Sized against an assembled height of ~2.3: chest ~0.55 wide, ~0.35 deep,
    # ~0.6 tall. The first pass used half-widths where it wanted widths and
    # produced a disc with wings.
    w = 0.27 * bulk
    sw = 0.13 * shoulder
    hull = [
        (-0.18, w * 0.70, 0.00, 0.16, 0.30),      # waist (back, -z)
        (-0.04, w * 1.00, 0.00, 0.50, 0.34),
        ( 0.10, w * 1.02, 0.02, 0.60, 0.32),      # chest
        ( 0.20, w * 0.84, 0.06, 0.56, 0.28),      # collar (front)
    ]
    m = loft(hull, 14)
    for side in (-1, 1):
        pad = loft([(-0.10, 0.055, 0.40, 0.50, 0.55),
                    ( 0.00, 0.075, 0.36, 0.58, 0.62),
                    ( 0.10, 0.055, 0.40, 0.50, 0.55)], 9)
        m.add(pad, scale=(sw / 0.075, 1.0, 1.0), off=(side * (w + sw * 0.75), 0.0, 0.03))
    return m


def phead(size=1.0):
    """Head. Small, faceted, with a visor band — the one part read at
    distance, so the silhouette carries a single strong horizontal."""
    m = loft([(-0.14, 0.13, 0.02, 0.24, 0.45),
              ( 0.02, 0.17, 0.00, 0.30, 0.40),
              ( 0.16, 0.12, 0.04, 0.24, 0.45)], 10)
    visor = loft([(-0.10, 0.02, 0.15, 0.22, 0.2),
                  ( 0.05, 0.03, 0.14, 0.23, 0.2),
                  ( 0.13, 0.02, 0.15, 0.22, 0.2)], 6)
    m.add(visor, scale=(6.2, 1.0, 1.0), off=(0.0, 0.0, 0.02))
    out = Mesh(); out.add(m, scale=(size, size, size))
    return out


def plimb(length=1.0, thick=1.0, taper=0.78):
    """One rigid segment, +y from the joint at origin to the far joint.

    Built along +y with its pivot AT THE ORIGIN, because the animation
    contract is single-axis rotation at the joint — a segment modelled about
    its centre has to be re-origined by every consumer, and that is how a
    limb ends up rotating around its middle."""
    r = 0.072 * thick
    L = 0.60 * length
    hull = [
        (0.00, r * 1.00, -r * 1.00, r * 1.00, 0.55),
        (L * 0.30, r * 0.94, -r * 0.94, r * 0.94, 0.50),
        (L * 0.72, r * taper, -r * taper, r * taper, 0.48),
        (L * 1.00, r * taper * 0.9, -r * taper * 0.9, r * taper * 0.9, 0.55),
    ]
    # loft runs along z; rotate into +y by swapping the axes on output
    seg = loft(hull, 10)
    m = Mesh()
    for (x, y, z) in seg.v:
        m.v.append((x, z, -y))
    m.f = list(seg.f)
    return m


def psphere(r=1.0, rows=7, cols=10):
    """Joint. Sits at every segment end so rotation never opens a gap."""
    m = Mesh(); m.add(sphere(rows, cols), scale=(r, r, r))
    return m


def phand(size=1.0, fingers=3):
    """Hand. A palm block and N stub digits — at bot scale a hand is a
    silhouette cue ('this end grips'), not an articulated prop."""
    m = loft([(-0.09, 0.07, -0.05, 0.05, 0.4),
              ( 0.02, 0.09, -0.07, 0.07, 0.45),
              ( 0.10, 0.07, -0.05, 0.05, 0.4)], 8)
    for i in range(fingers):
        t = (i / max(1, fingers - 1)) - 0.5
        f = loft([(0.00, 0.022, -0.022, 0.022, 0.6),
                  (0.11, 0.018, -0.018, 0.018, 0.6)], 6)
        m.add(f, off=(t * 0.10, -0.01, 0.10))
    out = Mesh(); out.add(m, scale=(size, size, size))
    return out


def pbevel(w=1.0, h=1.0):
    """Aux bevel — the filler wedge that hides a seam where two parts meet
    at an angle. Cheap, and it is what stops an assembly reading as a pile
    of primitives."""
    m = loft([(-0.10, 0.16 * w, 0.00, 0.06 * h, 0.15),
              ( 0.10, 0.10 * w, 0.00, 0.16 * h, 0.15)], 6)
    return m


# ─────────────────────────────────────────────────────────────── assembly

@dataclass
class Chassis:
    """Straight from content/chassis.json — same names, same numbers."""
    name: str
    h: float = 1.0
    bulk: float = 1.0
    arm: float = 1.0
    leg: float = 1.0
    head: float = 1.0
    shoulder: float = 1.0
    fold_spin: int = 220
    xform_dur: float = 1.0
    greeble: int = 0
    role: str = ""


CHASSIS = [
    Chassis("standard", 1.00, 1.00, 1.00, 1.00, 1.00, 1.00, 220, 1.00, 0,
            "The reference. Everything else is described as a departure from it."),
    Chassis("angular", 1.02, 0.88, 1.06, 1.02, 0.94, 1.05, 300, 0.78, 1,
            "Lean and fast-folding: the quickest transformation in the set, "
            "so it is the one that can change stance mid-fight."),
    Chassis("juggernaut", 0.96, 1.28, 1.10, 0.94, 0.90, 1.28, 160, 1.35, 2,
            "Wide and low with a slow fold. Commits to a stance and holds it; "
            "the shoulders are the silhouette."),
    Chassis("strider", 1.12, 0.82, 1.04, 1.12, 1.02, 0.90, 240, 0.92, 0,
            "Tall and narrow. Long legs, small shoulders — reads as reach "
            "rather than mass."),
    Chassis("raptor", 1.18, 0.78, 0.85, 1.32, 0.90, 0.82, 300, 0.80, 1,
            "Enormous legs, short arms, fast fold. The pouncer — its whole "
            "outline says it is about to move."),
    Chassis("bruiser", 0.86, 1.38, 1.40, 0.84, 1.06, 1.45, 170, 1.28, 2,
            "Squat, huge arms and shoulders, stubby legs. The only chassis "
            "whose arms outreach its legs."),
    Chassis("sentinel", 1.34, 0.76, 1.12, 1.18, 0.84, 0.80, 250, 1.05, 1,
            "Tallest and thinnest, small head. A watchtower that walks."),
]


def mount_point(c: Chassis):
    """Where a backpiece attaches: upper back, behind the collar. Returns the
    offset that a `spine_*` part (origin `base`, +y up) should be added at."""
    hip = 0.88 * c.h * c.leg
    chest_y = hip + 0.06 * c.h
    return (0.0, chest_y + 0.34 * c.h, 0.17 * c.bulk)


def build_bot(c: Chassis, spine=None) -> Mesh:
    """Assemble one bot. Proportions are the chassis multipliers, applied to
    the six shared parts — which is the point: improving a part improves all
    seven, and a new chassis costs six numbers."""
    m = Mesh()
    hip = 0.88 * c.h * c.leg
    chest_y = hip + 0.06 * c.h

    m.add(pchest(c.bulk, c.shoulder), off=(0.0, chest_y, 0.0))
    m.add(psphere(0.058 * c.head), off=(0.0, chest_y + 0.60 * c.h, 0.0))   # neck
    # phead spans y 0..0.30 from its origin, so the offset is the CHIN.
    m.add(phead(c.head * 0.86), off=(0.0, chest_y + 0.60 * c.h, 0.02))

    sx = 0.27 * c.bulk + 0.13 * c.shoulder * 0.75
    upper = 0.60 * 0.58 * c.arm
    for side in (-1, 1):
        sh = (side * sx, chest_y + 0.50 * c.h, 0.03)
        m.add(psphere(0.082 * c.shoulder), off=sh)
        m.add(plimb(0.58 * c.arm, 0.95 * c.bulk), scale=(1, -1, 1), off=sh)   # down
        el = (sh[0], sh[1] - upper, sh[2])
        m.add(psphere(0.062), off=el)
        m.add(plimb(0.54 * c.arm, 0.86 * c.bulk), scale=(1, -1, 1), off=el)
        wr = (el[0], el[1] - 0.60 * 0.54 * c.arm, el[2])
        m.add(psphere(0.048), off=wr)
        m.add(phand(0.62), off=(wr[0], wr[1] - 0.055, wr[2]))

    thigh = 0.60 * 0.72 * c.leg
    for side in (-1, 1):
        hp = (side * 0.145 * c.bulk, hip, 0.0)
        m.add(psphere(0.088 * c.bulk), off=hp)
        m.add(plimb(0.72 * c.leg, 1.12 * c.bulk), scale=(1, -1, 1), off=hp)
        kn = (hp[0], hp[1] - thigh, hp[2])
        m.add(psphere(0.072), off=kn)
        m.add(plimb(0.70 * c.leg, 1.0 * c.bulk), scale=(1, -1, 1), off=kn)
        ank = (kn[0], kn[1] - 0.60 * 0.70 * c.leg, kn[2])
        m.add(psphere(0.056), off=ank)
        # foot: a bevel wedge, toe forward (-z)
        # A foot is a wedge with a heel behind the ankle, not a plate centred
        # on it — a plate makes every chassis look like it is on skis.
        foot = loft([(-0.09, 0.052, 0.0, 0.075, 0.30),    # heel, behind (-z)
                     ( 0.04, 0.060, 0.0, 0.055, 0.28),
                     ( 0.15, 0.045, 0.0, 0.030, 0.30)], 8)  # toe, forward
        m.add(foot, scale=(c.bulk, 1.0, 1.0),
              off=(ank[0], ank[1] - 0.052, ank[2] - 0.01))

    if c.greeble:
        for i in range(c.greeble * 2):
            t = (i / max(1, c.greeble * 2 - 1)) - 0.5
            m.add(pbevel(0.4, 0.6), scale=(0.5, 0.5, 0.5),
                  off=(t * 0.34 * c.bulk, chest_y + 0.26, 0.20))
    if spine is not None:
        # backpieces are authored at a fixed scale; a bigger chassis carries
        # a bigger one, which is how mass reads across the roster
        k = 0.80 + 0.20 * c.bulk
        m.add(spine(), scale=(k, k, k), off=mount_point(c))
    return m
