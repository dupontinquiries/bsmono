"""
Port of `src/gestures/cardinal.rs::classify`.

Mirrors the classify decision tree:
    1. Tap (total < tap_radius_px AND duration < tap_max_ms AND
            max_excursion < TAP_MAX_EXCURSION_PX)        [hardened]
    2. Rotation (4+ angular regions, monotonic CW/CCW, loop close)
       — modeled here as a stub since it's a rare path
    3. Hairpin (end near start, big max_excursion)
    4. Cardinal (end_dist >= tap_radius_px)
    5. FLICK (max_excursion >= 15, duration < hairpin_max_ms)   [new]
    6. Unrecognized

Constants are mirrored from Rust:
    - TAP_MAX_EXCURSION_PX = 13.0       (cardinal.rs)
    - FLICK_MIN_EXCURSION_PX = 15.0     (cardinal.rs)
    - default tap_radius_px = 20.0
    - lenient tap_radius_px = 26.0
    - default hairpin_min_excursion_px = 60.0
    - lenient hairpin_min_excursion_px = 45.0
    - hairpin_return_tolerance_px = 25.0 (default), 40.0 (lenient)
    - tap_max_ms = 250 (default), 320 (lenient)
    - hairpin_max_ms = 600 (default), 800 (lenient)

When the Rust constants change, update this file. There's an
`assert_constants_match_rust` test in `tests/test_constants.py`
that grep-cross-checks at sim-startup.
"""

from __future__ import annotations
import math
from dataclasses import dataclass
from enum import Enum
from typing import Sequence

from .path import (
    SwipePoint,
    max_excursion_from_start,
    max_excursion_point,
    end_distance,
    total_length,
    direction_deg_from_to,
)


# ---------------------------------------------------------------------------
# Thresholds — mirror CardinalThresholds in cardinal.rs
# ---------------------------------------------------------------------------

@dataclass(frozen=True)
class CardinalThresholds:
    tap_radius_px: float
    tap_max_ms: int
    hairpin_min_excursion_px: float
    hairpin_return_tolerance_px: float
    hairpin_max_ms: int

    @classmethod
    def default(cls) -> "CardinalThresholds":
        # Mirrors `CardinalThresholds::default()` in cardinal.rs.
        return cls(
            tap_radius_px=20.0,
            tap_max_ms=250,
            hairpin_min_excursion_px=60.0,
            hairpin_return_tolerance_px=25.0,
            hairpin_max_ms=600,
        )

    @classmethod
    def lenient(cls) -> "CardinalThresholds":
        # Mirrors `CardinalThresholds::lenient()`. Wider radii,
        # longer time windows — meant for less-precise typists.
        return cls(
            tap_radius_px=26.0,
            tap_max_ms=320,
            hairpin_min_excursion_px=45.0,
            hairpin_return_tolerance_px=40.0,
            hairpin_max_ms=800,
        )


# Per-branch invariants. The cardinal classifier in Rust uses these
# constants directly; this file mirrors them.
TAP_MAX_EXCURSION_PX = 13.0     # cardinal.rs: hardened tap check
FLICK_MIN_EXCURSION_PX = 15.0   # cardinal.rs: FLICK branch threshold


# ---------------------------------------------------------------------------
# Gesture result types
# ---------------------------------------------------------------------------

class Direction(Enum):
    N = "N"
    NE = "NE"
    E = "E"
    SE = "SE"
    S = "S"
    SW = "SW"
    W = "W"
    NW = "NW"

    @classmethod
    def from_angle_deg(cls, angle_deg: float) -> "Direction":
        # Math convention: 0=E, 90=N. Wrap to [-180, 180].
        a = ((angle_deg + 180) % 360) - 180
        # 8-way bucketing at 45° increments.
        if -22.5 <= a < 22.5:
            return cls.E
        if 22.5 <= a < 67.5:
            return cls.NE
        if 67.5 <= a < 112.5:
            return cls.N
        if 112.5 <= a < 157.5:
            return cls.NW
        if -67.5 <= a < -22.5:
            return cls.SE
        if -112.5 <= a < -67.5:
            return cls.S
        if -157.5 <= a < -112.5:
            return cls.SW
        # 157.5..180 or -180..-157.5
        return cls.W


def fold_to_four_way(d: Direction, dx: float, dy_screen: float) -> Direction:
    """Fold 8-way direction to N/E/S/W. Mirrors `fold_to_four_way`
    in cardinal.rs. Uses the dominant axis: if |dx| > |dy|, the
    primary direction is horizontal (E/W); else vertical (N/S).

    Note: dy is in SCREEN coords (positive = down). Math convention
    flips this for the Direction enum.
    """
    if abs(dx) > abs(dy_screen):
        return Direction.E if dx > 0 else Direction.W
    else:
        return Direction.N if dy_screen < 0 else Direction.S


