#!/usr/bin/env python3
"""recipes.py — how each class of alt mode folds.

One fold recipe for every chassis was the honest first pass and it is wrong:
`raptor -> DART` and `bruiser -> CAIRN` were landing in the same shape with
different numbers. A flyer and a hovercraft do not fold alike, and the
difference is not proportion — it is *which limb becomes which part*.

    CAR        legs fold under as suspension, wheels deploy, arms lie back
    FLYER      arms swing OUT to horizontal and BECOME the wing; legs
               extend straight back as the fuselage; feet become tailfins
    HOVERER    legs splay outward and down to ring the base as the skirt;
               hands point down as the lift nozzles; nothing extends
    SHIP       everything compacts along the axis into a dense spindle;
               limbs fold in rather than out, because vacuum has no lift

A recipe returns the VEHICLE pose for a named part. The robot pose never
varies — a bot is a bot — so a recipe only says where the fold lands.

The arms-become-wings fold is the one that carries the most: it is the move
everyone recognises, and it is why a flyer chassis wants long arms while a
car chassis does not care.
"""
import math

HALF = math.pi / 2


def _base(c, part, side, geom):
    """Shared anchors so recipes only override what actually differs."""
    return geom


def car(c, spin, g):
    """Legs under, wheels out. The original recipe."""
    return {
        "chest":    dict(off=(0.0, 0.46 * c.h, -0.16), rot_x=-HALF * 0.92),
        "neck":     dict(off=(0.0, 0.40 * c.h, 0.46), scale=0.6),
        "head":     dict(off=(0.0, 0.30 * c.h, 0.62), rot_x=-HALF, scale=0.55),
        "shoulder": lambda s: dict(off=(s * g["sx"] * 1.06, 0.40 * c.h, -0.52)),
        "upperarm": lambda s: dict(off=(s * g["sx"] * 1.06, 0.40 * c.h, -0.52),
                                   rot_x=HALF, rot_y=s * spin * 0.35),
        "forearm":  lambda s: dict(off=(s * g["sx"] * 1.06, 0.40 * c.h, -0.52 + g["upper"]),
                                   rot_x=HALF, rot_y=s * spin * 0.5),
        "thigh":    lambda s: dict(off=(s * 0.16 * c.bulk, 0.42 * c.h, 0.22),
                                   rot_x=HALF, rot_y=s * spin * 0.2),
        "shin":     lambda s: dict(off=(s * 0.16 * c.bulk, 0.42 * c.h, 0.22 + g["thigh"]),
                                   rot_x=-HALF * 0.88, rot_y=s * spin * 0.3),
        "foot":     lambda s: dict(rot_x=HALF),
        "wheels":   True,
    }


def flyer(c, spin, g):
    """Arms OUT as the wing, legs BACK as the fuselage. No wheels — a wing
    that also has wheels reads as neither."""
    span = g["sx"] + g["upper"] + g["fore"]
    return {
        "chest":    dict(off=(0.0, 0.50 * c.h, -0.10), rot_x=-HALF * 0.96),
        "neck":     dict(off=(0.0, 0.46 * c.h, 0.40), scale=0.55),
        # a flyer's head IS the nose, so it stays proud rather than retracting
        "head":     dict(off=(0.0, 0.44 * c.h, 0.74), rot_x=-HALF * 1.05, scale=0.8),
        "shoulder": lambda s: dict(off=(s * g["sx"], 0.48 * c.h, -0.10)),
        # rot_x=HALF*... points the segment sideways: this is the wing
        "upperarm": lambda s: dict(off=(s * g["sx"], 0.48 * c.h, -0.10),
                                   rot_x=HALF, rot_y=s * HALF * 0.92),
        "forearm":  lambda s: dict(off=(s * (g["sx"] + g["upper"] * 0.92),
                                        0.48 * c.h, -0.10 + 0.06),
                                   rot_x=HALF, rot_y=s * HALF * 0.86),
        # legs straight back, together: the fuselage
        "thigh":    lambda s: dict(off=(s * 0.09 * c.bulk, 0.44 * c.h, 0.06),
                                   rot_x=HALF, rot_y=0.0),
        "shin":     lambda s: dict(off=(s * 0.09 * c.bulk, 0.44 * c.h, 0.06 + g["thigh"]),
                                   rot_x=HALF, rot_y=0.0),
        # feet kick up as tailfins
        "foot":     lambda s: dict(rot_x=-HALF * 0.5, rot_y=s * 0.25),
        "wheels":   False,
        "span_hint": span,
    }


