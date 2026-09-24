#!/usr/bin/env python3
"""orion_body.py — mechanical mass on the rig.

The rig is locked: 22 fixed-length bones, five key poses, prone fits the
tunnel, tilt is monotonic. This hangs a body on it.

## What "mechanical, not wireframe" means here

A limb is not a capsule. What makes a machine read as a machine, at a glance
and before any texture exists, is:

  * **a housing at every joint** — a cylinder whose AXIS is the hinge axis, so
    you can see which way the joint is allowed to bend. A sphere says nothing;
    a barrel says "this rotates about that line".
  * **a piston alongside the bone** — two nested cylinders offset from the
    bone's centreline. It is what an actuator looks like and it is the
    cheapest possible cue that something is being driven rather than posed.
  * **plating that does not wrap** — flat slabs on the outboard face of a
    limb, leaving the inboard side open. A fully enclosed limb reads as a
    sausage; an open one reads as structure with a skin on it.
  * **volume proportional to load** — the mass model already says legs are
    48% of the machine. The geometry should agree, or the render argues with
    the physics.

## Geometry is per-bone and RIGID

Each bone owns a mesh in its own local frame, built once. Posing transforms
it; nothing deforms. That is the whole benefit of having fixed-length bones,
and it is what lets the same geometry be baked for a game or rebuilt live in
a viewer from a few numbers.
"""
import math, sys
import rigpaths  # the repo's tools/forge, and this kit, on the path
from forge import Mesh, loft, sphere, tube, torus
import orion_rig as R
from orion_balance import RADIUS, DEFAULT_R


def _y_along(m):
    """forge's loft runs along z; bones run along +y in their own frame."""
    out = Mesh()
    out.v = [(x, z, -y) for (x, y, z) in m.v]
    out.f = list(m.f)
    return out


def segment(length, r0, r1, flat=0.72, chunks=3):
    """A limb as STACKED OVERLAPPING VOLUMES, not one tapered sweep.

    Measured against artist bots: they are 85-96% solid volumes and 3% beams.
    Ours was 24% beams, because every limb was a single loft running the whole
    bone -- a long thin island is a beam by definition, and no amount of
    profile tweaking changes that while it is one object.

    Three shorter solids that overlap at their ends read as a limb built from
    parts. Each is stubby enough to classify as a volume, the overlaps hide
    the joins, and the steps between them give a silhouette something to catch
    on -- which a smooth taper never does.
    """
    m = Mesh()
    for k in range(chunks):
        a = k / chunks
        b = (k + 1) / chunks
        # overlap into the neighbour so the limb has no gaps
        lo = length * max(0.0, a - 0.06)
        hi = length * min(1.0, b + 0.06)
        ra = r0 + (r1 - r0) * a
        rb = r0 + (r1 - r0) * b
        # each chunk swells slightly at its own middle: a stack of barrels,
        # not a stack of cones
        mid = (ra + rb) * 0.5 * (1.10 - 0.05 * k)
        piece = loft([(lo,                 ra * 0.92, -ra*flat*0.92, ra*flat*0.92, 0.34),
                      (lo + (hi-lo)*0.22,  mid,       -mid*flat,     mid*flat,     0.28),
                      (lo + (hi-lo)*0.78,  mid*0.97,  -mid*flat*0.97, mid*flat*0.97, 0.28),
                      (hi,                 rb * 0.90, -rb*flat*0.90, rb*flat*0.90, 0.34)],
                     11, sub=2)
        m.add(_y_along(piece))
    return m


def housing(r, width, axis='z'):
    """A joint barrel. Its axis IS the hinge axis, which is the point: you can
    see from the shape which way the joint is allowed to go."""
    t = tube(14)
    m = Mesh()
    if axis == 'z':      # hinge about z: barrel lies along z
        m.add(t, scale=(r, width, r))
        m.v = [(x, z, y) for (x, y, z) in m.v]
    elif axis == 'x':
        m.add(t, scale=(r, width, r))
        m.v = [(y, x, z) for (x, y, z) in m.v]
    else:
        m.add(t, scale=(r, width, r))
    return m


def piston(length, r, offset, mirror=True):
    """Two nested cylinders alongside the bone — a rod inside a barrel.

    SHORT and CLOSE and PAIRED. The first version ran 92% of the bone's length
    at 11 units off the centreline, on one side only, and in silhouette it read
    as a shoulder spike rather than an actuator. An actuator is short (it only
    has to span the joint it drives), it hugs the bone, and there are two of
    them — a single one sticking out of a limb is a horn."""
    m = Mesh()
    barrel = _y_along(loft([(0.0, r, -r, r, 0.9), (length*0.58, r, -r, r, 0.9)], 10, sub=1))
    rod    = _y_along(loft([(length*0.50, r*0.52, -r*0.52, r*0.52, 0.9),
                            (length,      r*0.52, -r*0.52, r*0.52, 0.9)], 8, sub=1))
    sides = (-1.0, 1.0) if mirror else (1.0,)
    for sx in sides:
        for part in (barrel, rod):
            base = len(m.v)
            m.v += [(x + sx*offset[0], y, z + offset[2]) for (x, y, z) in part.v]
            m.f += [(a+base, b+base, c+base) for (a, b, c) in part.f]
    return m


