"""analyze.py -- headless analysis of a built world: reachability, POI mix, tiers, roads.

Turns a World into a compact report a designer (or a CI gate) can read: is the whole map
one connected space or fragmented into islands? How are the POIs distributed across kinds,
tiers, and biome regions? How much road does it take to connect them? Agnostic -- pure
measurement over the World model.
"""
from __future__ import annotations
import math
from .score import score
from .path import components, build_roads


def analyze(world, spawn=(0.0, 0.0)):
    card = score(world)
    _, ncomp = components(world)
    pois = world.pois
    by_kind, by_tier, by_region = {}, {}, {}
    for p in pois:
        by_kind[p.kind] = by_kind.get(p.kind, 0) + 1
        by_tier[p.tier] = by_tier.get(p.tier, 0) + 1
        if p.region:
            by_region[p.region] = by_region.get(p.region, 0) + 1
    # mean nearest-neighbour spacing (how evenly spread the goals are)
    spac = []
    for i, a in enumerate(pois):
        nd = min((math.hypot(a.x - b.x, a.y - b.y) for j, b in enumerate(pois) if j != i),
                 default=0.0)
        spac.append(nd)
    mean_spacing = sum(spac) / len(spac) if spac else 0.0
    roads = build_roads(world)
    road_len = sum(
        sum(math.hypot(s[k + 1][0] - s[k][0], s[k + 1][1] - s[k][1]) for k in range(len(s) - 1))
        for s in roads)
    return {
        "score": round(card.total, 1), "grade": card.grade(),
        "connected": card.connected, "components": ncomp,
        "pois": len(pois), "by_kind": by_kind, "by_tier": by_tier, "by_region": by_region,
        "mean_spacing": round(mean_spacing, 1),
        "road_segments": len(roads), "road_length": round(road_len, 1),
    }


def report(world, spawn=(0.0, 0.0)) -> str:
    a = analyze(world, spawn)
    lines = [f"score {a['score']} {a['grade']}   connected={a['connected']} "
             f"({a['components']} region{'s' if a['components'] != 1 else ''})",
             f"POIs {a['pois']}  kinds={a['by_kind']}  tiers={a['by_tier']}",
             f"spacing~{a['mean_spacing']}  roads {a['road_segments']} seg / {a['road_length']} len"]
    if a["by_region"]:
        lines.append(f"biomes {a['by_region']}")
    return "\n".join(lines)
