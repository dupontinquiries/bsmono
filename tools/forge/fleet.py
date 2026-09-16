#!/usr/bin/env python3
"""fleet.py — the catalogue.

Every entry states the gap it fills, because procars.rs already proves that
is the useful documentation: "BREADBOX — taller and shorter than Octane:
maximum sponge, the ball settles on the roof shelf; slowest-feeling,
comfiest dribbler." A model whose reason you cannot state is a model nobody
knows when to pick.

Each class is laid out as a SPECTRUM rather than a set, so the roster has no
redundant members and a gap in it is visible. Numbers are in metres, nose at
-z, ground at y=0.
"""
from forge import Spec

# ─────────────────────────────────────────────────────────── ground cars
# Spectrum: hitbox shape. Rocket League's whole car roster is really three
# hitboxes wearing different shells, so these vary the hitbox and let the
# shell follow, not the other way round.

CARS = [
    Spec(
        name="ANVIL", cls="car",
        role="Short and tall: maximum sponge. The roof is a shelf the ball "
             "settles into, so first touches are forgiving. Slowest-feeling "
             "of the four and the one to learn dribbling on.",
        hull=[(-1.55, 0.52, 0.10, 0.52, 0.28),
              (-0.60, 0.94, 0.06, 0.92, 0.30),
              ( 0.55, 0.96, 0.06, 0.95, 0.30),
              ( 1.55, 0.60, 0.10, 0.62, 0.28)],
        wheels=[(-0.82, 0.30, -1.00, 0.30), (0.82, 0.30, -1.00, 0.30),
                (-0.84, 0.32,  1.02, 0.32), (0.84, 0.32,  1.02, 0.32)],
        pods=[(0.0, 0.97, -0.25, 0.14)],
    ),
    Spec(
        name="LANCE", cls="car",
        role="Long and flat: the flick blade. Almost no roof to balance on, "
             "but the nose reaches a ball nothing else gets to and the flat "
             "deck is the best flip-reset surface in the set.",
        hull=[(-2.30, 0.42, 0.06, 0.28, 0.22),
              (-0.90, 0.86, 0.04, 0.46, 0.26),
              ( 0.70, 0.88, 0.04, 0.50, 0.26),
              ( 2.20, 0.50, 0.06, 0.34, 0.22)],
        wheels=[(-0.80, 0.26, -1.55, 0.26), (0.80, 0.26, -1.55, 0.26),
                (-0.82, 0.28,  1.50, 0.28), (0.82, 0.28,  1.50, 0.28)],
    ),
    Spec(
        name="DRUM", cls="car",
        role="Wide and round: the immovable one. Hardest in the set to flip "
             "or shove off a ball, and the curved flanks turn bad contacts "
             "into glancing ones instead of spins.",
        hull=[(-1.70, 0.62, 0.12, 0.58, 0.72),
              (-0.55, 1.14, 0.08, 0.80, 0.80),
              ( 0.60, 1.14, 0.08, 0.80, 0.80),
              ( 1.70, 0.66, 0.12, 0.60, 0.72)],
        wheels=[(-1.02, 0.30, -1.05, 0.30), (1.02, 0.30, -1.05, 0.30),
                (-1.04, 0.32,  1.08, 0.32), (1.04, 0.32,  1.08, 0.32)],
    ),
    Spec(
        name="SPLIT", cls="car",
        role="Narrow and tall: threads gaps the others bounce off. Tiny "
             "contact patch makes it the twitchiest dribbler here — the "
             "hard-mode pick, and the fastest through cluttered geometry.",
        hull=[(-1.80, 0.30, 0.10, 0.58, 0.30),
              (-0.60, 0.52, 0.06, 1.00, 0.34),
              ( 0.60, 0.54, 0.06, 1.02, 0.34),
              ( 1.80, 0.34, 0.10, 0.62, 0.30)],
        wheels=[(-0.52, 0.30, -1.20, 0.30), (0.52, 0.30, -1.20, 0.30),
                (-0.54, 0.30,  1.22, 0.30), (0.54, 0.30,  1.22, 0.30)],
        fins=[(1.55, 0.42, 0.55, 0.0)],
    ),
]

