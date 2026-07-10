"""
Paperbound — Sigil Bank.

A single registry for every card ability ("sigil"): its display name, colour, a
one-line description, and a compact GLYPH archetype used to draw a recognisable
icon. The glyph is the accessibility win — a small badge floated above a unit on
the map tells you at a glance what it does, without hovering the card.

There are ~60 sigils, so rather than 60 bespoke icons we map each to one of a
handful of readable archetypes (shield, sword, burst, wing, drop, spiral, bolt,
skull, eye, ring, up-arrow, down-arrow, star, snowflake, flame, chain, heart,
web). Colour + archetype together read distinctly. draw_glyph() renders one with
cheap primitives; the game floats it in a rounded badge above the creature.

This module has NO hard dependency on game.py — it only needs pygame to draw. The
game passes in its own colour/description tables so the bank stays the single
source of truth without duplicating data.
"""
import math

# ── glyph archetypes: each sigil → a symbol family that hints at what it does ──
# (offense = warm, defense = cool, utility = neutral — reinforced by colour too)
GLYPH = {
    # offense
    "slam": "hammer", "knock": "hammer", "charge": "arrow", "rush": "arrow",
    "pounce": "arrow", "rampage": "burst", "frenzy": "burst", "doublestrike": "sword",
    "sharp": "sword", "quill": "spikes", "thorns": "spikes", "snipe": "bolt",
    "artillery": "arc", "siege": "arc", "gale": "wind", "tidal": "wave",
    "molten": "flame", "kindle": "flame", "toxic": "drop", "plague": "skull",
    "sap": "drop", "drinker": "drop", "siphon": "drop", "steal": "hand",
    "grasp": "hand", "echo": "ring", "dazzle": "star", "inksplash": "splash",
    "towerbane": "tower", "beach": "wave", "swiftdive": "wing",

    # defense / resilience
    "bulwark": "shield", "anchor": "anchor", "armored": "shield", "phalanx": "shield",
    "taunt": "shield", "rebirth": "heart", "relentless": "heart", "roll": "shield",
    "ghostly": "ghost", "stealth": "eye", "burrow": "down", "dormant": "moon",
    "chill": "snow", "freeze": "snow", "snare": "web",

    # utility / movement / support
    "feather": "wing", "leap": "up", "blink": "spark", "warp": "spark",
    "forge": "anvil", "radiance": "star", "beacon": "star", "harvest": "leaf",
    "carrier": "swarm", "builder": "anvil", "tunnel": "down", "manyfold": "copy",
    "mirror": "copy", "copycat": "copy", "wildcard": "star", "charm": "heart",
}

# a fallback for any sigil not explicitly mapped
_DEFAULT_GLYPH = "ring"


def glyph_for(sigil):
    return GLYPH.get(sigil, _DEFAULT_GLYPH)


