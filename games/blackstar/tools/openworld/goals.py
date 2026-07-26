"""goals.py -- goal setting: populate a world with POIs the net wants there.

Samples candidate sites (Poisson-ish spacing so goals don't clump), asks the ActivationNet
what archetype each site wants, and places the winners. Region-aware: when a RegionMap +
palette registry are supplied, each site is gated by the LOCAL biome's allow-mask and
tagged with its region, so a boss only grows where its biome permits. Assigns a difficulty
TIER from distance-to-spawn, and enforces constraints (bosses kept away from spawn, a
guaranteed landmark). Entirely agnostic: POIs are tagged points the host game maps onto its
own content.
"""
from __future__ import annotations
import math, random
from .world import POI
from .activation import ActivationNet


def place_goals(world, palette, n=14, seed=None, net: ActivationNet | None = None,
                min_spacing=None, region_map=None, registry=None, spawn=(0.0, 0.0),
                boss_min_dist=None, tiers=3):
    rng = random.Random(seed if seed is not None else world.seed * 7 + 1)
    net = net or ActivationNet()
    if min_spacing is None:
        min_spacing = world.size * 0.22
    if boss_min_dist is None:
        boss_min_dist = world.size * 0.45
    placed = []

    def far_enough(x, y):
        return all(math.hypot(x - p.x, y - p.y) >= min_spacing for p in placed)

    def local_palette(x, y):
        if region_map is not None and registry is not None:
            return registry.get(region_map.region_at(x, y))
        return palette

    # oversample candidates, keep the strongest (region-gated) activations first
    cands = []
    for _ in range(n * 14):
        x = rng.uniform(-world.size * 0.9, world.size * 0.9)
        y = rng.uniform(-world.size * 0.9, world.size * 0.9)
        pal = local_palette(x, y)
        arch, act = net.best(world, x, y, allow=set(pal.allow))
        if arch is None:
            continue
        cands.append((act * pal.weights.get(arch, 1.0), x, y, arch,
                      region_map.region_at(x, y) if region_map else palette.name))
    cands.sort(reverse=True, key=lambda c: c[0])

    for score_, x, y, arch, reg in cands:
        if len(placed) >= n:
            break
        if arch == "boss" and math.hypot(x - spawn[0], y - spawn[1]) < boss_min_dist:
            continue                      # bosses belong deeper in, not on your doorstep
        if far_enough(x, y):
            placed.append(POI(x, y, kind=arch, weight=round(score_, 3), region=reg))

    # guarantee a landmark for orientation (highest point if none placed)
    if not any(p.kind == "landmark" for p in placed):
        best, bh = None, -1
        for j in range(world.grid):
            for i in range(world.grid):
                if world.height[j][i] > bh:
                    bh = world.height[j][i]; best = (i, j)
        if best:
            lx, ly = world.to_world(*best)
            reg = region_map.region_at(lx, ly) if region_map else palette.name
            placed.append(POI(lx, ly, kind="landmark", weight=1.0, region=reg))

    assign_tiers(placed, spawn, world.size, tiers)
    world.pois = placed
    return placed


def assign_tiers(pois, spawn, size, tiers=3):
    """Difficulty tier 0..tiers-1 by distance from spawn (near = easy, far = hard)."""
    if not pois:
        return
    maxd = max(math.hypot(p.x - spawn[0], p.y - spawn[1]) for p in pois) or 1.0
    for p in pois:
        d = math.hypot(p.x - spawn[0], p.y - spawn[1]) / maxd
        p.tier = min(tiers - 1, int(d * tiers))
