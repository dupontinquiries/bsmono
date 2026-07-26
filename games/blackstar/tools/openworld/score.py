"""score.py -- optimization metric: a headless design scorecard for a World.

A faithful port of games/soldier's `mapeval` philosophy, generalised to the agnostic
World model. Sub-scores are 0..1; total is 0..100 with an S..F grade. Connectivity is a
HARD GATE (a world you can't traverse is near-worthless), exactly as in the original
`bestregion`-derived weighting.

    total = (openness*0.30 + flow*0.22 + cover*0.16 + verticality*0.16
             + scale*0.06 + (0.10 if landmark else 0)) * (1.0 if connected else 0.25) * 100
"""
from __future__ import annotations
import math
from dataclasses import dataclass


@dataclass
class Scorecard:
    openness: float
    flow: float
    cover: float
    verticality: float
    connected: bool
    has_landmark: bool
    scale: float
    total: float

    def grade(self) -> str:
        t = self.total
        return ("S" if t >= 90 else "A" if t >= 80 else "B" if t >= 70 else
                "C" if t >= 60 else "D" if t >= 45 else "F")

    def line(self) -> str:
        return (f"{self.total:5.1f} {self.grade()}  open {self.openness:.2f} "
                f"flow {self.flow:.2f} cover {self.cover:.2f} vert {self.verticality:.2f} "
                f"conn {int(self.connected)} land {int(self.has_landmark)}")


def _blocked_grid(world):
    """A grid marking cells blocked by solid obstacles (for connectivity + cover)."""
    g = world.grid
    blk = [[False] * g for _ in range(g)]
    cell_w = 2 * world.size / g
    for ob in world.obstacles:
        if not ob.solid:
            continue
        gx0 = int((ob.x - ob.hx + world.size) / (2 * world.size) * g)
        gx1 = int((ob.x + ob.hx + world.size) / (2 * world.size) * g)
        gy0 = int((ob.y - ob.hy + world.size) / (2 * world.size) * g)
        gy1 = int((ob.y + ob.hy + world.size) / (2 * world.size) * g)
        for j in range(max(0, gy0), min(g, gy1 + 1)):
            for i in range(max(0, gx0), min(g, gx1 + 1)):
                blk[j][i] = True
    return blk


def _connected(world, blk) -> bool:
    """Flood-fill: is the largest open component most of the walkable area?"""
    g = world.grid
    total_open = sum(1 for j in range(g) for i in range(g) if not blk[j][i])
    if total_open == 0:
        return False
    # start from the first open cell
    start = next((j * g + i for j in range(g) for i in range(g) if not blk[j][i]), None)
    seen = set([start]); stack = [start]
    while stack:
        c = stack.pop(); ci, cj = c % g, c // g
        for di, dj in ((1, 0), (-1, 0), (0, 1), (0, -1)):
            ni, nj = ci + di, cj + dj
            if 0 <= ni < g and 0 <= nj < g and not blk[nj][ni] and (nj * g + ni) not in seen:
                seen.add(nj * g + ni); stack.append(nj * g + ni)
    return len(seen) >= 0.85 * total_open


def score(world) -> Scorecard:
    g = world.grid
    blk = _blocked_grid(world)
    area_open = sum(1 for j in range(g) for i in range(g) if not blk[j][i])
    area = g * g

    # ── openness: how much of the world is open (sky/field) vs walled in ───────────
    open_frac = area_open / area
    if world.open_sky:
        openness = max(0.0, 1.0 - abs(open_frac - 0.82) / 0.82)
    else:
        openness = max(0.0, 1.0 - abs(open_frac - 0.62) / 0.62)

    # ── flow: sightline range mix (short/med/long). Cast rays from sample cells; a
    #    ray is broken by a TALL obstacle. Compare the range histogram to a healthy mix.
    tallset = [ob for ob in world.obstacles if ob.tall()]
    def ray_len(x, y, dx, dy):
        step, maxd = world.size * 0.06, world.size * 2
        d = 0.0
        while d < maxd:
            d += step; px, py = x + dx * d, y + dy * d
            if abs(px) > world.size or abs(py) > world.size:
                return d
            for ob in tallset:
                if abs(px - ob.x) < ob.hx and abs(py - ob.y) < ob.hy:
                    return d
        return maxd
    shorts = meds = longs = 0; total_rays = 0
    dirs = [(1, 0), (0, 1), (0.7, 0.7), (-0.7, 0.7)]
    for s in range(6):
        x = (s / 5 - 0.5) * 2 * world.size * 0.7
        for t in range(6):
            y = (t / 5 - 0.5) * 2 * world.size * 0.7
            for dx, dy in dirs:
                L = ray_len(x, y, dx, dy) / (world.size * 2)
                total_rays += 1
                if L < 0.18: shorts += 1
                elif L < 0.45: meds += 1
                else: longs += 1
    if total_rays:
        fs, fm, fl = shorts / total_rays, meds / total_rays, longs / total_rays
    else:
        fs = fm = fl = 0.33
    ideal = (0.33, 0.34, 0.33)
    flow = max(0.0, 1.0 - 0.5 * (abs(fs - ideal[0]) + abs(fm - ideal[1]) + abs(fl - ideal[2])))

    # ── cover: obstacle density sweet spot (~1 per 22 open tiles) ──────────────────
    ideal_d = 1.0 / 22.0
    density = len(world.obstacles) / max(1, area_open)
    cover = max(0.0, 1.0 - min(1.0, abs(density - ideal_d) / (ideal_d * 1.8)))

    # ── verticality: height spread of the substrate + platform bonus ───────────────
    hs = [world.height[j][i] for j in range(g) for i in range(g)]
    spread = (max(hs) - min(hs)) if hs else 0.0
    plat_bonus = min(0.2, 0.05 * sum(1 for ob in world.obstacles if ob.height > 3.0))
    verticality = min(1.0, spread * 0.8 + plat_bonus)

    # ── connectivity (gate) + landmark + scale ─────────────────────────────────────
    connected = _connected(world, blk)
    has_landmark = any(p.kind == "landmark" for p in world.pois)
    scale = min(1.0, (2 * world.size) ** 2 / 40000.0)

    weighted = (openness * 0.30 + flow * 0.22 + cover * 0.16 + verticality * 0.16
                + scale * 0.06 + (0.10 if has_landmark else 0.0))
    gate = 1.0 if connected else 0.25
    total = max(0.0, min(100.0, weighted * gate * 100.0))
    return Scorecard(openness, flow, cover, verticality, connected, has_landmark, scale, total)
