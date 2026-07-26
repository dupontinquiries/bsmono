#!/usr/bin/env python3
"""gen_world.py -- generates the arena base collision hull plus several
THEMED maps with genuinely distinct geometry (not just retextures), palettes
adapted from the unendlich biome set, and a story landmark each.

  arena_base.iqm  -- floor + perimeter walls only (collision hull; the game
                     keeps this for collision and moves the model off-screen)
  wastes.iqm      -- rusted expanse: broken slabs, leaning girders, dust
  frostline.iqm   -- ice shelf: cracked plates, jagged ice spires
  emberpit.iqm    -- sunken lava bowl ringed by obsidian tiers, center dais
  vault.iqm       -- crystal vault: pale floor, tall crystal columns, gold veins

Each themed map is VISUAL ONLY (no collision) so it can be swapped freely; the
shared base hull provides floor/wall collision and the FSM cover provides
destructible tactical collision on top.

Regen: python3 tools/gen_world.py
"""
import struct, os, math, random

S = 85.0  # arena half-size (170x170) -- expanded for larger fights


# ---- terrain noise: gradient (Perlin) noise + fBm + ridged, for natural relief ----
def _grad(ix, iz, seed):
    h = (ix * 374761393 + iz * 668265263 + seed * 1442695041) & 0xFFFFFFFF
    h = ((h ^ (h >> 13)) * 1274126177) & 0xFFFFFFFF
    a = (h & 0xFFFF) / 65535.0 * 6.28318530718
    return math.cos(a), math.sin(a)


def _perlin(x, z, seed):
    """2D gradient (Perlin) noise in ~[-0.75, 0.75] with a smootherstep fade."""
    ix, iz = math.floor(x), math.floor(z)
    fx, fz = x - ix, z - iz

    def dg(cx, cz):
        gx, gz = _grad(cx, cz, seed)
        return gx * (x - cx) + gz * (z - cz)

    u = fx * fx * fx * (fx * (fx * 6 - 15) + 10)
    v = fz * fz * fz * (fz * (fz * 6 - 15) + 10)
    n00, n10 = dg(ix, iz), dg(ix + 1, iz)
    n01, n11 = dg(ix, iz + 1), dg(ix + 1, iz + 1)
    nx0 = n00 + u * (n10 - n00)
    nx1 = n01 + u * (n11 - n01)
    return nx0 + v * (nx1 - nx0)


def _fbm(x, z, seed, oct=5, lac=2.0, gain=0.5):
    """Fractal Brownian motion -- summed octaves of Perlin, normalised to ~[-1, 1]."""
    a, f, s, norm = 0.5, 1.0, 0.0, 0.0
    for o in range(oct):
        s += a * _perlin(x * f, z * f, seed + o * 101)
        norm += a; a *= gain; f *= lac
    return s / norm if norm else 0.0


def _ridged(x, z, seed, oct=4):
    """Ridged multifractal -- sharp mountain ridges, in ~[0, 1]."""
    a, f, s, norm = 0.5, 1.0, 0.0, 0.0
    for o in range(oct):
        n = 1.0 - abs(_perlin(x * f, z * f, seed + o * 53))
        n *= n
        s += a * n; norm += a; a *= 0.5; f *= 2.0
    return s / norm if norm else 0.0


def _seg_dist(px, pz, ax, az, bx, bz):
    """Distance from point (px,pz) to segment (ax,az)-(bx,bz)."""
    dx, dz = bx - ax, bz - az
    ll = dx * dx + dz * dz
    if ll < 1e-6:
        return math.hypot(px - ax, pz - az)
    t = max(0.0, min(1.0, ((px - ax) * dx + (pz - az) * dz) / ll))
    return math.hypot(px - (ax + t * dx), pz - (az + t * dz))

def new_mesh():
    return {"floor": [], "wall": [], "rock": []}

