"""
PAPERBOUND — origami part engine.

Every creature on the field is folded from a small vocabulary of KNOWN-GOOD
"parts" (panels, wedges, wings, spines, legs, crests, tusks…). A card's field
model is just a *recipe*: an ordered list of parts, each a small JSON object

    {"part": "wing", "at": [0.9, -0.2], "size": 1.1, "ang": 0.3, "tone": 0.9}

so models are:
  • data-driven & JSON-serializable (they live alongside the card format and can
    be logged, diffed, and hand-authored),
  • safe to generate (an unknown part name or out-of-range number is dropped by
    fold_sanitize(), exactly like the card sanitizer — the AI Director can invent
    models and they can only ever reference known-good pieces),
  • composable (a recipe is built up from primitives; families provide sensible
    defaults, individual cards override or extend).

The engine is pure geometry + a tiny registry; the actual pixel-pushing reuses
the game's existing draw_prism / draw_poly_paper helpers, which are injected once
via bind_drawing() so this module has no hard dependency on game.py.

Coordinates in a recipe are in BODY-LOCAL units (roughly -1.4..1.4), multiplied
by the unit's on-screen size `s` and mirrored by facing `f`. y is up-negative,
matching the renderer.
"""
import math

# known part types → default params. A recipe entry may override any of these.
PART_TYPES = {
    "slab",     # the main body block (extruded quad)
    "panel",    # a flat folded quad (no extrusion) — plates, flanks, stripes
    "wedge",    # a triangular fold — heads, beaks, fins, horns
    "wing",     # a flapping triangular wing (animates for flyers)
    "spine",    # a row of little folded triangles along the back — scales/quills
    "leg",      # a small support prism under the body
    "crest",    # a raised fan/tuft on top — manes, crowns, plumes
    "tusk",     # a thin bright line pair — tusks/horns/antennae
    "tail",     # a trailing folded strip
    "shell",    # a domed cap — tortoises, beetles, buildings
    "orb",      # a small circle — eyes, seals, lanterns
}

# clamp envelopes so a generated/hand-authored recipe can't explode the renderer
_LIMITS = {
    "size": (0.05, 2.2), "ang": (-3.2, 3.2),
    "tone": (0.3, 1.8), "h": (0.0, 1.6), "count": (1, 7),
}
_ATX = (-1.8, 1.8)      # local x anchor range
_ATY = (-1.8, 1.4)      # local y anchor range

_DRAW = {}              # injected: {"prism":fn, "poly":fn, "shade":fn}


def bind_drawing(draw_prism, draw_poly_paper, shade):
    """Wire in the game's paper-drawing helpers (called once at import time)."""
    _DRAW["prism"] = draw_prism
    _DRAW["poly"] = draw_poly_paper
    _DRAW["shade"] = shade


def _clamp(v, lo, hi, d):
    try:
        v = float(v)
        if v != v:      # NaN
            return d
        return max(lo, min(hi, v))
    except Exception:
        return d


def fold_sanitize(recipe):
    """Coerce any list-of-dicts into a valid model recipe of known-good parts.
    Unknown part types and bad numbers are dropped/clamped. Returns [] if nothing
    usable — the caller then falls back to the family default."""
    if not isinstance(recipe, (list, tuple)):
        return []
    out = []
    for raw in recipe:
        if not isinstance(raw, dict):
            continue
        pt = str(raw.get("part", "")).lower()
        if pt not in PART_TYPES:
            continue
        at = raw.get("at", [0.0, 0.0])
        if not (isinstance(at, (list, tuple)) and len(at) >= 2):
            at = [0.0, 0.0]
        part = {
            "part": pt,
            "at": [_clamp(at[0], _ATX[0], _ATX[1], 0.0),
                   _clamp(at[1], _ATY[0], _ATY[1], 0.0)],
            "size": _clamp(raw.get("size", 1.0), *_LIMITS["size"], 1.0),
            "ang": _clamp(raw.get("ang", 0.0), *_LIMITS["ang"], 0.0),
            "tone": _clamp(raw.get("tone", 1.0), *_LIMITS["tone"], 1.0),
            "h": _clamp(raw.get("h", 0.5), *_LIMITS["h"], 0.5),
        }
        if pt == "spine":
            part["count"] = int(_clamp(raw.get("count", 3), *_LIMITS["count"], 3))
        out.append(part)
        if len(out) >= 14:          # a sane ceiling on model complexity
            break
    return out


