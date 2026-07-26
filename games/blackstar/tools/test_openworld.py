"""Self-test for the agnostic openworld library. Run: python3 tools/test_openworld.py"""
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from openworld import (build, REGISTRY, World, Obstacle, place_goals, score,
                       astar, navigable, patrol_tour, route_pois, optimize)


def test_build_and_score():
    w = build(size=100, seed=3, palette="frost", goals=14)
    assert 0.0 <= w.card.total <= 100.0
    assert w.card.grade() in "SABCDF"
    assert len(w.pois) >= 1
    assert any(p.kind == "landmark" for p in w.pois), "must guarantee a landmark"
    print("build+score:", w.card.line(), "| goals", len(w.pois))


def test_palettes():
    names = REGISTRY.names()
    assert "default" in names and "frost" in names and "ember" in names
    assert REGISTRY.get("nope").name == "default"   # unknown -> default
    # ember is enclosed (no open sky) and permits bosses
    assert not REGISTRY.get("ember").open_sky
    assert "boss" in REGISTRY.get("ember").allow
    print("palettes:", ", ".join(names))


def test_goal_masking():
    # a palette that forbids bosses must never place one
    from openworld import Palette
    REGISTRY.add(Palette("nobosses", allow=("objective", "cache", "landmark", "spawn")))
    w = World(size=90, seed=5, palette="nobosses")
    place_goals(w, REGISTRY.get("nobosses"), n=16, seed=5)
    assert all(p.kind != "boss" for p in w.pois), "allow-mask must gate archetypes"
    print("goal masking: ok, kinds =", sorted(set(p.kind for p in w.pois)))


def test_pathing():
    w = build(size=100, seed=7, palette="default", goals=10)
    # path between the two farthest POIs
    ps = w.pois
    a, b = ps[0], ps[-1]
    p = astar(w.world, (a.x, a.y), (b.x, b.y))
    assert p is not None and len(p) >= 2, "A* must find a route on a connected world"
    assert navigable(w.world, (a.x, a.y), (b.x, b.y))
    tour = patrol_tour(w.world)
    assert len(tour) == len(w.pois)
    full = route_pois(w.world, tour)
    assert len(full) >= len(tour)
    print("pathing: A* waypoints", len(p), "| tour stops", len(tour), "| full route", len(full))


def test_optimize_improves():
    # a deliberately over-cluttered world should improve (or hold) under the hill-climb
    w = World(size=100, seed=9)
    import random
    rng = random.Random(9)
    for _ in range(80):   # way too much cover
        w.obstacles.append(Obstacle(rng.uniform(-80, 80), rng.uniform(-80, 80),
                                     rng.uniform(2, 5), rng.uniform(2, 5), height=3.0))
    before = score(w).total
    best, hist = optimize(w, iterations=140, seed=9)
    assert best >= before - 1e-6, "hill-climb must never regress the kept score"
    assert score(w).connected, "optimiser must keep the world connected"
    print(f"optimize: {before:.1f} -> {best:.1f} (kept-best never regresses)")


if __name__ == "__main__":
    test_build_and_score()
    test_palettes()
    test_goal_masking()
    test_pathing()
    test_optimize_improves()
    print("\nOPENWORLD OK")


def test_biome_regions():
    # a multi-biome world: goals must be gated by whichever biome they land in
    w = build(size=120, seed=11, biomes=["frost", "ember", "verdant"], goals=18, roads=True)
    assert w.region_map is not None
    regions_used = set(p.region for p in w.pois if p.region)
    assert len(regions_used) >= 2, "a 3-biome world should place goals in >=2 regions"
    # ember permits bosses, verdant does not -> any boss must be in an ember cell
    for p in w.pois:
        if p.kind == "boss":
            assert REGISTRY.get(p.region).allow.count("boss"), "boss placed in a biome that forbids it"
    print("biome regions:", regions_used, "| roads", len(w.roads))


def test_tiers():
    w = build(size=100, seed=5, palette="default", goals=14, spawn=(0.0, -80.0))
    tiers = sorted(set(p.tier for p in w.pois))
    assert tiers and min(tiers) == 0, "tiers start at 0 (near spawn)"
    assert max(tiers) >= 1, "distant POIs should reach a higher tier"
    print("tiers:", {t: sum(1 for p in w.pois if p.tier == t) for t in tiers})


def test_cost_pathing_and_roads():
    from openworld import astar_cost, build_roads, components
    w = build(size=100, seed=7, palette="default", goals=10)
    a, b = w.pois[0], w.pois[-1]
    p = astar_cost(w.world, (a.x, a.y), (b.x, b.y))
    assert p and len(p) >= 2, "cost A* must find a route"
    roads = build_roads(w.world)
    assert len(roads) == len(w.pois) - 1, "MST road net links every POI (n-1 edges)"
    _, nc = components(w.world)
    assert nc >= 1
    print("cost pathing: wp", len(p), "| road edges", len(roads), "| components", nc)


def test_anneal_and_target():
    from openworld import World, Obstacle, anneal, score
    import random
    w = World(size=100, seed=9)
    rng = random.Random(9)
    for _ in range(70):
        w.obstacles.append(Obstacle(rng.uniform(-80, 80), rng.uniform(-80, 80),
                                     rng.uniform(2, 5), rng.uniform(2, 5), height=3.0))
    before = score(w).total
    card, hist = anneal(w, iterations=180, seed=9)
    assert card.connected, "anneal keeps the world connected"
    # target-profile anneal should move the profile toward the goal
    w2 = World(size=100, seed=4)
    for _ in range(30):
        w2.obstacles.append(Obstacle(rng.uniform(-80, 80), rng.uniform(-80, 80), 3, 3, height=2.5))
    from openworld import profile_distance
    d0 = profile_distance(score(w2), {"cover": 0.9})
    anneal(w2, iterations=160, seed=4, target={"cover": 0.9})
    d1 = profile_distance(score(w2), {"cover": 0.9})
    assert d1 <= d0 + 1e-6, "target anneal shouldn't worsen the targeted axis"
    print(f"anneal: {before:.1f} -> {card.total:.1f}  | target cover dist {d0:.2f} -> {d1:.2f}")


def test_report():
    w = build(size=120, seed=3, biomes=["frost", "ember"], goals=16, roads=True)
    r = w.report()
    assert "score" in r and "POIs" in r and "roads" in r
    print("report:\n  " + r.replace("\n", "\n  "))


if __name__ == "__main__":
    # re-run the extended suite (the base cases already ran above on import order)
    test_biome_regions()
    test_tiers()
    test_cost_pathing_and_roads()
    test_anneal_and_target()
    test_report()
    print("\nOPENWORLD EXTENDED OK")
