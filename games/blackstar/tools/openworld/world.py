"""world.py -- the agnostic data model for an open world.

Deliberately game-neutral: a World is a bounded 2.5D field of cells with a height
substrate, a set of placed POIs (points of interest / goals), and obstacles. Nothing
here knows about any particular game -- Cybertron, a shooter, an RPG map -- so the same
model can be scored, pathed, palletted and optimised by the rest of the library.

Ported in spirit from games/soldier's `Level` + procgen noise substrate: a multi-octave
fBm field is the "raw potential" of the space, and everything else reads it.
"""
from __future__ import annotations
import math, random
from dataclasses import dataclass, field
from typing import Optional


def _fbm(seed: int, n: int, octaves: int = 4, persistence: float = 0.5):
    """Tileable-ish multi-octave value noise in [0,1], n x n. Pure-python, no deps."""
    rng = random.Random(seed)
    # base lattice per octave, bilinearly upsampled and summed
    field_ = [[0.0] * n for _ in range(n)]
    amp, tot = 1.0, 0.0
    for o in range(octaves):
        cells = max(2, 2 ** (o + 1))
        grid = [[rng.random() for _ in range(cells + 1)] for _ in range(cells + 1)]
        for y in range(n):
            fy = y / n * cells; iy = int(fy); ty = fy - iy
            for x in range(n):
                fx = x / n * cells; ix = int(fx); tx = fx - ix
                a = grid[iy][ix] + (grid[iy][ix + 1] - grid[iy][ix]) * tx
                b = grid[iy + 1][ix] + (grid[iy + 1][ix + 1] - grid[iy + 1][ix]) * tx
                field_[y][x] += (a + (b - a) * ty) * amp
        tot += amp; amp *= persistence
    return [[v / tot for v in row] for row in field_]


@dataclass
class POI:
    """A point of interest -- a goal, objective, spawn, landmark, or event site."""
    x: float
    y: float
    kind: str = "generic"        # e.g. 'landmark', 'objective', 'spawn', 'cache', 'boss'
    radius: float = 4.0
    weight: float = 1.0          # importance (drives ordering / rewards)
    tier: int = 0                # difficulty tier (0 = near spawn, higher = deeper)
    region: str = ""             # biome/palette region this POI fell in
    tags: tuple = ()             # freeform, e.g. ('raid','phase1')

    def dist(self, ox: float, oy: float) -> float:
        return math.hypot(self.x - ox, self.y - oy)


@dataclass
class Obstacle:
    """A cover/blocker box. `tall` cover breaks sightlines; low cover does not."""
    x: float
    y: float
    hx: float
    hy: float
    height: float = 2.0
    solid: bool = True           # blocks pathing when True

    def tall(self, eye: float = 1.6) -> bool:
        return self.height >= eye


@dataclass
class World:
    """A bounded world: a height substrate + placed POIs + obstacles + a palette id."""
    size: float                          # world is [-size, size] on x and y
    grid: int = 48                       # substrate resolution
    seed: int = 1
    open_sky: bool = True                # outdoor (sky exposure counts toward openness)
    palette: str = "default"
    height: list = field(default_factory=list)      # grid x grid, 0..1
    pois: list = field(default_factory=list)        # list[POI]
    obstacles: list = field(default_factory=list)   # list[Obstacle]

    def __post_init__(self):
        if not self.height:
            self.height = _fbm(self.seed, self.grid, octaves=5)

    # --- substrate reads (the "diff it" step from soldier's procgen) ---
    def _cell(self, x: float, y: float):
        gx = int((x + self.size) / (2 * self.size) * (self.grid - 1))
        gy = int((y + self.size) / (2 * self.size) * (self.grid - 1))
        return max(0, min(self.grid - 1, gx)), max(0, min(self.grid - 1, gy))

    def height_at(self, x: float, y: float) -> float:
        gx, gy = self._cell(x, y)
        return self.height[gy][gx]

    def slope_at(self, x: float, y: float) -> float:
        gx, gy = self._cell(x, y)
        gx0, gx1 = max(0, gx - 1), min(self.grid - 1, gx + 1)
        gy0, gy1 = max(0, gy - 1), min(self.grid - 1, gy + 1)
        dx = self.height[gy][gx1] - self.height[gy][gx0]
        dy = self.height[gy1][gx] - self.height[gy0][gx]
        return math.hypot(dx, dy)

    def roughness_at(self, x: float, y: float, r: int = 2) -> float:
        gx, gy = self._cell(x, y)
        vals = []
        for j in range(max(0, gy - r), min(self.grid, gy + r + 1)):
            for i in range(max(0, gx - r), min(self.grid, gx + r + 1)):
                vals.append(self.height[j][i])
        if not vals:
            return 0.0
        m = sum(vals) / len(vals)
        return (sum((v - m) ** 2 for v in vals) / len(vals)) ** 0.5

    def to_world(self, gx: int, gy: int):
        x = gx / (self.grid - 1) * 2 * self.size - self.size
        y = gy / (self.grid - 1) * 2 * self.size - self.size
        return x, y
