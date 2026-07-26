"""palette.py -- paletting: named biome/theme palettes an open world can wear.

A Palette bundles the *look and rules* of a region independent of geometry: material
ids, a key/fill/accent colour set, which POI archetypes are allowed (a mask the goal
placer gates activations through -- ported from soldier's zone-archetype masking), and
spawn weights. Register your own; the library ships a few generic ones so it works out
of the box for any game.
"""
from __future__ import annotations
from dataclasses import dataclass, field


@dataclass
class Palette:
    name: str
    key: tuple = (1.0, 1.0, 1.0)          # key light / dominant colour (0..1 rgb)
    fill: tuple = (0.3, 0.3, 0.4)         # fill / ambient colour
    accent: tuple = (1.0, 0.7, 0.3)       # emissive / energy accent
    materials: tuple = ("floor", "wall", "rock")
    open_sky: bool = True
    # archetype mask: which POI kinds this palette permits (goal placer respects it)
    allow: tuple = ("landmark", "objective", "spawn", "cache", "boss")
    # relative spawn weight for ambient POIs per kind
    weights: dict = field(default_factory=lambda: {"objective": 1.0, "cache": 0.7, "spawn": 0.5})


class PaletteRegistry:
    def __init__(self):
        self._p = {}
        for p in _DEFAULTS:
            self.add(p)

    def add(self, p: Palette):
        self._p[p.name] = p

    def get(self, name: str) -> Palette:
        return self._p.get(name, self._p["default"])

    def names(self):
        return list(self._p)


# A handful of generic, game-agnostic palettes -- swap/extend for any project.
_DEFAULTS = [
    Palette("default", key=(1.0, 1.0, 1.05), fill=(0.3, 0.32, 0.4), accent=(1.0, 0.7, 0.3),
            materials=("floor", "wall", "rock")),
    Palette("frost", key=(0.8, 0.9, 1.1), fill=(0.35, 0.4, 0.55), accent=(0.55, 0.8, 1.0),
            materials=("ice", "ice", "rock"), allow=("landmark", "objective", "cache", "spawn")),
    Palette("ember", key=(1.15, 0.7, 0.4), fill=(0.4, 0.22, 0.18), accent=(1.0, 0.45, 0.2),
            materials=("molten", "obsidian", "rock"), open_sky=False,
            allow=("landmark", "objective", "boss", "spawn")),
    Palette("verdant", key=(0.95, 1.05, 0.85), fill=(0.3, 0.4, 0.3), accent=(0.5, 1.0, 0.5),
            materials=("grass", "stone", "rock")),
    Palette("void", key=(0.7, 0.75, 0.95), fill=(0.12, 0.14, 0.22), accent=(0.6, 0.5, 1.0),
            materials=("plate", "plate", "asteroid"), open_sky=True,
            allow=("landmark", "objective", "cache", "boss")),
]
