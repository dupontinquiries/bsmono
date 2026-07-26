"""regions.py -- biome REGIONS: paint a world with several palettes at once.

A real open world isn't one biome -- it's frost giving way to ember giving way to verdant.
A RegionMap assigns one of several palettes to each point via a low-frequency Voronoi-ish
field seeded across the map, so a single World wears many biomes. Goal placement, materials,
and spawn rules then read the LOCAL region's palette (its archetype allow-mask included), so
a boss only spawns where its biome permits it. Fully agnostic -- the host game maps regions
onto its own art sets.
"""
from __future__ import annotations
import math, random


class RegionMap:
    def __init__(self, size, seeds):
        """seeds: list of (x, y, palette_name). Nearest seed wins (Voronoi cells)."""
        self.size = size
        self.seeds = seeds

    def region_at(self, x, y) -> str:
        best, bd = None, 1e18
        for sx, sy, name in self.seeds:
            d = (x - sx) ** 2 + (y - sy) ** 2
            if d < bd:
                bd, best = d, name
        return best or (self.seeds[0][2] if self.seeds else "default")

    def boundary_strength(self, x, y) -> float:
        """0 deep inside a region .. ~1 near a border (for blending materials/fog)."""
        d = sorted(((x - sx) ** 2 + (y - sy) ** 2) for sx, sy, _ in self.seeds)
        if len(d) < 2:
            return 0.0
        a, b = math.sqrt(d[0]), math.sqrt(d[1])
        return max(0.0, 1.0 - (b - a) / (self.size * 0.35))

    def names(self):
        return sorted({n for _, _, n in self.seeds})


def scatter_regions(size, palettes, n=4, seed=1):
    """Place n biome seeds across the world, drawn from `palettes` (a list of names)."""
    rng = random.Random(seed)
    seeds = []
    for i in range(max(1, n)):
        sx = rng.uniform(-size * 0.8, size * 0.8)
        sy = rng.uniform(-size * 0.8, size * 0.8)
        seeds.append((sx, sy, palettes[i % len(palettes)]))
    return RegionMap(size, seeds)
