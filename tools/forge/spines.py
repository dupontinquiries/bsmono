#!/usr/bin/env python3
"""spines.py — the ten backpieces, one per signature.

`content/signatures.json` gives each a stat block; `part_groups.json` gives
each a mesh slot (`spine_*.iqm`, origin `base`). Ten backpieces across seven
chassis is seventy silhouettes from sixteen parts, which is why these are
worth more care than the chassis were.

## The rule these follow

**The silhouette states the trade before you read the numbers.** The stats
already form a clean axis — mortar is dmg 1.45 / speed 0.75 / splash 11,
recon is dmg 0.95 / speed 1.2 / splash 0 — so:

  * slow, heavy, high-damage  -> mass sits LOW and WIDE, close to the spine,
    with a visible muzzle. It should look like it is hard to carry.
  * fast, light, low-damage   -> thin, HIGH, swept back. Nothing below the
    shoulder line.
  * defensive (low taken_mul) -> presents a flat face outward; no muzzle.
  * control (knockback/slow)  -> one long protruding element, pointed.
  * area (splash)             -> radial symmetry, because splash is a circle.

A backpiece that breaks the rule is a bug, not a style choice: a player
learns the roster by silhouette long before they learn the table.

Every piece is built with its origin at the mount (`origin: base`), +y up,
-z forward, so the assembly does not have to know anything about them.
"""
import math
from forge import Mesh, loft, sphere, tube, torus


# ───────────────────────────────────────────────────── 0  wing_cannon
# dmg .95  taken 1.15  speed 1.1  +1 pellet — fast, fragile, spread.
def wing_cannon():
    m = Mesh()
    # a central mount, or the two wings read as two loose objects
    m.add(loft([(-0.05, 0.062, 0.00, 0.13, 0.55),
                ( 0.12, 0.070, 0.00, 0.15, 0.58)], 8), off=(0, 0.05, 0.05))
    for side in (-1, 1):
        # swept back and UP: nothing below the shoulder, reads as light
        w = loft([(-0.05, 0.030, -0.02, 0.02, 0.30),
                  ( 0.22, 0.075, -0.03, 0.05, 0.35),
                  ( 0.52, 0.030, -0.02, 0.02, 0.30)], 8)
        m.add(w, rot_y=side * 0.38, off=(side * 0.10, 0.26, 0.02))
        # two muzzles = the extra pellet, set INTO the wing's leading edge
        for k in (-1, 1):
            m.add(tube(8), scale=(0.019, 0.10, 0.019), rot_x=math.pi / 2,
                  off=(side * (0.26 + k * 0.045), 0.27, -0.15))
    return m


# ───────────────────────────────────────────────────── 1  heavy_cannon
# dmg 1.35  fire 1.35  speed .9  knock 6  splash 3 — one big gun.
def heavy_cannon():
    m = Mesh()
    m.add(loft([(-0.10, 0.10, 0.00, 0.20, 0.45),
                ( 0.16, 0.13, 0.00, 0.26, 0.50),
                ( 0.34, 0.09, 0.02, 0.19, 0.45)], 10), off=(0, 0.06, 0.06))
    # a single long barrel over the shoulder, muzzle forward
    HALF = math.pi / 2
    barrel = tube(12)
    m.add(barrel, scale=(0.055, 0.32, 0.055), rot_x=HALF, off=(0.07, 0.30, -0.14))
    m.add(barrel, scale=(0.078, 0.07, 0.078), rot_x=HALF, off=(0.07, 0.30, -0.50))
    m.add(sphere(6, 8), scale=(0.08, 0.08, 0.09), off=(0.07, 0.30, 0.16))
    return m


# ───────────────────────────────────────────────────── 2  crest
# taken .8  knock 4 — a brace. Defensive: a flat face, no muzzle.
def crest():
    m = Mesh()
    fin = loft([(-0.06, 0.022, 0.00, 0.34, 0.15),
                ( 0.10, 0.030, 0.00, 0.44, 0.15),
                ( 0.30, 0.020, 0.00, 0.22, 0.15)], 6)
    m.add(fin, off=(0.0, 0.06, 0.02))
    for side in (-1, 1):
        m.add(fin, scale=(0.8, 0.72, 0.9), rot_y=side * 0.30,
              off=(side * 0.13, 0.05, 0.05))
    return m


