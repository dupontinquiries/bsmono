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

# ── JSON-defined parts ───────────────────────────────────────────────────────
# Beyond the built-ins above, extra parts can ship as DATA in parts.json beside
# this module (and packs may layer more via load_parts_json). A JSON part is a
# small template the generic renderer folds like any built-in:
#
#   "sail": {
#     "desc":  "a tall dorsal fin",
#     "kind":  "poly",                  # poly | prism | lines | disc
#     "points": [[-0.5, 0.1], [0.0, -1.1], [0.5, 0.1]],   # body-local, ×size
#     "raised": true,                   # sit on top of the body height (y -= h)
#     "pair":  false,                   # draw twice, mirrored about the anchor
#     "thick": 2,                       # outline weight (poly) / line width
#     "tone_mult": 1.0,                 # multiplies the recipe's tone
#     "sway":  0.12,                    # idle sway amplitude (breathe-driven)
#     "flap":  0.0                      # flap coupling (wing-like motion)
#   }
#
# Registered names join PART_TYPES, so fold_sanitize() accepts them and the AI
# Director / pack editor can use them exactly like built-ins.
_JSON_PARTS = {}

def load_parts_json(path=None, merge=True):
    """Load extra part templates from JSON. Missing file is fine (returns 0).
    Bad entries are skipped, never fatal — same tolerance as fold_sanitize."""
    import json, os
    if path is None:
        path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "parts.json")
    try:
        with open(path) as fh:
            doc = json.load(fh)
    except Exception:
        return 0
    n = 0
    for name, spec in (doc.get("parts", doc) or {}).items():
        try:
            name = str(name).lower()
            if not name or name in PART_TYPES and name not in _JSON_PARTS and not merge:
                continue
            kind = spec.get("kind", "poly")
            if kind not in ("poly", "prism", "lines", "disc"):
                continue
            pts = spec.get("points", [])
            if kind in ("poly", "prism", "lines"):
                pts = [(float(p[0]), float(p[1])) for p in pts]
                if kind in ("poly", "prism") and len(pts) < 3:
                    continue
                if kind == "lines" and len(pts) < 2:
                    continue
            _JSON_PARTS[name] = {
                "kind": kind,
                "points": pts,
                "radius": float(spec.get("radius", 0.4)),
                "raised": bool(spec.get("raised", False)),
                "pair": bool(spec.get("pair", False)),
                "thick": int(spec.get("thick", 2)),
                "tone_mult": float(spec.get("tone_mult", 1.0)),
                "sway": float(spec.get("sway", 0.0)),
                "flap": float(spec.get("flap", 0.0)),
            }
            PART_TYPES.add(name)
            n += 1
        except Exception:
            continue
    return n


