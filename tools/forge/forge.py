#!/usr/bin/env python3
"""forge.py — original vehicles for blackstar-rs, generated from parameters.

Why generate rather than import: the imported library is 57% rights-holder
material by filename, and a mesh you cannot ship is worth nothing however
good it is. But the replacement is not a like-for-like redraw — a parametric
model is a better artifact than a mesh in four ways that matter here:

  * **It compresses absurdly.** A hull is ~6 control sections of 5 floats.
    The imported GLBs run 40,000+ vertices; a spec is ~200 bytes and expands
    to whatever resolution the target wants.
  * **It is flexible.** Change one number and every derived asset follows —
    LODs, collision proxies, silhouette variants, the whole family.
  * **It explains itself.** Every vehicle here carries the gameplay role it
    exists to fill, in the style procars.rs already uses ("BREADBOX —
    maximum sponge, the ball settles on the roof shelf").
  * **It is ours.**

Style brief is skydribble's, verbatim: *"kindergarten shapes at silly scales
— rings, balls, capsules, boxes, lollipops. Readable silhouettes over
realism."* And the section tuple is blackstar-rs's own:
`(z, half_width, floor, roof, roundness)`, nose at -z.
"""
import json, math, os, sys
from dataclasses import dataclass, field, asdict


# ------------------------------------------------------------------ mesh

class Mesh:
    __slots__ = ("v", "f")

    def __init__(self):
        self.v = []          # [(x,y,z)]
        self.f = []          # [(a,b,c)] 0-based

    def add(self, other, scale=(1, 1, 1), rot_y=0.0, off=(0, 0, 0), rot_x=0.0):
        """Bake a child in. Matches meshgen::bake — scale, rotate, translate,
        then merge; no scene graph, because a baked part is one draw call and
        the wheels still spin via their recorded pivot."""
        base = len(self.v)
        c, s = math.cos(rot_y), math.sin(rot_y)
        cx, sx = math.cos(rot_x), math.sin(rot_x)
        for (x, y, z) in other.v:
            x, y, z = x * scale[0], y * scale[1], z * scale[2]
            # pitch first, then yaw. tube()/wheel() have fixed axes (+y and
            # +x), so laying a barrel down needs rot_x -- without it every
            # "barrel" comes out standing on end.
            y, z = y * cx - z * sx, y * sx + z * cx
            x, z = x * c - z * s, x * s + z * c
            self.v.append((x + off[0], y + off[1], z + off[2]))
        for (a, b, d) in other.f:
            self.f.append((a + base, b + base, d + base))
        return self

    def bounds(self):
        xs, ys, zs = zip(*self.v) if self.v else ((0,), (0,), (0,))
        return (min(xs), min(ys), min(zs)), (max(xs), max(ys), max(zs))

    def obj(self, name="model"):
        out = [f"o {name}"]
        out += [f"v {x:.5f} {y:.5f} {z:.5f}" for (x, y, z) in self.v]
        out += [f"f {a+1} {b+1} {c+1}" for (a, b, c) in self.f]
        return "\n".join(out) + "\n"


def _quad(m, a, b, c, d):
    m.f.append((a, b, c)); m.f.append((a, c, d))


def catmull(p0, p1, p2, p3, t):
    t2, t3 = t * t, t * t * t
    return tuple(0.5 * ((2 * b) + (-a + c) * t + (2 * a - 5 * b + 4 * c - d) * t2
                        + (-a + 3 * b - 3 * c + d) * t3)
                 for a, b, c, d in zip(p0, p1, p2, p3))


def loft(sections, ring=16, sub=7, cap=True):
    """Sweep a rounded-rect cross-section along z.

    `roundness` blends the profile between a box (0) and an ellipse (1),
    which is the whole style dial: 0.15 reads as a chiselled hull, 0.9 as a
    pod. Control sections are Catmull-Rom resampled for the same reason
    meshgen gives — five hand-placed rings make ridges run across the body
    instead of around it.
    """
    if len(sections) < 2:
        return Mesh()
    if len(sections) >= 3:
        n = len(sections)
        get = lambda i: sections[max(0, min(n - 1, i))]
        dense = []
        for i in range(n - 1):
            for s in range(sub):
                dense.append(catmull(get(i - 1), get(i), get(i + 1), get(i + 2), s / sub))
        dense.append(sections[-1])
        sections = dense

    m = Mesh()
    rings = []
    for (z, hw, floor, roof, rnd) in sections:
        cy, ry = (floor + roof) * 0.5, (roof - floor) * 0.5
        idx = []
        for k in range(ring):
            a = 2 * math.pi * k / ring
            ca, sa = math.cos(a), math.sin(a)
            # box profile: project the circle onto the rectangle edge
            mx = max(abs(ca), abs(sa)) or 1.0
            bx, by = ca / mx, sa / mx
            x = hw * (rnd * ca + (1 - rnd) * bx)
            y = cy + ry * (rnd * sa + (1 - rnd) * by)
            idx.append(len(m.v))
            m.v.append((x, y, z))
        rings.append(idx)
    for r0, r1 in zip(rings, rings[1:]):
        for k in range(ring):
            j = (k + 1) % ring
            _quad(m, r0[k], r0[j], r1[j], r1[k])
    if cap:
        for r, flip in ((rings[0], True), (rings[-1], False)):
            cz = sum(m.v[i][2] for i in r) / len(r)
            cx = sum(m.v[i][0] for i in r) / len(r)
            cy = sum(m.v[i][1] for i in r) / len(r)
            c = len(m.v); m.v.append((cx, cy, cz))
            for k in range(ring):
                j = (k + 1) % ring
                m.f.append((c, r[j], r[k]) if flip else (c, r[k], r[j]))
    return m


