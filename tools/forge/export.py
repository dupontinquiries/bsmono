#!/usr/bin/env python3
"""export.py — forge model -> GLB with named nodes, ready for blackstar-rs."""
import math, os, sys
from forge import Mesh, loft, wheel, sphere, tube, torus, build, _hw_at, arch
from fleet import ALL
from bots import CHASSIS, pchest, phead, phand, plimb, psphere, pbevel
from glb import Glb, wheel_name


def export_vehicle(spec, path):
    g = Glb()
    # hull as one node at the origin
    hull = loft(spec.hull, spec.ring)
    g.add_node("hull", g.add_mesh(f"hull:{spec.name}", hull, "hull"))
    # ONE wheel mesh, referenced four times with its axle as the node
    # translation — that translation IS the pivot the game wants, and it no
    # longer has to be found by clustering vertices.
    if spec.wheels:
        wm = g.add_mesh("wheel", wheel(), "wheel")
        # Nose is at -z (forge.Spec's own convention), so split about the
        # midpoint of the axle positions. `zs[len//2]` picked the UPPER of two
        # values and tagged every wheel Front.
        zw = [w[2] for w in spec.wheels]
        mid = (min(zw) + max(zw)) * 0.5
        for (x, y, z, r) in spec.wheels:
            front = z < mid
            g.add_node(wheel_name(x, front), wm,
                       translation=(x, y, z), scale=(r*0.5, r, r))
            a = arch(_hw_at(spec.hull, z) or abs(x), y, z, r, r*0.5)
            g.add_node(f"arch_{'l' if x < 0 else 'r'}_{'f' if front else 'b'}",
                       g.add_mesh(f"arch:{spec.name}:{z:.2f}:{x<0}", a, "arch"))
    # skirt / wings / fins / rings — absent from the first pass, which is why
    # DART exported 380 of its 928 verts and came out a third of its width.
    if spec.skirt > 0:
        lo, hi = spec.hull[0], spec.hull[-1]
        widest = max(h[1] for h in spec.hull)
        floor = min(h[2] for h in spec.hull)
        sk = [(lo[0]*0.92, widest*0.80, floor-spec.skirt, floor+0.02, 0.55),
              ((lo[0]+hi[0])*0.5, widest*1.04, floor-spec.skirt*1.15, floor+0.02, 0.55),
              (hi[0]*0.92, widest*0.84, floor-spec.skirt, floor+0.02, 0.55)]
        g.add_node("skirt", g.add_mesh(f"skirt:{spec.name}", loft(sk, spec.ring), "skirt"))
    for i, (z, span, chord, y, dih) in enumerate(spec.wings):
        wmesh = loft([(-chord*0.5, 0.02, -0.02, 0.02, 0.2),
                      (0.0, chord*0.5, -0.05, 0.06, 0.35),
                      (chord*0.5, 0.02, -0.02, 0.02, 0.2)], 10)
        mi = g.add_mesh(f"wing:{spec.name}:{i}", wmesh, "wing")
        for side, sfx in ((-1, "l"), (1, "r")):
            # rot_y = pi/2 as a quaternion about +Y
            q = (0.0, math.sin(math.pi/4), 0.0, math.cos(math.pi/4))
            g.add_node(f"wing_{sfx}_{i}", mi, rotation=q,
                       translation=(side*span*0.5, y + dih*span*0.5, z))
    for i, (z, h_, chord, roll) in enumerate(spec.fins):
        fmesh = loft([(-chord*0.5, 0.03, -0.02, 0.02, 0.2),
                      (0.0, chord*0.5, 0.0, h_, 0.25),
                      (chord*0.5, 0.03, -0.02, 0.02, 0.2)], 8)
        q = (0.0, math.sin(roll*0.5), 0.0, math.cos(roll*0.5))
        g.add_node(f"fin_{i}", g.add_mesh(f"fin:{spec.name}:{i}", fmesh, "fin"),
                   rotation=q, translation=(0, 0, z))
    for i, (z, r, t) in enumerate(spec.rings_deco):
        g.add_node(f"ring_{i}", g.add_mesh(f"ring:{spec.name}:{i}",
                   torus(t/max(r,1e-6), 20, 8), "ring"),
                   translation=(0, 0, z), scale=(r, r, r))
    for i, (x, y, z, r) in enumerate(spec.nozzles):
        g.add_node(f"nozzle_{i}", g.add_mesh("nozzle", tube(12), "nozzle"),
                   translation=(x, y, z), scale=(r, r*1.2, r))
    for i, (x, y, z, r) in enumerate(spec.pods):
        g.add_node(f"pod_{i}", g.add_mesh("pod", sphere(8, 12), "pod"),
                   translation=(x, y, z), scale=(r, r, r))
    return g.write(path, spec.name)