# ── drawing ──────────────────────────────────────────────────────────────────
def draw_glyph(surf, kind, cx, cy, r, col, ink=(28, 22, 18)):
    """Draw one glyph archetype centred at (cx,cy) with radius r in colour `col`.
    Uses only cheap pygame primitives so it's fine to draw many per frame."""
    import pygame
    x, y = int(cx), int(cy)
    r = max(3, int(r))
    lw = max(1, r // 4)

    def line(a, b, w=lw): pygame.draw.line(surf, col, a, b, w)
    def circ(c, rad, w=0): pygame.draw.circle(surf, col, c, max(1, int(rad)), w)
    def poly(pts, w=0): pygame.draw.polygon(surf, col, pts, w)

    if kind == "shield":
        poly([(x, y - r), (x + r, y - r * 0.4), (x, y + r), (x - r, y - r * 0.4)])
    elif kind == "sword":
        line((x, y - r), (x, y + r * 0.4), lw + 1)
        line((x - r * 0.5, y + r * 0.3), (x + r * 0.5, y + r * 0.3))
    elif kind == "hammer":
        line((x, y - r * 0.2), (x, y + r), lw + 1)
        pygame.draw.rect(surf, col, (x - r * 0.7, y - r, r * 1.4, r * 0.7), border_radius=2)
    elif kind == "arrow":
        line((x - r, y), (x + r * 0.5, y), lw + 1)
        poly([(x + r, y), (x + r * 0.3, y - r * 0.5), (x + r * 0.3, y + r * 0.5)])
    elif kind == "up":
        line((x, y + r), (x, y - r * 0.3), lw + 1)
        poly([(x, y - r), (x - r * 0.5, y - r * 0.3), (x + r * 0.5, y - r * 0.3)])
    elif kind == "down":
        line((x, y - r), (x, y + r * 0.3), lw + 1)
        poly([(x, y + r), (x - r * 0.5, y + r * 0.3), (x + r * 0.5, y + r * 0.3)])
    elif kind == "burst":
        for a in range(8):
            ang = a / 8 * math.tau
            line((x, y), (x + math.cos(ang) * r, y + math.sin(ang) * r))
    elif kind == "spikes":
        for i in (-1, 0, 1):
            bx = x + i * r * 0.6
            poly([(bx - r * 0.25, y + r * 0.5), (bx, y - r), (bx + r * 0.25, y + r * 0.5)])
    elif kind == "bolt":
        line((x + r * 0.6, y - r), (x - r * 0.2, y + r), lw + 1)
        poly([(x + r, y - r), (x - r, y + r), (x + r * 0.4, y)], )
    elif kind == "arc":
        pygame.draw.arc(surf, col, (x - r, y - r * 0.4, r * 2, r * 2), math.pi * 0.15, math.pi * 0.85, lw + 1)
        circ((int(x + r * 0.7), int(y + r * 0.4)), r * 0.3)
    elif kind == "wave":
        pts = [(x - r + i, y + math.sin(i / r * math.pi * 2) * r * 0.5) for i in range(0, int(r * 2), 2)]
        if len(pts) > 1: pygame.draw.lines(surf, col, False, pts, lw + 1)
    elif kind == "wind":
        for dy in (-r * 0.4, 0, r * 0.4):
            pygame.draw.arc(surf, col, (x - r, y + dy - r * 0.3, r * 1.8, r * 0.7), 0.2, 2.6, lw)
    elif kind == "flame":
        poly([(x, y - r), (x + r * 0.6, y + r * 0.3), (x + r * 0.3, y + r), (x - r * 0.3, y + r),
              (x - r * 0.6, y + r * 0.3)])
    elif kind == "drop":
        poly([(x, y - r), (x + r * 0.7, y + r * 0.4), (x, y + r), (x - r * 0.7, y + r * 0.4)])
    elif kind == "splash":
        circ((x, y), r * 0.4)
        for a in range(6):
            ang = a / 6 * math.tau
            circ((int(x + math.cos(ang) * r), int(y + math.sin(ang) * r)), r * 0.18)
    elif kind == "skull":
        circ((x, y - r * 0.2), r * 0.7)
        pygame.draw.rect(surf, col, (x - r * 0.4, y + r * 0.1, r * 0.8, r * 0.6))
        pygame.draw.circle(surf, ink, (int(x - r * 0.3), int(y - r * 0.2)), max(1, int(r * 0.18)))
        pygame.draw.circle(surf, ink, (int(x + r * 0.3), int(y - r * 0.2)), max(1, int(r * 0.18)))
    elif kind == "eye":
        poly([(x - r, y), (x, y - r * 0.6), (x + r, y), (x, y + r * 0.6)], lw)
        circ((x, y), r * 0.35)
    elif kind == "ghost":
        circ((x, y - r * 0.2), r * 0.7)
        pygame.draw.rect(surf, col, (x - r * 0.7, y - r * 0.2, r * 1.4, r))
    elif kind == "ring":
        circ((x, y), r, lw)
    elif kind == "star":
        pts = []
        for a in range(10):
            ang = a / 10 * math.tau - math.pi / 2
            rad = r if a % 2 == 0 else r * 0.45
            pts.append((x + math.cos(ang) * rad, y + math.sin(ang) * rad))
        poly(pts)
    elif kind == "snow":
        for a in range(6):
            ang = a / 6 * math.tau
            line((x, y), (x + math.cos(ang) * r, y + math.sin(ang) * r))
    elif kind == "wing":
        pygame.draw.arc(surf, col, (x - r, y - r, r * 2, r * 2), 0.1, 1.6, lw + 1)
        pygame.draw.arc(surf, col, (x - r, y - r, r * 2, r * 2), 1.6, 3.0, lw + 1)
    elif kind == "heart":
        circ((int(x - r * 0.4), int(y - r * 0.2)), r * 0.4)
        circ((int(x + r * 0.4), int(y - r * 0.2)), r * 0.4)
        poly([(x - r * 0.75, y), (x + r * 0.75, y), (x, y + r)])
    elif kind == "anchor":
        line((x, y - r), (x, y + r * 0.7), lw + 1)
        circ((x, y - r), r * 0.3, lw)
        pygame.draw.arc(surf, col, (x - r, y - r * 0.3, r * 2, r * 1.4), math.pi, math.tau, lw + 1)
    elif kind == "web":
        for a in range(6):
            ang = a / 6 * math.tau
            line((x, y), (x + math.cos(ang) * r, y + math.sin(ang) * r))
        circ((x, y), r * 0.6, lw)
    elif kind == "spark":
        for a in range(4):
            ang = a / 4 * math.tau
            line((x, y), (x + math.cos(ang) * r, y + math.sin(ang) * r), lw + 1)
    elif kind == "anvil":
        pygame.draw.rect(surf, col, (x - r, y - r * 0.2, r * 2, r * 0.5))
        pygame.draw.rect(surf, col, (x - r * 0.3, y + r * 0.3, r * 0.6, r * 0.5))
    elif kind == "leaf":
        poly([(x, y - r), (x + r * 0.6, y), (x, y + r), (x - r * 0.6, y)])
        line((x, y - r * 0.6), (x, y + r * 0.6))
    elif kind == "swarm":
        for dx, dy in ((-r * 0.5, -r * 0.3), (r * 0.4, -r * 0.4), (0, r * 0.4), (r * 0.5, r * 0.3)):
            circ((int(x + dx), int(y + dy)), r * 0.22)
    elif kind == "copy":
        pygame.draw.rect(surf, col, (x - r * 0.8, y - r * 0.6, r * 1.1, r * 1.1), lw)
        pygame.draw.rect(surf, col, (x - r * 0.3, y - r * 0.1, r * 1.1, r * 1.1), lw)
    elif kind == "tower":
        pygame.draw.rect(surf, col, (x - r * 0.6, y - r * 0.6, r * 1.2, r * 1.6))
        poly([(x - r * 0.7, y - r * 0.6), (x, y - r), (x + r * 0.7, y - r * 0.6)])
    elif kind == "hand":
        pygame.draw.rect(surf, col, (x - r * 0.5, y - r * 0.2, r, r), border_radius=2)
        for i in range(3):
            line((x - r * 0.4 + i * r * 0.4, y - r * 0.2), (x - r * 0.4 + i * r * 0.4, y - r))
    elif kind == "moon":
        circ((x, y), r)
        pygame.draw.circle(surf, ink, (int(x + r * 0.4), int(y - r * 0.2)), int(r * 0.8))
    else:
        circ((x, y), r, lw)


def badge_size(base=9):
    """Suggested badge radius; the game scales this by the unit's on-screen size."""
    return base


if __name__ == "__main__":
    # offline self-test: every in-use sigil resolves to a known glyph archetype,
    # and every archetype is drawable without error on a dummy surface.
    import os
    os.environ.setdefault("SDL_VIDEODRIVER", "dummy")
    import pygame
    pygame.init()
    surf = pygame.Surface((64, 64), pygame.SRCALPHA)
    kinds = set(GLYPH.values()) | {_DEFAULT_GLYPH}
    for k in kinds:
        surf.fill((0, 0, 0, 0))
        draw_glyph(surf, k, 32, 32, 12, (220, 180, 90))
    # a handful of representative sigils map sensibly
    assert glyph_for("slam") == "hammer"
    assert glyph_for("stealth") == "eye"
    assert glyph_for("freeze") == "snow"
    assert glyph_for("rebirth") == "heart"
    assert glyph_for("totally_unknown_sigil") == _DEFAULT_GLYPH
    print(f"✅ sigils self-test: {len(GLYPH)} sigils mapped to "
          f"{len(kinds)} glyph archetypes, all drawable")