class Builder:
    def __init__(self):
        self.verts = []  # (pos, uv, normal)
        self.tris = []   # (a,b,c)
        self.buckets = {} # material -> list of (v0,nv,t0,nt)
        self.colliders = []  # (cx,cy,cz,hx,hy,hz) of solid blockers
    def _quad(self, mat, a, b, c, d, n, uvs=0.12, double=True):
        base = len(self.verts)
        ax = 0 if abs(n[0]) < 0.9 else 2
        ay = 1 if abs(n[1]) < 0.9 else 2
        for p in (a, b, c, d):
            self.verts.append((p, (p[ax]*uvs, p[ay]*uvs), n))
        t0 = len(self.tris)
        self.tris.append((base, base+1, base+2)); self.tris.append((base, base+2, base+3))
        if double:
            self.tris.append((base, base+2, base+1)); self.tris.append((base, base+3, base+2))
        self.buckets.setdefault(mat, []).append((base, 4, t0, 4 if double else 2))
    def city_tower(self, mat, x, z, h, w, emit="energon.png"):
        """A READABLE tower: tapered setback tiers, lit window bands between them, a
        roof cap + antenna. One collider (the trunk); the dressing is free."""
        tiers = 3
        for t in range(tiers):
            th = h * (0.42 if t == 0 else 0.32 if t == 1 else 0.26)
            ty = sum(h * f for f in ([0.42, 0.32] [:t])) if t else 0.0
            tw = w * (1.0 - t * 0.18)
            self.box(mat, x, ty + th * 0.5, z, tw, th * 0.5, tw * 1.25, collide=(t <= 1))
            if t < tiers - 1:   # lit band at each setback
                self.box(emit, x, ty + th, z, tw * 0.92, 0.16, tw * 1.18, collide=False)
        self.box(mat, x, h + 0.25, z, w * 0.5, 0.25, w * 0.6, collide=False)      # roof cap
        self.spire(mat, x, z, h * 0.22, 0.14, collide=False)                       # antenna
        self.box(emit, x, h + h * 0.22, z, 0.3, 0.3, 0.3, collide=False)           # beacon
    def stack(self, mat, x, z, h, r=1.4, emit="molten.png"):
        """A refinery CHIMNEY: base drum, tapered trunk, lit vent ring, top lip --
        industrial silhouette instead of a naked pole."""
        self.box(mat, x, 0.9, z, r * 1.5, 0.9, r * 1.5)                            # base drum
        self.box(mat, x, h * 0.5, z, r, h * 0.5, r)                                # trunk
        self.box(emit, x, h * 0.72, z, r * 1.12, 0.25, r * 1.12, collide=False)    # lit vent ring
        self.box(mat, x, h + 0.2, z, r * 1.25, 0.2, r * 1.25, collide=False)       # top lip
    def derrick(self, mat, x, z, h, emit="energon.png"):
        """A mining DERRICK: splayed leg pair, cap platform, wheel-house glow, and a
        cable mast -- reads as machinery, not a spike."""
        self.box(mat, x - 1.6, h * 0.45, z, 0.5, h * 0.45, 0.5)                    # leg (collides)
        self.box(mat, x + 1.6, h * 0.45, z, 0.5, h * 0.45, 0.5)                    # leg (collides)
        self.box(mat, x, h * 0.9, z, 2.6, 0.3, 1.6, collide=False)                 # cap platform
        self.box(emit, x, h * 0.9 + 0.5, z, 0.7, 0.5, 0.7, collide=False)          # wheel-house glow
        self.spire(mat, x, z, h * 1.25, 0.3, collide=False)                        # cable mast
    def height_bands(self, cx, cz, base_y, top_y, hx, hz, mats):
        # SC2 "VALUE-CONTRAST ELEVATION" (Blizzard best-practices: height tiers
        # need contrasting color VALUE so players read high vs low ground). Our
        # mesh has no per-vertex colour, so value contrast is done by MATERIAL:
        # ring the structure with thin lit bands whose texture steps from a dim
        # base to a bright top -- elevation now reads as increasing brightness.
        n = len(mats)
        if n < 2 or top_y <= base_y: return
        for i in range(n):
            t = i / (n - 1)
            y = base_y + (top_y - base_y) * t
            m = mats[i]
            self.box(m, cx, y, cz + hz, hx * 1.02, 0.10, 0.05, collide=False)  # front band
            self.box(m, cx, y, cz - hz, hx * 1.02, 0.10, 0.05, collide=False)  # back band
    def eye_trail(self, emit, x0, z0, x1, z1, n=6, w=0.6):
        # SC2 "LEAD THE EYE" (Blizzard best-practices, Painting & Effects): a line
        # of low lit markers from (x0,z0) toward a key area, so the player's eye is
        # drawn along the route instead of guessing. Pure dressing (collide=False).
        import math as _m
        for i in range(n):
            t = (i + 0.5) / n
            mx = x0 + (x1 - x0) * t; mz = z0 + (z1 - z0) * t
            self.box(emit, mx, 0.06, mz, w, 0.05, w, collide=False)
    def varied_scatter(self, ground, props, cx, cz, r=10.0, count=8, solid_frac=0.4, seedy=1):
        # DESTINY/open-world IMMERSION: unlike ground_cluster (one repeated prop),
        # this mixes a LIST of prop meshes at varied sizes + rotations with no
        # overlap, and makes solid_frac of them REAL destructible-style colliders
        # (interactive cover you move through/around, not just decoration). Turns
        # big patrol spaces from repeated-prop wallpaper into varied, tactical
        # terrain. Grounded on a texture patch so props read as planted.
        self.box(ground, cx, 0.04, cz, r*1.15, 0.04, r*1.15, collide=False)   # grounding patch
        rs = random.Random(0xD5A1 ^ (int(cx)*733) ^ (int(cz)*917) ^ seedy)
        placed = []
        for _ in range(count*5):
            if len(placed) >= count: break
            a = rs.uniform(0, 6.283); rr = rs.uniform(r*0.2, r*0.92)
            px, pz = cx + rr*math.cos(a), cz + rr*math.sin(a)
            if any((px-qx)**2 + (pz-qz)**2 < 5.5 for qx,qz in placed): continue   # spacing
            placed.append((px, pz))
            prop = props[rs.randrange(len(props))]                 # VARIED: pick from the set
            solid = (rs.random() < solid_frac)                     # solid = real cover
            if solid:
                # eye-height cover you hide behind: taller, chunkier. Wide height
                # range so the field mixes low blocks with tall pillars (verticality).
                s = rs.uniform(1.0, 3.0)
                hx, hz = s*rs.uniform(0.55,0.9), s*rs.uniform(0.55,0.9)
                self.box(prop, px, s, pz, hx, s, hz, collide=True)
                if rs.random() < 0.4:                              # some get a broken cap
                    self.box(prop, px+rs.uniform(-0.3,0.3), s*2+0.2, pz+rs.uniform(-0.3,0.3),
                             hx*0.6, s*0.35, hz*0.6, collide=False)
            else:
                # low decorative debris: shoot over it, reads as scattered scrap
                s = rs.uniform(0.4, 0.9)
                hx, hz = s*rs.uniform(0.5,0.9), s*rs.uniform(0.5,0.9)
                self.box(prop, px, s*0.6, pz, hx, s*0.6, hz, collide=False)
    def ground_cluster(self, ground, prop, cx, cz, r=6.0, count=5, seedy=1):
        # SC2 "DETAIL IN THE MARGINS, GROUNDED" (best-practices, Doodads +
        # Painting): a SPACED cluster of small props (no overlap -- the doodad
        # density rule) sitting on a painted ground patch so they read as planted,
        # not floating on mismatched terrain. Margin dressing, all collide=False.
        self.box(ground, cx, 0.04, cz, r * 1.15, 0.04, r * 1.15, collide=False)  # grounding patch
        rs = random.Random(0x51C2 ^ (int(cx) * 733) ^ (int(cz) * 917) ^ seedy)
        placed = []
        for _ in range(count * 4):
            if len(placed) >= count: break
            a = rs.uniform(0, 6.283); rr = rs.uniform(r * 0.25, r * 0.9)
            px, pz = cx + rr * math.cos(a), cz + rr * math.sin(a)
            if any((px - qx) ** 2 + (pz - qz) ** 2 < 4.0 for qx, qz in placed): continue  # spacing, no overlap
            placed.append((px, pz))
            s = rs.uniform(0.4, 0.9)
            self.box(prop, px, s, pz, s * 0.6, s, s * 0.6, collide=False)
    def trim(self, mat, cx, y_top, cz, hx, hz, emit="energon.png"):
        # ANIMATION-STUDIO LANDMARK DRESSING (all collide=False): a parapet of
        # crown ridges on the four top edges + one lit service-strip on the
        # front face, so plain towers read as BUILT structures. One call per
        # tower inside existing loops multiplies detail across whole maps.
        t = 0.16
        self.box(mat, cx,      y_top+t, cz+hz,  hx+t, t, t, collide=False)
        self.box(mat, cx,      y_top+t, cz-hz,  hx+t, t, t, collide=False)
        self.box(mat, cx+hx,   y_top+t, cz,     t, t, hz+t, collide=False)
        self.box(mat, cx-hx,   y_top+t, cz,     t, t, hz+t, collide=False)
        self.box(emit, cx, y_top-0.45, cz+hz+0.06, hx*0.55, 0.08, 0.06, collide=False)
    def tier_pad(self, mat, cx, cz, hx, hz, h, rim="energon.png", ground="rust.png"):
        # SC2 LESSON (Blizzard 'Mastering Mapmaking'): height levels must READ --
        # a raised platform whose edge is marked with a contrasting rim so players
        # can tell they're approaching high ground. Plus (Nibbits) a darker ground
        # decal under the landmark so it pops from the floor. The pad itself
        # collides; rim + decal are dressing (collide=False).
        self.box(mat, cx, h*0.5, cz, hx, h*0.5, hz)                                  # the raised deck
        # contrasting rim around the top edge -- the 'cliff colour value' read
        r = 0.14
        self.box(rim, cx, h+r, cz+hz, hx+r, r, r, collide=False)
        self.box(rim, cx, h+r, cz-hz, hx+r, r, r, collide=False)
        self.box(rim, cx+hx, h+r, cz, r, r, hz+r, collide=False)
        self.box(rim, cx-hx, h+r, cz, r, r, hz+r, collide=False)
        # darker ground decal ringing the base -- reinforces the landmark
        self.box(ground, cx, 0.04, cz, hx+1.4, 0.04, hz+1.4, uvs=0.22, collide=False)
    def pylon(self, mat, x, z, h, emit="energon.png"):
        """A READABLE mast: base collar, trunk, two crossarms with lit insulators,
        and a beacon -- instead of a naked pole."""
        self.box(mat, x, 0.5, z, 1.3, 0.5, 1.3)                                    # base collar
        self.spire(mat, x, z, h, 0.55)                                             # the trunk
        for k, hy in ((0, h * 0.62), (1, h * 0.85)):
            aw = 2.2 - k * 0.7
            self.box(mat, x, hy, z, aw, 0.14, 0.3, collide=False)                  # crossarm
            self.box(emit, x - aw * 0.85, hy - 0.3, z, 0.16, 0.3, 0.16, collide=False)
            self.box(emit, x + aw * 0.85, hy - 0.3, z, 0.16, 0.3, 0.16, collide=False)
        self.box(emit, x, h + 0.3, z, 0.22, 0.22, 0.22, collide=False)             # beacon
    def box(self, mat, cx, cy, cz, hx, hy, hz, uvs=0.12, collide=True):
        if collide: self.colliders.append((cx,cy,cz,hx,hy,hz))
        x0,x1,y0,y1,z0,z1 = cx-hx,cx+hx,cy-hy,cy+hy,cz-hz,cz+hz
        self._quad(mat,(x0,y1,z0),(x0,y1,z1),(x1,y1,z1),(x1,y1,z0),(0,1,0),uvs)
        self._quad(mat,(x0,y0,z1),(x0,y0,z0),(x1,y0,z0),(x1,y0,z1),(0,-1,0),uvs)
        self._quad(mat,(x0,y0,z1),(x1,y0,z1),(x1,y1,z1),(x0,y1,z1),(0,0,1),uvs)
        self._quad(mat,(x1,y0,z0),(x0,y0,z0),(x0,y1,z0),(x1,y1,z0),(0,0,-1),uvs)
        self._quad(mat,(x1,y0,z1),(x1,y0,z0),(x1,y1,z0),(x1,y1,z1),(1,0,0),uvs)
        self._quad(mat,(x0,y0,z0),(x0,y0,z1),(x0,y1,z1),(x0,y1,z0),(-1,0,0),uvs)
    def floor(self, mat, half=S, y=0.0):
        self._quad(mat,(-half,y,-half),(-half,y,half),(half,y,half),(half,y,-half),(0,1,0),0.08)
    def walls(self, mat, half=S, h=8.0, t=1.0):
        self.box(mat, 0, h/2,  half+t, half+2*t, h/2, t, collide=False)
        self.box(mat, 0, h/2, -half-t, half+2*t, h/2, t, collide=False)
        self.box(mat, half+t, h/2, 0, t, h/2, half, collide=False)
        self.box(mat, -half-t, h/2, 0, t, h/2, half, collide=False)
    def spire(self, mat, cx, cz, h, r, collide=True):
        # a tapered 4-sided spike (two stacked shrinking boxes)
        self.box(mat, cx, h*0.25, cz, r, h*0.25, r, collide=collide)
        self.box(mat, cx, h*0.7, cz, r*0.55, h*0.25, r*0.55, collide=collide)
    def arch(self, mat, cx, cz, w, h, d=2.0):
        # a gateway: two legs + a lintel across the top
        lr = w*0.5
        self.box(mat, cx-lr, h*0.5, cz, d*0.5, h*0.5, d*0.5)          # left leg
        self.box(mat, cx+lr, h*0.5, cz, d*0.5, h*0.5, d*0.5)          # right leg
        self.box(mat, cx, h+d*0.5, cz, lr+d*0.5, d*0.5, d*0.5)        # lintel
    def ring(self, mat, cx, cz, r, n, ph, pr=1.6):
        # a ring of n standing pillars (a henge / colonnade)
        for k in range(n):
            a = k/float(n)*6.28318
            self.box(mat, cx+math.cos(a)*r, ph*0.5, cz+math.sin(a)*r, pr, ph*0.5, pr)
    def _prism(self, mat, cx, cz, y0, y1, r_bot, r_top, sides, uvs=0.12):
        # a closed N-sided vertical prism (the round-tower mesh). Purely visual;
        # collision is added separately by cyl_tower as an inset box ring.
        import math as _m
        for k in range(sides):
            a0 = k/float(sides)*6.28318; a1 = (k+1)/float(sides)*6.28318
            x0b,z0b = cx+_m.cos(a0)*r_bot, cz+_m.sin(a0)*r_bot
            x1b,z1b = cx+_m.cos(a1)*r_bot, cz+_m.sin(a1)*r_bot
            x0t,z0t = cx+_m.cos(a0)*r_top, cz+_m.sin(a0)*r_top
            x1t,z1t = cx+_m.cos(a1)*r_top, cz+_m.sin(a1)*r_top
            nx,nz = _m.cos((a0+a1)*0.5), _m.sin((a0+a1)*0.5)
            self._quad(mat,(x0b,y0,z0b),(x1b,y0,z1b),(x1t,y1,z1t),(x0t,y1,z0t),(nx,0,nz),uvs)
        # top cap
        for k in range(sides):
            a0 = k/float(sides)*6.28318; a1 = (k+1)/float(sides)*6.28318
            self._quad(mat,(cx,y1,cz),(cx+_m.cos(a0)*r_top,y1,cz+_m.sin(a0)*r_top),
                       (cx+_m.cos(a1)*r_top,y1,cz+_m.sin(a1)*r_top),(cx,y1,cz),(0,1,0),uvs)
    def cyl_tower(self, mat, cx, cz, h, r, sides=8, taper=0.7, collide=True):
        # a genuinely ROUND tower: an N-sided prism mesh + an inset ring of small
        # collision boxes hugging the circle, so you no longer clip invisible
        # square corners the way a single AABB round-tower would give you.
        self._prism(mat, cx, cz, 0.0, h, r, r*taper, sides)
        if collide:
            import math as _m
            # inset octagon of collider boxes: each covers one facet, sized so the
            # union approximates the disc without overshooting the visible edge.
            m = max(4, sides//1)
            bw = r*_m.sin(_m.pi/m)*1.15               # facet half-width
            ir = r*_m.cos(_m.pi/m)*0.55               # push boxes inward so corners don't poke out
            for k in range(m):
                a = (k+0.5)/float(m)*6.28318
                bx,bz = cx+_m.cos(a)*ir, cz+_m.sin(a)*ir
                self.colliders.append((bx, h*0.5, bz, bw, h*0.5, bw))
    def tunnel(self, mat, cx, cz, w, h, d, wall=1.2, axis="x"):
        # a HOLLOW passage you can walk/drive THROUGH: two side walls + a roof
        # lintel, open floor + open bore. Collision is only the walls/roof, so
        # the interior stays clear (unlike a solid AABB block).
        hw = w*0.5
        if axis == "x":   # bore runs along x; walls on +/- z
            self.box(mat, cx, h*0.5, cz+hw+wall*0.5, d*0.5, h*0.5, wall*0.5)     # far wall
            self.box(mat, cx, h*0.5, cz-hw-wall*0.5, d*0.5, h*0.5, wall*0.5)     # near wall
            self.box(mat, cx, h+wall*0.5, cz, d*0.5, wall*0.5, hw+wall)          # solid roof cap
        else:             # bore runs along z; walls on +/- x
            self.box(mat, cx+hw+wall*0.5, h*0.5, cz, wall*0.5, h*0.5, d*0.5)
            self.box(mat, cx-hw-wall*0.5, h*0.5, cz, wall*0.5, h*0.5, d*0.5)
            self.box(mat, cx, h+wall*0.5, cz, hw+wall, wall*0.5, d*0.5)
    def dome(self, mat, cx, cz, r, h, rings=4, sides=10, collide=True):
        # a rounded DOME: stacked shrinking prism rings following a hemisphere
        # curve, with collision boxes that follow the curve instead of one big
        # square block. Hollow-friendly: only the shell collides.
        import math as _m
        for ri in range(rings):
            t0 = ri/float(rings); t1 = (ri+1)/float(rings)
            y0 = h*_m.sin(t0*_m.pi*0.5); y1 = h*_m.sin(t1*_m.pi*0.5)
            r0 = r*_m.cos(t0*_m.pi*0.5); r1 = r*_m.cos(t1*_m.pi*0.5)
            self._prism(mat, cx, cz, y0, y1, r0, r1, sides)
            if collide and ri < rings-1:
                m = max(4, sides//1)
                bw = r0*_m.sin(_m.pi/m)*1.1
                for k in range(m):
                    a = (k+0.5)/float(m)*6.28318
                    self.colliders.append((cx+_m.cos(a)*r0*0.82, (y0+y1)*0.5,
                                           cz+_m.sin(a)*r0*0.82, bw, (y1-y0)*0.5+0.2, bw))
    def mound(self, mat, cx, cz, r, h):
        # a low, wide natural rise -- a walkable bump for terrain variation
        self.box(mat, cx, h*0.5, cz, r, h*0.5, r*0.85)
    def ramp(self, mat, cx, cz, length, width, rise, dz=1):
        # a stepped ramp climbing along +/-z: steps of increasing height so the
        # player can walk up onto elevated ground (traversal + verticality)
        steps = max(3, int(rise/0.45))
        seg = length/steps
        for s in range(steps):
            sy = rise*(s+1)/steps
            sz = cz + dz*(seg*(s+0.5) - length*0.5)
            self.box(mat, cx, sy*0.5, sz, width*0.5, sy*0.5, seg*0.62)
    def skyway(self, mat, x0, z0, x1, z1, y, width=3.5, pylon=True):
        # an elevated flat ROADWAY between two points -- a vehicle-mode sky highway. Laid as a
        # chain of flat deck segments (so it can run diagonally) with slim support pylons.
        # Longer segments keep the collider count low (each deck box is a blocker).
        n = max(2, int(math.hypot(x1 - x0, z1 - z0) / 11.0))
        for i in range(n + 1):
            t = i / n; cx = x0 + (x1 - x0) * t; cz = z0 + (z1 - z0) * t
            self.box(mat, cx, y, cz, width, 0.45, width)             # deck segment (flat top)
            if pylon and i % 2 == 0 and y > 3.0:
                self.box(mat, cx, y * 0.5, cz, 0.7, y * 0.5, 0.7, collide=False)   # thin support (visual)

    def ice_slab(self, mat, cx, cz, w, d, h, ramp_dir=1):
        # a big ICE CHUNK with a FLAT drivable top + an ice ramp up onto it, so the frozen
        # maps have broad flat surfaces to drive across between the spikes.
        self.box(mat, cx, h * 0.5, cz, w, h * 0.5, d)               # the slab (flat top at y=h)
        self.ramp(mat, cx, cz - ramp_dir * (d + 5), w * 1.4, w * 0.8, h, dz=ramp_dir)

    def terrain_field(self, mat, seed, half=None, res=88, amp=5.2, warp=10.0, style=None,
                      ridge_mix=0.46, spawn_flat=13.0, paths=None, road_h=0.35, road_w=9.0,
                      dunes=False):
        """A rolling ground MESH driven by domain-warped fBm + ridged noise -- real relief.
        CARVES flat drivable PATHS toward `paths` (list of (ax,az,bx,bz) segments) so vehicles
        get smooth routes through the hills, and can favour long DUNE ridges. Kept mostly
        positive + flattened toward spawn."""
        if half is None:
            half = S

        def hgt(x, z):
            wx = x + warp * _perlin(x * 0.015 + 11.3, z * 0.015 + 4.7, seed + 31)  # domain warp
            wz = z + warp * _perlin(x * 0.015 + 2.1, z * 0.015 + 8.9, seed + 57)
            base = _fbm(wx * 0.012, wz * 0.012, seed, oct=5) * 0.5 + 0.5           # 0..1 rolling
            rid = _ridged(wx * 0.02, wz * 0.02, seed + 13, oct=4)                  # 0..1 ridges
            if dunes:
                # long wind-blown DUNES: stretched ridged waves in one direction, crisp crests
                dv = _ridged(wx * 0.010, wz * 0.028, seed + 71, oct=3)
                h = (0.35 * base + 0.65 * dv) * amp * 1.15
            else:
                h = (base * (1.0 - ridge_mix) + rid * ridge_mix) * amp
            if paths:                                # CARVE flat driving ribbons toward the paths
                best = min(_seg_dist(x, z, *seg) for seg in paths)
                if best < road_w:
                    t = best / road_w                # 0 on the path .. 1 at its edge
                    blend = t * t * (3.0 - 2.0 * t)  # smootherstep shoulder
                    h = h * blend + road_h * (1.0 - blend)   # flatten to a low road height
            base = h * min(1.0, math.hypot(x, z) / spawn_flat)                     # level at spawn
            base += (math.sin(x*0.91+seed) * math.sin(z*0.87+seed*1.7) +
                     math.sin(x*1.63-z*1.11+seed*2.3)*0.5) * 0.14    # fine SURFACE GRAIN
            # NASA STYLE PASS (imaging-derived landforms, composited on the base):
            if style == "pluto":       # Sputnik Planitia: polygonal convection CELLS
                cx2, cz2 = math.floor(x/26.0), math.floor(z/26.0)
                jx = (math.sin(cx2*127.1+cz2*311.7)*0.5+0.5)*14.0
                jz = (math.sin(cx2*269.5+cz2*183.3)*0.5+0.5)*14.0
                dcx, dcz = x - (cx2*26.0+6.0+jx), z - (cz2*26.0+6.0+jz)
                dc = math.sqrt(dcx*dcx+dcz*dcz)
                base = base*0.35 + max(0.0, 1.0 - abs(dc-11.0)/3.5)*1.5   # raised cell RIMS
            elif style == "europa":    # parallel LINEAE: long double-ridge bands
                pr2 = x*0.766 + z*0.643                                    # band direction
                base = base*0.4 + (abs(math.sin(pr2*0.11))**0.35)*2.0
            elif style == "mars":      # crater bowls + transverse dune ripples
                for (ccx, ccz, cr, cd) in ((-38,22,17,2.6),(31,-45,12,2.0),(12,55,9,1.5)):
                    dd2 = math.sqrt((x-ccx)**2 + (z-ccz)**2)
                    if dd2 < cr*1.25:
                        base += max(0.0, 1.0-abs(dd2-cr)/ (cr*0.25))*1.4   # raised rim
                        if dd2 < cr: base -= cd*(1.0-(dd2/cr)**2)          # the bowl
                base += math.sin(x*0.55 + math.sin(z*0.2)*2.0)*0.22        # ripples
            elif style == "titan":     # longitudinal dune seas + a methane channel
                base = base*0.45 + (math.sin(z*0.085 + math.sin(x*0.03)*1.5)*0.5+0.5)*2.3
                ch = x - math.sin(z*0.045)*18.0
                base -= max(0.0, 1.0-abs(ch)/6.0)*1.6                      # the channel carves
            elif style == "io":        # volcanic plains + caldera pits with rims
                for (ccx, ccz, cr, cd) in ((-30,-18,20,3.2),(42,35,14,2.4)):
                    dd2 = math.sqrt((x-ccx)**2 + (z-ccz)**2)
                    if dd2 < cr*1.3:
                        base += max(0.0, 1.0-abs(dd2-cr)/(cr*0.3))*1.8
                        if dd2 < cr: base -= cd*(1.0-(dd2/cr)**2)
            return base

        step = 2 * half / res
        # precompute a (res+1)^2 height grid -- reused for the mesh AND written out as a
        # runtime heightmap (.hgt) so the game can sample terrain for vehicle suspension.
        grid = [[hgt(-half + i*step, -half + j*step) for i in range(res+1)] for j in range(res+1)]
        self.hgt = {"res": res, "half": half, "grid": grid}
        # PER-VERTEX SMOOTH NORMALS from the height gradient (central differences): each
        # vertex gets its OWN normal, so lighting blends across quads and the terrain reads
        # as a continuous rolling surface instead of big flat facets.
        def vnorm(i, j):
            i0, i1 = max(0, i-1), min(res, i+1)
            j0, j1 = max(0, j-1), min(res, j+1)
            dx = (grid[j][i1] - grid[j][i0]) / ((i1 - i0) * step)
            dz = (grid[j1][i] - grid[j0][i]) / ((j1 - j0) * step)
            nl = math.sqrt(dx*dx + 1.0 + dz*dz)
            return (-dx / nl, 1.0 / nl, -dz / nl)
        for j in range(res):
            for i in range(res):
                x0 = -half + i * step; z0 = -half + j * step; x1 = x0 + step; z1 = z0 + step
                y00, y10 = grid[j][i], grid[j][i+1]
                y11, y01 = grid[j+1][i+1], grid[j+1][i]
                base = len(self.verts)
                t0 = len(self.tris)
                pts = ((x0, y00, z0), (x0, y01, z1), (x1, y11, z1), (x1, y10, z0))
                nms = (vnorm(i, j), vnorm(i, j+1), vnorm(i+1, j+1), vnorm(i+1, j))
                for pp, nn in zip(pts, nms):
                    self.verts.append((pp, (pp[0]*0.06, pp[2]*0.06), nn))
                self.tris.append((base, base+1, base+2)); self.tris.append((base, base+2, base+3))
                # CRITICAL: register the quad in the material BUCKET -- the writer exports
                # meshes from buckets; unbucketed tris silently vanish (the flat-terrain bug).
                self.buckets.setdefault(mat, []).append((base, 4, t0, 2))

    def natural_terrain(self, mat, seed, n=15, spread=95, max_h=2.8, plateau=True, field=True,
                        dunes=False, roads=True, style=None):
        # A rolling/dune noise GROUND with flat CARVED driving paths (a ring road + crossings),
        # then mounds CLUSTERED on the high ground but kept OFF the paths so routes stay clear.
        r = random.Random(seed)
        paths = None
        if roads:
            rr = min(S, spread) * 0.62
            pts = [(rr*math.cos(k/8*6.283), rr*math.sin(k/8*6.283)) for k in range(8)]
            paths = [(pts[k][0], pts[k][1], pts[(k+1) % 8][0], pts[(k+1) % 8][1]) for k in range(8)]
            paths.append((-rr, -rr, rr, rr)); paths.append((-rr, rr, rr, -rr))   # two crossing routes
        if field:
            self.terrain_field(mat, seed, half=min(S, spread + 2), paths=paths, dunes=dunes, style=style)
        placed, tries = 0, 0
        while placed < n and tries < n * 25:
            tries += 1
            cx = r.uniform(-spread, spread); cz = r.uniform(-spread, spread)
            if abs(cx) < 16 and abs(cz) < 16:
                continue
            if paths and min(_seg_dist(cx, cz, *seg) for seg in paths) < 11.0:
                continue                                     # keep mounds off the driving paths
            hn = _fbm(cx * 0.02, cz * 0.02, seed, oct=4) * 0.5 + 0.5
            if hn < 0.52:
                continue
            rad = r.uniform(4.0, 11.0); h = r.uniform(0.5, max_h) * (0.55 + 0.85 * hn)
            self.mound(mat, cx, cz, rad, h); placed += 1
        if plateau:
            px = r.choice([-1, 1]) * r.uniform(45, 70); pz = r.choice([-1, 1]) * r.uniform(45, 70)
            self.box(mat, px, 2.0, pz, 14, 2.0, 14)          # raised vantage platform
            self.ramp(mat, px, pz - (16 if pz > 0 else -16), 18, 8, 4.0, dz=(1 if pz > 0 else -1))
    def write_colliders(self, path):
        # emit big solid blockers as AABBs for in-game collision (skip tiny bits)
        cpath = path.rsplit(".",1)[0] + ".col"
        with open(cpath,"w") as f:
            for (cx,cy,cz,hx,hy,hz) in self.colliders:
                if max(hx,hz) < 1.2: continue          # skip small greebles
                f.write("%.3f %.3f %.3f %.3f %.3f %.3f\n"%(cx,cy,cz,hx,hy,hz))
    def write_heightmap(self, path):
        # emit the terrain heightmap (.hgt) for runtime vehicle suspension: "res half" then
        # (res+1)^2 row-major floats. Only when terrain_field laid a grid.
        if not hasattr(self, "hgt") or self.hgt is None:
            return
        hpath = path.rsplit(".", 1)[0] + ".hgt"
        g = self.hgt
        with open(hpath, "w") as f:
            f.write("%d %.5f\n" % (g["res"], g["half"]))
            for row in g["grid"]:
                f.write(" ".join("%.5f" % v for v in row) + "\n")

    def write(self, path, materials=None):
        self.write_colliders(path)
        self.write_heightmap(path)
        # buckets are keyed by texture filename; emit them in a stable order
        text = b"\x00"; offs = {}
        order = list(self.buckets.keys())
        for tex in order:
            if tex not in offs:
                offs[tex] = len(text); text += tex.encode()+b"\x00"
        name_off = len(text); text += b"map\x00"
        nv_list, nt_list, recs = [], [], []
        for tex in order:
            fv, ft = len(nv_list), len(nt_list)
            for (v0, nvn, t0, ntn) in self.buckets[tex]:
                remap = {}
                for i in range(nvn):
                    remap[v0+i] = len(nv_list); nv_list.append(self.verts[v0+i])
                for t in range(ntn):
                    a,b,c = self.tris[t0+t]
                    nt_list.append((remap[a],remap[b],remap[c]))
            recs.append((tex, fv, len(nv_list)-fv, ft, len(nt_list)-ft))
        num_v, num_t, num_m = len(nv_list), len(nt_list), len(recs)
        def u(*a): return struct.pack("<"+"I"*len(a), *a)
        ofs = 16+4+4*26
        ofs_text=ofs; ofs+=len(text); ofs+=(-ofs)%4
        ofs_meshes=ofs; ofs+=num_m*6*4
        ofs_va=ofs; ofs+=3*5*4
        ofs_pos=ofs; ofs+=num_v*12
        ofs_uv=ofs; ofs+=num_v*8
        ofs_nrm=ofs; ofs+=num_v*12
        ofs_tri=ofs; ofs+=num_t*12
        filesize=ofs
        out=b"INTERQUAKEMODEL\x00"+u(2,filesize,0)+u(len(text),ofs_text)+u(num_m,ofs_meshes)
        out+=u(3,num_v,ofs_va)+u(num_t,ofs_tri,0)+u(0,0)+u(0,0)+u(0,0)+u(0,0,0,0)+u(0,0)+u(0,0)
        out+=text+b"\x00"*((-(ofs_text+len(text)))%4)
        for tex,fv,nv,ft,nt in recs:
            out+=u(name_off, offs[tex], fv, nv, ft, nt)
        out+=u(0,0,7,3,ofs_pos)+u(1,0,7,2,ofs_uv)+u(2,0,7,3,ofs_nrm)
        for p,_,_ in nv_list: out+=struct.pack("<3f",*p)
        for _,uv,_ in nv_list: out+=struct.pack("<2f",*uv)
        for _,_,n in nv_list: out+=struct.pack("<3f",*n)
        for t in nt_list: out+=u(*t)
        assert len(out)==filesize,(len(out),filesize)
        open(path,"wb").write(out)
        print(f"  {os.path.basename(path)}: {num_v} verts {num_t} tris {num_m} meshes")

DATA = os.path.join(os.path.dirname(__file__), "..", "engine_src", "data")

# --- base collision hull: MASSIVE floor + tall perimeter walls (collision only).
# The world is huge so robots can fly, launch off ramps, and fall long drops. ---
BIG = 1440.0
b = Builder(); b.floor("floor.png", half=BIG); b.walls("wall.png", half=BIG, h=140.0, t=3.0)
b.write(os.path.join(DATA,"arena_base.iqm"), {"floor":"floor.png","wall":"wall.png"})

# --- WASTES (rusted expanse): broken slabs + leaning girders ---
b = Builder(); b.floor("rust.png"); b.walls("rust.png")
random.seed(11)
for i in range(14):
    a=random.uniform(0,6.283); r=random.uniform(14,50)
    x,z=r*math.cos(a), r*math.sin(a)
    b.box("rock.png", x, random.uniform(0.4,1.2), z, random.uniform(2,5), random.uniform(0.4,1.2), random.uniform(2,5))  # toppled slabs
for i in range(6):  # leaning girders (tall thin boxes)
    a=i/6*6.283; r=30
    b.pylon("wall.png", r*math.cos(a), r*math.sin(a), 6.0, emit="hazard.png")   # dead relay masts, dressed
b.box("rock.png", 0, 1.0, 0, 6, 1.0, 6)  # central wreck mound (story: the dead relay)
b.ring("wall.png", 0, 0, 30, 9, 7.0, 1.2)   # a ruined henge of standing girders around the relay
# FALLEN HULL: big reared-up Cybertron plates + snapped tower spars + a broken gateway,
# so the battlefield has a war-torn vertical skyline instead of flat rubble.
b.box("wall.png", 20, 6.0, -18, 4, 6.0, 3)               # a reared-up hull slab
b.box("rock.png", -25, 5.0, 15, 5, 5.0, 4)               # a great toppled block
b.spire("wall.png", 30, -30, 16, 2.2)                    # snapped tower spars
b.spire("wall.png", -35, -20, 14, 2.0)
b.arch("wall.png", 0, 42, 16, 10, d=2.5)                 # a broken gateway to the ruins
for k in range(4):
    a=random.uniform(0,6.283); r=random.uniform(22,48)
    b.box("rock.png", r*math.cos(a), random.uniform(3,6), r*math.sin(a),
          random.uniform(3,5), random.uniform(3,6), random.uniform(3,5))
b.natural_terrain("rust.png", 71, dunes=True)
# DESTINY-STYLE VARIED, INTERACTIVE SCATTER: mixed scrap props (not one repeated
# doodad), ~40% solid so they double as cover you weave through -- an immersive,
# tactical junk field for the patrol experience rather than flat rubble.
_scrap = ["rust.png", "wall.png", "brushmetal.png", "plate_blue.png", "rock.png"]
b.varied_scatter("wastes_scrap.png", _scrap,  38,  38, r=13, count=10, solid_frac=0.4, seedy=1)
b.varied_scatter("wastes_scrap.png", _scrap, -44,  30, r=12, count=9,  solid_frac=0.4, seedy=2)
b.varied_scatter("wastes_scrap.png", _scrap,  10, -50, r=14, count=11, solid_frac=0.35, seedy=3)
b.write(os.path.join(DATA,"wastes.iqm"), {"floor":"wastes_scrap.png","wall":"wastes_scrap.png","rock":"rock.png"})

# --- FROSTLINE (ice shelf): cracked plates + jagged spires ---
b = Builder(); b.floor("ice.png"); b.walls("norm_wall.png")
random.seed(23)
for i in range(18):
    a=random.uniform(0,6.283); r=random.uniform(10,52)
    b.spire("spec_floor.png", r*math.cos(a), r*math.sin(a), random.uniform(3,8), random.uniform(1,2.2))
# raised ice dais at center (story: the frozen archive)
b.box("ice.png", 0, 1.0, 0, 7, 1.0, 7)
# TITANIC ICE: sheared glacier monoliths + tall jagged spires + a frozen arch + a pillar
# ring, so the shelf has real vertical relief (it was nearly flat at vert 0.15).
for k in range(5):
    a=random.uniform(0,6.283); r=random.uniform(20,55)
    b.box("ice.png", r*math.cos(a), random.uniform(5,10), r*math.sin(a),
          random.uniform(3,5), random.uniform(5,10), random.uniform(3,5))   # sheared ice blocks
b.spire("ice.png", -30, -40, 18, 2.6)
b.spire("ice.png", 42, 22, 20, 2.8)
b.spire("ice.png", 18, -50, 16, 2.4)
b.arch("ice.png", 0, 44, 16, 12, d=2.5)                                     # a frozen arch
b.ring("ice.png", 0, 0, 40, 8, 9.0, 1.8)                                    # ice-pillar ring around the archive
# ICE SLABS: big frozen chunks with FLAT drivable tops (+ ice ramps up), so there are broad
# flat surfaces to race across between the spikes.
b.ice_slab("ice.png", -44, 30, 12, 12, 4.0, ramp_dir=1)
b.ice_slab("ice.png", 46, -28, 14, 11, 5.0, ramp_dir=-1)
b.ice_slab("ice.png", 20, 52, 11, 13, 3.5, ramp_dir=1)
b.natural_terrain("ice.png", 72)
b.write(os.path.join(DATA,"frostline.iqm"), {"floor":"ice.png","wall":"norm_wall.png","rock":"ice.png"})

# --- EMBERPIT (sunken lava bowl): obsidian tiers ringing a center dais ---
b = Builder(); b.floor("obsidian.png"); b.walls("obsidian.png", h=10)
# concentric obsidian rings (stepped tiers) using boxes around a circle
for ring,(rr,hh) in enumerate([(46,1.0),(38,2.0),(30,3.0)]):
    seg=24
    for k in range(seg):
        a=k/seg*6.283
        b.box("rock.png", rr*math.cos(a), hh*0.5, rr*math.sin(a), 3.2, hh*0.5, 3.2)
        b.box("molten.png", rr*math.cos(a), hh+0.12, rr*math.sin(a), 2.6, 0.12, 2.6, collide=False)   # ember-lit tier rim
b.box("rock.png", 0, 1.5, 0, 5, 1.5, 5)  # center dais (story: the ember titan's seat)
b.box("obsidian.png", 0, 3.4, 0, 1.6, 0.5, 1.6)          # monument collar on the dais
b.spire("wall.png", 0, 0, 6, 1.4)                        # the broken shaft
b.box("molten.png", 0, 5.2, 0, 1.2, 0.2, 1.2, collide=False)   # ember ring, still hot
# the pit BREATHES: four obsidian vent stacks on the outer rim, molten-lit
for k in range(4):
    a=k*1.5708+0.785
    b.stack("obsidian.png", 62*math.cos(a), 62*math.sin(a), 9.0+k%2*2.0, 1.5)
b.natural_terrain("rock", 84, n=10, spread=88, max_h=2.4, plateau=False)
b.spire("obsidian.png", 78, 12, 20.0, 2.3)                  # two PIT-CROWN spires, towering over the rim
b.spire("obsidian.png", -74, -30, 18.0, 2.3)
# a jagged CROWN of more spires around the rim -- varied tall heights so the bowl
# bristles with obsidian, stretching the height range above the stepped tiers.
random.seed(626)
for k in range(6):
    a = k*1.0472 + 0.3
    rr = random.uniform(66, 78)
    b.spire("obsidian.png", rr*math.cos(a), rr*math.sin(a), random.uniform(13.0, 22.0), 2.0)
b.write(os.path.join(DATA,"emberpit.iqm"), {"floor":"floor.png","wall":"wall.png","rock":"rock.png"})

# --- VAULT (crystal): pale floor + tall crystal columns + gold veins ---
b = Builder(); b.floor("white.png"); b.walls("white.png", h=12)
random.seed(31)
for i in range(10):  # tall crystal columns in a loose grid
    x=(i%5-2)*20 + random.uniform(-4,4)
    z=(i//5-0.5)*40 + random.uniform(-4,4)
    b.spire("spec_rock.png", x, z, random.uniform(7,11), random.uniform(1.5,2.5))
b.box("spec_rock.png", 0, 0.5, 0, 8, 0.5, 8)  # central plinth (story: the sealed relic)
# BROKEN MONOLITHS: fallen + snapped crystal slabs at mixed scales -- the mid-size
# cover the columns-and-ziggurat plan lacked
for mx0, mz0, mw0, mh0 in ((-62,10,3.2,2.2),(48,-58,4.5,1.6),(66,18,2.6,3.0),
                            (-14,-64,5.0,1.2),(-58,-44,3.8,2.6),(30,62,2.2,3.4)):
    b.box("spec_rock.png", mx0, mh0, mz0, mw0, mh0, mw0*0.6)
    b.box("white.png", mx0+mw0*0.8, 0.5, mz0+1.0, mw0*0.7, 0.5, mw0*0.5)   # its snapped-off head
for vt in ((-66,-62,13.0),(64,58,14.0)):                     # archive TOWERS at the far corners
    b.box("riftcrystal.png", vt[0], vt[2]*0.5, vt[1], 2.8, vt[2]*0.5, 2.8)
    b.trim("white.png", vt[0], vt[2], vt[1], 2.8, 2.8)        # dressed: parapet + service light
for ot in ((-83,-3,10.0),(83,3,11.0)):                        # OUTER SENTINEL spires, reach toward the walls
    b.spire("riftcrystal.png", ot[0], ot[1], ot[2], 2.0)
random.seed(131)                                              # SCATTERED LOW SHARDS: vault had almost no low
for _ls in range(20):                                          # cover (7 low vs 57 high) -- fix the balance
    _lx = random.uniform(-78,78); _lz = random.uniform(-70,70)
    if abs(_lx) < 12 and abs(_lz) < 12: continue                # keep the central plinth clear
    b.box("spec_rock.png", _lx, 0.55, _lz, random.uniform(1.3,2.2), 0.55, random.uniform(1.3,2.2))
for lv in range(5):                                          # low crystal shards (shoot-over)
    b.box("spec_rock.png", random.uniform(-55,55), 0.8, random.uniform(-55,55), random.uniform(1.8,3), 0.8, random.uniform(1.5,2.5))
# THE SEALED ARCHIVE, expanded: a great crystal ZIGGURAT rises over the vault floor,
# flanked by rows of tall archive shards and ringed by crystal arches -- scale + height.
for tier in range(4):                                       # the ziggurat: stacked crystal decks
    w = 20 - tier*4.5; b.box("riftcrystal.png", 0, 1.4 + tier*2.6, -46, w, 1.3, w*0.8)
b.spire("riftcrystal.png", 0, -46, 20, 2.4)                 # its crowning shard
for k in range(5):                                          # archive shard rows (tall, luminous)
    b.spire("riftcrystal.png", -58+k*12, 44, 10+(k%2)*4, 1.5)
    b.spire("riftcrystal.png", -58+k*12, 58, 8+(k%2)*4, 1.3)
b.arch("white.png", -52, 0, 14, 10, 2.0)                    # perimeter arches frame the approaches
b.arch("white.png",  52, 0, 14, 10, 2.0)
b.ring("white.png", 0, 0, 30, 10, 2.2, 1.2)                 # a colonnade ring around the heart
# SC2 BEST-PRACTICES PASS (docs/SC2_MAPCRAFT.md): lead the eye to the relic,
#  value-contrast the tower heights, ground the margin detail.
for ex0, ez0 in ((0, 70), (0, -70), (78, 0), (-78, 0)):      # each edge -> the central relic plinth
    b.eye_trail("energon.png", ex0, ez0, 0, 0, n=6, w=0.7)
for vt in ((-66, -62, 13.0), (64, 58, 14.0)):                # value bands up the archive towers
    b.height_bands(vt[0], vt[1], 2.0, vt[2], 2.8, 2.8,
                   ["spec_rock.png", "spec_rock.png", "riftcrystal.png", "white.png"])
for cx4, cz4 in ((-40, 66), (44, -30)):                       # grounded shards in bare margins
    b.ground_cluster("spec_rock.png", "riftcrystal.png", cx4, cz4, r=6.0, count=4)
b.write(os.path.join(DATA,"vault.iqm"), {"floor":"white.png","wall":"white.png","rock":"spec_rock.png"})


# --- FOUNDRY (industrial): the worst place to fight the Corrupted -- lots of
# metal to absorb. Gantries, pipe stacks, cargo containers, a central smelter. ---
b = Builder(); b.floor("plate_blue.png"); b.walls("hazard.png", h=10)
random.seed(53)
# cargo containers (long boxes) in rows
for i in range(12):
    x=(i%4-1.5)*22; z=(i//4-1)*30
    b.box("wall.png", x, 1.5, z, 4.0, 1.5, 2.0)
    b.box("hazard.png", x, 3.15, z, 4.2, 0.15, 2.2, collide=False)     # caution cap rail
    b.box("wall.png", x-3.2, 3.9, z, 0.35, 0.75, 0.35, collide=False)  # end posts
    b.box("wall.png", x+3.2, 3.9, z, 0.35, 0.75, 0.35, collide=False)
# pipe stacks (clusters of tall thin boxes)
for i in range(8):
    a=i/8*6.283; r=34
    cx,cz=r*math.cos(a), r*math.sin(a)
    for k in range(3):
        b.box("rock.png", cx+(k-1)*0.6, 3.0, cz, 0.35, 3.0, 0.35)
# central smelter dais + chimney (story: where the apex Corrupted lairs)
b.box("rock.png", 0, 2.0, 0, 6, 2.0, 6)
b.spire("wall.png", 0, 0, 9, 2.5)
b.natural_terrain("rock", 80, n=13, spread=92, max_h=2.6)
# VERTICALITY PASS: the foundry works in the AIR -- two crane gantries crossing the
# works (drivable), pour towers with molten crowns.
b.skyway("brushmetal.png", -60, 20, 60, 20, 9.0)
b.skyway("brushmetal.png", -20, -60, -20, 60, 7.0)
for k in range(4):
    a=k*1.5708+0.6; tx, tz = math.cos(a)*44, math.sin(a)*44
    b.box("cybertron.png", tx, 6.5, tz, 2.4, 6.5, 2.4)
    b.spire("molten.png", tx, tz, 3, 0.8, collide=False)
for k in range(3):                                        # the foundry SMOKES: molten stacks
    b.stack("wall.png", -50.0+k*50.0, 62.0, 12.0+k*2.0, 1.6)
b.write(os.path.join(DATA,"foundry.iqm"), {"floor":"floor.png","wall":"wall.png","rock":"rock.png"})


# --- SHIELD EDGE (the shield boundary from Trilogy Two): deep-space starfield
# floor, a luminous barrier wall on one side, sparse floating platforms. Where
# the Watch Bearer holds the perimeter against the Hijacks. ---
b = Builder(); b.floor("stars.png"); b.walls("plate_blue.png", h=14)
random.seed(71)
# the shield barrier: a tall glowing wall of energon panels along +x edge
for k in range(int(S//4)):
    b.box("plate_blue.png", S-2, 0.7, -S + k*8, 1.1, 0.7, 3.8)                    # emitter base collar
    b.box("energon.png", S-2, 5.0, -S + k*8, 0.6, 5.0, 3.5)                       # the field slab
    b.box("plate_blue.png", S-2, 10.4, -S + k*8, 0.9, 0.35, 3.7, collide=False)   # cap rail
# scattered floating combat platforms
for i in range(9):
    a=random.uniform(0,6.283); r=random.uniform(20,70)
    px3, pz3 = r*math.cos(a), r*math.sin(a)
    ph3 = random.uniform(0.5,3.0); pw3 = random.uniform(4,7); pd3 = random.uniform(4,7)
    b.box("plate_blue.png", px3, ph3, pz3, pw3, 0.6, pd3)
    b.box("energon.png", px3, ph3+0.68, pz3, pw3*0.96, 0.08, pd3*0.96, collide=False)  # lit deck rim
    b.spire("plate_blue.png", px3, pz3, ph3, 0.5, collide=False)                       # support stalk
for k in range(3):
    b.pylon("plate_blue.png", -60+k*45, -62, 8.0)          # perimeter relay pylons watch the line
b.box("energon.png", 0, 1.0, 0, 6, 1.0, 6)  # command dais (story: the perimeter watch)
b.write(os.path.join(DATA,"shield_edge.iqm"), {"floor":"stars.png","wall":"shield_wall.png","rock":"energon.png"})

# --- MINING STATION (outer rim): catwalks, ore containers, drill towers,
# hazard markings. Scattered Autobots defend these per the Dominion overview. ---
b = Builder(); b.floor("floor.png"); b.walls("rust.png", h=12)
random.seed(83)
# ore container stacks
for i in range(14):
    x=(i%5-2)*28 + random.uniform(-3,3); z=(i//5-1)*34 + random.uniform(-3,3)
    hbase=random.choice([1.5,3.0])
    b.box("hazard.png", x, hbase, z, 4.0, hbase, 3.0)
# drill towers (tall spires) around the rim
for i in range(6):
    a=i/6*6.283; r=64
    b.derrick("rust.png", r*math.cos(a), r*math.sin(a), random.uniform(8,12))   # rigs, not spikes
# central refinery pad
b.box("plate_blue.png", 0, 1.2, 0, 8, 1.2, 8)
b.spire("hazard.png", 0, 0, 7, 2)
# HEADFRAMES: winch towers over the shafts -- the mining silhouette the map lacked.
# Tall paired legs, a crossbeam, and a wheel-house box riding the top.
for hx0, hz0 in ((-38, -22), (30, 34), (12, -48)):
    b.spire("rust.png", hx0-2.2, hz0, 12, 0.8)
    b.spire("rust.png", hx0+2.2, hz0, 12, 0.8)
    b.box("brushmetal.png", hx0, 11.4, hz0, 3.4, 0.5, 1.2)          # crossbeam
    b.box("hazard.png", hx0, 12.6, hz0, 1.6, 0.8, 1.4)              # wheel-house
b.skyway("brushmetal.png", -38, -22, 30, 34, 9.5)                    # the ore conveyor high-road
b.natural_terrain("rock.png", 78)
b.write(os.path.join(DATA,"mining.iqm"), {"floor":"mining_strata.png","wall":"mining_strata.png","rock":"hazard.png","plate":"plate_blue.png"})


# --- OUTER RIM (Dominion): scattered asteroid platforms over deep space, an
# abandoned mining rig, drifting ore. The Autobots defend the scattered rim. ---
b = Builder(); b.floor("nebula.png"); b.walls("asteroid.png", h=16)
random.seed(91)
# large asteroid platforms at varied heights (islands in space)
for i in range(11):
    a=random.uniform(0,6.283); r=random.uniform(18,72)
    hh=random.uniform(0.5,4.0)
    b.box("asteroid.png", r*math.cos(a), hh, r*math.sin(a),
          random.uniform(5,9), hh, random.uniform(5,9))
# the mining rig: a cluster of girders + a drill at center
for k in range(5):
    a=k/5*6.283
    b.box("rust.png", 6*math.cos(a), 3.0, 6*math.sin(a), 0.4, 3.0, 0.4)
b.spire("hazard.png", 0, 0, 10, 2.4)
b.box("carbon.png", 0, 1.0, 0, 5, 1.0, 5)              # rig deck: carbon-weave plate
# UNICRON FRAGMENTS: colossal machine-shell architecture littering the rim -- a great
# segmented rib-ring, broken mechanical portals, and jagged hull-teeth (the planet-eater's
# wreckage scattered across the outer system). Gives the field scale + a Cybertronian read.
b.ring("plate_blue.png", 0, 0, 74, 10, 16, pr=2.2)     # huge segmented shell-rib ring
b.arch("rust.png", -46, 20, 14, 10, d=2.5)             # broken machine portals
b.arch("rust.png", 40, -40, 14, 10, d=2.5)
b.spire("asteroid.png", -30, -55, 16, 2.2)             # jagged mechanical teeth
# VOYAGER'S OUTER SYSTEM: the frozen Kuiper frontier the Voyager probes charted --
# pathable asteroid SPACEWALKS (stepping platforms bridged by catwalks), icy MINI PLANETS
# (dwarf worlds + a crystalline moon), GIANT ICE SPIKES, and rare DIAMOND crystals.
random.seed(191)
prev=None                                              # a rising ring of stepping platforms + catwalk bridges
for i in range(9):
    a=i/9*6.283; r=30.0+(i%2)*13.0; px,pz=r*math.cos(a),r*math.sin(a); ph=1.2+i*0.35
    b.box("asteroid.png", px, ph, pz, 3.4, 0.5, 3.4)   # walkable asteroid step
    if prev:
        mx,mz=(px+prev[0])/2,(pz+prev[1])/2
        b.box("plate_blue.png", mx, (ph+prev[2])/2, mz, 1.1, 0.3, 4.2)   # catwalk bridge
    prev=(px,pz,ph)
b.mound("ice.png", -62, 56, 13, 11)                    # a frozen dwarf planet (Voyager's icy worlds)
b.mound("riftcrystal.png", 63, -52, 9, 8)              # a crystalline moon
b.mound("asteroid.png", 56, 60, 8, 6)                  # a captured rock moon
for k in range(7):                                     # giant ice spikes jutting from the field
    a=random.uniform(0,6.283); r=random.uniform(20,66)
    b.spire("ice.png", r*math.cos(a), r*math.sin(a), random.uniform(12,22), random.uniform(1.5,2.8))
for k in range(6):                                     # rare diamond crystals (glinting, non-blocking)
    a=random.uniform(0,6.283); r=random.uniform(24,60)
    b.box("riftcrystal.png", r*math.cos(a), 1.3, r*math.sin(a), 0.55, 1.3, 0.55, collide=False)
b.spire("asteroid.png", 55, 30, 18, 2.4)
b.spire("asteroid.png", -60, 40, 14, 2.0)
for k in range(6):                                     # terrain relief so it isn't flat
    a=random.uniform(0,6.283); r=random.uniform(22,60)
    b.mound("asteroid.png", r*math.cos(a), r*math.sin(a), random.uniform(4,8), random.uniform(1.5,3.5))
b.natural_terrain("asteroid.png", 73)
b.write(os.path.join(DATA,"outer_rim.iqm"), {"floor":"nebula.png","wall":"asteroid.png","rock":"asteroid.png","plate":"plate_blue.png","rust":"rust.png","haz":"hazard.png"})

# --- CYBERTRON (Destiny): the homeworld. Golden-metal ruins, toppled towers,
# a central citadel plinth, the shield-gold sky. Where the trilogy converges. ---
b = Builder(); b.floor("cybertron.png"); b.walls("cybertron.png", h=18)
random.seed(97)
# toppled and standing towers (tall metal columns of varied height)
for i in range(14):
    a=i/14*6.283 + random.uniform(-0.2,0.2); r=random.uniform(24,74)
    hh=random.uniform(4,14)
    b.box("cybertron.png", r*math.cos(a), hh*0.5, r*math.sin(a), 1.6, hh*0.5, 1.6)
# ringed causeway blocks (broken road)
for k in range(16):
    a=k/16*6.283; r=40
    b.box("shield_gold.png", r*math.cos(a), 0.4, r*math.sin(a), 2.2, 0.4, 2.2)
# central citadel plinth (story: the seat of the homeworld)
b.box("cybertron.png", 0, 2.0, 0, 9, 2.0, 9)
b.spire("shield_gold.png", 0, 0, 12, 3)
# CITY SKYLINE: proper Cybertronian BUILDINGS -- wide towers of varied footprint + height,
# many stepped with rooftop spires, forming a metropolis around the citadel (the thin columns
# above now read as toppled ruins among the standing skyline).
random.seed(197)
for i in range(17):
    a = i/17*6.283 + random.uniform(-0.16, 0.16)
    r = random.uniform(28, 78)
    bx, bz = r*math.cos(a), r*math.sin(a)
    w = random.uniform(2.6, 5.4); d = w*random.uniform(0.7, 1.3); h = random.uniform(6, 21)
    if i % 3 == 0:
        # ROUND tower: real prism mesh + inset round-matched collision (no square
        # corners poking past the visible edge)
        b.cyl_tower("cybertron.png", bx, bz, h, w*0.95, sides=8, taper=0.72)
        if h > 12:
            b.box("shield_gold.png", bx, h+random.uniform(1.0,3.0), bz, w*0.5, random.uniform(1.5,3.0), d*0.5, collide=False)
    else:
        b.box("cybertron.png", bx, h*0.5, bz, w, h*0.5, d)                       # the tower shaft
        if h > 12:
            b.box("shield_gold.png", bx, h+random.uniform(1.0,3.0), bz, w*0.62, random.uniform(1.5,3.0), d*0.62)  # stepped crown
    if random.random() < 0.45:
        b.spire("shield_gold.png", bx, bz, random.uniform(3,6), 0.6)         # rooftop antenna spire
b.tunnel("cybertron.png", 0, 34, 14, 11, 6, axis="x")    # monumental gateway you walk THROUGH (hollow)
b.tunnel("cybertron.png", 0, -34, 14, 11, 6, axis="x")
# SKY HIGHWAYS + GROUND ROADS: elevated vehicle-mode roadways threading between the towers
# (drive them in the sky), and a carved ground ring road below -- built for both registers.
random.seed(297)
sky_r = 54
sp = [(sky_r*math.cos(k/4*6.283+0.4), sky_r*math.sin(k/4*6.283+0.4)) for k in range(4)]
for k in range(4):
    b.skyway("shield_gold.png", sp[k][0], sp[k][1], sp[(k+1)%4][0], sp[(k+1)%4][1], 12.0)   # elevated ring
b.skyway("shield_gold.png", -62, 0, 62, 0, 15.0)      # high cross-city span
gr = 38                                                # ground ring road (flat carved causeway, visual)
for k in range(22):
    a=k/22*6.283; b.box("shield_gold.png", gr*math.cos(a), 0.25, gr*math.sin(a), 3.0, 0.25, 3.0, collide=False)
b.write(os.path.join(DATA,"cybertron.iqm"), {"floor":"cybertron.png","wall":"glyph.png","rock":"shield_gold.png"})


# --- THE RECLAIM (Dominion, organic-metal corridors): an overgrown Cybertronian
# world. Three high-speed LANES run the length, separated by dense JUNGLE cover;
# defensive turret plinths guard the lanes; objective platforms anchor each end.
b = Builder(); b.floor("reclaim.png")
import random as _r; _r.seed(303)
# three lanes: raised worn-metal strips running along z (paved corridors)
for lane_x in (-45, 0, 45):
    b.box("lane.png", lane_x, 0.05, 0, 14, 0.05, S*0.9, collide=False)   # lane surface strip (visual, walkable)
# jungle cover between lanes: clustered vine-pillars + rocks (thinned for flow)
for k in range(32):
    zx = _r.uniform(-S*0.85, S*0.85); zz = _r.uniform(-S*0.85, S*0.85)
    # keep jungle OUT of the lane strips
    if any(abs(zx-lx) < 16 for lx in (-45,0,45)): continue
    hh = _r.uniform(1.5, 5.0)
    b.box("reclaim.png", zx, hh*0.5, zz, _r.uniform(0.8,2.0), hh*0.5, _r.uniform(0.8,2.0))
    if _r.random() < 0.3:
        b.spire("rock.png", zx+2, zz, hh*1.6, 0.6)               # vine spires
# objective platforms at the two lane ends (raised capture pads)
for zz in (-S*0.8, S*0.8):
    b.box("plate_blue.png", 0, 0.6, zz, 8, 0.6, 8)
    b.spire("hazard.png", 0, zz, 5, 1.2)
# LANE GATES: each corridor passes under an arch at both ends -- thresholds, not empty
# strips (the lanes now READ as routes)
for lane_x in (-45, 0, 45):
    for gz in (-S*0.62, S*0.62):
        b.arch("reclaim.png", lane_x, gz, 12, 8, 1.6)
b.natural_terrain("reclaim.png", 74)
# VERTICALITY PASS: the reclamation GROWS UPWARD -- regrowth spires and an elevated
# tram line crossing the district (drivable), with support pylons as mid-height cover.
random.seed(505)
for k in range(6):
    x=random.uniform(-70,70); z=random.uniform(-70,70)
    if abs(x)<18 and abs(z)<18: continue
    b.spire("reclaim_growth.png", x, z, random.uniform(12,19), 1.7)
# tall regrowth TOWERS -- the reclamation's great columns, genuine landmarks that
# stretch the district's height range beyond the mid-spire band.
b.cyl_tower("reclaim_growth.png", -52, 40, 17.0, 3.2, sides=8, taper=0.6)
b.cyl_tower("reclaim_growth.png",  50, -44, 15.0, 3.0, sides=8, taper=0.6)
b.skyway("plate_blue.png", -64, -30, 64, 30, 8.0)
for k in range(4):
    t=k/3.0; px=-64+128*t; pz=-30+60*t
    b.box("plate_blue.png", px, 4.0, pz, 1.6, 4.0, 1.6)
b.write(os.path.join(DATA,"reclaim.iqm"), {"floor":"reclaim_growth.png","lane":"lane.png","rock":"rock.png","plate":"plate_blue.png","haz":"hazard.png"})

# --- TRENCH (beneath the pacific): abyssal floor, hydrothermal vent spires,
#     tilted buried Cybertronian monoliths, a central rift dais (the devourer's edge)
b = Builder(); b.floor("trench_abyss.png"); b.walls("abyssrock.png", h=16)
random.seed(61)
for i in range(14):  # a scattered field of vent spires (tall, thin, uneven)
    x=random.uniform(-70,70); z=random.uniform(-70,70)
    if abs(x)<14 and abs(z)<14: continue                 # keep the center clear
    b.spire("abyssrock.png", x, z, random.uniform(6,14), random.uniform(1.3,2.6))
for k in range(6):   # buried monoliths surfacing at a lean (ancient tech)
    a=k/6*6.283; rr=44
    b.box("plate_blue.png", rr*math.cos(a), 3.0, rr*math.sin(a), 2.4, 5.0, 5.5)
b.box("plate_blue.png", 0, 1.0, 0, 10, 1.0, 10)          # central rift dais
b.box("riftcrystal.png", 0, 2.2, 0, 6, 0.4, 6, collide=False)  # the glowing crystal rift (the buried devourer)
b.natural_terrain("seabed.png", 75, plateau=False)
random.seed(161)                                              # LOW seabed rubble: trench was almost all
for _tr in range(16):                                          # high vent spires (7 low vs 32 high)
    _tx=random.uniform(-80,80); _tz=random.uniform(-80,80)
    if abs(_tx)<16 and abs(_tz)<16: continue
    b.box("abyssrock.png", _tx, 0.6, _tz, random.uniform(1.4,2.4), 0.6, random.uniform(1.4,2.4))
b.write(os.path.join(DATA,"trench.iqm"), {"floor":"trench_abyss.png","wall":"abyssrock.png","rock":"abyssrock.png"})

# --- PLUTO (the pluto line): the solar system's outer containment station, at the
#     dark edge of the system. A ring of defense pylons, gate arches on the
#     approaches, and a central sealed containment core (the thing being held).
b = Builder(); b.floor("obsidian.png"); b.walls("plate_blue.png", h=14)
b.ring("plate_blue.png", 0, 0, 56, 12, 11.0, 1.9)          # the defense-pylon line
b.arch("plate_blue.png", 0,  42, 16, 10, 2.2)              # north gate
b.arch("plate_blue.png", 0, -42, 16, 10, 2.2)              # south gate
b.box("plate_blue.png", 0, 1.0, 0, 9, 1.0, 9)              # command dais
b.spire("plate_blue.png", 0, 0, 15, 2.6)                   # the containment pillar
b.box("riftcrystal.png", 0, 3.2, 0, 3, 2.6, 3, collide=False)  # the sealed core (glows)
# raise + widen the station: tall containment towers at the corners (scale + verticality),
# two more gate arches, and a taller command spire base.
for k in range(4):
    a=k*(math.pi/2)+0.5; r=68
    b.box("plate_blue.png", r*math.cos(a), 9.0, r*math.sin(a), 3, 9.0, 3)   # tall corner towers
    b.trim("brushmetal.png", r*math.cos(a), 18.0, r*math.sin(a), 3, 3)      # dressed: parapet + light
b.box("plate_blue.png", 0, 7.0, 0, 4, 7.0, 4)              # taller command spire base
b.arch("plate_blue.png",  42, 0, 16, 11, 2.2)              # east gate
b.arch("plate_blue.png", -42, 0, 16, 11, 2.2)              # west gate
random.seed(88)
for i in range(9):                                         # defense-grid sensor masts, dressed
    x=random.uniform(-72,72); z=random.uniform(-72,72)
    if abs(x)<22 and abs(z)<22: continue
    b.pylon("plate_blue.png", x, z, random.uniform(5,9))
# THE CONTAINMENT LINE, expanded: the wall itself now stands -- long rampart sections
# with watch bastions, twin relay pylons, and the ice-dock gantry where the fleet ties up.
for k in range(4):                                          # rampart sections (the LINE)
    a=k*1.5708+0.4; cx,cz=math.cos(a)*62, math.sin(a)*62
    b.box("plate_blue.png", cx, 3.2, cz, 9.0*abs(math.sin(a))+2.0, 3.2, 9.0*abs(math.cos(a))+2.0)
    b.box("plate_blue.png", cx, 7.2, cz, 2.2, 1.0, 2.2)     # watch bastion atop each section
b.pylon("plate_blue.png", 30, 30, 11.0)                     # relay pylon pair, dressed
b.pylon("plate_blue.png", 38, 24, 9.0)
b.skyway("plate_blue.png", -58, -20, -20, -58, 7.0)         # the ice-dock gantry (drivable)
for th2 in ((-40,26),(30,-38),(52,44),(-56,-30)):          # THOLIN DRIFTS: the red-brown
    b.box("tholin.png", th2[0], 0.10, th2[1], 9.0, 0.10, 7.0, collide=False)   # photochemical paint
# ---- ART PASS (ART_BIBLE 2.4): THE SOLAR DEFENSE GRID -- the weapons lattice overhead ----
for _lz in (-45.0, 0.0, 45.0):                              # E-W truss lines, high over the station
    b.box("brushmetal.png", 0, 16.5, _lz, 70.0, 0.18, 0.18, collide=False)
    b.box("brushmetal.png", _lz, 16.5, 0, 0.18, 0.18, 70.0, collide=False)    # N-S truss lines
for _lx in (-45.0, 0.0, 45.0):                              # weapon nodes at the 9 lattice crossings
    for _lz in (-45.0, 0.0, 45.0):
        b.box("energon.png", _lx, 16.5, _lz, 0.6, 0.6, 0.6, collide=False)
for k in range(4):                                          # RETROFIT: human patch-plates + energon
    a=k*(math.pi/2)+0.5; r=68                               # seams on the cybertronian corner towers
    _tx, _tz = r*math.cos(a), r*math.sin(a)
    b.box("rust.png", _tx, 11.0+k*1.3, _tz+3.05, 1.8, 1.1, 0.12, collide=False)     # bolted patch plate
    b.box("energon.png", _tx+3.05, 8.0, _tz, 0.10, 5.5, 0.28, collide=False)        # a live energon seam
# SC2 BEST-PRACTICES PASS (docs: Blizzard "Best Practices in Art and Performance"):
#  - LEAD THE EYE: lit trails from the command dais out to each gate approach.
#  - DETAIL IN THE MARGINS, GROUNDED: spaced debris clusters in the far corners,
#    each on a painted ground patch so they read as planted, not floating.
for gx, gz in ((0, 40), (0, -40), (40, 0), (-40, 0)):        # dais -> each gate
    b.eye_trail("energon.png", 0, 0, gx, gz, n=5, w=0.7)
for cx2, cz2 in ((-74, -74), (74, 74), (-74, 74), (74, -74)): # margin debris, grounded
    b.ground_cluster("tholin.png", "abyssrock.png", cx2, cz2, r=7.0, count=5)
b.write(os.path.join(DATA,"pluto.iqm"))

# --- BEACH (the beach opens): Earth's shore tearing apart as energon breaches the
#     mantle. A jagged glowing TEAR down the middle with heaved broken ground on
#     both sides, a collapsing PIER marching out to the edge, and beach debris.
b = Builder(); b.floor("sand.png"); b.walls("rock.png", h=12)
for k in range(-4,5):                                  # THE TEAR: a jagged energon seam
    z = k*14.0; off = math.sin(k*1.3)*6.0
    b.box("energon.png", off, 0.6, z, 2.0, 0.5, 6.5, collide=False)   # the glow
    b.box("rock.png", off-6.5, 1.6, z, 2.2, 1.6, 4.0)                 # heaved ground L
    b.box("rock.png", off+6.5, 1.8, z, 2.4, 1.8, 4.0)                 # heaved ground R
random.seed(41)
for i in range(8):                                     # COLLAPSING PIER: pilings to the edge
    h = 6.0 - i*0.4
    b.box("plate_blue.png", 40.0+i*7.0, h*0.5, -50.0+i*2.0, 1.4, max(1.0,h*0.5), 1.4)
for i in range(6):                                     # beach debris
    x=random.uniform(-72,72); z=random.uniform(-72,72)
    if abs(x)<12: continue
    b.box("rock.png", x, 1.0, z, random.uniform(1.5,3.0), 1.0, random.uniform(1.5,3.0))
# LOS ANGELES SHOREFRONT, BREAKING: the destruction around Aiden. Gutted boardwalk
# buildings (walls standing, roofs half-fallen), a snapped highway overpass stepping down
# into the sand, burning wreck piles, listing palm trunks, and abandoned lifeguard posts.
random.seed(377)
for bx, bz in ((-52, 46), (-14, 58), (30, 52)):        # gutted boardwalk buildings
    b.box("rock.png", bx-4.5, 2.2, bz, 0.7, 2.2, 5.0)  # standing side wall
    b.box("rock.png", bx+4.5, 1.6, bz, 0.7, 1.6, 5.0)  # sheared side wall (lower)
    b.box("rock.png", bx, 0.8, bz-4.6, 4.2, 0.8, 0.6)  # broken front face
    b.box("rust.png", bx+1.0, 3.4, bz+1.0, 3.6, 0.25, 3.2)   # the roof, half-fallen, tilted rest
for k in range(4):                                     # the snapped OVERPASS: slabs stepping down
    b.box("rock.png", -58+k*9, 3.2-k*0.8, -52, 4.4, 0.35, 3.2)
b.box("rock.png", -58, 1.4, -52, 0.9, 1.4, 0.9)        # a surviving pylon
for k in range(5):                                     # burning wreck piles (glow, non-blocking)
    a=random.uniform(0,6.283); r=random.uniform(20,58)
    b.box("molten.png", r*math.cos(a), 0.35, r*math.sin(a), 0.9, 0.35, 0.9, collide=False)
for k in range(7):                                     # listing palm trunks
    a=random.uniform(0,6.283); r=random.uniform(26,72)
    b.spire("rust.png", r*math.cos(a), r*math.sin(a), random.uniform(4,7), 0.35, collide=False)
b.box("hazard.png", 14, 0.9, 44, 1.0, 0.9, 1.0)        # lifeguard posts, abandoned
b.box("hazard.png", -38, 0.9, 30, 1.0, 0.9, 1.0)
b.natural_terrain("sand.png", 77, plateau=False, dunes=True)
# TSUNAMI REFUGE TOWERS: three stepped wreck-platforms rising over wave height, each with a
# climbable stair of stacked decks -- the parkour line you sprint when the sea pulls back.
for tx, tz in ((-32, 18), (2, -26), (34, 30)):
    b.box("rock.png", tx, 0.4, tz, 5.0, 0.4, 5.0)            # first deck  (low -- reachable at a run)
    b.box("rust.png", tx+2.5, 0.8, tz+2.5, 3.4, 0.4, 3.4)    # second deck (stepped up + across)
    b.box("brushmetal.png", tx+5.0, 1.2, tz, 2.4, 0.4, 2.4)  # crow's deck (safe over any wave)
    b.spire("rust.png", tx+5.0, tz, 5, 0.5, collide=False)   # a mast to spot it from the surf
# ---- SANTA MONICA, UNMISTAKABLY (mission accuracy pass): the wave chapter is scripted
# in Los Angeles, so the map now SAYS so -- a downtown skyline wall, the pier with its
# FERRIS WHEEL, palm lines along the boardwalk, and the snapped PCH.
# THE SKYLINE: a stepped wall of towers along the inland (east) edge -- LA reads at a glance
random.seed(2029)
for k in range(7):
    tz = -66.0 + k*22.0
    th = 8.0 + (k*37 % 11)                                # varied heights
    b.city_tower("cybertron.png", 76.0, tz, th, 3.6)      # setback tiers + lit bands + antenna
b.box("hazard.png", 76.0, 2.0, 10.0, 4.8, 2.0, 3.0)       # one tower COLLAPSED into rubble
# THE PIER, PROPER: a deck running west over the sand to the sea-line, on the old pilings
for i in range(7):
    b.box("plate_blue.png", -40.0-i*5.5, 5.6, -50.0, 3.0, 0.35, 5.0)    # deck sections (drivable)
b.box("plate_blue.png", -40.0, 6.4, -44.0, 14.0, 0.3, 0.8, collide=False)  # rail
# THE FERRIS WHEEL: the Santa Monica silhouette -- a vertical ring of lit cars
wx, wz, wr = -58.0, -58.0, 7.5
b.box("brushmetal.png", wx, 4.0, wz, 0.7, 4.0, 0.7)       # A-frame support
for k in range(10):
    a = k*(math.pi*2/10)
    b.box("energon.png", wx+math.cos(a)*wr, 8.0+math.sin(a)*wr, wz, 0.9, 0.9, 0.7, collide=False)  # lit cars
b.box("brushmetal.png", wx, 8.0, wz, 1.1, 1.1, 0.9, collide=False)      # hub
# PALM LINES: listing trunks along the boardwalk
random.seed(310)
for k in range(8):
    px2 = -20.0 + k*9.0
    b.spire("rust.png", px2, 34.0, 4.5+random.uniform(0,1.5), 0.45, collide=False)
    b.box("reclaim.png", px2, 5.4+random.uniform(0,1.0), 34.0, 1.6, 0.35, 1.6, collide=False)  # frond crown
# THE PCH: a broken coast highway strip with a center divider, stepping down where snapped
for k in range(5):
    b.box("hazard.png", -70.0+k*30.0, 0.35, 56.0, 13.0, 0.35, 4.0)
b.box("white.png", -12.0, 0.85, 56.0, 55.0, 0.12, 0.25, collide=False)   # the divider line
# ---- ART PASS (ART_BIBLE 2.1, "Earth Splits"): the map's single wound of light ----
b.box("molten.png", -81.0, 0.35, 0.0, 0.9, 0.35, 84.0, collide=False)    # THE SEAM: one glowing red line, horizon to horizon
b.box("molten.png", -80.0, 0.12, 0.0, 5.5, 0.12, 5.5, collide=False)     # THE EYE: an ember disc beneath the crust
for _ea in range(8):                                                      # ...ringed by dark crust plates (it flickers UNDER)
    _aa = _ea*(math.pi*2/8)
    b.box("obsidian.png", -80.0+math.cos(_aa)*6.5, 0.5, math.sin(_aa)*6.5, 2.2, 0.5, 2.2, collide=False)
b.box("cryoglass.png", -72.0, 0.06, 0.0, 3.5, 0.05, 84.0, collide=False)  # DRAINED SEA: the pale receded waterline band
b.box("rust.png", 60.0, 7.75, -20.0, 1.6, 7.75, 1.6)        # a collapsed WATER TOWER leg, standing alone
b.box("hazard.png", -66.0, 7.0, 66.0, 1.7, 7.0, 1.7)        # a jack-knifed crane counterweight column
# SC2 BEST-PRACTICES PASS (Blizzard "Best Practices in Art and Performance"):
#  - LEAD THE EYE: a lit trail down the pier toward the sea-line, so the route reads.
#  - MARGIN DETAIL, GROUNDED: spaced beach-wrack clusters inland, on sand patches.
b.eye_trail("energon.png", -40.0, -50.0, -73.0, -50.0, n=6, w=0.6)        # pier deck -> the water
for cx3, cz3 in ((66, 66), (-70, -6), (66, 40)):                           # inland margins, grounded
    b.ground_cluster("sand.png", "rust.png", cx3, cz3, r=6.5, count=4)
# COLLAPSED RUINS: remnants of the drowned city's inner blocks, spread across the
# open field so verticality lives where the fight is -- not just the tower row at
# the east edge. Each ruin is a tall husk + a mid stump + a low slab (height spread).
random.seed(451)
for rx, rz in ((-20,-8),(8,14),(-34,20),(22,-24),(-8,-40),(34,8),(-46,-24),(14,42)):
    hh = random.uniform(9.0, 18.0)                                  # the standing husk (tall)
    b.box("brushmetal.png", rx, hh*0.5, rz, random.uniform(2.4,3.6), hh*0.5, random.uniform(2.2,3.4))
    b.box("obsidian.png", rx+random.uniform(-4,4), 0.6, rz+random.uniform(-4,4), 2.6, 0.6, 2.4, collide=False)  # rubble slab
b.write(os.path.join(DATA,"beach.iqm"))

# --- MARS (the red extraction): a Cybertron fragment came down here and the
#     scavengers are digging it out. A big tilted metallic FRAGMENT half-buried at
#     center, EXTRACTION derricks ringing the dig, and red rock mesas for cover.
b = Builder(); b.floor("mars.png"); b.walls("rock.png", h=12)
b.box("cybertron.png", 0, 4.0, 0, 7, 4.0, 5)              # the fragment body
b.box("cybertron.png", 3, 7.0, -2, 3, 2.0, 2)             # jutting shard
b.box("shield_gold.png", 0, 2.0, 6, 4, 2.0, 2)            # cracked glowing seam
# the fragment is COLOSSAL -- rear it up higher with fractured spars + an extraction arch,
# and ring the dig with taller mesas so the field reads vertical, not flat.
b.box("cybertron.png", 0, 9.5, 0, 4, 5.5, 3)              # fragment superstructure (tall)
b.spire("cybertron.png", -5, 4, 18, 2.6)                  # jutting fractured spars
b.spire("cybertron.png", 6, -3, 16, 2.4)
b.arch("plate_blue.png", 24, -22, 14, 11, d=2.5)          # an extraction gantry arch
for k in range(4):                                        # taller mesas -> vertical cover
    a=random.uniform(0,6.283); r=random.uniform(30,60)
    b.box("rock.png", r*math.cos(a), random.uniform(4,7), r*math.sin(a),
          random.uniform(3,5), random.uniform(4,7), random.uniform(3,5))
for k in range(4):                                        # extraction derricks + rig pads
    a = k*(math.pi/2) + 0.4
    rx, rz = math.cos(a)*30.0, math.sin(a)*30.0
    b.spire("plate_blue.png", rx, rz, 14, 2.0)
    b.box("plate_blue.png", rx, 1.0, rz, 4, 1.0, 4)
random.seed(77)
for i in range(7):                                        # red rock mesas (cover)
    x=random.uniform(-70,70); z=random.uniform(-70,70)
    if abs(x)<18 and abs(z)<18: continue
    h=random.uniform(2.0,5.0)
    b.box("rock.png", x, h*0.5, z, random.uniform(3.0,6.0), h*0.5, random.uniform(3.0,6.0))
b.natural_terrain("mars.png", 76, dunes=True, style="mars")
b.write(os.path.join(DATA,"mars.iqm"))

# --- CORE (the mantle dive / the Forge): Cybertron's molten heart, deep underworld
#     (War-for-Cybertron industrial + the script's Forge where Hot Rod takes the
#     Matrix). Dark metal floor cut by glowing molten channels, a central Forge dais
#     with the Matrix core ringed by pillars, and descent shafts around the edge.
b = Builder(); b.floor("molten.png"); b.walls("cybertron.png", h=16)
# molten channels: a glowing cross of energon rivers (non-colliding light)
for t in range(-6,7):
    b.box("energon.png", t*11.0, 0.45, 0, 4.5, 0.4, 1.6, collide=False)   # E-W river
    b.box("energon.png", 0, 0.45, t*11.0, 1.6, 0.4, 4.5, collide=False)   # N-S river
# the FORGE: a raised dais ringed by pillars, the Matrix core glowing at its heart
b.ring("cybertron.png", 0, 0, 18, 8, 9.0, 1.6)      # forge pillar ring
b.box("cybertron.png", 0, 1.2, 0, 8, 1.2, 8)        # forge dais
b.spire("cybertron.png", 0, 0, 11, 2.2)             # the anvil column
b.box("shield_gold.png", 0, 3.4, 0, 2.4, 2.4, 2.4, collide=False)  # the Matrix core (glows)
# SLAG FIELD: cooled molten shelves + slag heaps at mixed scales -- cover between
# the rivers and the forge, where the plan had nothing but open dark metal
for gx0, gz0, gw0, gh0 in ((-58,24,4.2,1.6),(46,-56,5.5,1.1),(64,30,3.0,2.4),
                            (-62,-46,3.6,2.0),(18,64,4.8,1.4),(-20,-64,2.8,2.8),
                            (52,58,3.4,1.3),(-38,52,4.0,1.8),(34,-24,2.4,1.0),(-46,4,3.0,1.2)):
    b.box("obsidian.png", gx0, gh0, gz0, gw0, gh0, gw0*0.7)
    b.box("molten.png", gx0, gh0*2.0+0.15, gz0, gw0*0.5, 0.15, gw0*0.35, collide=False)   # a molten crown, still cooling
for ls2 in range(8):                                          # LOW slag lumps: true shoot-over cover
    b.box("obsidian.png", random.uniform(-60,60), 0.6, random.uniform(-60,60), random.uniform(2.2,3.5), 0.6, random.uniform(1.8,2.8))
for ct2 in ((-66,60,12.0),(66,-62,13.5)):                    # frozen FLOW TOWERS: eruption columns cooled mid-climb
    b.box("obsidian.png", ct2[0], ct2[2]*0.5, ct2[1], 2.6, ct2[2]*0.5, 2.4)
    b.box("molten.png", ct2[0], ct2[2]+0.3, ct2[1], 1.6, 0.3, 1.5, collide=False)
    b.trim("obsidian.png", ct2[0], ct2[2], ct2[1], 2.6, 2.4, emit="molten.png")   # dressed rim
    # SC2 VALUE-CONTRAST: heat rises -- bands brighten with height (obsidian->rust->molten)
    b.height_bands(ct2[0], ct2[1], 2.0, ct2[2], 2.6, 2.4,
                   ["obsidian.png", "rust.png", "molten.png"])
# SC2 MARGIN DETAIL, GROUNDED: cooled slag heaps in the bare far corners
for cx5, cz5 in ((-70, -20), (70, 24), (20, 70)):
    b.ground_cluster("obsidian.png", "molten.png", cx5, cz5, r=6.5, count=4)
for ov in ((-82,10,11.0),(82,-8,12.0)):                       # OUTER vent spires, reach toward the walls
    b.spire("obsidian.png", ov[0], ov[1], ov[2], 2.2)
    b.box("molten.png", ov[0], 0.9, ov[1], 2.6, 0.5, 2.6, collide=False)     # glowing base vent
random.seed(313)                                              # MORE LOW slag: core had almost no low
for _ls2 in range(18):                                         # cover (8 low vs 49 high) -- fix the balance
    _lx2 = random.uniform(-76,76); _lz2 = random.uniform(-76,76)
    if abs(_lx2) < 24 and abs(_lz2) < 24: continue              # keep the forge dais clear
    b.box("obsidian.png", _lx2, 0.55, _lz2, random.uniform(1.3,2.2), 0.55, random.uniform(1.3,2.2))
# descent shafts: tall industrial towers at the corners suggesting the deep
random.seed(303)
for k in range(6):
    a = k*(math.pi/3) + 0.5; rx, rz = math.cos(a)*58.0, math.sin(a)*58.0
    b.city_tower("cybertron.png", rx, rz, 14.0, 2.6, emit="molten.png")   # tiered descent shafts
    b.box("molten.png", rx, 1.0, rz, 3.4, 0.5, 3.4, collide=False)        # glowing base vent
# THE FORGE OF THE MATRIX, expanded: a central CRUCIBLE with a molten pour, a raised
# catwalk ring for the fight, and great forge columns -- cover + scale in the deep.
b.ring("obsidian.png", 0, 0, 20, 10, 3.0, 1.4)              # the crucible rim
b.box("molten.png", 0, 0.25, 0, 14, 0.25, 14, collide=False)   # the molten pour (glow floor)
b.ring("cybertron.png", 0, 0, 40, 12, 1.2, 1.0)             # raised catwalk ring (steppable)
for k in range(4):                                          # great forge columns
    a=k*1.5708+0.785; b.box("cybertron.png", math.cos(a)*30, 6.0, math.sin(a)*30, 2.6, 6.0, 2.6)
    b.spire("molten.png", math.cos(a)*30, math.sin(a)*30, 9, 0.9, collide=False)   # pour-light
b.write(os.path.join(DATA,"core.iqm"))

# --- KUIPER (Blackstar's scavenger fleet): an asteroid-belt salvage yard. Drifting
#     asteroids for cover + verticality, rusted salvage platforms at varied heights
#     reached by gantry ramps, wreckage strewn between, and Blackstar's flagship deck
#     with its mast at the center. Modelled on the A-grade cybertron layout. ---
b = Builder(); b.floor("stars.png"); b.walls("asteroid.png", h=16)
random.seed(88)
for i in range(16):    # drifting asteroids (varied-height rock -- cover + verticality)
    a = i/16*6.283 + random.uniform(-0.25,0.25); r = random.uniform(24,90)
    hh = random.uniform(3.0,13.0)
    b.box("asteroid.png", r*math.cos(a), hh*0.5, r*math.sin(a), random.uniform(2,4), hh*0.5, random.uniform(2,4))
# icy Kuiper touches: giant ice spikes among the rock + a couple of rare diamond crystals
for k in range(5):
    a=random.uniform(0,6.283); r=random.uniform(28,80)
    b.spire("ice.png", r*math.cos(a), r*math.sin(a), random.uniform(10,18), random.uniform(1.4,2.4), collide=False)
for k in range(4):
    a=random.uniform(0,6.283); r=random.uniform(30,70)
    b.box("riftcrystal.png", r*math.cos(a), 1.2, r*math.sin(a), 0.5, 1.2, 0.5, collide=False)
for k in range(4):     # salvage platforms at varied heights + a gantry ramp up to each
    a = k/4*6.283 + 0.4; r = 46
    px, pz = r*math.cos(a), r*math.sin(a); ph = random.uniform(3.5,5.5)
    b.box("rust.png", px, ph, pz, 9, 0.5, 9)
    b.ramp("rust.png", px, pz - 12*(1 if pz<=0 else -1), 13, 6, ph*2, dz=(1 if pz<=0 else -1))
for i in range(9):     # strewn wreckage (low cover), kept clear of spawn
    wx = random.uniform(-86,86); wz = random.uniform(-86,86)
    if abs(wx) < 15 and abs(wz) < 15: continue
    b.box("plate_blue.png", wx, 0.8, wz, random.uniform(1.6,3.2), 0.8, random.uniform(1.6,3.2))
for i2 in range(30):   # MORE strewn wreckage: kuiper's cover was badly skewed toward
    wx2 = random.uniform(-88,88); wz2 = random.uniform(-88,88)   # high (12 low vs 104 high);
    if abs(wx2) < 15 and abs(wz2) < 15: continue                  # stays at 146 colliders, under the 150 cap
    b.box("rust.png", wx2, 0.55, wz2, random.uniform(1.3,2.2), 0.55, random.uniform(1.3,2.0))
b.box("rust.png", 0, 2.2, 0, 11, 2.2, 11)       # Blackstar's flagship deck
b.spire("hazard.png", 0, 0, 15, 2.8)            # the mast (tall landmark)
# ---- ART PASS (ART_BIBLE 2.2): THE BLACKSTAR ARRAY, aft of the mast ----
b.box("obsidian.png", 0, 5.2, 18, 3.4, 0.8, 3.4)                          # the plinth (colliding: it sits on deck level)
_arx, _ary, _arz = 0.0, 9.2, 18.0                                          # sphere center
random.seed(888)
for _sh in range(9):                                                       # SHATTERED CRYSTAL SPHERE: jagged shard cluster
    _sa = _sh*(math.pi*2/9) + random.uniform(-0.2,0.2)
    _sr = random.uniform(1.2,2.6)
    b.box("riftcrystal.png", _arx+math.cos(_sa)*_sr, _ary+random.uniform(-1.6,2.0), _arz+math.sin(_sa)*_sr,
          random.uniform(0.7,1.4), random.uniform(1.0,2.2), random.uniform(0.7,1.4), collide=False)
b.box("riftcrystal.png", _arx, _ary, _arz, 1.8, 1.8, 1.8, collide=False)   # the fractured heart
for _fb in ((1.9,0.8,0.4),(-1.6,-0.6,1.2),(0.3,1.9,-1.5),(-0.9,0.9,1.8)): # embedded CHICAGO transport-event fragments
    b.box("plate_blue.png", _arx+_fb[0], _ary+_fb[1], _arz+_fb[2], 0.5, 0.7, 0.5, collide=False)
for _lb in ((-1.2,-3.0),(0.0,0.0),(1.4,2.6)):                              # DEEP RED LENS BEAMS piercing into space
    b.box("molten.png", _arx+_lb[0], 20.0, _arz+_lb[1], 0.22, 9.5, 0.22, collide=False)
# BLACKSTAR WRECKAGE: towering broken ship-hull spars rearing out of the belt -- a
# salvage skyline (kept few + thin so we stay under the collider cap).
b.spire("rust.png", -40, -30, 20, 2.4)
b.spire("rust.png", 45, 25, 22, 2.6)
b.spire("rust.png", 38, -46, 18, 2.2)
b.write(os.path.join(DATA,"kuiper.iqm"), {"floor":"stars.png","wall":"asteroid.png","rock":"asteroid.png","rust":"rust.png","plate":"plate_blue.png","haz":"hazard.png"})

# --- UNICRON (the finale): inside the god-machine's chest. A vast dark chamber, the
#     HEART pulsing at the center, ribbed machinery pillars, energon veins radiating
#     outward, and concentric tiers descending toward the core. ---
b = Builder(); b.floor("obsidian.png"); b.walls("cybertron.png", h=22)
random.seed(66)
b.box("obsidian.png", 0, 1.5, 0, 10, 1.5, 10)               # the heart dais (boss focus)
b.box("energon.png", 0, 5.0, 0, 3, 3.5, 3, collide=False)   # the pulsing heart (glows)
b.spire("cybertron.png", 0, 0, 9, 3)                        # the core column
for k in range(8):     # energon veins radiating from the heart (glowing, non-collide)
    a = k/8*6.283
    for seg in range(3,9):
        r = seg*7.0
        b.box("energon.png", r*math.cos(a), 0.4, r*math.sin(a), 2.2, 0.35, 2.2, collide=False)
for i in range(14):    # ribbed machinery pillars (Unicron's innards), varied height
    a = i/14*6.283 + random.uniform(-0.15,0.15); r = random.uniform(30,82)
    hh = random.uniform(5,16)
    b.box("cybertron.png", r*math.cos(a), hh*0.5, r*math.sin(a), random.uniform(2,3.5), hh*0.5, random.uniform(2,3.5))
for (rr,hh) in [(52,1.0),(40,2.0),(28,3.0)]:   # concentric descent tiers -- BREAKING as
    for k in range(18):                         # he stirs: gapped, jittered, some heaved up
        if (k*7 + int(rr)) % 3 == 0: continue                    # a third of each ring is GONE
        a = k/18*6.283 + random.uniform(-0.14,0.14)
        rj = rr + random.uniform(-2.5,2.5)
        hj = hh * random.uniform(0.7,1.6)                        # some segments heaved higher
        b.box("obsidian.png", rj*math.cos(a), hj*0.5, rj*math.sin(a), 2.6, hj*0.5, 2.6)
b.write(os.path.join(DATA,"unicron.iqm"), {"floor":"obsidian.png","wall":"cybertron.png","rock":"cybertron.png"})


# ============================ NASA-ARCHIVE WORLDS ============================
# Three real outer-system worlds, built from the mission archives (Voyager, Galileo,
# Cassini-Huygens, Juno) and threaded with the Blackstar-era lore: the Fallen's survey
# fleet charted these moons an age before Earth; their beacons still tick under the ice.

# --- EUROPA (Galileo/Juno): Jupiter's ocean moon. A shell of cracked ice scored by long
#     double-ridged LINEAE, jumbled CHAOS TERRAIN where the shell broke and refroze, and a
#     south-polar PLUME field venting the buried ocean. Lore: an ancient Cybertronian
#     listening post sits over the warm ocean -- the first place the Fallen's fleet heard
#     something ANSWER from below the ice. ---
b = Builder(); b.floor("europa_ice.png"); b.walls("europa_ice.png", h=14)
random.seed(301)
# CHAOS TERRAIN: where the shell BROKE and refroze -- rafted ice blocks upthrust and
# canted, climbable, casting long shadows across the lineae (Conamara Chaos, in spirit)
for cb in range(8):
    cx0 = random.uniform(-55, 55); cz0 = random.uniform(-55, 55)
    if abs(cx0) < 12 and abs(cz0) < 12: continue
    ch0 = random.uniform(2.2, 4.6)
    b.box("europa_ice.png", cx0, ch0, cz0, random.uniform(4,7), ch0, random.uniform(3,5))
    b.box("white.png", cx0+2.0, ch0*2.0+0.4, cz0-1.0, random.uniform(2.5,4), 0.5, random.uniform(2,3), collide=False)  # tilted raft cap
for pt in range(2):                                        # frozen plume towers over the vents
    b.spire("white.png", -44+pt*88, 50-pt*100, random.uniform(9,13), 1.6)
for tt in ((-62,-8,7.5),(58,-58,6.5),(8,62,8.0)):          # upthrust TOWERS: the shell driven skyward
    b.box("europa_ice.png", tt[0], tt[2], tt[1], 3.0, tt[2], 2.6)
for lw in range(6):                                        # low refrozen slabs (shoot-over cover)
    b.box("white.png", random.uniform(-60,60), 0.9, random.uniform(-60,60), random.uniform(2.5,4), 0.9, random.uniform(2,3))
for li in range(3):                       # the great lineae: paired ridge lines crossing the shell,
    ang = 0.5 + li*1.1; ca, sa = math.cos(ang), math.sin(ang)   # broken into low STEPPABLE segments
    for seg in range(-3, 4):
        if seg == 0: continue             # a gap at every crossing so routes stay open
        d = seg*19.0
        for off in (-2.2, 2.2):           # double ridge (the classic Europa signature)
            b.box("plate_blue.png", ca*d + off*sa, 0.45, sa*d - off*ca + (li-1)*20, 7.0, 0.45, 2.0)
for i in range(9):                        # chaos terrain: tilted refrozen ice rafts
    a=random.uniform(0,6.283); r=random.uniform(26,62)
    b.box("europa_ice.png", r*math.cos(a), random.uniform(1.0,2.6), r*math.sin(a),
          random.uniform(3.5,6.0), random.uniform(1.0,2.6), random.uniform(3.5,6.0))
b.ring("riftcrystal.png", 40, -48, 9, 6, 3.0, 0.9)          # the south plume field
b.spire("riftcrystal.png", 40, -48, 14, 1.2, collide=False) # a frozen plume column
b.box("plate_blue.png", 0, 1.4, 0, 6, 1.4, 6)               # the listening post over the warm ocean
b.spire("plate_blue.png", 0, 0, 11, 1.6)                    # its antenna mast
b.mound("europa_ice.png", -52, 40, 11, 6)                          # an upthrust ice dome
b.natural_terrain("europa_ice.png", 301, n=8, max_h=1.8, style="europa")
b.write(os.path.join(DATA,"europa.iqm"), {"floor":"europa_ice.png","wall":"europa_ice.png","rock":"plate_blue.png"})

# --- IO (Voyager 1, 1979 / Galileo): the most volcanically active body known. Sulfur
#     plains, LAVA LAKES (Loki Patera), towering plume vents, and sheer mountain blocks.
#     Lore: the magma-clones were first poured here -- Io's tides never let the forges
#     cool, so the clone-fires have burned since the age before the war. ---
b = Builder(); b.floor("io_sulfur.png"); b.walls("obsidian.png", h=14)
random.seed(307)
for k in range(3):                        # lava lakes (Loki Patera and kin) -- glowing, flat
    a=k/3*6.283+0.5; r=38
    lx,lz=r*math.cos(a),r*math.sin(a)
    b.box("molten.png", lx, 0.12, lz, random.uniform(7,11), 0.12, random.uniform(6,9), collide=False)
    b.ring("obsidian.png", lx, lz, 13, 6, 1.6, 1.0)          # the caldera rim
for k in range(4):                        # volcano cones w/ plume vents
    a=random.uniform(0,6.283); r=random.uniform(30,66)
    vx,vz=r*math.cos(a),r*math.sin(a)
    b.mound("obsidian.png", vx, vz, 8, 5)
    b.box("obsidian.png", vx, random.uniform(6,10), vz, 2.0, random.uniform(6,10), 2.0)          # solid vent core (tall)
    b.spire("hazard.png", vx, vz, random.uniform(10,16), 1.0, collide=False)   # the plume
for k in range(5):                        # sheer mountain blocks (Boosaule-class: Io has the tallest peaks)
    a=random.uniform(0,6.283); r=random.uniform(40,74)
    hh2=random.uniform(10,18)
    b.box("obsidian.png", r*math.cos(a), hh2, r*math.sin(a), 3.2, hh2, 3.2)
b.tier_pad("obsidian.png", 0, 0, 7, 7, 2.4, rim="hazard.png", ground="obsidian.png")  # the forge-plinth as a READABLE landmark
b.eye_trail("hazard.png", 40, 0, 9, 0, n=6)                 # lead the eye to the forge from the east
b.eye_trail("hazard.png", 0, -40, 0, -9, n=6)              # ...and from the south
# VARIED, INTERACTIVE VOLCANIC DEBRIS (was 16 identical obsidian boxes): mixed
# cooled-lava chunks + slag at varied sizes, ~35% solid cover you weave through.
random.seed(1307)                                            # SULFUR RUBBLE: io was sparse for its size
for _ir in range(16):                                         # (27 colliders over a 187u map)
    _ix=random.uniform(-78,78); _iz=random.uniform(-78,78)
    if abs(_ix)<10 and abs(_iz)<10: continue                  # keep the forge-plinth clear
    b.box("obsidian.png", _ix, 0.6, _iz, random.uniform(1.4,2.4), 0.6, random.uniform(1.4,2.2))
b.natural_terrain("io_sulfur.png", 307, n=8, max_h=2.0, style="io")
b.write(os.path.join(DATA,"io.iqm"), {"floor":"io_sulfur.png","wall":"obsidian.png","rock":"obsidian.png"})

# --- TITAN (Cassini-Huygens, 2005): Saturn's haze moon. METHANE SEAS in the north
#     (Kraken + Ligeia Mare), belts of long hydrocarbon DUNES, a cryovolcano, and the
#     Huygens landing site kept as a shrine. Lore: Blackstar's scavenger clans tithe fuel
#     from the seas -- the only world where the fuel falls as rain. ---
b = Builder(); b.floor("titan_dune.png"); b.walls("rust.png", h=13)
random.seed(311)
b.box("seabed.png", -34, 0.10, -44, 22, 0.10, 15, collide=False)   # Kraken Mare (methane sea)
b.box("seabed.png", 40, 0.10, -50, 13, 0.10, 10, collide=False)    # Ligeia Mare
b.ring("rust.png", -34, -44, 26, 7, 1.2, 0.9)                      # shoreline reefs
b.mound("rock.png", 44, 42, 10, 7)                                  # the cryovolcano (water-ice lava)
b.spire("ice.png", 44, 42, 9, 1.2, collide=False)
b.box("plate_blue.png", 0, 0.9, 34, 1.6, 0.9, 1.6)                 # the Huygens lander shrine
b.spire("plate_blue.png", 0, 34, 6, 0.7, collide=False)
for k in range(4):                        # scavenger fuel-tithe rigs on the sea edge
    b.box("rust.png", -34+k*7-10, 2.0, -30, 1.8, 2.0, 1.8)
for k in range(5):                        # labyrinth mesas (Titan's carved highlands) -- verticality
    a=random.uniform(0,6.283); r=random.uniform(34,70)
    b.box("rock.png", r*math.cos(a), random.uniform(3.5,7.0), r*math.sin(a), 3.2, random.uniform(3.5,7.0), 3.2)
b.spire("rust.png", 26, 8, 12, 1.8)                                # the tithe-master's mast
b.box("obsidian.png", -80, 5.5, 58, 3.2, 5.5, 3.0)                  # an OUTER labyrinth mesa, near the wall
for k2 in range(10):                       # more low tithe-rig cover, scattered wider
    tx3=random.uniform(-76,76); tz3=random.uniform(-76,76)
    if abs(tx3+34)<20 and abs(tz3+44)<18: continue                  # keep clear of Kraken Mare
    if abs(tx3-40)<16 and abs(tz3+50)<13: continue                  # keep clear of Ligeia Mare
    b.box("rust.png", tx3, 0.6, tz3, random.uniform(1.4,2.2), 0.6, random.uniform(1.4,2.2))
b.natural_terrain("titan_dune.png", 311, n=10, max_h=2.4, dunes=True, style="titan")   # the equatorial dune belts
b.write(os.path.join(DATA,"titan.iqm"), {"floor":"titan_dune.png","wall":"rust.png","rock":"rock.png"})

# ============================ THE CLONE HOMEWORLDS ============================
# The warlords are not singular -- they are CLONE LINES, and each line has a homeworld
# where its civilization grew strange around its element. Fight a warlord at home and you
# fight the PRIME of the line, wearing a body built of its native element.

# --- GLACIEM (Cryotek's line): the ice-clones' vault-world. A civilization that lives
#     SLOW -- grown in lattice pools over centuries, bodies of layered frost that never
#     rust. They carve their cities DOWN into the glacier and preserve the first war's
#     archives in clear ice; nothing is ever thrown away, nothing ever hurried. ---
b = Builder(); b.floor("glaciem_vault.png"); b.walls("glaciem_vault.png", h=16)
random.seed(331)
for tier in range(3):                     # the carved vault-ziggurat, descending ring by ring
    rr = 16 + tier*13; hh = 3.2 - tier*0.8
    b.ring("glaciem_vault.png", 0, 0, rr, 8+tier*2, hh, 1.6)
b.spire("riftcrystal.png", 0, 0, 18, 2.2)                    # the frozen throne-spire
for k in range(5):                        # lattice pools: where the clones grow, slow
    a=k/5*6.283+0.3; r=48
    px,pz=r*math.cos(a),r*math.sin(a)
    b.ring("glaciem_vault.png", px, pz, 6, 5, 1.4, 0.8)
    b.box("riftcrystal.png", px, 0.9, pz, 1.6, 0.9, 1.6, collide=False)   # a clone half-formed in the ice
for k in range(6):                        # aurora crystal groves
    a=random.uniform(0,6.283); r=random.uniform(30,70)
    b.spire("riftcrystal.png", r*math.cos(a), r*math.sin(a), random.uniform(6,12), 0.9, collide=False)
b.skyway("ice.png", -60, -20, 60, -20, 6.0)                  # a glacier-carved high road
b.ice_slab("ice.png", -46, 44, 12, 12, 4.0, ramp_dir=1)      # archive slabs (flat, drivable)
b.ice_slab("ice.png", 48, 40, 11, 11, 3.2, ramp_dir=-1)
# ICE MONOLITHS: sheared vault-towers at the far reach -- the civilization's tall bones
for gm2 in ((-64,-52,12.0),(60,-60,14.0),(-58,56,11.0)):
    b.box("glaciem_vault.png", gm2[0], gm2[2]*0.5, gm2[1], 2.6, gm2[2]*0.5, 2.4)
    b.box("riftcrystal.png", gm2[0], gm2[2]+0.4, gm2[1], 1.4, 0.4, 1.3, collide=False)   # aurora crown
for lc2 in range(5):                                       # calved ice chunks: true shoot-over cover
    b.box("ice.png", random.uniform(-58,58), 0.6, random.uniform(-58,58), random.uniform(2,3.2), 0.6, random.uniform(1.6,2.6))
for lc3 in range(10):                                       # MORE calved ice chunks, reaching further out
    b.box("ice.png", random.uniform(-80,80), 0.6, random.uniform(-80,80), random.uniform(1.6,2.6), 0.6, random.uniform(1.4,2.2))
b.box("glaciem_vault.png", -82, 9.0, -6, 2.6, 9.0, 2.4)      # an OUTER vault-tower, reaching toward the walls
b.trim("ice.png", -82, 18.0, -6, 2.6, 2.4, emit="cryoglass.png")   # dressed: ice parapet + cold light
b.spire("riftcrystal.png", 82, 8, 22.0, 2.3)                 # a tall AURORA SPIRE at the far edge
b.natural_terrain("ice.png", 331, n=7, max_h=1.6, style="pluto")
b.write(os.path.join(DATA,"glaciem.iqm"), {"floor":"glaciem_vault.png","wall":"glaciem_vault.png","rock":"riftcrystal.png"})

# --- CARRION (Blackstar's line): the scavenger-clones' drift-world -- a planet that IS a
#     wreck-pile. The clans live FAST and loud in stacked hull-cities; every body is a
#     patchwork of salvage, and status is the rarest wreck worn openly. Nothing is made
#     new here; everything is taken, traded, or torn off the dead. ---
b = Builder(); b.floor("carrion_hull.png"); b.walls("carrion_hull.png", h=15)
random.seed(337)
mats=["carrion_hull.png","brushmetal.png","hazard.png"]
for c in range(5):                        # hull-stack cities: leaning towers of mismatched wreck
    a=c/5*6.283+0.6; r=44
    cx,cz=r*math.cos(a),r*math.sin(a); yy=0.0
    for lvl in range(random.randint(3,4)):
        w=random.uniform(2.6,4.6); hh=random.uniform(1.6,3.0)
        b.box(mats[(c+lvl)%3], cx+random.uniform(-1.2,1.2), yy+hh, cz+random.uniform(-1.2,1.2), w, hh, w*random.uniform(0.7,1.2))
        yy += hh*2
    b.spire("hazard.png", cx, cz, yy+4, 0.8, collide=False)   # the clan mast + banner light
b.skyway("rust.png", -50, 30, 50, -30, 8.0)                   # a gantry high-road between stacks
for k in range(4):                        # shipbreak trenches (rows of torn hull plate)
    a=k/4*6.283; r=64
    b.box("brushmetal.png", r*math.cos(a), 1.6, r*math.sin(a), 6.0, 1.6, 1.4)
b.box("molten.png", 0, 0.3, 0, 4, 0.3, 4, collide=False)      # the trade-pyre at the meet
b.ring("rust.png", 0, 0, 12, 6, 1.8, 1.0)                     # the meet-ring where clans barter
# THE ASHES' THRONE: blackstar's court seat -- a dais of stacked salvage, a tall
# angular seat-back of torn dreadnought plate, twin harpoon standards, and salvaged
# banner plates. every plate torn from a rival's hull.
b.box("brushmetal.png", 0, 0.9, -46, 8.0, 0.9, 6.0)           # dais tier one
b.box("rust.png",       0, 2.2, -47.5, 5.5, 0.5, 4.0)         # dais tier two
b.box("brushmetal.png", 0, 5.2, -50.5, 4.2, 2.6, 0.8)         # the seat-back: torn hull, canted
b.box("rust.png",       0, 3.2, -49.0, 2.2, 0.7, 1.6)         # the seat itself
b.spire("obsidian.png", -4.6, -50.0, 7, 0.5, collide=False)   # harpoon standard, port
b.spire("obsidian.png",  4.6, -50.0, 7, 0.5, collide=False)   # harpoon standard, starboard
b.box("energon.png", -4.6, 4.6, -49.6, 0.9, 1.3, 0.08, collide=False)  # banner: cold light
b.box("energon.png",  4.6, 4.6, -49.6, 0.9, 1.3, 0.08, collide=False)
b.natural_terrain("carrion_hull.png", 337, n=8, max_h=2.2)
b.write(os.path.join(DATA,"carrion.iqm"), {"floor":"carrion_hull.png","wall":"carrion_hull.png","rock":"brushmetal.png"})

# --- HELIX (Jhiaxus's line): the architect's garden-world. The corruption-clones are
#     RE-ENGINEERED between lives -- bodies grown seamless from organic-metal, "perfected"
#     past identity. Everything here is symmetric, spiral, and quiet: gene-vats in ordered
#     rows, helix towers turning as they rise, groves that are half plant, half machine. ---
b = Builder(); b.floor("helix_bio.png"); b.walls("helix_bio.png", h=16)
random.seed(347)
for t in range(2):                        # the twin helix towers: stacked boxes spiralling up
    sx = -30 if t==0 else 30
    for lvl in range(7):
        a = lvl*0.75 + t*3.14; rr = 4.5
        b.box("plate_blue.png", sx+rr*math.cos(a), 1.2+lvl*2.2, rr*math.sin(a), 2.4, 1.1, 2.4)
for k in range(6):                        # the gene-vat rows, perfectly ordered
    vx = -50 + k*20
    b.ring("helix_bio.png", vx, 52, 4, 5, 1.6, 0.7)
    b.box("riftcrystal.png", vx, 1.1, 52, 1.2, 1.1, 1.2, collide=False)   # a body mid-growth
    b.ring("helix_bio.png", vx, -52, 4, 5, 1.6, 0.7)
    b.box("riftcrystal.png", vx, 1.1, -52, 1.2, 1.1, 1.2, collide=False)
for k in range(4):                        # mirrored obelisk avenue (the symmetric approach)
    b.box("plate_blue.png", -66+k*14, 3.5, 0, 1.3, 3.5, 1.3)
    b.box("plate_blue.png",  66-k*14, 3.5, 0, 1.3, 3.5, 1.3)
b.dome("helix_bio.png", 0, 30, 9, 5, collide=False)            # organic-metal growth domes (decorative)
b.dome("helix_bio.png", 0, -30, 9, 5, collide=False)
b.box("plate_blue.png", 0, 2.2, 0, 7, 2.2, 7)                 # the architect's dais
b.natural_terrain("helix_bio.png", 347, n=6, max_h=1.8)
b.write(os.path.join(DATA,"helix.iqm"), {"floor":"helix_bio.png","wall":"glyph.png","rock":"plate_blue.png"})

# --- ENCELADUS (the geyser moon): blinding-bright ice over a buried ocean. Four TIGER
# STRIPES -- glowing teal fissures -- run the field; geyser mounds vent at fixed points
# (the game erupts them on a cycle and LAUNCHES anyone standing in the plume); a survey
# outpost hugs the safe ground. Saturn's frontier, Blackstar's hunting line.
b = Builder(); b.floor("enceladus_ice.png"); b.walls("enceladus_ice.png", h=14)
b.natural_terrain("enceladus_ice.png", 353, max_h=2.2)
random.seed(353)
for st in range(4):                                        # the TIGER STRIPES
    sz0 = -62 + st*17
    b.box("riftcrystal.png", 0, 0.18, sz0, 66, 0.18, 1.1, collide=False)   # glowing fissure
    # flanking WALLS of raised ice the fissure is cut into -- varied height so
    # the stripes read as canyons in a shelf, not lines on a flat floor.
    rh = 2.6 + st*0.5
    b.box("enceladus_ice.png", -20+st*11, rh, sz0-2.6, 14, rh, 1.4)
    b.box("enceladus_ice.png",  22-st*9,  rh*0.8, sz0+2.6, 12, rh*0.8, 1.4)
# GEYSER MOUNDS: one per sulcus, ON the fissure lines (z=-62/-45/-28/-11) -- on the
# real moon the plumes erupt FROM the tiger stripes (Damascus, Baghdad, Cairo,
# Alexandria sulci), so ours do too.
for gx, gz in ((-8,-62),(5,-45),(-30,-28),(25,-11)):
    # a TOWERING ICE PLUME: an eruption frozen mid-blast -- stacked shrinking
    # ice blocks rising ~8u, genuine climbable high ground over the flat field.
    b.box("enceladus_ice.png", gx, 2.0, gz, 5.5, 2.0, 5.5)                 # broad frozen base
    b.box("enceladus_ice.png", gx, 5.0, gz, 3.4, 1.2, 3.4)                 # mid shelf (a step up)
    b.box("enceladus_ice.png", gx, 7.2, gz, 1.9, 1.2, 1.9)                 # upper throat
    b.spire("riftcrystal.png", gx, gz, 9.0, 0.7, collide=False)           # the vent glow, riding high
for k in range(7):                                         # shard clusters for cover
    x=random.uniform(-66,66); z=random.uniform(-66,66)
    if abs(x)<14 and abs(z)<14: continue
    # dramatic height variety: towering ice spikes down to low shards, so the
    # field has real vertical range rather than uniform bumps.
    b.spire("enceladus_ice.png", x, z, random.uniform(3,16), 1.6)
# TOWERING ICE COLUMNS: round frozen spires as landmarks -- genuine tall high
# ground that stretches the map's vertical range and reads across the field.
b.cyl_tower("enceladus_ice.png", -40, -40, 16.0, 3.4, sides=8, taper=0.6)
b.cyl_tower("enceladus_ice.png",  38,  34, 14.0, 3.0, sides=8, taper=0.6)
b.cyl_tower("enceladus_ice.png", -52,  20, 17.0, 3.6, sides=8, taper=0.55)
b.tunnel("enceladus_ice.png", -46, 12, 10, 8, 14, axis="z")   # ice tube over the stripe (walk through)
b.tunnel("enceladus_ice.png",  46,-12, 10, 8, 14, axis="z")
b.box("plate_blue.png", 52, 2.6, 52, 7.0, 2.6, 7.0)        # survey outpost block
b.box("plate_blue.png", 52, 6.4, 52, 1.4, 1.2, 1.4)        # its relay mast
b.ring("plate_blue.png", 52, 52, 12, 8, 1.6, 1.0)          # perimeter ring
# SC2 READABILITY PASS: raised survey tier-pads with glowing rims so the height
# change reads at a glance, and a marker trail leading to the outpost landmark.
# The SC2 lesson is distinct cliff LEVELS the player moves between -- so these are
# genuine high-ground (6.5/4.5u over the ~1.2u ice field), not cosmetic bumps.
b.tier_pad("plate_blue.png", 20, 40, 6.5, 6.5, 6.5, rim="riftcrystal.png", ground="plate_blue.png")
b.tier_pad("plate_blue.png", -36, 30, 5.5, 5.5, 4.5, rim="riftcrystal.png", ground="plate_blue.png")
b.eye_trail("riftcrystal.png", 0, 0, 45, 45, n=8)          # lead the eye to the outpost
b.skyway("plate_blue.png", -58, -58, 40, 46, 6.5)          # the survey skyway (drivable)
b.write(os.path.join(DATA,"enceladus.iqm"), {"floor":"enceladus_ice.png","wall":"enceladus_ice.png","rock":"plate_blue.png"})

# --- THE HOLLOW (cavern arena): a sunken cave system UNDER a broken surface crust.
#     The fight happens below: a rolling cave floor, a slab CEILING pierced by three
#     light shafts, fat support columns, hanging stalactites. Enclosed = it echoes.
b = Builder(); b.floor("rock.png")
b.natural_terrain("rock.png", 401, n=8, spread=80, max_h=2.0, plateau=False, roads=False)
b.walls("rock.png", h=13)
random.seed(401)
# the CEILING: overlapping slabs at y~11, with three shaft GAPS left open for light
for cx0 in range(-3, 4):
    for cz0 in range(-3, 4):
        px, pz = cx0*24, cz0*24
        if (cx0, cz0) in ((-2,1),(2,-2),(0,2)):   # the light shafts stay open
            continue
        b.box("sediment.png", px, 11.4, pz, 13.4, 0.9, 13.4, collide=False)
# shaft RIMS: broken crust edges around each opening, catching the shaft light
for sx0, sz0 in ((-48,24),(48,-48),(0,48)):
    b.box("sediment.png", sx0-11, 10.6, sz0, 2.2, 1.6, 12, collide=False)
    b.box("sediment.png", sx0+11, 10.6, sz0, 2.2, 1.6, 12, collide=False)
# support COLUMNS: fat rock pillars floor-to-ceiling (real cover)
for px0, pz0 in ((-40,-30),(30,40),(-18,34),(44,6),(-52,-2),(12,-44)):
    b.spire("rock.png", px0, pz0, 12, 2.6)
# STALACTITES: hanging teeth (visual only; undersides for the cling system)
for k in range(14):
    x=random.uniform(-62,62); z=random.uniform(-62,62)
    b.box("sediment.png", x, 9.2, z, 0.55, random.uniform(1.2,2.6), 0.55, collide=False)
# a collapsed-rubble ramp under each shaft (the way the light gets used)
for sx0, sz0 in ((-48,24),(48,-48),(0,48)):
    b.mound("rock.png", sx0, sz0, 6.0, 2.2)
b.write(os.path.join(DATA,"hollow.iqm"), {"floor":"rock.png","wall":"sediment.png","rock":"rock.png"})

# --- THE ROOTS (Jhiaxus under-garden): the horror UNDER the garden of helix.
#     Bio-metal cave: fleshy floor, a grown ceiling, glowing spore pods, thorn
#     columns -- and the walls feel like they are digesting. His crawlers OWN this.
b = Builder(); b.floor("helix_bio.png")
b.natural_terrain("helix_bio.png", 347, n=7, spread=78, max_h=1.9, plateau=False, roads=False)
b.walls("helix_bio.png", h=12)
random.seed(409)
# grown ceiling membrane with two torn light rents
for cx0 in range(-3, 4):
    for cz0 in range(-3, 4):
        if (cx0, cz0) in ((-1,-2),(2,2)): continue
        b.box("helix_bio.png", cx0*24, 10.6, cz0*24, 13.2, 0.8, 13.2, collide=False)
# THORN COLUMNS: paired canted spikes that meet -- jhiaxus's ribcage architecture
for px0, pz0 in ((-36,-24),(28,36),(-14,30),(40,0),(-48,8),(8,-40),(52,-30)):
    b.spire("glyph.png", px0, pz0, 11, 1.9)
    b.spire("helix_bio.png", px0+3.0, pz0+2.0, 7, 1.0, collide=False)
# SPORE PODS: glowing growths clustered low (the only light between the rents)
for k in range(16):
    x=random.uniform(-60,60); z=random.uniform(-60,60)
    b.mound("riftcrystal.png", x, z, random.uniform(1.4,2.6), random.uniform(0.7,1.3))
# digestion pools: sunken glowing basins (visual dread, no collide)
for gx, gz in ((-24,12),(18,-18),(38,22)):
    b.box("riftcrystal.png", gx, 0.12, gz, 5.5, 0.12, 4.0, collide=False)
# BIO SHELVES: fungal platform growths at height, ringing the outer walls -- vertical
# routes for the fight (and perches the crawlers will love)
for sx0, sz0, sh0 in ((-58,-40,3.6),(38,-55,4.4),(-24,52,5.0),(62,8,3.2),(6,-30,4.8)):
    b.box("helix_bio.png", sx0, sh0, sz0, random.uniform(4.5,6.5), 0.7, random.uniform(3.5,5))
    b.spire("glyph.png", sx0+1.5, sz0+1.0, sh0+3, 0.7, collide=False)   # its feeder stalk
for sc0 in range(2):                                       # spore chimneys: tall outer growth
    b.spire("helix_bio.png", -62+sc0*124, 58-sc0*116, 14, 1.8)
for tc0 in ((-44,-8),(-40,-14),(-47,-16)):                 # a tight thorn CLUSTER (one bad grove)
    b.spire("glyph.png", tc0[0], tc0[1], 8, 1.2)
b.spire("helix_bio.png", 58, -20, 12, 1.8)                 # and one tall LONER far from anything
# grown PILLAR-TRUNKS: towering round organic columns (the hive's great stalks)
b.cyl_tower("helix_bio.png", -30, -58, 15.0, 3.0, sides=7, taper=0.65)
b.cyl_tower("helix_bio.png",  48,  44, 16.0, 3.2, sides=7, taper=0.65)
for lp2 in range(4):                                       # low pod clusters (shoot-over)
    b.box("glyph.png", random.uniform(-55,55), 0.6, random.uniform(-55,55), random.uniform(1.6,2.4), 0.6, random.uniform(1.4,2))
for ot0, oz0 in ((-81,-14),(80,20)):                        # OUTER thorn spires, reaching toward the walls
    b.spire("glyph.png", ot0, oz0, 11.5, 2.0)
    b.box("glyph.png", ot0, 0.5, oz0+6, 1.6, 0.5, 1.6)       # a low root-knot at its base -- widens the height spread
b.write(os.path.join(DATA,"roots.iqm"), {"floor":"helix_bio.png","wall":"helix_bio.png","rock":"glyph.png"})

# --- THE DREADNOUGHT (Blackstar's flagship): fighting ON the segmented warship --
#     a hull deck under open void. From the profile: "salvaged Cybertronian
#     dreadnought armor... solar-sail structures like metallic wings... plasma
#     harpoon cannons... smaller Decepticons dock INTO him." Boarding-battle turf:
#     hull ribs arch overhead, corsair docking cradles line the rails, cargo is
#     lashed in uneven clusters, sail-masts and the bridge tower own the sky. ---
b = Builder(); b.floor("brushmetal.png"); b.walls("obsidian.png", h=10)
random.seed(501)
for rb in range(5):                                        # HULL RIBS: the ship's bones arch over the deck
    rz2 = -68 + rb*34
    b.arch("obsidian.png", 0, rz2, 78, 9 + (rb%2)*2, 2.4)
for cg in range(9):                                        # LASHED CARGO: uneven clusters (spread)
    cx2 = random.uniform(-72, 72); cz2 = random.uniform(-66, 66)
    n2 = random.choice([1,1,2,3])
    for c2 in range(n2):
        hb2 = random.choice([0.6, 0.6, 1.7])               # mostly low lashings, some tall crates
        b.box("hazard.png", cx2+c2*3.2, 0.5+hb2, cz2+random.uniform(-1,1), 2.6, hb2, 2.2)
for dk in range(4):                                        # CORSAIR CRADLES: docking alcoves on the rails
    sd2 = 1 if dk%2 else -1
    dz2 = -52 + (dk//2)*104
    b.box("rust.png", sd2*80, 2.0, dz2, 4.5, 2.0, 7.0)
    b.box("energon.png", sd2*80, 4.4, dz2, 3.2, 0.25, 5.5, collide=False)   # cradle guide-lights
for sm in range(3):                                        # SOLAR-SAIL MASTS: metallic wings raked aft
    mx2 = -52 + sm*52
    b.spire("obsidian.png", mx2, 78, 13 + sm*2, 1.1)
    b.box("carbon.png", mx2+4.0, 9.0+sm, 75.5, 6.5, 4.5, 0.15, collide=False)   # the sail membrane
b.box("obsidian.png", 0, 5.5, -84, 8.0, 5.5, 5.0)          # THE BRIDGE TOWER (blackstar watches)
b.box("energon.png", 0, 10.2, -82.4, 5.5, 0.9, 0.2, collide=False)   # its viewport burns cold
b.spire("obsidian.png", -30, -80, 8, 0.9); b.spire("obsidian.png", 30, -80, 8, 0.9)   # harpoon batteries
for lw2 in range(16):                                      # DECK CLUTTER: low shoot-over cover across
    # the deck -- ammo crates, hatches, tie-down blocks, conduit runs. A warship
    # deck is covered in this gear; it also fixes the cover balance (was 86% high).
    lx = random.uniform(-72,72); lz = random.uniform(-70,64)
    if abs(lx)<6 and abs(lz)<6: continue                   # keep the very center clear
    lh = random.choice([0.5, 0.6, 0.6, 0.9])               # all LOW (shoot over)
    b.box("brushmetal.png", lx, lh, lz, random.uniform(1.6,3.0), lh, random.uniform(1.4,2.4))
b.write(os.path.join(DATA,"dreadnought.iqm"), {"floor":"brushmetal.png","wall":"obsidian.png","rock":"rust.png"})

# --- THE DRIFTFIELD (dense belt swarm, MICRO-GRAVITY): fighting IN an asteroid
#     stream -- rocks hang at every height, a shattered ore-hauler drifts at the
#     center, and nothing pulls you down harder than a whisper. Jets are king. ---
b = Builder(); b.floor("stars.png"); b.walls("asteroid.png", h=16)
random.seed(601)
# the SWARM travels in CLUMPS: three tight clusters, a loose scatter, and loners --
# real belt objects gravitate into families (and the spread metric agrees)
for ctr in ((-52,-40),(44,30),(-18,58)):
    for ck in range(4):
        dx3 = ctr[0]+random.uniform(-9,9); dz3 = ctr[1]+random.uniform(-9,9)
        dh3 = random.uniform(1.0, 12.0); dw3 = random.uniform(2.0, 5.0)
        b.box("asteroid.png", dx3, dh3, dz3, dw3, dw3*0.7, dw3*0.9)
for ln in ((70,-62,9.0),(-72,18,4.0),(20,-70,11.0),(64,66,6.5)):   # LONERS far out
    b.box("asteroid.png", ln[0], ln[2], ln[1], random.uniform(3,5.5), random.uniform(2,3.5), random.uniform(2.5,4.5))
for lr in range(6):                                        # LOW drift-boulders (shoot-over)
    b.box("asteroid.png", random.uniform(-70,70), 0.7, random.uniform(-70,70), random.uniform(2,3.4), 0.7, random.uniform(1.8,3))
# THE SHATTERED HAULER: an ore ship broken in three, drifting nose-down
b.box("brushmetal.png", -6, 6.5, 4, 5.0, 3.2, 9.0)         # midship, canted high
b.box("rust.png", 8, 3.0, 16, 4.0, 2.6, 6.0)               # stern section
b.box("brushmetal.png", -16, 9.5, -10, 3.4, 2.2, 5.0)      # bow, tumbled highest
b.box("energon.png", -6, 10.1, 4, 3.4, 0.2, 6.5, collide=False)   # spilled cargo: raw energon glow
b.spire("rust.png", 8, 16, 7, 0.8)                          # a snapped mast
for gl in range(5):                                         # loose glittering ore
    b.box("energon.png", random.uniform(-60,60), random.uniform(1,8), random.uniform(-60,60), 0.5, 0.5, 0.5, collide=False)
for ln2 in ((82,-40,7.0),(-80,64,8.5)):                     # two more distant LONERS, out near the belt's edge
    b.box("asteroid.png", ln2[0], ln2[2], ln2[1], random.uniform(3,4.5), random.uniform(2,3), random.uniform(2.5,3.8))
for lr2 in range(10):                                        # MORE low drift-boulders: driftfield was
    b.box("asteroid.png", random.uniform(-76,76), 0.65, random.uniform(-76,76), random.uniform(1.6,2.6), 0.65, random.uniform(1.5,2.2))  # 6 low vs 19 high
b.write(os.path.join(DATA,"driftfield.iqm"), {"floor":"stars.png","wall":"asteroid.png","rock":"asteroid.png"})

# --- THE DERELICT (abandoned colony ship, MICRO-GRAVITY): a generation ship that
#     never arrived. Keel and ribs open to the stars, cryo racks still glowing
#     faint, the reactor sphere dark aft. Fights drift down her length. ---
b = Builder(); b.floor("stars.png"); b.walls("obsidian.png", h=14)
random.seed(607)
b.box("brushmetal.png", 0, 1.2, 0, 3.2, 1.2, 84)           # the KEEL: her spine, stem to stern
for rb2 in range(6):                                       # hull RIBS, some broken
    rz3 = -70 + rb2*28
    if rb2 == 2 or rb2 == 4:                               # the breach: two ribs snapped
        b.box("obsidian.png", -34, 5.0, rz3, 2.0, 5.0, 2.2)
        b.box("obsidian.png", 30, 8.0, rz3, 2.0, 3.0, 2.2)
    else:
        b.arch("obsidian.png", 0, rz3, 62, 11, 2.4)
b.box("plate_blue.png", 0, 4.0, -76, 7.0, 4.0, 7.0)        # the REACTOR DRUM, dark aft
b.mound("plate_blue.png", 0, -76, 8.5, 5.0)                # its sphere-cap crown
b.box("energon.png", 0, 3.0, -76, 2.0, 2.0, 2.0, collide=False)   # its faint heartbeat
b.box("brushmetal.png", 0, 3.6, 74, 10.0, 3.6, 6.0)        # the BRIDGE, forward
b.box("energon.png", 0, 7.6, 71.5, 7.0, 0.7, 0.2, collide=False)  # dead viewport, still lit
for cy2 in range(8):                                       # CRYO RACKS: rows amidships, faint glow
    cx4 = -24 + (cy2%4)*16; cz4 = -18 + (cy2//4)*30
    b.box("plate_blue.png", cx4, 1.6, cz4, 2.6, 1.6, 1.2)
    b.box("cryoglass.png", cx4, 3.5, cz4, 1.8, 0.25, 0.9, collide=False)   # frosted pod glass, still lit
for hp2 in range(6):                                       # torn hull plates adrift at height
    b.box("brushmetal.png", random.uniform(-55,55), random.uniform(4,11), random.uniform(-60,60),
          random.uniform(2.5,4.5), 0.4, random.uniform(2,3.5))
for lw3 in range(5):                                       # deck debris (shoot-over)
    b.box("rust.png", random.uniform(-50,50), 0.6, random.uniform(-60,60), random.uniform(1.8,3), 0.6, random.uniform(1.5,2.4))
for lw4 in range(12):                                       # MORE deck debris: derelict was almost all
    b.box("rust.png", random.uniform(-58,58), 0.55, random.uniform(-80,80), random.uniform(1.4,2.2), 0.55, random.uniform(1.4,2.0))  # ribs/keel (5 low vs 34 high)
b.write(os.path.join(DATA,"derelict.iqm"), {"floor":"stars.png","wall":"obsidian.png","rock":"rust.png"})

# --- CITYFALL (Earth, downtown): the mission-city fight the films live in --
#     towers with lit windows, an elevated freeway, plazas, stalled traffic,
#     and alleys that reward the flank. Aerial assassination country. ---
b = Builder(); b.floor("plate_blue.png"); b.walls("brushmetal.png", h=16)
random.seed(701)
# tower BLOCKS in irregular clusters (a real skyline is families + loners)
for tc in ((-52,-44),(38,-52),(-46,40),(52,46)):
    for tk in range(3):
        tx3 = tc[0]+random.uniform(-10,10); tz3 = tc[1]+random.uniform(-10,10)
        b.city_tower("plate_blue.png", tx3, tz3, random.uniform(9,15), random.uniform(2.2,3.4))
b.city_tower("plate_blue.png", 66, -8, 17, 3.6)            # the tallest, standing alone
b.city_tower("plate_blue.png", -68, -10, 12, 2.8)
b.skyway("brushmetal.png", -60, 12, 58, -18, 7.5)          # the ELEVATED FREEWAY, cracked across downtown
for cv in range(9):                                        # stalled traffic: cars as low cover
    b.box("brushmetal.png", random.uniform(-55,55), 0.55, random.uniform(-55,55), 1.4, 0.55, 0.7)
for pl in range(4):                                        # plaza planters + kiosks
    b.box("rust.png", random.uniform(-40,40), 0.8, random.uniform(-40,40), random.uniform(1.6,2.6), 0.8, random.uniform(1.2,2))
b.box("hazard.png", 6, 1.4, 8, 5.0, 1.4, 2.2)              # an overturned hauler at the meet
b.box("energon.png", 6, 3.0, 8, 3.2, 0.2, 1.4, collide=False)   # its cargo, cracked open + glowing
b.natural_terrain("plate_blue.png", 701, n=5, max_h=0.9)   # buckled street
b.write(os.path.join(DATA,"cityfall.iqm"), {"floor":"plate_blue.png","wall":"brushmetal.png","rock":"rust.png"})

# --- LUNA (Earth's moon): regolith, hard shadows, and THE ARK -- the ship that
#     started everything, half-buried in the far crater wall since the crash. ---
b = Builder(); b.floor("moon_dust.png" if False else "sand.png"); b.walls("asteroid.png", h=13)
random.seed(707)
for cr2 in ((-34,22,11),(28,-30,14),(48,42,8),(-52,-44,9)):      # crater RIMS
    b.ring("asteroid.png", cr2[0], cr2[1], cr2[2], 8, 3.2, 1.6)  # raised walls -- craters read as pits
for pn2 in ((-64,-18,15.0),(62,58,16.0)):                         # sheared crater-wall PINNACLES (towering)
    b.cyl_tower("asteroid.png", pn2[0], pn2[1], pn2[2], 3.0, sides=7, taper=0.5)
# THE ARK: a vast hull half-buried nose-down in the far wall
b.box("brushmetal.png", -6, 7.0, -70, 16.0, 7.0, 7.0)            # buried bow mass, canted
b.box("obsidian.png", 10, 4.0, -58, 7.0, 4.0, 5.0)               # torn midship
b.box("brushmetal.png", -20, 3.0, -60, 5.0, 3.0, 4.5)            # engine block, sheared
b.box("energon.png", -6, 12.6, -68.5, 9.0, 0.5, 0.4, collide=False)  # the dark viewport line
b.spire("obsidian.png", 22, -64, 9, 1.0)                          # a snapped antenna mast
for db2 in range(6):                                              # crash debris field, fanned south
    b.box("rust.png", -6+random.uniform(-30,30), 0.7, -40+random.uniform(-14,20), random.uniform(1.6,3), 0.7, random.uniform(1.4,2.4))
for bl2 in ((-60,8,4.5),(56,-14,3.5),(14,58,5.5)):                # ejecta BOULDERS, far-flung
    b.box("asteroid.png", bl2[0], bl2[2]*0.55, bl2[1], bl2[2]*0.7, bl2[2]*0.55, bl2[2]*0.6)
for lr3 in range(4):                                               # scattered low regolith rocks
    b.box("asteroid.png", random.uniform(-55,55), 0.55, random.uniform(-30,55), random.uniform(1.6,2.6), 0.55, random.uniform(1.4,2.2))
b.box("plate_blue.png", 44, 1.2, 20, 2.2, 1.2, 2.2)               # the survey lander, abandoned
b.box("hazard.png", 44, 2.7, 20, 0.4, 0.3, 0.4)
b.natural_terrain("sand.png", 707, n=8, max_h=2.0)
b.write(os.path.join(DATA,"luna.iqm"), {"floor":"sand.png","wall":"asteroid.png","rock":"asteroid.png"})

# --- CHICAGO (Dark-of-the-Moon aftermath): the city that survived. The river cuts
#     the map, crossed by two bascule bridges (one leaf jammed OPEN since the
#     battle); the skyline stands half-proud, one tower down; and everywhere the
#     RECONSTRUCTION: cranes, scaffolding, staged girders. This is where the
#     blackstar record begins -- the first page of the reference material. ---
b = Builder(); b.floor("plate_blue.png"); b.walls("brushmetal.png", h=16)
random.seed(711)
# THE RIVER: a sunken channel crossing the map east-west at z=0 (water region in-game)
b.box("obsidian.png", 0, 0.55, 0, 78, 0.04, 9.0, collide=False)     # dark water plane (visual, aligned to WATER_REGIONS surf=0.55)
b.box("rust.png", 0, 0.8, -10.5, 78, 0.8, 1.2)                       # north embankment wall
b.box("rust.png", 0, 0.8,  10.5, 78, 0.8, 1.2)                       # south embankment wall
# TWO BASCULE BRIDGES: west span intact; east span's leaf JAMMED OPEN since the battle
b.skyway("brushmetal.png", -34, -13, -34, 13, 2.6, width=4.5)        # west bridge, level
b.box("brushmetal.png", 30, 2.6, -7.0, 2.2, 0.35, 5.5)               # east bridge north approach
b.box("brushmetal.png", 30, 5.2,  4.0, 2.2, 0.35, 5.5)               # the raised LEAF (canted up)
b.box("hazard.png", 30, 6.8, 7.5, 2.0, 0.3, 1.0, collide=False)      # its warning-striped tip
b.spire("rust.png", 26, -12, 5, 0.7); b.spire("rust.png", 34, -12, 5, 0.7)   # counterweight towers
# LAUNCH RAMP: closes the vertical gap between the approach deck (top y=2.95) and the
# raised leaf (top y=5.55) -- verified a straight vertical hop can't clear 2.6m for
# wheeled/tracked vehicles, so this is the real crossing; jumping it is still the dare.
for _rs in range(4):
    _rt = (_rs + 0.5) / 4.0
    b.box("brushmetal.png", 30, 2.95 + (5.55 - 2.95) * _rt, -4.5 + 3.0 * _rt, 2.2, 0.18, 0.62)
# THE SKYLINE: north bank families + the tallest standing sentinel
for tc2 in ((-56,-46),(-14,-52),(40,-50)):
    for tk2 in range(3):
        b.city_tower("plate_blue.png", tc2[0]+random.uniform(-9,9), tc2[1]+random.uniform(-8,8),
                     random.uniform(9,15), random.uniform(2.2,3.2))
b.city_tower("plate_blue.png", 62, -34, 18, 3.8)                     # the sentinel tower (tallest)
# THE FALLEN TOWER: one that did not survive -- a leaning slab stack, south bank
b.box("brushmetal.png", -48, 3.0, 34, 4.5, 3.0, 4.0)
b.box("brushmetal.png", -44, 6.5, 38, 4.0, 2.2, 3.6)                 # upper mass, slid off-line
b.box("rust.png", -38, 1.0, 44, 5.0, 1.0, 3.0)                       # its crown, in the street
for rb3 in range(5):                                                 # rubble fan
    b.box("rust.png", -44+random.uniform(-12,16), 0.6, 30+random.uniform(-4,18), random.uniform(1.6,2.8), 0.6, random.uniform(1.4,2.2))
# THE RECONSTRUCTION: cranes + scaffolding + staged girders (the aftermath at work)
for cn2 in ((16,42),(52,30)):
    b.spire("hazard.png", cn2[0], cn2[1], 13, 0.8)                   # crane mast
    b.box("hazard.png", cn2[0]+4.5, 12.4, cn2[1], 5.5, 0.25, 0.5)    # the jib
    b.box("brushmetal.png", cn2[0]+8.5, 9.0, cn2[1], 0.15, 3.2, 0.15, collide=False)  # hoist cable
for sc2 in ((-8,36),(4,48)):
    b.box("rust.png", sc2[0], 2.6, sc2[1], 2.8, 2.6, 0.35)           # scaffolding sheets
    b.box("hazard.png", sc2[0], 5.4, sc2[1], 3.2, 0.2, 0.5)          # top walkboard
for gd2 in range(4):                                                 # staged girder stacks
    b.box("brushmetal.png", 20+random.uniform(-8,8), 0.5, 20+random.uniform(-4,10), 3.5, 0.5, 0.9)
# THE MEMORIAL: where the record begins -- an obelisk with a cold energon flame
b.box("obsidian.png", -12, 2.4, 20, 1.2, 2.4, 1.2)
b.box("energon.png", -12, 5.2, 20, 0.5, 0.5, 0.5, collide=False)     # the flame that doesn't burn
# ---- ART PASS (ART_BIBLE 2.3): the transport-event SCAR -- pillar shards around the memorial ----
random.seed(1031)
for _ps in range(7):                                                  # the shard field: what the space-bridge left
    _pa=_ps*(math.pi*2/7)+random.uniform(-0.25,0.25); _pr=random.uniform(4.0,7.5)
    b.box("plate_blue.png", -12+math.cos(_pa)*_pr, random.uniform(0.8,1.6), 20+math.sin(_pa)*_pr,
          0.35, random.uniform(0.8,1.6), 0.35, collide=False)
b.box("plate_blue.png", -17.5, 2.6, 24.5, 0.4, 2.6, 0.4, collide=False)   # the tall one, the scar's flagpole
b.box("energon.png", -17.5, 5.4, 24.5, 0.22, 0.3, 0.22, collide=False)    # a faint glow still trapped in its tip
b.natural_terrain("plate_blue.png", 711, n=5, max_h=0.8)             # battle-buckled streets
# SC2 BEST-PRACTICES PASS (docs/SC2_MAPCRAFT.md): lead the eye to the crossings,
#  ground the reconstruction rubble in the open south margins.
b.eye_trail("energon.png", -34, 24, -34, 8, n=5, w=0.7)             # south approach -> west bridge
b.eye_trail("energon.png", 30, 24, 30, 9, n=5, w=0.7)               # south approach -> east (jammed) bridge
for cx6, cz6 in ((-62, 52), (62, 52)):                              # staged reconstruction rubble, grounded (far south)
    b.ground_cluster("plate_blue.png", "rust.png", cx6, cz6, r=6.0, count=4)
b.write(os.path.join(DATA,"chicago.iqm"), {"floor":"plate_blue.png","wall":"brushmetal.png","rock":"rust.png"})

# smooth ARMOUR-PLATE template for the skeleton pool (brushed metal -- no checker grid)
b = Builder(); b.box("brushmetal.png", 0, 0.5, 0, 0.5, 0.5, 0.5)
b.write(os.path.join(DATA, "plate.iqm"), {"floor":"brushmetal.png","wall":"brushmetal.png","rock":"brushmetal.png"})
for _ext in (".col", ".hgt"):
    _p = os.path.join(DATA, "plate" + _ext)
    if os.path.exists(_p): os.remove(_p)
print("world maps generated")