def tube(seg=16, closed=True):
    """Unit cylinder, axis +y, radius 1, y in [-1,1]."""
    m = Mesh()
    top, bot = [], []
    for k in range(seg):
        a = 2 * math.pi * k / seg
        x, z = math.cos(a), math.sin(a)
        bot.append(len(m.v)); m.v.append((x, -1.0, z))
        top.append(len(m.v)); m.v.append((x, 1.0, z))
    for k in range(seg):
        j = (k + 1) % seg
        _quad(m, bot[k], bot[j], top[j], top[k])
    if closed:
        for ring_, y, flip in ((bot, -1.0, True), (top, 1.0, False)):
            c = len(m.v); m.v.append((0.0, y, 0.0))
            for k in range(seg):
                j = (k + 1) % seg
                m.f.append((c, ring_[j], ring_[k]) if flip else (c, ring_[k], ring_[j]))
    return m


def sphere(rows=10, cols=16):
    m = Mesh()
    grid = []
    for i in range(rows + 1):
        phi = math.pi * i / rows
        row = []
        for k in range(cols):
            th = 2 * math.pi * k / cols
            row.append(len(m.v))
            m.v.append((math.sin(phi) * math.cos(th), math.cos(phi), math.sin(phi) * math.sin(th)))
        grid.append(row)
    for i in range(rows):
        for k in range(cols):
            j = (k + 1) % cols
            _quad(m, grid[i][k], grid[i][j], grid[i + 1][j], grid[i + 1][k])
    return m


def torus(ratio=0.3, segs=20, sides=10):
    m = Mesh()
    grid = []
    for i in range(segs):
        a = 2 * math.pi * i / segs
        row = []
        for k in range(sides):
            b = 2 * math.pi * k / sides
            r = 1 + ratio * math.cos(b)
            row.append(len(m.v))
            m.v.append((r * math.cos(a), ratio * math.sin(b), r * math.sin(a)))
        grid.append(row)
    for i in range(segs):
        ii = (i + 1) % segs
        for k in range(sides):
            kk = (k + 1) % sides
            _quad(m, grid[i][k], grid[i][kk], grid[ii][kk], grid[ii][k])
    return m


def wheel(width=0.55, seg=18):
    """A tyre, not a squashed sphere — procars.rs records that the squashed
    sphere is 'most of why the older cars read as toys'."""
    m = Mesh()
    inner, outer = [], []
    for k in range(seg):
        a = 2 * math.pi * k / seg
        y, z = math.cos(a), math.sin(a)
        inner.append(len(m.v)); m.v.append((-width, y * 0.82, z * 0.82))
        outer.append(len(m.v)); m.v.append((width, y * 0.82, z * 0.82))
    tread_i, tread_o = [], []
    for k in range(seg):
        a = 2 * math.pi * k / seg
        y, z = math.cos(a), math.sin(a)
        tread_i.append(len(m.v)); m.v.append((-width * 0.86, y, z))
        tread_o.append(len(m.v)); m.v.append((width * 0.86, y, z))
    for k in range(seg):
        j = (k + 1) % seg
        _quad(m, tread_i[k], tread_i[j], tread_o[j], tread_o[k])   # tread band
        _quad(m, inner[k], inner[j], tread_i[j], tread_i[k])       # shoulders
        _quad(m, tread_o[k], tread_o[j], outer[j], outer[k])
    for ring_, x, flip in ((inner, -width, True), (outer, width, False)):
        c = len(m.v); m.v.append((x, 0.0, 0.0))
        for k in range(seg):
            j = (k + 1) % seg
            m.f.append((c, ring_[j], ring_[k]) if flip else (c, ring_[k], ring_[j]))
    return m


