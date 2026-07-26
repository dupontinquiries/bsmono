"""activation.py -- the "little neural net with activators" for goal placement.

Ported from games/soldier's procgen ActivationNet: for a region we build a small feature
vector from the substrate (height, slope, roughness, openness, recursion depth) and run
it through a compact, hand-initialisable perceptron whose outputs are activation scores
per POI archetype. The highest activation (gated by the palette's allow-mask) decides
what goal grows there. Swap the weights -- hand-set, learned, or evolved -- to change the
placer's "taste" without touching the machinery.
"""
from __future__ import annotations
import math

# feature order: [height, slope, roughness, openness, depth, bias]
FEATURES = ("height", "slope", "roughness", "openness", "depth", "bias")

# Default hand-set weights per archetype. Rows are archetypes; columns match FEATURES.
# Tuned so: landmarks like high + rough spots; bosses like low, enclosed arenas;
# objectives like open mid-ground; caches like tucked rough corners; spawns like flat open.
DEFAULT_WEIGHTS = {
    "landmark":  ( 1.4,  0.6,  0.9, -0.2, -0.3, -0.4),
    "boss":      (-0.9, -0.4,  0.3, -1.1,  0.6,  0.1),
    "objective": ( 0.0, -0.3, -0.2,  1.2, -0.1, -0.2),
    "cache":     (-0.2,  0.5,  1.1, -0.3,  0.4, -0.3),
    "spawn":     (-0.4, -0.9, -0.6,  0.9, -0.5,  0.0),
}


def _sigmoid(x: float) -> float:
    return 1.0 / (1.0 + math.exp(-max(-30.0, min(30.0, x))))


class ActivationNet:
    def __init__(self, weights: dict | None = None):
        self.weights = dict(weights) if weights else {k: v for k, v in DEFAULT_WEIGHTS.items()}

    def features(self, world, x: float, y: float, depth: float = 0.0):
        openness = 1.0 - min(1.0, world.roughness_at(x, y) * 3.0)
        return {
            "height": world.height_at(x, y),
            "slope": min(1.0, world.slope_at(x, y) * 4.0),
            "roughness": min(1.0, world.roughness_at(x, y) * 5.0),
            "openness": openness,
            "depth": depth,
            "bias": 1.0,
        }

    def activate(self, world, x: float, y: float, depth: float = 0.0, allow=None):
        """Return {archetype: activation 0..1}, gated to `allow` if given."""
        f = self.features(world, x, y, depth)
        fv = [f[k] for k in FEATURES]
        out = {}
        for arch, w in self.weights.items():
            if allow is not None and arch not in allow:
                continue
            out[arch] = _sigmoid(sum(wi * xi for wi, xi in zip(w, fv)))
        return out

    def best(self, world, x: float, y: float, depth: float = 0.0, allow=None):
        acts = self.activate(world, x, y, depth, allow)
        if not acts:
            return None, 0.0
        arch = max(acts, key=acts.get)
        return arch, acts[arch]