# ──────────────────────────────────────────────────────────────── hoverers
# Spectrum: cushion footprint. The skirt is not decoration — procars.rs
# makes the point that an imported hovercar has "nothing on it that says why
# it floats", so every one of these carries a visible plenum and nozzles
# placed where the physics samples the cushion.

HOVERERS = [
    Spec(
        name="CAIRN", cls="hoverer",
        role="A wide low plenum: the siege hoverer. Slowest to turn, but the "
             "cushion is broad enough that it barely notices terrain, so it "
             "holds a firing line where wheeled vehicles get bucked.",
        hull=[(-1.60, 0.70, 0.42, 0.86, 0.45),
              (-0.50, 1.20, 0.38, 1.10, 0.50),
              ( 0.55, 1.22, 0.38, 1.12, 0.50),
              ( 1.60, 0.76, 0.42, 0.88, 0.45)],
        skirt=0.30,
        nozzles=[(-0.85, 0.22, -0.90, 0.16), (0.85, 0.22, -0.90, 0.16),
                 (-0.85, 0.22,  0.95, 0.16), (0.85, 0.22,  0.95, 0.16)],
    ),
    Spec(
        name="WISP", cls="hoverer",
        role="A pod on a tiny cushion: fastest thing on the ground and the "
             "least stable. Two nozzles instead of four, so it yaws by "
             "differential thrust and can spin on the spot.",
        hull=[(-0.85, 0.34, 0.46, 0.74, 0.85),
              ( 0.00, 0.58, 0.42, 0.98, 0.90),
              ( 0.90, 0.36, 0.46, 0.76, 0.85)],
        skirt=0.16,
        nozzles=[(-0.42, 0.30, 0.55, 0.13), (0.42, 0.30, 0.55, 0.13)],
        pods=[(0.0, 0.92, -0.35, 0.16)],
    ),
    Spec(
        name="SLAB", cls="hoverer",
        role="A long flat plenum: the hover plank. Carries the ball on a "
             "deck with no roof curve at all, which makes it the transport "
             "pick — and an enormous, slow target.",
        hull=[(-2.40, 0.56, 0.40, 0.62, 0.20),
              (-0.80, 1.00, 0.36, 0.70, 0.22),
              ( 0.80, 1.00, 0.36, 0.70, 0.22),
              ( 2.40, 0.60, 0.40, 0.64, 0.20)],
        skirt=0.24,
        nozzles=[(-0.78, 0.22, -1.70, 0.15), (0.78, 0.22, -1.70, 0.15),
                 (-0.78, 0.22,  1.70, 0.15), (0.78, 0.22,  1.70, 0.15)],
    ),
]

# ───────────────────────────────────────────────────────────────── flyers
# Spectrum: wing loading. Big wing = slow and floaty, small wing = fast and
# committed. Fins give each one a distinct silhouette from below, which is
# the angle you actually see an enemy flyer from.