def hoverer(c, spin, g):
    """Legs splay out and DOWN to ring the base — that ring is the plenum.
    Hands point down as the lift nozzles, which is the only part of this
    fold that has to be visible from below."""
    return {
        "chest":    dict(off=(0.0, 0.52 * c.h, -0.04), rot_x=-HALF * 0.80),
        "neck":     dict(off=(0.0, 0.50 * c.h, 0.30), scale=0.6),
        "head":     dict(off=(0.0, 0.46 * c.h, 0.44), rot_x=-HALF * 0.9, scale=0.65),
        "shoulder": lambda s: dict(off=(s * g["sx"] * 1.10, 0.46 * c.h, -0.18)),
        # The first pass splayed these far enough that the limbs left the
        # body and it read as an exploded view. A skirt is a RING AROUND the
        # hull, so every joint stays in contact and only the ANGLE opens.
        "upperarm": lambda s: dict(off=(s * g["sx"] * 0.98, 0.42 * c.h, -0.12),
                                   rot_x=HALF * 0.78, rot_y=s * 0.40),
        "forearm":  lambda s: dict(off=(s * (g["sx"] * 0.98 + g["upper"] * 0.55),
                                        0.34 * c.h, -0.12 + g["upper"] * 0.55),
                                   rot_x=HALF * 0.58, rot_y=s * 0.62),
        # thigh out and down, shin continuing the same arc — together they
        # are the plenum wall
        "thigh":    lambda s: dict(off=(s * 0.15 * c.bulk, 0.36 * c.h, 0.10),
                                   rot_x=HALF * 0.88, rot_y=s * 0.55),
        "shin":     lambda s: dict(off=(s * (0.15 * c.bulk + g["thigh"] * 0.50),
                                        0.30 * c.h, 0.10 + g["thigh"] * 0.52),
                                   rot_x=HALF * 0.62, rot_y=s * 0.78),
        # the foot points DOWN: it is the nozzle
        # the foot points DOWN and sits under the hull: it is the nozzle
        "foot":     lambda s: dict(rot_x=math.pi * 0.86, scale=0.9),
        "wheels":   False,
    }


def ship(c, spin, g):
    """Compact along the axis. In vacuum nothing needs span, so limbs fold IN
    against the hull and the silhouette is a spindle."""
    return {
        "chest":    dict(off=(0.0, 0.44 * c.h, -0.06), rot_x=-HALF),
        "neck":     dict(off=(0.0, 0.44 * c.h, 0.34), scale=0.5),
        "head":     dict(off=(0.0, 0.44 * c.h, 0.52), rot_x=-HALF, scale=0.6),
        "shoulder": lambda s: dict(off=(s * g["sx"] * 0.70, 0.44 * c.h, -0.24)),
        "upperarm": lambda s: dict(off=(s * g["sx"] * 0.70, 0.44 * c.h, -0.24),
                                   rot_x=HALF, rot_y=s * spin * 0.6),
        "forearm":  lambda s: dict(off=(s * g["sx"] * 0.70, 0.44 * c.h,
                                        -0.24 + g["upper"] * 0.9),
                                   rot_x=HALF, rot_y=s * spin * 0.8),
        "thigh":    lambda s: dict(off=(s * 0.10 * c.bulk, 0.44 * c.h, 0.10),
                                   rot_x=HALF, rot_y=s * spin * 0.2),
        "shin":     lambda s: dict(off=(s * 0.10 * c.bulk, 0.44 * c.h,
                                        0.10 + g["thigh"] * 0.95),
                                   rot_x=HALF, rot_y=s * spin * 0.25),
        "foot":     lambda s: dict(rot_x=HALF, scale=0.8),
        "wheels":   False,
    }


BY_CLASS = {"car": car, "flyer": flyer, "hoverer": hoverer, "spaceship": ship}