@dataclass(frozen=True)
class CardinalGesture:
    kind: str  # "Tap", "Cardinal", "Hairpin", "Rotation", "Flick", "Unrecognized"
    direction: Direction | None = None
    # Diagnostic fields filled by classify() so the caller can see WHY:
    reason: str = ""

    def __repr__(self) -> str:
        if self.direction:
            return f"{self.kind}({self.direction.value})"
        return self.kind


# ---------------------------------------------------------------------------
# classify — the decision tree
# ---------------------------------------------------------------------------

def classify(
    path: Sequence[SwipePoint],
    thresholds: CardinalThresholds | None = None,
    cardinal_diagonals: bool = False,
) -> CardinalGesture:
    """Port of `cardinal::classify`. Returns the classified gesture
    with a human-readable `reason` field explaining the decision.

    `cardinal_diagonals=True` allows 8-way; False folds diagonal
    flicks to the dominant 4-way axis (matches the Rust default).
    """
    if thresholds is None:
        thresholds = CardinalThresholds.default()

    if len(path) < 2:
        return CardinalGesture("Tap", reason="path has < 2 points")

    start = path[0]
    end = path[-1]
    duration_ms = max(0, end.timestamp_ms - start.timestamp_ms)
    total_len = total_length(path)
    max_excursion = max_excursion_from_start(path)
    end_dist = end_distance(path)

    # 1. Tap check — hardened with max_excursion gate, total dropped.
    #
    # The check used to be (total < tap_radius AND duration < tap_max
    # AND max_excursion < TAP_MAX_EXCURSION_PX). The total < tap_radius
    # gate has been DROPPED: the sim's
    # tap_with_jitter_classified_as_tap scenario showed that real
    # finger jitter accumulates cumulative path length that can
    # exceed tap_radius even when max_excursion is tiny (a 5px-radius
    # random walk over 100ms easily totals 30-40px). Such taps were
    # failing Tap → Cardinal (end_dist tiny) → Flick (max_excursion
    # tiny) → Unrecognized — silent no-op for the user.
    #
    # max_excursion is the strongest signal of "user didn't really
    # move" — if the farthest point is within 13px of start, the
    # finger jittered in place. Total is redundant for the recoil-
    # flick rejection (max_excursion catches that too).
    if (max_excursion < TAP_MAX_EXCURSION_PX
            and duration_ms < thresholds.tap_max_ms):
        return CardinalGesture(
            "Tap",
            reason=f"max_excursion={max_excursion:.1f}<{TAP_MAX_EXCURSION_PX}, "
                   f"dt={duration_ms}<{thresholds.tap_max_ms}",
        )

    # 2. Rotation — STUB. The Rust impl detects 4+ angular regions
    # visited monotonically CW/CCW with loop close. For sim purposes
    # we skip rotation; real-world rotation gestures are rare and
    # don't drive the bugs we're investigating.
    # (If needed, port from cardinal.rs::analyze_rotation later.)

    # 3. Hairpin — end near start, big max_excursion, modest duration.
    if (end_dist < thresholds.hairpin_return_tolerance_px
            and max_excursion >= thresholds.hairpin_min_excursion_px
            and duration_ms < thresholds.hairpin_max_ms):
        return CardinalGesture(
            "Hairpin",
            reason=f"end_dist={end_dist:.1f}<{thresholds.hairpin_return_tolerance_px}, "
                   f"max_excursion={max_excursion:.1f}>={thresholds.hairpin_min_excursion_px}",
        )

    # 4a. Cardinal — end_dist >= tap_radius, take direction from
    # start → end.
    if end_dist >= thresholds.tap_radius_px:
        angle = direction_deg_from_to(start, end)
        d = Direction.from_angle_deg(angle)
        if not cardinal_diagonals:
            d = fold_to_four_way(d, end.x - start.x, end.y - start.y)
        return CardinalGesture(
            "Cardinal", direction=d,
            reason=f"end_dist={end_dist:.1f}>={thresholds.tap_radius_px}, "
                   f"angle={angle:.0f}°",
        )

    # 4b. FLICK — short directional flick with recoil.
    #
    # The new branch added to recover the dead-zone where the user's
    # short directional flicks fell through every other check.
    # Direction comes from the max-excursion point (since end is too
    # close to start to give a reliable angle).
    if (max_excursion >= FLICK_MIN_EXCURSION_PX
            and duration_ms < thresholds.hairpin_max_ms):
        peak = max_excursion_point(path)
        angle = direction_deg_from_to(start, peak)
        d = Direction.from_angle_deg(angle)
        if not cardinal_diagonals:
            d = fold_to_four_way(d, peak.x - start.x, peak.y - start.y)
        return CardinalGesture(
            "Flick", direction=d,
            reason=f"max_excursion={max_excursion:.1f}>={FLICK_MIN_EXCURSION_PX}, "
                   f"dt={duration_ms}<{thresholds.hairpin_max_ms}, "
                   f"angle={angle:.0f}°",
        )

    # 5. Unrecognized
    return CardinalGesture(
        "Unrecognized",
        reason=f"total={total_len:.1f}, end_dist={end_dist:.1f}, "
               f"max_excursion={max_excursion:.1f}, dt={duration_ms}",
    )