FLYERS = [
    Spec(
        name="KITE", cls="flyer",
        role="Huge wing, tiny body: floats. Slowest in the air and the only "
             "one that can hold a hover-turn, so it is the spotter — long "
             "loiter, no punch.",
        hull=[(-1.40, 0.26, 0.30, 0.54, 0.75),
              (-0.20, 0.44, 0.24, 0.72, 0.80),
              ( 1.30, 0.22, 0.30, 0.50, 0.75)],
        wings=[(-0.10, 4.20, 0.95, 0.52, 0.10)],
        fins=[(1.15, 0.52, 0.60, 0.0)],
    ),
    Spec(
        name="DART", cls="flyer",
        role="Stub wings on a spike: dives. Cannot turn with anything, but "
             "arrives first and hits hardest on the way through. The "
             "interceptor — one pass, then reposition.",
        hull=[(-2.10, 0.14, 0.34, 0.50, 0.85),
              (-0.70, 0.40, 0.26, 0.74, 0.70),
              ( 0.60, 0.36, 0.28, 0.68, 0.70),
              ( 1.90, 0.18, 0.34, 0.48, 0.85)],
        wings=[(0.25, 1.70, 0.60, 0.46, 0.22)],
        fins=[(1.65, 0.46, 0.50, 0.0), (1.65, 0.38, 0.44, 1.5708)],
        nozzles=[(0.0, 0.40, 1.95, 0.17)],
    ),
    Spec(
        name="SPAN", cls="flyer",
        role="Long thin wing on a slim fuselage: the glider. Best "
             "energy-retention in the set — it trades height for speed and "
             "back again, so it wins any fight that lasts.",
        hull=[(-1.90, 0.20, 0.32, 0.50, 0.80),
              (-0.40, 0.36, 0.26, 0.66, 0.80),
              ( 1.70, 0.18, 0.32, 0.46, 0.80)],
        wings=[(-0.05, 6.00, 0.62, 0.48, 0.06)],
        fins=[(1.45, 0.60, 0.52, 0.0)],
    ),
]

# ───────────────────────────────────────────────────────────── spaceships
# Spectrum: mass distribution. No wings — nothing to bite on — so these read
# by silhouette alone, which is why each gets one strong non-hull element.

SPACESHIPS = [
    Spec(
        name="HOLLOW", cls="spaceship",
        role="A ring around a small core: almost no mass at the ends, so it "
             "rotates instantly. The skirmisher — always facing you, never "
             "where it was.",
        hull=[(-0.70, 0.34, -0.30, 0.30, 0.90),
              ( 0.00, 0.50, -0.42, 0.42, 0.92),
              ( 0.70, 0.36, -0.30, 0.32, 0.90)],
        rings_deco=[(0.0, 1.55, 0.14)],
        nozzles=[(-0.30, 0.0, 0.80, 0.14), (0.30, 0.0, 0.80, 0.14)],
    ),
    Spec(
        name="TUSK", cls="spaceship",
        role="All mass at the nose: turns like a barge, hits like one. Built "
             "to be pointed once and committed — the ram, and the only hull "
             "here that survives a head-on.",
        hull=[(-2.60, 0.62, -0.50, 0.52, 0.55),
              (-1.20, 0.74, -0.60, 0.64, 0.50),
              ( 0.40, 0.40, -0.34, 0.36, 0.60),
              ( 2.40, 0.26, -0.22, 0.24, 0.70)],
        nozzles=[(0.0, 0.0, 2.55, 0.22)],
        fins=[(1.90, 0.55, 0.70, 0.0), (1.90, 0.55, 0.70, 1.5708)],
    ),
    Spec(
        name="CLUSTER", cls="spaceship",
        role="A spine with pods bolted along it: mass spread wide, so it "
             "yaws slowly and pitches worse. The carrier — it is a platform "
             "for whatever is on the pods, not a fighter.",
        hull=[(-2.20, 0.26, -0.22, 0.24, 0.80),
              (-0.60, 0.34, -0.30, 0.32, 0.82),
              ( 0.80, 0.34, -0.30, 0.32, 0.82),
              ( 2.20, 0.24, -0.20, 0.22, 0.80)],
        pods=[(-0.95, 0.0, -0.80, 0.46), (0.95, 0.0, -0.80, 0.46),
              (-0.95, 0.0,  0.85, 0.46), (0.95, 0.0,  0.85, 0.46)],
        nozzles=[(0.0, 0.0, 2.35, 0.18)],
    ),
]

ALL = CARS + HOVERERS + FLYERS + SPACESHIPS
BY_CLASS = {"car": CARS, "hoverer": HOVERERS, "flyer": FLYERS, "spaceship": SPACESHIPS}