def export_bot(c, path):
    g = Glb()
    hip = 0.88 * c.h * c.leg
    chest_y = hip + 0.06 * c.h
    sx = 0.27 * c.bulk + 0.13 * c.shoulder * 0.75
    upper, fore = 0.60*0.58*c.arm, 0.60*0.54*c.arm
    thigh, shin = 0.60*0.72*c.leg, 0.60*0.70*c.leg
    DOWN = (math.sin(math.pi/2), 0.0, 0.0, math.cos(math.pi/2))   # rot_x = pi

    g.add_node("torso", g.add_mesh(f"chest:{c.name}", pchest(c.bulk, c.shoulder)),
               translation=(0, chest_y, 0))
    g.add_node("head", g.add_mesh(f"head:{c.name}", phead(c.head*0.86)),
               translation=(0, chest_y + 0.60*c.h, 0.02))
    jm = g.add_mesh("joint", psphere(1.0), "joint")
    lm = g.add_mesh(f"limb:{c.name}", plimb(1.0, 1.0), "limb")
    hm = g.add_mesh(f"hand:{c.name}", phand(1.0), "hand")

    for side, sfx in ((-1, "l"), (1, "r")):
        sh = (side*sx, chest_y + 0.50*c.h, 0.03)
        el = (sh[0], sh[1]-upper, sh[2]); wr = (el[0], el[1]-fore, el[2])
        # Each joint is a NODE TRANSLATION. That is the hinge — no inference.
        g.add_node(f"shoulder_{sfx}", jm, translation=sh,
                   scale=(0.082*c.shoulder,)*3)
        g.add_node(f"upperarm_{sfx}", lm, translation=sh, rotation=DOWN,
                   scale=(0.95*c.bulk, 0.58*c.arm, 0.95*c.bulk))
        g.add_node(f"elbow_{sfx}", jm, translation=el, scale=(0.062,)*3)
        g.add_node(f"forearm_{sfx}", lm, translation=el, rotation=DOWN,
                   scale=(0.86*c.bulk, 0.54*c.arm, 0.86*c.bulk))
        g.add_node(f"wrist_{sfx}", jm, translation=wr, scale=(0.048,)*3)
        g.add_node(f"hand_{sfx}", hm, translation=(wr[0], wr[1]-0.055, wr[2]),
                   scale=(0.62,)*3)

        hp = (side*0.145*c.bulk, hip, 0.0)
        kn = (hp[0], hp[1]-thigh, hp[2]); ak = (kn[0], kn[1]-shin, kn[2])
        g.add_node(f"hip_{sfx}", jm, translation=hp, scale=(0.088*c.bulk,)*3)
        g.add_node(f"thigh_{sfx}", lm, translation=hp, rotation=DOWN,
                   scale=(1.12*c.bulk, 0.72*c.leg, 1.12*c.bulk))
        g.add_node(f"knee_{sfx}", jm, translation=kn, scale=(0.072,)*3)
        g.add_node(f"shin_{sfx}", lm, translation=kn, rotation=DOWN,
                   scale=(1.0*c.bulk, 0.70*c.leg, 1.0*c.bulk))
        g.add_node(f"ankle_{sfx}", jm, translation=ak, scale=(0.056,)*3)
        foot = loft([(-0.09,0.052,0.0,0.075,0.30),(0.04,0.060,0.0,0.055,0.28),
                     (0.15,0.045,0.0,0.030,0.30)], 8)
        g.add_node(f"foot_{sfx}", g.add_mesh(f"foot:{c.name}", foot, "foot"),
                   translation=(ak[0], ak[1]-0.052, ak[2]-0.01),
                   scale=(c.bulk, 1.0, 1.0))
    return g.write(path, c.name)


if __name__ == "__main__":
    out = sys.argv[1] if len(sys.argv) > 1 else "glb"
    os.makedirs(out, exist_ok=True)
    tot = 0
    for s in ALL:
        n = export_vehicle(s, f"{out}/{s.name}.glb"); tot += n
        print(f"  {s.name:<10} {s.cls:<10} {n:>8} B")
    for c in CHASSIS:
        n = export_bot(c, f"{out}/{c.name}.glb"); tot += n
        print(f"  {c.name:<10} {'bot':<10} {n:>8} B")
    print(f"\n{len(ALL)+len(CHASSIS)} GLB, {tot/1024:.0f} KB total")