def plate(length, w, t, y0, y1, side=1.0):
    """A flat slab on the outboard face. Deliberately not wrapped."""
    m = _y_along(loft([(y0, w*0.7, -t, t, 0.2),
                       (y0 + (y1-y0)*0.3, w, -t, t, 0.15),
                       (y1, w*0.75, -t, t, 0.2)], 8, sub=1))
    return Mesh() if length <= 0 else _shift(m, (side*w*0.55, 0, 0))


def sweep(points, radius, sides=7):
    """A continuous tube through a polyline.

    Both the ribs and the joint coils were built as independent segments
    placed end to end, and they rendered as scattered chunks rather than as a
    rib or a spring -- detail that reads as noise is worse than no detail.
    One shared sweep fixes both: consecutive rings are STITCHED, so the result
    is one surface.

    Frames are propagated along the curve rather than recomputed per segment,
    which is what stops the tube twisting where the curve bends."""
    n = len(points)
    if n < 2:
        return Mesh()
    m = Mesh()
    # initial frame
    d0 = [points[1][i] - points[0][i] for i in range(3)]
    L = math.sqrt(sum(x*x for x in d0)) or 1.0
    ux = [x/L for x in d0]
    up = (0, 1, 0) if abs(ux[1]) < 0.9 else (1, 0, 0)
    rx = [up[1]*ux[2]-up[2]*ux[1], up[2]*ux[0]-up[0]*ux[2], up[0]*ux[1]-up[1]*ux[0]]
    rl = math.sqrt(sum(x*x for x in rx)) or 1.0
    rx = [x/rl for x in rx]
    ry = [ux[1]*rx[2]-ux[2]*rx[1], ux[2]*rx[0]-ux[0]*rx[2], ux[0]*rx[1]-ux[1]*rx[0]]

    rings = []
    for k, pt in enumerate(points):
        if k > 0:
            d = [points[min(k+1, n-1)][i] - points[k-1][i] for i in range(3)]
            L = math.sqrt(sum(x*x for x in d)) or 1.0
            nx = [x/L for x in d]
            # rotate the previous frame onto the new tangent, minimally
            rx = [rx[i] - nx[i]*sum(rx[j]*nx[j] for j in range(3)) for i in range(3)]
            rl = math.sqrt(sum(x*x for x in rx)) or 1.0
            rx = [x/rl for x in rx]
            ux = nx
            ry = [ux[1]*rx[2]-ux[2]*rx[1], ux[2]*rx[0]-ux[0]*rx[2],
                  ux[0]*rx[1]-ux[1]*rx[0]]
        r = radius(k/(n-1)) if callable(radius) else radius
        ring = []
        for j in range(sides):
            a = math.tau * j / sides
            c, si = math.cos(a)*r, math.sin(a)*r
            ring.append(len(m.v))
            m.v.append(tuple(pt[i] + rx[i]*c + ry[i]*si for i in range(3)))
        rings.append(ring)
    for a, b in zip(rings, rings[1:]):
        for j in range(sides):
            k = (j+1) % sides
            m.f.append((a[j], a[k], b[k]))
            m.f.append((a[j], b[k], b[j]))
    for ring, flip in ((rings[0], True), (rings[-1], False)):
        c = len(m.v)
        m.v.append(tuple(sum(m.v[i][a] for i in ring)/len(ring) for a in range(3)))
        for j in range(sides):
            k = (j+1) % sides
            m.f.append((c, ring[k], ring[j]) if flip else (c, ring[j], ring[k]))
    return m


def _shift(m, d):
    out = Mesh()
    out.v = [(x+d[0], y+d[1], z+d[2]) for (x, y, z) in m.v]
    out.f = list(m.f)
    return out


# ───────────────────────────────────────────────── per-bone construction

RIB_COUNT = 6


def rib_hinge(i, L, r):
    """Where rib `i` pivots, and how far it can open.

    The cage is a set of hinged frames, not a fixed shell: each rib swings
    outward about its own root on the spine. Closed, it is a ribcage; open, it
    is a rack with a panel-sized gap between each pair of bars. Returned so
    the viewer can drive the opening from the transform, and so the cladding
    can eventually be told which slot it belongs in."""
    u = i / max(1, RIB_COUNT - 1)
    return {
        'y': L * (0.60 - 0.34 * u),
        'axis': (1.0, 0.0, 0.0),
        'open_deg': 26.0 - 9.0 * u,
        'span': r * (1.62 - 0.24 * u),
    }


