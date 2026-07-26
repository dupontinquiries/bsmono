"""openworld -- an agnostic open-world helper library.

Built on the concepts from games/soldier (procgen noise+activation, mapeval scoring) and
generalised so any game can use it. Pieces:

    world     -- agnostic World / POI / Obstacle model + noise substrate
    palette   -- named biome/theme palettes (materials, colours, archetype masks)
    regions   -- multi-biome REGION maps (many palettes in one world)
    score     -- soldier's mapeval design metric, ported (0..100 + grade)
    activation-- the "little net with activators" for goal placement
    goals     -- populate a world with POIs (region-gated, tiered) -- goal setting
    path      -- grid A*, cost-aware A*, road networks, reachability, tours -- pathing
    optimize  -- hill-climb OR simulated-annealing toward a score or a target profile
    analyze   -- headless report: reachability, POI mix, tiers, biomes, roads

Quick start:

    from openworld import build
    w = build(size=120, seed=3, biomes=["frost","ember","verdant"], goals=16,
              optimize_iters=150, roads=True)
    print(w.card.line()); print(w.report())
"""
from dataclasses import dataclass, field
from .world import World, POI, Obstacle
from .palette import Palette, PaletteRegistry
from .regions import RegionMap, scatter_regions
from .score import score, Scorecard
from .activation import ActivationNet
from .goals import place_goals, assign_tiers
from .path import (astar, astar_cost, navigable, route_pois, patrol_tour,
                   components, build_roads, cost_at)
from .optimize import optimize, anneal, profile_distance
from .analyze import analyze, report as _report

REGISTRY = PaletteRegistry()


@dataclass
class BuiltWorld:
    world: World
    card: Scorecard
    pois: list
    region_map: object = None
    roads: list = field(default_factory=list)

    def report(self, spawn=(0.0, 0.0)):
        return _report(self.world, spawn)

    def analyze(self, spawn=(0.0, 0.0)):
        return analyze(self.world, spawn)

    def __getattr__(self, k):        # convenience passthrough to the World
        return getattr(self.world, k)


def build(size=100.0, seed=1, palette="default", grid=48, goals=14,
          seed_obstacles=18, optimize_iters=0, anneal_iters=0, target=None,
          biomes=None, spawn=(0.0, 0.0), roads=False):
    """One-call pipeline: substrate -> cover -> (biome regions) -> (optimise/anneal) ->
    region-gated tiered goals -> (roads) -> score + report.

    biomes: optional list of palette names -> the world is painted with those regions and
            goals are gated by whichever biome they land in.
    target: optional {openness/flow/cover/verticality: 0..1} -> anneal toward that feel.
    """
    import random
    base_pal = REGISTRY.get(biomes[0] if biomes else palette)
    w = World(size=size, grid=grid, seed=seed, open_sky=base_pal.open_sky,
              palette=(biomes[0] if biomes else palette))
    rng = random.Random(seed * 3 + 2)
    for _ in range(seed_obstacles):
        w.obstacles.append(Obstacle(
            rng.uniform(-size * 0.85, size * 0.85), rng.uniform(-size * 0.85, size * 0.85),
            rng.uniform(1.5, 4.0), rng.uniform(1.5, 4.0), height=rng.choice([1.2, 2.5, 4.0])))

    region_map = scatter_regions(size, biomes, n=len(biomes), seed=seed) if biomes else None

    if anneal_iters > 0:
        anneal(w, iterations=anneal_iters, seed=seed, target=target)
    elif optimize_iters > 0:
        optimize(w, iterations=optimize_iters, seed=seed)

    place_goals(w, base_pal, n=goals, seed=seed, region_map=region_map,
                registry=REGISTRY, spawn=spawn)
    road_lines = build_roads(w) if roads else []
    return BuiltWorld(world=w, card=score(w), pois=w.pois,
                      region_map=region_map, roads=road_lines)