# ------------------------------------------------------------------ spec

@dataclass
class Spec:
    """One vehicle, as numbers. This is the thing that gets stored."""
    name: str
    cls: str                       # hoverer | car | flyer | spaceship
    role: str                      # the gameplay hypothesis, one line
    hull: list                     # [(z, hw, floor, roof, roundness)]
    ring: int = 16
    wheels: list = field(default_factory=list)   # [(x,y,z,r)]
    skirt: float = 0.0             # hoverer plenum, 0 = none
    nozzles: list = field(default_factory=list)  # [(x,y,z,r)]
    wings: list = field(default_factory=list)    # [(z,span,chord,y,dihedral)]
    fins: list = field(default_factory=list)     # [(z,h,chord,roll)]
    pods: list = field(default_factory=list)     # [(x,y,z,r)] spheres
    rings_deco: list = field(default_factory=list)  # [(z,r,tube)] toroids


def arch(hw, y, z, r, width):
    """A flared band over a wheel. Without it a wheel taller than the body
    stands proud and reads as bolted on — procars.rs calls these haunches and
    they are most of what makes a lofted hull look like bodywork."""
    a = Mesh()
    n = 12
    inner, outer = [], []
    for k in range(n + 1):
        t = math.pi * k / n                      # over the top, front to back
        cz, cy = math.cos(t), math.sin(t)
        for lst, xoff in ((inner, hw * 0.86), (outer, hw * 1.02)):
            lst.append(len(a.v))
            a.v.append((xoff, y + cy * r * 1.18, z - cz * r * 1.18))
    for k in range(n):
        _quad(a, inner[k], inner[k + 1], outer[k + 1], outer[k])
        _quad(a, outer[k], outer[k + 1], inner[k + 1], inner[k])   # two-sided
    _ = width
    return a


def build(s: Spec, ring=None) -> Mesh:
    m = loft(s.hull, ring or s.ring)
    for (x, y, z, r) in s.wheels:
        # hull half-width at this z, so the arch sits on the bodywork
        hw = _hw_at(s.hull, z)
        m.add(arch(hw if hw > 0 else abs(x), y, z, r, r * 0.5),
              scale=(1 if x >= 0 else -1, 1, 1))
    for (x, y, z, r) in s.wheels:
        m.add(wheel(), scale=(r * 0.5, r, r), off=(x, y, z))
    if s.skirt > 0:
        lo, hi = s.hull[0], s.hull[-1]
        zmid = (lo[0] + hi[0]) * 0.5
        span = abs(hi[0] - lo[0]) * 0.5
        widest = max(h[1] for h in s.hull)
        floor = min(h[2] for h in s.hull)
        # the plenum a cushion needs to work — procars.rs's own reasoning
        sk = [(lo[0] * 0.92, widest * 0.80, floor - s.skirt, floor + 0.02, 0.55),
              (zmid, widest * 1.04, floor - s.skirt * 1.15, floor + 0.02, 0.55),
              (hi[0] * 0.92, widest * 0.84, floor - s.skirt, floor + 0.02, 0.55)]
        m.add(loft(sk, ring or s.ring))
        _ = span
    for (x, y, z, r) in s.nozzles:
        m.add(tube(12), scale=(r, r * 1.2, r), off=(x, y, z))
    for (z, span, chord, y, dih) in s.wings:
        for side in (-1, 1):
            w = loft([(-chord * 0.5, 0.02, -0.02, 0.02, 0.2),
                      (0.0, chord * 0.5, -0.05, 0.06, 0.35),
                      (chord * 0.5, 0.02, -0.02, 0.02, 0.2)], 10)
            m.add(w, scale=(1, 1, 1), rot_y=math.pi / 2,
                  off=(side * span * 0.5, y + dih * span * 0.5, z))
    for (z, h, chord, roll) in s.fins:
        f = loft([(-chord * 0.5, 0.03, -0.02, 0.02, 0.2),
                  (0.0, chord * 0.5, 0.0, h, 0.25),
                  (chord * 0.5, 0.03, -0.02, 0.02, 0.2)], 8)
        m.add(f, rot_y=roll, off=(0, 0, z))
    for (x, y, z, r) in s.pods:
        m.add(sphere(8, 12), scale=(r, r, r), off=(x, y, z))
    for (z, r, t) in s.rings_deco:
        m.add(torus(t / max(r, 1e-6), 20, 8), scale=(r, r, r), rot_y=0.0, off=(0, 0, z))
    return m