def build_chest(L, r):
    m = Mesh()
    # a real torso: wide at the shoulders, narrow at the waist, with a
    # distinct chest plate rather than one smooth barrel
    # The waist: thicker than it was. A bulky machine narrows at the belt but
    # it does not neck in -- 0.78 of the chest radius read as a wasp waist
    # under a cage this wide.
    m.add(_y_along(loft([(0.0,     r*0.92, -r*0.60,  r*0.60, 0.30),
                         (L*0.35,  r*1.10, -r*0.70,  r*0.70, 0.28),
                         (L*0.80,  r*1.22, -r*0.66,  r*0.66, 0.26),
                         (L,       r*1.08, -r*0.54,  r*0.54, 0.30)], 14, sub=3)))
    # chest plate, proud of the front (-z is forward once the body stands)
    m.add(_shift(_y_along(loft([(L*0.42, r*0.72, -0.1*r, 0.1*r, 0.18),
                                (L*0.68, r*0.86, -0.1*r, 0.1*r, 0.14),
                                (L*0.92, r*0.62, -0.1*r, 0.1*r, 0.18)], 10, sub=2)),
                 (0, 0, -r*0.60)))
    # intake slots either side of it
    for sx in (-1, 1):
        m.add(_shift(_y_along(loft([(L*0.46, r*0.16, -0.06*r, 0.06*r, 0.1),
                                    (L*0.78, r*0.20, -0.06*r, 0.06*r, 0.1)], 6, sub=1)),
                     (sx*r*0.80, 0, -r*0.40)))
    # RIBS -- a BULKY machine's ribcage, which is not a human one.
    #
    # The first version was inverted on every axis that matters. It put the
    # widest ribs at the BOTTOM (span 13.6 rising to 18.7), started them high
    # on the collar, and swept every one DOWN toward the sternum. Measured
    # against a chest 17 units in radius they were also simply too small to
    # register -- a rib that does not exceed the torso it wraps is a groove,
    # not a rib.
    #
    # What a heavy armoured chest actually does:
    #   * the TOP ribs are the big ones. They carry the shoulder load, so they
    #     are the widest and the thickest, and the cage tapers DOWN to a waist.
    #   * they start LOW, under the armpit, not up on the collar -- a rib
    #     springing from the collar leaves no shoulder socket.
    #   * they CURL UP as they wrap forward. Human ribs fall toward the
    #     sternum; these lift, which is what gives a bulky chest its shelf and
    #     stops the front reading as a slumped barrel.
    for i in range(6):
        u = i / 5.0                              # 0 = top rib
        root_y = L * (0.60 - 0.34 * u)           # starts under the armpit
        lift = L * (0.13 - 0.07 * u)             # top ribs curl up hardest
        # HUG THE TORSO. The rib sweeps back-to-front correctly, but at span
        # 27.5 on a chest of radius 17 it flared 60% beyond the body and read
        # as a plate sticking out sideways rather than as a cage wrapping it.
        # A ribcage is barely wider than what it encloses.
        # BIGGER, and hollow. A ribcage that hugs the torso has nowhere to put
        # anything; these stand well clear of the chest so there is a CAVITY
        # between the cage and the spine, and that volume is what the car's
        # panels fold into. The bars are thicker to carry it.
        span = r * (1.62 - 0.24 * u)
        thick = r * (0.30 - 0.08 * u)
        depth = 0.96
        for sx in (-1, 1):
            pts = []
            n = 13
            for k in range(n):
                t2 = k / (n - 1)
                a2 = math.pi * (0.06 + 0.86 * t2)
                pts.append((sx * span * math.sin(a2),
                            root_y + lift * t2 * t2,       # curls UP
                            -span * depth * math.cos(a2)))
            m.add(sweep(pts, lambda t, th=thick: th * (1.0 - 0.28 * t), 7))
            # a short strut from each rib back to the spine, so the cage reads
            # as a frame with a void inside rather than a set of loose hoops
            mid = pts[len(pts)//2]
            m.add(sweep([(mid[0]*0.18, mid[1], mid[2]*0.18), mid],
                        lambda t, th=thick: th * (0.42 + 0.20 * t), 5))
            # the spur, at the widest point, out and back
            wk = max(range(n), key=lambda k: abs(pts[k][0]))
            bp = pts[wk]
            m.add(sweep([bp, (bp[0] + sx * r * 0.26, bp[1] - r * 0.04,
                              bp[2] + r * 0.22)],
                        lambda t, th=thick: th * (1.3 - 1.15 * t), 5))
    # sternum: the plate the ribs curl up to, so the front closes
    m.add(_shift(_y_along(loft([(L*0.28,  r*0.20, -0.09*r, 0.09*r, 0.2),
                                (L*0.52,  r*0.30, -0.09*r, 0.09*r, 0.15),
                                (L*0.74,  r*0.22, -0.09*r, 0.09*r, 0.2)],
                               8, sub=2)),
                 (0, 0, -r*0.62)))
    return m


def build_head(L, r):
    m = Mesh()
    m.add(_y_along(loft([(0.0,    r*0.62, -r*0.55, r*0.55, 0.40),
                         (L*0.45, r*0.86, -r*0.74, r*0.74, 0.32),
                         (L*0.86, r*0.72, -r*0.62, r*0.62, 0.36),
                         (L,      r*0.42, -r*0.36, r*0.36, 0.5)], 12, sub=2)))
    # visor band: the one feature that survives at distance
    m.add(_shift(_y_along(loft([(L*0.46, r*0.80, -0.12*r, 0.12*r, 0.12),
                                (L*0.66, r*0.86, -0.12*r, 0.12*r, 0.10),
                                (L*0.78, r*0.74, -0.12*r, 0.12*r, 0.12)], 8, sub=1)),
                 (0, 0, -r*0.62)))
    # A FACE, in the order features stop being visible as you walk away: the
    # visor first (done), then the brow, then the jaw, then the cheek vents.
    # Anything finer than this is invisible at the distance a bot is fought at.
    # brow ridge, proud of the visor and angled down at the outer ends
    for sx in (-1, 1):
        m.add(_shift(_y_along(loft([(0.0,      r*0.10, -0.07*r, 0.07*r, 0.2),
                                    (r*0.62,   r*0.08, -0.05*r, 0.05*r, 0.2)], 6, sub=1)),
                     (sx*r*0.06, L*0.80, -r*0.66)))
    # jaw: a wedge below the visor, narrower than the skull
    m.add(_shift(_y_along(loft([(L*0.16, r*0.52, -0.20*r, 0.06*r, 0.30),
                                (L*0.36, r*0.60, -0.24*r, 0.04*r, 0.26),
                                (L*0.48, r*0.50, -0.18*r, 0.02*r, 0.30)], 9, sub=2)),
                 (0, 0, -r*0.42)))
    # cheek vents, three bars each side
    for sx in (-1, 1):
        for k in range(3):
            m.add(_shift(_y_along(loft([(0.0,    r*0.055, -0.03*r, 0.03*r, 0.2),
                                        (r*0.30, r*0.045, -0.03*r, 0.03*r, 0.2)], 5, sub=1)),
                         (sx*r*0.56, L*(0.34 + k*0.10), -r*0.30)))
    # Crest fins: SHORT. The first version ran to L*1.10 -- half a head-length
    # above the skull -- and the two of them met in silhouette as a spike, so
    # the machine wore a party hat. They stop at the crown now and sit back,
    # where a crest reads as armour instead of as a horn.
    for sx in (-1, 1):
        m.add(_shift(_y_along(loft([(L*0.50, r*0.09, -0.04*r, 0.20*r, 0.25),
                                    (L*0.84, r*0.07, -0.04*r, 0.10*r, 0.25)], 6, sub=1)),
                     (sx*r*0.56, 0, r*0.22)))
    return m


def build_limb(name, L, r):
    """A driven limb: tapered section, joint barrel at the top, a piston down
    the back, and a plate on the outboard face."""
    m = Mesh()
    lo = r
    hi = r * (0.74 if 'thigh' in name or 'uparm' in name else 0.82)
    m.add(segment(L, lo, hi))
    m.add(housing(r*1.22, r*1.5, 'z'))
    # and a ball inside the housing: the barrel is open at its ends, so a limb
    # rotated away from the viewer showed straight through its own joint
    m.add(sphere(8, 10), scale=(r*1.02, r*1.02, r*1.02))
    # SUSPENSION at the joint: a short coil wrapped around the bone just below
    # the housing. Not decoration -- a joint that visibly carries a spring
    # explains why the machine can land, and it is the difference between a
    # hinge and a suspended one.
    cr = r * 0.88
    pts = []
    turns, seg = 2.6, 40
    for k in range(seg + 1):
        u = k / seg
        a2 = math.tau * turns * u
        pts.append((cr * math.cos(a2), L * (0.10 + 0.15 * u), cr * math.sin(a2)))
    coil = sweep(pts, r * 0.10, 5)
    m.add(coil)
    side = -1.0 if name.endswith('_l') else 1.0
    # spans the upper HALF of the bone -- the part next to the joint it drives
    m.add(piston(L*0.52, r*0.26, (r*0.62, 0, r*0.52)))
    m.add(plate(L, r*0.78, r*0.16, L*0.18, L*0.86, side))
    return m


def thrusters(L, r, n=2):
    """Double-jump ports on the back of the shin, above the ankle.

    They exist because the flared fins expose something: a fin that opens to
    reveal a blank surface is decoration, and one that opens onto a nozzle is
    a reason the fin opens. Two per leg -- the machine has a double jump, so
    it has two.
    """
    m = Mesh()
    for k in range(n):
        u = 0.30 + 0.26 * k
        cone = _y_along(loft([(0.0,      r*0.30, -r*0.30, r*0.30, 0.95),
                              (r*0.26,   r*0.38, -r*0.38, r*0.38, 0.95),
                              (r*0.62,   r*0.30, -r*0.30, r*0.30, 0.95)], 10, sub=1))
        base = len(m.v)
        for (x, y, z) in cone.v:
            # pointing back and down, so the thrust vector is up and forward
            m.v.append((x, L * u - z * 0.55, z + r * 0.95 + y * 0.30))
        m.f += [(a+base, b+base, c+base) for (a, b, c) in cone.f]
        # A shallow housing around each port.
        #
        # torus(10, 8) takes SEGMENT COUNTS, not a radius -- it comes back 22
        # units across. Multiplying that by r*0.46 scaled an already-11-unit
        # shape by seven, and the shin mesh went from the thigh's 44x117 to
        # 156x158: whoopee cushions on the ankles. Normalise it first.
        ring = torus(10, 8)
        rr = max(max(abs(v[a]) for v in ring.v) for a in range(3)) or 1.0
        k = r * 0.46 / rr
        base = len(m.v)
        for (x, y, z) in ring.v:
            m.v.append((x * k, L * u + y * k, z * k * 0.40 + r * 0.92))
        m.f += [(a+base, b+base, c+base) for (a, b, c) in ring.f]
    return m


def build_foot(L, r):
    m = Mesh()
    # An ankle ball, first. The shin's tapered end meets the foot at ninety
    # degrees and nothing bridged the corner, so the foot read as floating even
    # though the joint gap measures 0.0. A sphere at the origin fills it, and
    # it is the same trick every real joint uses for the same reason.
    m.add(sphere(8, 10), scale=(r*1.05, r*1.05, r*1.05))
    # a foot is a wedge with a heel behind the ankle -- centred on the ankle
    # it reads as a ski, which was the exact note on the forge bots
    m.add(_y_along(loft([(0.0,  r*0.86, -r*0.42, r*0.42, 0.28),
                         (L*0.5, r*0.96, -r*0.34, r*0.34, 0.24),
                         (L,     r*0.62, -r*0.22, r*0.22, 0.28)], 10, sub=2)))
    m.add(housing(r*0.9, r*1.2, 'z'))
    # GRAVITY PISTONS. Two short struts from the ankle down to the sole, one
    # fore and one aft. They are the visible reason the machine does not just
    # clatter onto the floor: a foot that takes load through a damper reads as
    # weighted, and it is the same cue a car's suspension gives.
    for dz, lean in ((-r*0.55, -0.22), (r*0.62, 0.26)):
        strut = _y_along(loft([(0.0,  r*0.20, -r*0.20, r*0.20, 0.85),
                               (r*0.55, r*0.20, -r*0.20, r*0.20, 0.85)], 8, sub=1))
        rod   = _y_along(loft([(r*0.45, r*0.11, -r*0.11, r*0.11, 0.85),
                               (r*0.95, r*0.11, -r*0.11, r*0.11, 0.85)], 6, sub=1))
        for part in (strut, rod):
            base = len(m.v)
            # lean them slightly so they splay, which reads as bracing
            m.v += [(x + lean*y, -y*0.9 + r*0.1, z + dz) for (x, y, z) in part.v]
            m.f += [(a+base, b+base, c+base) for (a, b, c) in part.f]
    return m


def build_hand(L, r):
    m = Mesh()
    m.add(sphere(7, 9), scale=(r*0.95, r*0.95, r*0.95))   # wrist ball, same reason
    m.add(_y_along(loft([(0.0, r*0.9, -r*0.55, r*0.55, 0.35),
                         (L*0.6, r*0.95, -r*0.6, r*0.6, 0.32),
                         (L, r*0.7, -r*0.45, r*0.45, 0.4)], 8, sub=1)))
    for i in (-1, 0, 1):
        m.add(_shift(_y_along(loft([(L*0.75, r*0.20, -r*0.18, r*0.18, 0.5),
                                    (L*1.45, r*0.16, -r*0.14, r*0.14, 0.5)], 6, sub=1)),
                     (i*r*0.46, 0, -r*0.10)))
    return m


def build_bone(name, length, r):
    if length <= 0:
        return Mesh()
    if name == 'chest':  return build_chest(length, r)
    if name == 'head':   return build_head(length, r)
    if name.startswith('foot'):  return build_foot(length, r)
    if name.startswith('shin'):
        m = build_limb(name, length, r)
        m.add(thrusters(length, r))
        return m
    if name.startswith('hand'):  return build_hand(length, r)
    if name in ('pelvis', 'spine'):
        m = Mesh()
        m.add(_y_along(loft([(0.0,      r*1.00, -r*0.64, r*0.64, 0.30),
                             (length*0.5, r*0.86, -r*0.56, r*0.56, 0.28),
                             (length,   r*0.94, -r*0.60, r*0.60, 0.30)], 12, sub=2)))
        m.add(housing(r*1.1, r*1.4, 'z'))
        return m
    if name.startswith('hip'):
        # NO housing and no ball at the root. A hip is a short connector that
        # lives INSIDE the pelvis, and both were placing a barrel at the same
        # point in space -- measured 0.00 apart, which is coincident surfaces
        # and exactly what z-fights. The joint that matters is at the far end,
        # where the thigh hangs.
        # Start the geometry PART WAY ALONG the bone. Both the hip and the
        # pelvis had their first ring at the same root point -- measured 0.00
        # apart -- so their surfaces were exactly coincident and the depth
        # buffer had no way to choose. Beginning at 45% of the bone means the
        # hip emerges from inside the pelvis instead of starting on its skin.
        m = Mesh()
        seg = _y_along(loft([(length*0.45, r*0.80, -r*0.58, r*0.58, 0.45),
                             (length*0.72, r*0.90, -r*0.64, r*0.64, 0.40),
                             (length,      r*0.92, -r*0.66, r*0.66, 0.42)],
                            10, sub=2))
        m.add(seg)
        return m
    if name in ('neck',) or name.startswith('clav'):
        m = Mesh()
        m.add(segment(length, r, r*0.9))
        m.add(housing(r*1.15, r*1.3, 'z'))
        return m
    return build_limb(name, length, r)


def _orient(m, rest):
    """Point a mesh built along +y along the bone's ACTUAL rest direction.

    THE BUG THIS FIXES. Every builder here works along +y, but a bone's rest
    direction is whatever `BONES` says -- (0,-1,0) for arms and legs, (0,0,-1)
    for clavicles and hips, (-1,0,0) for feet. So a limb's geometry grew out of
    the WRONG SIDE of its joint: measured, the nearest shin vertex to the ankle
    was 52.8 units away on a 64-unit bone, and the body rendered as a scatter of
    disconnected chunks that no amount of joint balls could close.

    Cheap fix, because +y -> any unit vector is a single rotation."""
    rx, ry, rz = rest
    L = math.sqrt(rx*rx + ry*ry + rz*rz)
    if L < 1e-9:
        return m
    d = (rx/L, ry/L, rz/L)
    if abs(d[1] - 1.0) < 1e-9:
        return m
    if abs(d[1] + 1.0) < 1e-9:                    # straight down: flip
        out = Mesh(); out.v = [(x, -y, z) for (x, y, z) in m.v]
        out.f = [(a, c, b) for (a, b, c) in m.f]  # flip winding with the mesh
        return out
    # rotate +y onto d: axis = y x d, angle = acos(d.y)
    ax = (d[2], 0.0, -d[0])
    al = math.sqrt(ax[0]**2 + ax[2]**2) or 1.0
    ax = (ax[0]/al, 0.0, ax[2]/al)
    ang = math.acos(max(-1.0, min(1.0, d[1])))
    c, si = math.cos(ang), math.sin(ang)
    out = Mesh()
    for (x, y, z) in m.v:
        # Rodrigues
        dot = ax[0]*x + ax[1]*y + ax[2]*z
        cx = ax[1]*z - ax[2]*y
        cy = ax[2]*x - ax[0]*z
        cz = ax[0]*y - ax[1]*x
        out.v.append((x*c + cx*si + ax[0]*dot*(1-c),
                      y*c + cy*si + ax[1]*dot*(1-c),
                      z*c + cz*si + ax[2]*dot*(1-c)))
    out.f = list(m.f)
    return out


def grit(mesh, amp=0.35, freq=0.09, seed=1):
    """Push every vertex a little along a cheap 3-D hash.

    Machined surfaces are not perfectly smooth and flat shading on a perfectly
    smooth surface is what makes the body read as plastic. This is not a
    normal map -- the geometry actually moves, so the grit survives silhouette
    and shows on the rim where a texture would not.

    Amplitude is small on purpose: 0.35 of a unit on a body ~300 tall. Enough
    to break the specular, not enough to change a shape.
    """
    out = Mesh()
    out.f = list(mesh.f)
    for (x, y, z) in mesh.v:
        h = math.sin((x*freq + seed) * 12.9898 + (y*freq) * 78.233
                     + (z*freq) * 37.719) * 43758.5453
        n = (h - math.floor(h)) * 2.0 - 1.0
        h2 = math.sin((y*freq*1.7 + seed) * 93.989 + (z*freq*1.3) * 47.11
                      + (x*freq*0.9) * 21.7) * 24634.6345
        n2 = (h2 - math.floor(h2)) * 2.0 - 1.0
        out.v.append((x + n*amp, y + n2*amp, z + (n+n2)*0.5*amp))
    return out


def joint_weight(mesh, length):
    """Per-vertex 0..1: 1 at the bone's joints, 0 at its middle.

    Feeds the shading. Metal near a joint is worked metal -- scuffed, polished
    by movement, discoloured by heat -- and metal in the middle of a limb is
    just panel. Having the number per vertex means the viewer can darken and
    roughen the right places without a texture existing.
    """
    w = []
    L = max(1e-6, length)
    for (x, y, z) in mesh.v:
        t = min(1.0, abs(y) / L)
        w.append(round(1.0 - min(1.0, min(t, abs(1.0 - t)) * 2.6), 3))
    return w


def rivets(length, radius, rest, n=7, seed=0):
    """Studs along a bone's seams. The small end of the size range.

    Measured against artist models: they span 128x to 513x between their
    smallest piece and their largest -- a bolt and a body panel in the same
    mesh. Ours spanned 19x once the scale armour landed, because every piece
    had become mid-sized. The median was right and the RANGE was not.

    A rivet is 0.4% of the machine. Cheap, and it is the thing that makes a
    surface read as assembled rather than moulded: an edge with fasteners on
    it was bolted down by somebody.
    """
    m = Mesh()
    # TWO SIZES, an order apart. One class of fastener only widened the size
    # range from 19x to 56x against the artists' 128x to 513x, because the
    # range is max over min and a single small size sets one floor. A machine
    # has bolts AND pins.
    for r, count, ring, sy in ((max(0.9, radius * 0.11), n, 1.02, 0.62),
                               (max(0.32, radius * 0.030), n * 3, 1.03, 0.55)):
        for i in range(count):
            u = 0.10 + 0.80 * (i / max(1, count - 1))
            for side in (-1, 1):
                a = math.tau * (0.18 * i + (0.5 if side < 0 else 0.0)
                                + (0.07 if r < 1.0 else 0.0))
                cx = math.cos(a) * radius * ring
                cz = math.sin(a) * radius * ring
                head = sphere(5, 6) if r > 1.0 else sphere(4, 5)
                base = len(m.v)
                for (x, y, z) in head.v:
                    m.v.append((cx + x * r, length * u + y * r * sy, cz + z * r))
                m.f += [(x + base, y + base, z + base) for (x, y, z) in head.f]
    return m


# Which way a bone's geometry FACES, as a rotation about its own long axis.
#
# The skeleton says left-and-right is z: clav_l points (0,0,-1) and clav_r
# (0,0,+1). But the chest, head and pelvis were all modelled with their FRONT
# on -z -- the chest plate, the visor, the ribs' sternum. So the machine had
# one arm out of its chest and one out of its back, shoulders separated 95
# units fore-and-aft and 0 across. Both conventions were internally consistent
# and they disagreed with each other.
#
# Turning the torso geometry a quarter turn puts its front on -x, square to
# the shoulder line, without touching the skeleton every pose is authored
# against.
FACE_TURN = {'chest': -90.0, 'head': -90.0, 'neck': -90.0,
             'spine': -90.0, 'pelvis': -90.0}


def _spin_about_y(m, deg):
    if not deg:
        return m
    c, s2 = math.cos(math.radians(deg)), math.sin(math.radians(deg))
    out = Mesh()
    out.v = [(x * c + z * s2, y, -x * s2 + z * c) for (x, y, z) in m.v]
    out.f = list(m.f)
    return out


def body_meshes(rough=True, studs=True):
    """One rigid mesh per bone, oriented along that bone's rest direction."""
    out = {}
    for name, (parent, length, rest) in R.BONES.items():
        r = RADIUS.get(name, DEFAULT_R)
        m = build_bone(name, length, r)
        if rough and m.v:
            m = grit(m, amp=min(0.45, r * 0.035))
        if studs and length > 14 and m.v:
            # rivets are added AFTER the grit, so they stay round -- a
            # displaced rivet is a lump, and the point of them is that they
            # are the one crisp thing on a worked surface
            m.add(rivets(length, r, rest, n=max(4, int(length / 18))))
        m = _spin_about_y(m, FACE_TURN.get(name, 0.0))
        out[name] = _orient(m, rest)
    return out


def posed(pose, meshes=None, on_floor=True):
    """The whole body at one pose, as a single mesh in world space.

    Grounded on the MESH, not on the joints. `R.solve` drops the rig until its
    lowest weight-bearing JOINT is at the floor, but a foot's geometry hangs
    below its ankle and a hand's below its wrist -- so a rig that is correctly
    grounded still renders sunk or floating once it has a body on it. The
    difference was 14 units on the prone pose."""
    meshes = meshes or body_meshes()
    sol = R.solve(pose)
    world = Mesh()
    for name, m in meshes.items():
        if not m.v:
            continue
        head, tail, mat = sol[name]
        base = len(world.v)
        for (x, y, z) in m.v:
            wx = mat[0][0]*x + mat[0][1]*y + mat[0][2]*z + head[0]
            wy = mat[1][0]*x + mat[1][1]*y + mat[1][2]*z + head[1]
            wz = mat[2][0]*x + mat[2][1]*y + mat[2][2]*z + head[2]
            world.v.append((wx, wy, wz))
        world.f += [(a+base, b+base, c+base) for (a, b, c) in m.f]
    if on_floor and world.v:
        dy = R.FLOOR - min(v[1] for v in world.v)
        world.v = [(x, y+dy, z) for (x, y, z) in world.v]
    return world


if __name__ == '__main__':
    ms = body_meshes()
    tri = sum(len(m.f) for m in ms.values())
    print(f"{len([m for m in ms.values() if m.v])} bones with geometry, "
          f"{tri} triangles total\n")
    print(f"{'bone':<11}{'tris':>7}{'len':>7}{'r':>6}")
    for n in ('chest', 'head', 'thigh_l', 'shin_l', 'foot_l', 'uparm_l', 'hand_l',
              'pelvis', 'spine'):
        print(f"  {n:<9}{len(ms[n].f):>7}{R.BONES[n][1]:>7.0f}"
              f"{RADIUS.get(n, DEFAULT_R):>6.0f}")
    for name, p in R.POSES:
        w = posed(p, ms)
        lo = [min(v[a] for v in w.v) for a in range(3)]
        hi = [max(v[a] for v in w.v) for a in range(3)]
        print(f"  {name:<8} {len(w.f):>6} tris   "
              f"{hi[0]-lo[0]:>5.0f} x {hi[1]-lo[1]:>5.0f} x {hi[2]-lo[2]:>5.0f}"
              f"   floor {lo[1]:>6.1f}")


# ──────────────────────────────────────────────────── the core, and shards

CORE_BONE = 'pelvis'          # the cube rides the pelvis: the body's centre
CORE_SIZE = 46.0


def build_core(t=0.0, size=CORE_SIZE):
    """The lattice cube, and what it is FOR.

    `compact.rs` states the mechanic: "matter goes somewhere on the way, and
    the somewhere is a lattice." This is that somewhere, made visible.

    The cube ABSORBS. As the machine folds down it swallows shards off the
    skeleton -- small voxels that peel away from the limbs and fall into it --
    and that is what frees the volume for the car's panels to occupy. Running
    the other way, the cube sheds them and the skeleton reassembles. So `t`
    here is the same t as the transform: at t=0 (prone, folded, inside the
    car) the cube is full and large; at t=1 (standing) it has given everything
    back and shrunk to a core.

    The shards are drawn as an orbiting shell because a cube that merely
    changes size reads as a scaling bug. Matter has to be seen going in.
    """
    m = Mesh()
    s = size * (1.0 - 0.55 * t)          # full when folded, small when stood
    # the cube itself, bevelled so it catches light on its edges
    k = s * 0.5
    b = k * 0.14
    m.add(_y_along(loft([(-k,      k-b, -(k-b), k-b, 0.10),
                         (-k+b,    k,   -k,     k,   0.05),
                         ( k-b,    k,   -k,     k,   0.05),
                         ( k,      k-b, -(k-b), k-b, 0.10)], 4, sub=1)))
    # SHARDS: voxels part-way between the skeleton and the cube. Their
    # distance from the cube is (1-t), so at t=0 they have arrived and at t=1
    # they are back out on the limbs.
    n = 14
    for i in range(n):
        a = math.tau * i / n + t * 2.2
        rise = math.sin(a * 2.3 + i) * 0.5 + 0.5
        dist = s * (0.62 + 1.9 * (1.0 - t) * (0.35 + 0.65 * rise))
        vx = dist * math.cos(a)
        vz = dist * math.sin(a)
        vy = (rise - 0.5) * s * 1.1
        e = s * (0.10 + 0.06 * rise)
        cube = _y_along(loft([(-e, e, -e, e, 0.08), (e, e, -e, e, 0.08)], 4, sub=1))
        base = len(m.v)
        ca, sa = math.cos(a * 1.7), math.sin(a * 1.7)
        for (x, y, z) in cube.v:
            m.v.append((x*ca - z*sa + vx, y + vy, x*sa + z*ca + vz))
        m.f += [(x+base, y+base, z+base) for (x, y, z) in cube.f]
    return m


def foot_compression(sol, name, floor=6.0, travel=0.30):
    """How far a foot's gravity piston is compressed, 0..1.

    A damper that never moves is a pipe. This is the load proxy: a foot
    bearing weight is the one nearest the floor, and it compresses; a foot in
    the air extends. Cheap, and it means the struts visibly work during the
    get-up instead of being decoration that happens to be shaped like a strut.
    """
    y = min(sol[name][0][1], sol[name][1][1])
    near = max(0.0, 1.0 - (y - floor) / 40.0)
    return max(0.0, min(1.0, near)) * travel