# ─────────────────────────── procedural family recipes ─────────────────────
# Each family returns a recipe. These are the "defaults"; a card may ship its own
# `model` recipe in JSON to override, or `model_extra` to append accents.
def _recipe_for_family(fam, rng):
    j = rng.uniform          # small deterministic jitter for organic variety
    if fam in ("bird",):
        return [
            {"part": "slab", "at": [0, 0.05], "size": 0.9, "h": 0.4, "tone": 1.0},
            {"part": "wing", "at": [-0.2, -0.2], "size": 1.15, "ang": 0.5, "tone": 0.9},
            {"part": "wing", "at": [0.5, -0.15], "size": 1.05, "ang": -0.3, "tone": 1.05},
            {"part": "wedge", "at": [0.9, -0.1], "size": 0.6, "ang": -0.2, "tone": 1.1},  # beak
            {"part": "tail", "at": [-0.9, 0.1], "size": 0.7, "ang": 0.3, "tone": 0.85},
        ]
    if fam in ("insect",):
        return [
            {"part": "slab", "at": [0, 0.05], "size": 0.8, "h": 0.45, "tone": 1.0},
            {"part": "wing", "at": [-0.1, -0.25], "size": 0.9, "ang": 0.7, "tone": 0.8},
            {"part": "wing", "at": [0.3, -0.25], "size": 0.9, "ang": -0.7, "tone": 0.8},
            {"part": "tusk", "at": [0.8, -0.2], "size": 0.5, "ang": 0.2},   # antennae
            {"part": "spine", "at": [-0.2, -0.1], "size": 0.5, "count": 3, "tone": 0.7},
        ]
    if fam in ("fish", "cephalopod", "amphibian", "jelly", "wave"):
        return [
            {"part": "slab", "at": [0, 0.05], "size": 0.95, "h": 0.4, "tone": 1.0},
            {"part": "wedge", "at": [-0.9, 0.0], "size": 0.7, "ang": 0.3, "tone": 0.9},  # tail fin
            {"part": "wedge", "at": [0.1, -0.35], "size": 0.5, "ang": 0.0, "tone": 1.1}, # dorsal
            {"part": "orb", "at": [0.75, -0.15], "size": 0.14, "tone": 0.4},
        ]
    if fam in ("tree",):
        return [
            {"part": "slab", "at": [0, 0.2], "size": 0.5, "h": 1.0, "tone": 0.85},   # trunk
            {"part": "crest", "at": [0, -0.7], "size": 1.2, "h": 0.55, "tone": 1.0}, # crown
        ]
    if fam in ("monolith", "golem", "shelled", "armored"):
        return [
            {"part": "shell", "at": [0, -0.1], "size": 1.1, "h": 0.9, "tone": 1.0},
            {"part": "panel", "at": [0, 0.2], "size": 0.9, "tone": 0.7},
            {"part": "spine", "at": [-0.1, -0.5], "size": 0.5, "count": 4, "tone": 1.1},
        ]
    if fam in ("tusker",):
        return [
            {"part": "slab", "at": [0, 0.0], "size": 1.05, "h": 0.6, "tone": 1.0},
            {"part": "leg", "at": [-0.6, 0.4], "size": 0.5, "h": 0.5}, {"part": "leg", "at": [0.6, 0.4], "size": 0.5, "h": 0.5},
            {"part": "wedge", "at": [0.8, -0.3], "size": 0.7, "ang": -0.2, "tone": 1.08},  # head
            {"part": "tusk", "at": [0.9, 0.0], "size": 0.9, "ang": 0.35},
        ]
    if fam in ("bird_big", "rabbit"):
        return [
            {"part": "slab", "at": [0, 0.0], "size": 0.95, "h": 0.55, "tone": 1.0},
            {"part": "crest", "at": [0.2, -0.5], "size": 0.7, "h": 0.4, "tone": 1.1},  # ears/plume
            {"part": "leg", "at": [-0.5, 0.4], "size": 0.4, "h": 0.4}, {"part": "leg", "at": [0.5, 0.4], "size": 0.4, "h": 0.4},
        ]
    # default quadruped: feline / canine / mustelid / ape / figure / blob / wisp
    return [
        {"part": "slab", "at": [0, 0.0], "size": 1.0, "h": 0.55, "tone": 1.0},
        {"part": "leg", "at": [-0.6, 0.4], "size": 0.45, "h": 0.5},
        {"part": "leg", "at": [0.55, 0.4], "size": 0.45, "h": 0.5},
        {"part": "wedge", "at": [0.8, -0.35], "size": 0.75, "ang": -0.15, "tone": 1.08},  # head
        {"part": "crest", "at": [0.0, -0.4], "size": 0.55, "h": 0.35, "tone": 1.05},      # mane
        {"part": "tail", "at": [-0.9, -0.1], "size": 0.6, "ang": 0.4, "tone": 0.9},
    ]