# ───────────────────────────────────────────────────── 3  cable
# dmg .85  fire .7  speed 1.15  slow .3 — a debuffer. Trailing, no mass.
def cable():
    m = Mesh()
    m.add(loft([(-0.05, 0.075, 0.00, 0.14, 0.55),
                ( 0.14, 0.085, 0.00, 0.16, 0.60)], 8), off=(0, 0.05, 0.05))
    for i, side in enumerate((-1, 1, -1, 1)):
        # a slack catenary, drooping behind — the only piece with mass BELOW
        # the mount, because a cable is the one thing that hangs. Links
        # overlap so it reads as a chain rather than as scattered beads.
        n, spread = 11, 1 + 0.35 * (i // 2)
        for k in range(n):
            u = k / (n - 1)
            x = side * (0.045 + 0.055 * u) * spread
            y = 0.13 - 0.34 * (u ** 1.8)
            z = 0.09 + 0.30 * u
            r = 0.026 * (1 - 0.45 * u)          # tapers toward the tip
            m.add(sphere(4, 6), scale=(r, r, r), off=(x, y, z))
    return m


# ───────────────────────────────────────────────────── 4  harpoon
# knock 10  slow .4 — control. One long pointed element, unmistakable.
def harpoon():
    m = Mesh()
    m.add(loft([(-0.06, 0.075, 0.00, 0.15, 0.50),
                ( 0.16, 0.085, 0.00, 0.17, 0.52)], 8), off=(0, 0.05, 0.06))
    # the spike: long, forward, barbed
    m.add(loft([(-0.42, 0.014, -0.014, 0.014, 0.7),
                (-0.20, 0.030, -0.030, 0.030, 0.7),
                ( 0.16, 0.024, -0.024, 0.024, 0.7)], 8), off=(0.0, 0.26, 0.0))
    for side in (-1, 1):                              # barbs
        m.add(loft([(-0.02, 0.010, -0.010, 0.010, 0.5),
                    ( 0.09, 0.028, -0.028, 0.028, 0.5)], 6),
              rot_y=side * 0.9, off=(side * 0.02, 0.26, -0.20))
    m.add(torus(0.35, 12, 6), scale=(0.075, 0.075, 0.075), off=(0.0, 0.20, 0.18))  # reel
    return m


# ───────────────────────────────────────────────────── 5  halo
# splash 6  speed 1.05 — area. Splash is a circle, so this is a circle.
def halo():
    m = Mesh()
    m.add(loft([(-0.05, 0.065, 0.00, 0.12, 0.6),
                ( 0.12, 0.070, 0.00, 0.13, 0.6)], 8), off=(0, 0.04, 0.05))
    r = 0.30
    ring = torus(0.085 / r, 22, 8)
    m.add(ring, scale=(r, r, r), off=(0.0, 0.40, 0.04))     # lying flat, above
    for k in range(6):                                       # emitters on the ring
        a = 2 * math.pi * k / 6
        m.add(sphere(5, 7), scale=(0.030, 0.030, 0.030),
              off=(r * math.cos(a), 0.40, 0.04 + r * math.sin(a)))
    m.add(loft([(0.0, 0.020, 0.00, 0.30, 0.6),
                (0.06, 0.020, 0.00, 0.30, 0.6)], 6), off=(0.0, 0.10, 0.04))  # mast
    return m


# ───────────────────────────────────────────────────── 6  solar
# dmg 1.2  splash 4 — panels. Flat area held out, collecting.
def solar():
    m = Mesh()
    m.add(loft([(-0.05, 0.070, 0.00, 0.13, 0.5),
                ( 0.14, 0.078, 0.00, 0.15, 0.5)], 8), off=(0, 0.05, 0.05))
    # a mast the panels actually hang off — free-floating panels read as an
    # exploded view, not as hardware
    m.add(loft([(0.0, 0.018, 0.00, 0.44, 0.6),
                (0.05, 0.018, 0.00, 0.44, 0.6)], 6), off=(0.0, 0.10, 0.07))
    for side in (-1, 1):
        for k, tilt in enumerate((0.20, -0.16)):
            panel = loft([(-0.18, 0.012, -0.012, 0.012, 0.1),
                          ( 0.00, 0.115, -0.014, 0.014, 0.1),
                          ( 0.18, 0.012, -0.012, 0.012, 0.1)], 6)
            m.add(panel, rot_y=side * (0.5 + tilt),
                  off=(side * 0.10, 0.26 + k * 0.17, 0.08))
    return m


# ───────────────────────────────────────────────────── 7  aegis
# taken .7 — the best survivability in the set. A wall, presented outward.
def aegis():
    m = Mesh()
    m.add(loft([(-0.06, 0.085, 0.00, 0.14, 0.4),
                ( 0.16, 0.095, 0.00, 0.16, 0.4)], 8), off=(0, 0.05, 0.06))
    for side in (-1, 1):
        slab = loft([(-0.14, 0.028, 0.00, 0.30, 0.10),
                     ( 0.00, 0.034, 0.00, 0.38, 0.10),
                     ( 0.14, 0.026, 0.00, 0.28, 0.10)], 6)
        # rotated to face OUTWARD and forward: a shield you hide behind
        m.add(slab, rot_y=side * 1.25, off=(side * 0.19, 0.08, -0.02))
    return m


# ───────────────────────────────────────────────────── 8  recon
# speed 1.2 (fastest)  dmg .95 — minimal. Almost nothing there.
def recon():
    m = Mesh()
    m.add(loft([(-0.04, 0.052, 0.00, 0.10, 0.65),
                ( 0.10, 0.058, 0.00, 0.11, 0.65)], 8), off=(0, 0.04, 0.05))
    m.add(loft([(0.0, 0.010, 0.00, 0.46, 0.8),
                (0.04, 0.008, 0.00, 0.46, 0.8)], 6), off=(0.03, 0.09, 0.06))
    m.add(sphere(6, 9), scale=(0.055, 0.040, 0.055), off=(0.03, 0.50, 0.06))
    m.add(torus(0.30, 14, 6), scale=(0.10, 0.10, 0.10), rot_y=0.4,
          off=(-0.06, 0.34, 0.06))
    return m


# ───────────────────────────────────────────────────── 9  mortar
# dmg 1.45  knock 14  splash 11  speed .75 — the heaviest thing here.
def mortar():
    m = Mesh()
    # a broad low base: it should look like it is hard to carry
    m.add(loft([(-0.14, 0.135, 0.00, 0.18, 0.35),
                ( 0.06, 0.150, 0.00, 0.22, 0.38),
                ( 0.26, 0.120, 0.00, 0.17, 0.35)], 10), off=(0, 0.04, 0.06))
    for (dx, dz) in ((-0.085, -0.04), (0.085, -0.04), (0.0, 0.10)):
        # tubes angled UP and back: indirect fire, stated by the angle
        m.add(tube(10), scale=(0.048, 0.20, 0.048), off=(dx, 0.30, dz + 0.04))
        m.add(torus(0.30, 10, 6), scale=(0.055, 0.055, 0.055), off=(dx, 0.50, dz + 0.04))
    m.add(sphere(6, 8), scale=(0.085, 0.060, 0.085), off=(0.0, 0.08, 0.20))   # counterweight
    return m


SPINES = [
    ("wing_cannon",  wing_cannon,  "fast, fragile, spread: swept wings, twin muzzles, nothing below the shoulder"),
    ("heavy_cannon", heavy_cannon, "one big gun: breech mass behind, muzzle brake forward"),
    ("crest",        crest,        "a brace: flat faces, no muzzle anywhere"),
    ("cable",        cable,        "a debuffer: the only piece with mass BELOW the mount, because cables hang"),
    ("harpoon",      harpoon,      "control: one long barbed spike and a reel"),
    ("halo",         halo,         "area: splash is a circle, so this is a circle"),
    ("solar",        solar,        "panels held out flat, two tiers"),
    ("aegis",        aegis,        "best survivability: a wall presented outward"),
    ("recon",        recon,        "fastest, least damage: a mast and a dish, almost nothing there"),
    ("mortar",       mortar,       "heaviest and slowest: broad low base, three tubes angled up"),
]
