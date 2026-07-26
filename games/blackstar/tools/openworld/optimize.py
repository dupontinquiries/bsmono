"""optimize.py -- iterate a world toward a better design score.

The whole point of the score harness (per soldier's mapeval note) is to *iterate*:
generate, score, perturb the weak axis, re-score, keep improvements. This is a small
hill-climber that mutates obstacle count/placement and re-scores, honouring connectivity
as a hard constraint. Game-agnostic: it only touches the World's obstacles + substrate.
"""
from __future__ import annotations
import random
from .score import score
from .world import Obstacle


def _mutate(world, rng):
    """One random structural tweak. Returns an undo closure."""
    roll = rng.random()
    if roll < 0.4 and world.obstacles:                 # move an obstacle
        ob = rng.choice(world.obstacles)
        old = (ob.x, ob.y)
        ob.x = max(-world.size, min(world.size, ob.x + rng.uniform(-1, 1) * world.size * 0.15))
        ob.y = max(-world.size, min(world.size, ob.y + rng.uniform(-1, 1) * world.size * 0.15))
        return lambda: setattr(ob, "x", old[0]) or setattr(ob, "y", old[1])
    elif roll < 0.7:                                   # add an obstacle
        ob = Obstacle(rng.uniform(-world.size * 0.85, world.size * 0.85),
                      rng.uniform(-world.size * 0.85, world.size * 0.85),
                      rng.uniform(1.5, 4.0), rng.uniform(1.5, 4.0),
                      height=rng.choice([1.2, 2.5, 4.0]))
        world.obstacles.append(ob)
        return lambda: world.obstacles.remove(ob)
    elif world.obstacles:                              # remove an obstacle
        ob = rng.choice(world.obstacles)
        idx = world.obstacles.index(ob)
        world.obstacles.remove(ob)
        return lambda: world.obstacles.insert(idx, ob)
    return lambda: None


def optimize(world, iterations=120, seed=None):
    """Hill-climb: keep any mutation that raises the total, revert the rest."""
    rng = random.Random(seed if seed is not None else world.seed * 13 + 5)
    best = score(world).total
    history = [best]
    for _ in range(iterations):
        undo = _mutate(world, rng)
        s = score(world)
        # reject anything that disconnects the world or lowers the score
        if s.connected and s.total > best + 1e-6:
            best = s.total
        else:
            undo()
        history.append(best)
    return best, history


# ---- simulated annealing + target-profile optimisation ----

def profile_distance(card, target):
    """Distance from a desired sub-score profile (0 = perfect match). `target` is a dict of
    any of openness/flow/cover/verticality in 0..1. Lets a caller optimise toward a FEEL
    (e.g. 'open and vertical') rather than the single blended total."""
    keys = ("openness", "flow", "cover", "verticality")
    d = 0.0; k = 0
    for key in keys:
        if key in target:
            d += abs(getattr(card, key) - target[key]); k += 1
    return d / k if k else 0.0


def anneal(world, iterations=200, seed=None, target=None, t0=0.35, t1=0.01):
    """Simulated annealing -- accepts some WORSE moves early (temperature t0->t1) to escape
    the local optima a pure hill-climb gets stuck in. Optimises the design total, or, if a
    `target` profile is given, minimises distance to it. Connectivity stays a hard gate.
    Returns the BEST configuration seen (snapshotted), not merely the final walk position."""
    import random, math, copy
    rng = random.Random(seed if seed is not None else world.seed * 17 + 3)

    def energy():
        c = score(world)
        if not c.connected:
            return 1e6
        return profile_distance(c, target) if target else (1.0 - c.total / 100.0)

    cur = energy(); best = cur; hist = [cur]
    best_obs = [copy.copy(o) for o in world.obstacles]
    for it in range(iterations):
        temp = t0 + (t1 - t0) * (it / max(1, iterations - 1))
        undo = _mutate(world, rng)
        e = energy()
        if e < cur or rng.random() < math.exp(-(e - cur) / max(1e-4, temp)):
            cur = e
            if e < best:
                best = e
                best_obs = [copy.copy(o) for o in world.obstacles]
        else:
            undo()
        hist.append(cur)
    world.obstacles = best_obs           # keep the best we ever found, not the last step
    return score(world), hist