_RECIPE_CACHE = {}
def recipe_for(key, family, card=None, rng=None):
    """Resolve a card's model recipe. Priority:
        1. card['model']        — an explicit JSON recipe (validated),
        2. family default       — procedural, deterministic per key,
        + card['model_extra']   — optional appended accent parts.
    Cached by key so it's computed once."""
    if key in _RECIPE_CACHE:
        return _RECIPE_CACHE[key]
    import random as _r
    rng = rng or _r.Random(hash(("model", key)) & 0xffffffff)
    recipe = []
    if card and isinstance(card.get("model"), (list, tuple)):
        recipe = fold_sanitize(card["model"])
    if not recipe:
        recipe = _recipe_for_family(family, rng)
    if card and isinstance(card.get("model_extra"), (list, tuple)):
        recipe = recipe + fold_sanitize(card["model_extra"])
    _RECIPE_CACHE[key] = recipe
    return recipe


def clear_cache():
    _RECIPE_CACHE.clear()


# ───────────────────────────── part drawing ────────────────────────────────
def _pp(x, y, s, f, ax, ay):
    """body-local (ax,ay) → screen point around anchor (x,y) at scale s, facing f."""
    return (x + ax * s * f, y + ay * s)


def draw_part(screen, part, x, y, s, f, base, hgt, tnow, flap, anim=None):
    """Render ONE known-good part. Unknown types are ignored (defensive).

    `anim` (optional) drives per-part motion — a dict with:
        t        : real-time seconds (for cycles),
        move     : 0..1 how fast the unit is walking (drives leg/tail),
        phase    : per-unit phase offset so a crowd doesn't move in lockstep,
        attack   : 0..1 attack lunge progress,
        breathe  : shared breathing bob (small vertical),
    All cosmetic; never touches the sim."""
    prism = _DRAW["prism"]; poly = _DRAW["poly"]; shade = _DRAW["shade"]
    pt = part["part"]
    ax, ay = part["at"]
    ps = max(0.05, part["size"]) * s
    ang = part["ang"]
    tone = part["tone"]
    ph = part["h"] * hgt * 1.6
    col = shade(base, tone)

    # ── per-part animation offsets ──
    dx = dy = 0.0
    if anim:
        t = anim.get("t", tnow)
        move = anim.get("move", 0.0)
        phase = anim.get("phase", 0.0)
        breathe = anim.get("breathe", 0.0)
        # body & shell "breathe" — a subtle vertical bob
        if pt in ("slab", "shell", "panel"):
            dy += breathe * s * 0.05
        # head/beak/horn bob + a little forward jut on attack
        if pt in ("wedge", "tusk"):
            dy += math.sin(t * 3.0 + phase) * s * 0.03
            dx += anim.get("attack", 0.0) * s * 0.25 * f
        # crest/mane/plume flutter
        if pt == "crest":
            dy += math.sin(t * 4.0 + phase) * s * 0.04
        # tail sway — faster when moving
        if pt == "tail":
            sway = math.sin(t * (3.0 + move * 4.0) + phase)
            dx += sway * s * 0.10 * (0.5 + move)
            dy += abs(sway) * s * 0.03
        # legs: a walk cycle — alternate legs lift while the unit moves
        if pt == "leg" and move > 0.02:
            side = -1 if ax < 0 else 1        # left legs vs right legs
            step = math.sin(t * (6.0 + move * 6.0) + phase + (0 if side < 0 else math.pi))
            dy += -max(0, step) * s * 0.12 * move       # lift on the up-beat
            dx += step * s * 0.04 * move * f            # small stride
    cx, cy = x + ax * s * f + dx, y + ay * s + dy

    if pt == "slab":
        body = [(cx - ps * f, cy - ps * 0.28), (cx + ps * 0.9 * f, cy - ps * 0.32),
                (cx + ps * f, cy + ps * 0.34), (cx - ps * 1.05 * f, cy + ps * 0.36)]
        return prism(screen, body, ph, col)
    if pt == "panel":
        quad = [(cx - ps * f, cy - ps * 0.3), (cx + ps * f, cy - ps * 0.32),
                (cx + ps * f, cy + ps * 0.3), (cx - ps * f, cy + ps * 0.3)]
        poly(screen, quad, col, thick=1)
    elif pt == "shell":
        dome = [(cx - ps * f, cy + ps * 0.3), (cx - ps * 0.55 * f, cy - ps * 0.5),
                (cx + ps * 0.55 * f, cy - ps * 0.5), (cx + ps * f, cy + ps * 0.3)]
        return prism(screen, dome, ph, col)
    elif pt == "wedge":
        ca, sa = math.cos(ang), math.sin(ang)
        tip = (cx + (ps * 0.9 * ca) * f, cy - ph - ps * 0.5 + ps * 0.9 * sa)
        tri = [(cx - ps * 0.4 * f, cy - ph + ps * 0.1),
               tip, (cx + ps * 0.5 * f, cy - ph + ps * 0.2)]
        poly(screen, tri, col, thick=2)
    elif pt == "wing":
        # flap animates: `flap` is a signed factor from the caller
        lift = flap * ps * 0.5 * (1 if ang >= 0 else -1)
        tip = (cx + (ps * (0.6 if ang >= 0 else -0.6)) * f, cy - ps - ph - lift)
        w = [(cx, cy - ph * 0.4), tip, (cx + ps * 0.5 * f, cy - ps * 0.1 - ph * 0.4)]
        poly(screen, w, shade(base, tone * 0.96), thick=2)
    elif pt == "spine":
        n = int(part.get("count", 3))
        for i in range(n):
            px = cx + (i - (n - 1) / 2) * ps * 0.5 * f
            poly(screen, [(px, cy - ph), (px + ps * 0.18 * f, cy - ph - ps * 0.5),
                          (px + ps * 0.36 * f, cy - ph)], col, thick=1)
    elif pt == "leg":
        leg = [(cx - ps * 0.18 * f, cy), (cx + ps * 0.18 * f, cy),
               (cx + ps * 0.12 * f, cy + ps * 0.5), (cx - ps * 0.12 * f, cy + ps * 0.5)]
        import pygame
        pygame.draw.polygon(screen, shade(base, tone * 0.6), leg)
    elif pt == "crest":
        crest = [(cx - ps * f, cy + ps * 0.1 - ph), (cx, cy - ps * 0.7 - ph),
                 (cx + ps * f, cy + ps * 0.1 - ph)]
        return prism(screen, crest, ph * 0.7, col)
    elif pt == "tusk":
        import pygame
        ca, sa = math.cos(ang), math.sin(ang)
        for dx in (-0.12, 0.12):
            a = (cx + dx * ps * f, cy - ph)
            bx = a[0] + ps * 0.7 * ca * f
            by = a[1] - ps * 0.5 * (1 - abs(sa)) - ps * 0.3 * sa
            pygame.draw.line(screen, (234, 228, 210), a, (bx, by), max(2, int(ps * 0.12)))
    elif pt == "tail":
        ca, sa = math.cos(ang), math.sin(ang)
        tail = [(cx, cy - ph * 0.4), (cx - ps * ca * f, cy - ph * 0.4 - ps * 0.5 * sa),
                (cx - ps * 0.7 * ca * f, cy + ps * 0.1)]
        poly(screen, tail, col, thick=2)
    elif pt == "orb":
        import pygame
        pygame.draw.circle(screen, shade(base, tone), (int(cx), int(cy - ph)), max(2, int(ps)))
    return None