def json_part_names():
    return sorted(_JSON_PARTS.keys())


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
    if fam in ("monolith", "golem", "shelled"):
        return [
            {"part": "shell", "at": [0, -0.1], "size": 1.1, "h": 0.9, "tone": 1.0},
            {"part": "panel", "at": [0, 0.2], "size": 0.9, "tone": 0.7},
            {"part": "spine", "at": [-0.1, -0.5], "size": 0.5, "count": 4, "tone": 1.1},
        ]
    if fam in ("armored",):
        # pangolin: a low hunched body sheathed in overlapping scale-plates, small
        # pointed head, no big legs — reads as living armour, not a stone block
        return [
            {"part": "shell", "at": [-0.1, -0.05], "size": 1.0, "h": 0.7, "tone": 1.0},   # plated back
            {"part": "panel", "at": [-0.35, 0.05], "size": 0.75, "tone": 0.82},           # under-plate
            {"part": "spine", "at": [-0.15, -0.35], "size": 0.4, "count": 5, "tone": 1.12}, # scale ridge
            {"part": "wedge", "at": [0.85, 0.05], "size": 0.5, "ang": 0.15, "tone": 1.08}, # low pointed head
            {"part": "leg", "at": [0.3, 0.42], "size": 0.28, "h": 0.3},                    # stubby foreleg
            {"part": "tail", "at": [-0.95, 0.15], "size": 0.65, "ang": 0.15, "tone": 0.9}, # thick scaled tail
        ]
    if fam in ("tusker",):
        # heavy build: bulky body with a shoulder hump, four thick legs, a raised
        # head carrying big curved tusks and a short trunk
        return [
            {"part": "slab", "at": [-0.05, 0.0], "size": 1.2, "h": 0.72, "tone": 1.0},    # big body
            {"part": "crest", "at": [-0.3, -0.45], "size": 0.6, "h": 0.4, "tone": 1.05},  # shoulder hump
            {"part": "leg", "at": [-0.7, 0.42], "size": 0.5, "h": 0.55}, {"part": "leg", "at": [-0.35, 0.45], "size": 0.48, "h": 0.55},
            {"part": "leg", "at": [0.4, 0.45], "size": 0.48, "h": 0.55}, {"part": "leg", "at": [0.7, 0.42], "size": 0.5, "h": 0.55},
            {"part": "wedge", "at": [0.85, -0.32], "size": 0.72, "ang": -0.15, "tone": 1.08},  # head raised
            {"part": "tusk", "at": [1.0, -0.05], "size": 0.95, "ang": 0.4},               # tusk lower
            {"part": "tusk", "at": [0.95, 0.1], "size": 0.8, "ang": 0.15, "tone": 0.95},  # trunk/second tusk
        ]
    if fam in ("rabbit",):
        # jackalope: a crouched hopping body, tall upright ears, and antler forks —
        # springy and alert, not a generic quadruped
        return [
            {"part": "slab", "at": [0, 0.05], "size": 0.9, "h": 0.5, "tone": 1.0},        # crouched body
            {"part": "wedge", "at": [0.6, -0.35], "size": 0.5, "ang": -0.2, "tone": 1.08}, # head up
            {"part": "tusk", "at": [0.55, -0.75], "size": 0.55, "ang": 0.45, "tone": 1.05}, # ear L (tall)
            {"part": "tusk", "at": [0.72, -0.72], "size": 0.55, "ang": 0.15, "tone": 1.05}, # ear R
            {"part": "spine", "at": [0.6, -0.85], "size": 0.3, "count": 3, "tone": 1.15},  # antler forks
            {"part": "leg", "at": [-0.35, 0.42], "size": 0.5, "h": 0.55},                  # big hind leg (hop)
            {"part": "leg", "at": [0.45, 0.45], "size": 0.32, "h": 0.35},                  # small foreleg
            {"part": "orb", "at": [-0.85, 0.15], "size": 0.16, "tone": 1.1},               # cotton tail
        ]
    if fam in ("bird_big",):
        return [
            {"part": "slab", "at": [0, 0.0], "size": 0.95, "h": 0.55, "tone": 1.0},
            {"part": "crest", "at": [0.2, -0.5], "size": 0.7, "h": 0.4, "tone": 1.1},
            {"part": "leg", "at": [-0.5, 0.4], "size": 0.4, "h": 0.4}, {"part": "leg", "at": [0.5, 0.4], "size": 0.4, "h": 0.4},
        ]
    if fam in ("ape",):
        # upright, heavy-shouldered: a broad torso, low-slung arms, small head
        return [
            {"part": "slab", "at": [0, -0.1], "size": 1.05, "h": 0.85, "tone": 1.0},   # torso
            {"part": "leg", "at": [-0.7, -0.15], "size": 0.55, "h": 0.7, "tone": 0.9},  # long arm L
            {"part": "leg", "at": [0.7, -0.15], "size": 0.55, "h": 0.7, "tone": 0.9},   # long arm R
            {"part": "leg", "at": [-0.35, 0.45], "size": 0.4, "h": 0.4},                # foot L
            {"part": "leg", "at": [0.35, 0.45], "size": 0.4, "h": 0.4},                 # foot R
            {"part": "wedge", "at": [0, -0.75], "size": 0.55, "ang": 0.0, "tone": 1.1}, # head
        ]
    if fam in ("figure",):
        # a robed humanoid: tall tapering body, hood/crest, two arm folds
        return [
            {"part": "slab", "at": [0, 0.1], "size": 0.7, "h": 1.1, "tone": 1.0},       # robe
            {"part": "crest", "at": [0, -0.75], "size": 0.55, "h": 0.45, "tone": 1.1},  # hood
            {"part": "wing", "at": [-0.35, -0.1], "size": 0.6, "ang": 0.8, "tone": 0.85},  # sleeve L
            {"part": "wing", "at": [0.35, -0.1], "size": 0.6, "ang": -0.8, "tone": 0.85},  # sleeve R
            {"part": "orb", "at": [0, -0.55], "size": 0.1, "tone": 0.4},                # face shadow
        ]
    if fam in ("blob",):
        # amorphous: a low wide lump with a couple of pseudopod bulges, no legs
        return [
            {"part": "shell", "at": [0, 0.1], "size": 1.15, "h": 0.5, "tone": 1.0},
            {"part": "crest", "at": [-0.4, -0.05], "size": 0.5, "h": 0.35, "tone": 0.9},
            {"part": "crest", "at": [0.45, 0.0], "size": 0.45, "h": 0.3, "tone": 1.05},
            {"part": "orb", "at": [0.15, -0.2], "size": 0.12, "tone": 0.35},            # eye
        ]
    if fam in ("wisp",):
        # ethereal: a floating flame-body with trailing wisps, no ground contact
        return [
            {"part": "crest", "at": [0, -0.15], "size": 0.85, "h": 0.7, "tone": 1.0},   # flame body
            {"part": "spine", "at": [0, -0.55], "size": 0.5, "count": 3, "tone": 1.15}, # licks
            {"part": "tail", "at": [-0.3, 0.3], "size": 0.5, "ang": 0.5, "tone": 0.8},  # trail
            {"part": "orb", "at": [0, -0.25], "size": 0.13, "tone": 1.3},               # core glow
        ]
    if fam in ("feline",):
        # sleek cat: low body, upright triangular ears, long curved tail
        return [
            {"part": "slab", "at": [0, 0.0], "size": 1.0, "h": 0.5, "tone": 1.0},
            {"part": "leg", "at": [-0.6, 0.42], "size": 0.42, "h": 0.5},
            {"part": "leg", "at": [0.55, 0.42], "size": 0.42, "h": 0.5},
            {"part": "wedge", "at": [0.82, -0.3], "size": 0.62, "ang": -0.1, "tone": 1.08},   # head
            {"part": "tusk", "at": [0.78, -0.62], "size": 0.32, "ang": 0.5, "tone": 1.05},    # ear L
            {"part": "tusk", "at": [0.95, -0.6], "size": 0.32, "ang": -0.2, "tone": 1.05},    # ear R
            {"part": "tail", "at": [-0.95, -0.25], "size": 0.8, "ang": 0.7, "tone": 0.92},    # long curl tail
        ]
    if fam in ("canine",):
        # dog/wolf: bushy tail up, pointed snout, alert ears
        return [
            {"part": "slab", "at": [0, 0.0], "size": 1.0, "h": 0.55, "tone": 1.0},
            {"part": "leg", "at": [-0.6, 0.42], "size": 0.45, "h": 0.55},
            {"part": "leg", "at": [0.58, 0.42], "size": 0.45, "h": 0.55},
            {"part": "wedge", "at": [0.9, -0.28], "size": 0.72, "ang": -0.2, "tone": 1.08},   # snout
            {"part": "tusk", "at": [0.7, -0.55], "size": 0.3, "ang": 0.3, "tone": 1.05},      # ear
            {"part": "crest", "at": [-0.85, -0.35], "size": 0.7, "h": 0.5, "tone": 0.95},     # bushy tail (up)
        ]
    if fam in ("mustelid",):
        # weasel/badger: long low tube body, short legs, small round ears, stub tail
        return [
            {"part": "slab", "at": [0, 0.05], "size": 1.15, "h": 0.4, "tone": 1.0},           # long low body
            {"part": "leg", "at": [-0.7, 0.35], "size": 0.32, "h": 0.35},
            {"part": "leg", "at": [0.6, 0.35], "size": 0.32, "h": 0.35},
            {"part": "wedge", "at": [0.95, -0.18], "size": 0.55, "ang": -0.1, "tone": 1.1},   # head low
            {"part": "orb", "at": [0.82, -0.42], "size": 0.12, "tone": 0.85},                 # round ear
            {"part": "tail", "at": [-1.0, 0.0], "size": 0.5, "ang": 0.25, "tone": 0.9},       # stub tail
        ]
    # default quadruped: figure / blob / wisp fall through to here only if their
    # specific recipes above didn't catch (kept as a safe generic base)
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


