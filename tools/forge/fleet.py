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


# ───────────────────────────────────────────── the default, written by hand
#
# Every other entry in this file varies one axis of a spectrum. This one does
# not: it is the car the game opens on, so it has a different job — be legible
# in three seconds, be forgiving to drive, and look deliberate enough that a
# new player does not think they are looking at placeholder art.
#
# Design decisions, each of which is a departure from the spectrum cars:
#
#  * MID-SET HITBOX. Not the sponge of ANVIL nor the blade of LANCE. l/w ~1.9
#    and a roofline that is neither a shelf nor a wedge, because the default
#    should not teach a habit the player has to unlearn when they switch.
#  * A WAIST. The hull pinches behind the front axle and flares again over the
#    rear. Every other car here is monotonic nose-to-tail, which is why they
#    read as loaves; a waist is the cheapest line that says "designed".
#  * TWO-STAGE ROOF. A low cowl, then a distinct cabin step. The one silhouette
#    cue that survives at 30 metres on a phone screen.
#  * REAR HAUNCH WIDER THAN THE FRONT. Reads as drive-at-the-back without
#    needing a single texture.
#  * NINE HULL SECTIONS instead of four. The Catmull-Rom resample makes
#    character lines from control points, so more points is more line, and
#    this is the one model where that cost is worth paying.

MOIRE = Spec(
    name="MOIRE", cls="car",
    role="The default. Mid-set hitbox, forgiving roof, and enough shape to "
         "read at a glance — it teaches nothing you have to unlearn when you "
         "switch to a specialist.",
    hull=[
        (-2.05, 0.44, 0.10, 0.34, 0.30),   # nose, low and narrow
        (-1.55, 0.72, 0.07, 0.46, 0.34),   # front axle line
        (-1.05, 0.88, 0.05, 0.52, 0.32),   # shoulder
        (-0.55, 0.80, 0.05, 0.60, 0.30),   # WAIST — pinched, then flares
        (-0.05, 0.86, 0.05, 0.74, 0.32),   # cabin front
        ( 0.50, 0.92, 0.05, 0.78, 0.32),   # cabin rear
        ( 1.05, 0.99, 0.06, 0.70, 0.30),   # REAR HAUNCH, widest point
        ( 1.60, 0.86, 0.09, 0.60, 0.32),
        # Tail cut SHORT and kept deep. The first version tapered to
        # (0.58 wide, 0.29 deep) and read as a blade under flat shading — a
        # thin flat surface at a shallow angle catches light like a wing. A
        # tail wants to end, not thin out.
        ( 1.92, 0.76, 0.10, 0.58, 0.30),
    ],
    ring=20,
    wheels=[(-0.86, 0.31, -1.42, 0.31), (0.86, 0.31, -1.42, 0.31),
            (-0.96, 0.34,  1.30, 0.34), (0.96, 0.34,  1.30, 0.34)],
    pods=[(0.0, 0.80, -0.62, 0.10)],       # a sensor blister on the cowl
)

CARS.append(MOIRE)
ALL.append(MOIRE)
BY_CLASS["car"].append(MOIRE)