def _hw_at(hull, z):
    """Linear half-width of the control polygon at z. Good enough to place an
    arch; the lofted surface is a spline through the same points."""
    pts = sorted(hull, key=lambda h: h[0])
    if z <= pts[0][0]: return pts[0][1]
    if z >= pts[-1][0]: return pts[-1][1]
    for a, b in zip(pts, pts[1:]):
        if a[0] <= z <= b[0]:
            t = (z - a[0]) / max(1e-6, b[0] - a[0])
            return a[1] * (1 - t) + b[1] * t
    return pts[-1][1]


def stats(m):
    (lo, hi) = m.bounds()
    return {"verts": len(m.v), "tris": len(m.f),
            "span": [round(hi[i] - lo[i], 3) for i in range(3)]}


# ───────────────────────────────────────────────── grit: the detail pass

def cbox(bevel=0.3):
    """Bevelled unit box, -1..1 on each axis. meshgen::cbox's counterpart —
    the workhorse for light bars, sills and splitters."""
    b = max(0.0, min(0.49, bevel))
    m = Mesh()
    k = 1.0 - b
    pts = []
    for sx in (-1, 1):
        for sy in (-1, 1):
            for sz in (-1, 1):
                # three bevel-chamfered corner verts per octant
                for (a, c, d) in ((k, 1, 1), (1, k, 1), (1, 1, k)):
                    pts.append((sx*a if (a, c, d) == (k, 1, 1) else sx*(a if a != 1 else 1),
                                sy*c, sz*d))
    # simpler and adequate: a loft with square-ish rings and rounded ends
    return loft([(-1.0, k, -k, k, 0.25),
                 (-k,   1.0, -1.0, 1.0, 0.18),
                 ( k,   1.0, -1.0, 1.0, 0.18),
                 ( 1.0, k, -k, k, 0.25)], 12)


def detail_car(s, m=None):
    """The grit pass, following procars.rs's own recipe: a glasshouse set into
    the hull, a light bar at each end, sill strips along the battery floor,
    and a front splitter. These are what separate a lofted blob from a
    vehicle — procars states it plainly and the first forge cars had none
    of them.

    Returns a list of (name, Mesh) so the GLB exporter can name each one;
    a merged mesh is the wrong shape now that named nodes are the point.
    """
    out = []
    zs = [h[0] for h in s.hull]
    z0, z1 = min(zs), max(zs)
    L = z1 - z0
    widest = max(h[1] for h in s.hull)
    roof = max(h[3] for h in s.hull)
    floor = min(h[2] for h in s.hull)
    mid = s.hull[len(s.hull) // 2]

    # glasshouse — a smaller loft riding on top, inset and smoked
    gz0, gz1 = z0 + L * 0.30, z0 + L * 0.78
    # sunk INTO the roof, not perched on it — at roof*0.90 the cabin's own
    # floor sat above the bodywork and the whole glasshouse read as a canopy
    # balanced on top.
    gw, gy = widest * 0.74, roof * 0.62
    cabin = [(gz0,                    gw * 0.72, gy,        gy + 0.02 * roof, 0.90),
             (gz0 + (gz1 - gz0) * .35, gw,       gy - 0.01, roof * 1.02,      0.85),
             (gz1,                    gw * 0.80, gy - 0.01, roof * 0.94,      0.90)]
    out.append(("glass", loft(cabin, 18)))

    # light bars, front and rear
    for nm, z, w in (("lightbar_f", z0 + 0.02, s.hull[0][1] * 0.86),
                     ("lightbar_r", z1 - 0.02, s.hull[-1][1] * 0.88)):
        b = Mesh()
        b.add(cbox(0.4), scale=(w, 0.016 * roof / 0.5, 0.030),
              off=(0.0, (floor + roof) * 0.5 + 0.02, z))
        out.append((nm, b))

    # NO SILLS.
    #
    # procars.rs has them ("the giveaway that the floor is a battery") and
    # they are the right idea, but three attempts failed here: a straight bar
    # pokes through wherever the hull tapers, and a loft that follows the
    # taper came out asymmetric because keeping one half of a rounded-rect
    # ring by sign of x does not cut evenly. Shipping a broken element is
    # worse than shipping none, so this is deliberately absent rather than
    # half-working. The fix is to build the strip as its own open surface
    # instead of carving it out of a closed loft.

    # front splitter — low, wide, and the single cheapest thing that makes a
    # car read as planted rather than floating
    sp = Mesh()
    # tucked UNDER the nose and pulled in: a splitter that overhangs the
    # bodywork in plan view reads as a shelf bolted on, not as aero.
    sp.add(cbox(0.2), scale=(s.hull[0][1] * 1.06, 0.009, L * 0.040),
           off=(0.0, floor + 0.008, z0 + L * 0.035))
    out.append(("splitter", sp))
    return out