def _crease(screen, center, ps, f, col, vertical=True, cross=False):
    """Draw a subtle fold-crease line across a body part so it reads as folded
    paper rather than a solid block. A lighter highlight beside a darker valley,
    the way a mountain-fold catches light. `cross` adds a fainter perpendicular
    fold for a richer, hand-creased look (used on larger parts). Cheap; cosmetic."""
    import pygame
    shade = _DRAW["shade"]
    cx, cy = center
    hi = shade(col, 1.14); lo = shade(col, 0.82)
    if vertical:
        x = int(cx)
        pygame.draw.line(screen, lo, (x, int(cy - ps * 0.42)), (x, int(cy + ps * 0.42)), 1)
        pygame.draw.line(screen, hi, (x + 1, int(cy - ps * 0.40)), (x + 1, int(cy + ps * 0.40)), 1)
        if cross:                     # a fainter perpendicular valley-fold
            faint = shade(col, 0.90)
            y = int(cy)
            pygame.draw.line(screen, faint, (int(cx - ps * 0.5 * f), y), (int(cx + ps * 0.5 * f), y), 1)
    else:
        y = int(cy)
        pygame.draw.line(screen, lo, (int(cx - ps * 0.7 * f), y), (int(cx + ps * 0.7 * f), y), 1)
        pygame.draw.line(screen, hi, (int(cx - ps * 0.7 * f), y + 1), (int(cx + ps * 0.7 * f), y + 1), 1)
        if cross:
            faint = shade(col, 0.90)
            x = int(cx)
            pygame.draw.line(screen, faint, (x, int(cy - ps * 0.34)), (x, int(cy + ps * 0.34)), 1)


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
        style = anim.get("style", "")
        # ── family motion flavour (applies to the whole body, cheap) ──
        if style == "undulate":       # cephalopods / waves: a travelling S-wave
            dy += math.sin(t * 3.5 + ax * 2.0 + phase) * s * 0.09
            dx += math.cos(t * 3.0 + ay * 2.0 + phase) * s * 0.05 * f
        elif style == "pulse":        # jellies: rhythmic squash/stretch
            dy += (math.sin(t * 3.0 + phase) ** 2 - 0.5) * s * 0.10
        elif style == "flicker":      # wisps: a nervous flame waver
            dx += math.sin(t * 9.0 + ay * 4 + phase) * s * 0.05 * f
            dy += math.sin(t * 7.0 + phase) * s * 0.05
        elif style == "wobble":       # blobs: a slow gelatinous sway
            dx += math.sin(t * 2.2 + phase) * s * 0.06 * f
        elif style == "skitter":      # insects: a fast tiny jitter
            dy += math.sin(t * 16.0 + phase) * s * 0.02
            dx += math.cos(t * 14.0 + phase) * s * 0.015 * f
        elif style == "hop" and move > 0.02:   # amphibians: bob on the move
            dy += -abs(math.sin(t * 5.0 + phase)) * s * 0.10 * move
        elif style == "swim":         # fish: a lateral tail-led glide
            dx += math.sin(t * 4.0 + phase) * s * 0.04 * f
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
        # tail sway — faster when moving; family style adds character
        if pt == "tail":
            rate = 3.0 + move * 4.0
            if style == "undulate": rate += 2.0        # cephalopod: livelier whip
            elif style == "swim": rate += 1.0
            sway = math.sin(t * rate + phase)
            dx += sway * s * 0.10 * (0.5 + move)
            dy += abs(sway) * s * 0.03
        # spine/scale rows ripple for undulating/pulsing bodies (out of phase by
        # their anchor, so a row of scales looks like it's waving down the back)
        if pt == "spine" and style in ("undulate", "pulse", "flicker"):
            dy += math.sin(t * 4.0 + ax * 3.0 + phase) * s * 0.05
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
        r = prism(screen, body, ph, col)
        _crease(screen, (cx, cy - ph), ps, f, col, vertical=True, cross=(ps > 22))
        return r
    if pt == "panel":
        quad = [(cx - ps * f, cy - ps * 0.3), (cx + ps * f, cy - ps * 0.32),
                (cx + ps * f, cy + ps * 0.3), (cx - ps * f, cy + ps * 0.3)]
        poly(screen, quad, col, thick=1)
        _crease(screen, (cx, cy), ps * 0.85, f, col, vertical=False)
    elif pt == "shell":
        dome = [(cx - ps * f, cy + ps * 0.3), (cx - ps * 0.55 * f, cy - ps * 0.5),
                (cx + ps * 0.55 * f, cy - ps * 0.5), (cx + ps * f, cy + ps * 0.3)]
        r = prism(screen, dome, ph, col)
        _crease(screen, (cx, cy - ph), ps, f, col, vertical=True, cross=(ps > 22))
        return r
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
    elif pt in _JSON_PARTS:
        # ── generic renderer for JSON-defined parts ──
        spec = _JSON_PARTS[pt]
        col2 = shade(base, tone * spec["tone_mult"])
        base_y = cy - ph if spec["raised"] else cy
        # idle sway (breathe-driven) and optional flap coupling
        sway = 0.0
        if anim and spec["sway"]:
            sway = anim.get("breathe", 0.0) * spec["sway"]
        lift2 = flap * ps * spec["flap"]
        ca, sa = math.cos(ang + sway), math.sin(ang + sway)
        def _tx(p, mirror):
            lx, ly = p[0] * ps * mirror, p[1] * ps - lift2
            return (cx + (lx * ca - ly * sa) * f, base_y + lx * sa + ly * ca)
        mirrors = (1, -1) if spec["pair"] else (1,)
        for mir in mirrors:
            if spec["kind"] == "disc":
                import pygame
                px, py = _tx((0.0, 0.0), mir)
                pygame.draw.circle(screen, col2, (int(px), int(py)),
                                   max(2, int(spec["radius"] * ps)))
            elif spec["kind"] == "lines":
                import pygame
                pts2 = [_tx(p, mir) for p in spec["points"]]
                pygame.draw.lines(screen, col2, False, pts2,
                                  max(1, int(spec["thick"])))
            elif spec["kind"] == "prism":
                r2 = prism(screen, [_tx(p, mir) for p in spec["points"]], ph, col2)
                if not spec["pair"]:
                    return r2
            else:   # poly
                poly(screen, [_tx(p, mir) for p in spec["points"]], col2,
                     thick=spec["thick"])
    return None


# auto-load the base parts.json beside this module (extra parts ship as data)
load_parts_json()


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