if __name__ == "__main__":
    # offline self-test: family recipes valid, sanitizer survives garbage
    import random
    rng = random.Random(1)
    fams = ["bird", "insect", "fish", "tree", "monolith", "tusker", "feline", "wisp"]
    for fam in fams:
        rec = _recipe_for_family(fam, rng)
        assert rec and all(p["part"] in PART_TYPES for p in rec), fam
    # sanitizer must clamp / drop bad input
    bad = [None, {}, {"part": "LASER"}, {"part": "wing", "size": 1e9, "at": "x"},
           {"part": "spine", "count": 999}, [1, 2, 3]]
    clean = fold_sanitize(bad)
    assert all(p["part"] in PART_TYPES for p in clean)
    assert all(_LIMITS["size"][0] <= p["size"] <= _LIMITS["size"][1] for p in clean)
    # an explicit JSON model resolves and is validated
    card = {"model": [{"part": "shell", "at": [0, 0], "size": 1.0},
                      {"part": "orb", "at": [0.5, -0.2], "size": 0.15}]}
    r = recipe_for("testcard", "monolith", card, rng)
    assert len(r) == 2 and r[0]["part"] == "shell"
    print(f"✅ origami self-test: {len(fams)} family recipes valid, sanitizer "
          f"survived adversarial input, JSON model recipes resolve")
