"""
Path types + generators for the swipe sim harness.

Mirrors `src/gestures/swipe.rs::SwipePoint` (which is just
`{ x: f64, y: f64, timestamp_ms: i64 }`).

The generators produce paths that approximate what a real touchscreen
delivers — a sequence of (x, y, timestamp_ms) tuples with realistic
sampling rate, jitter, and end-of-gesture recoil.
"""

from __future__ import annotations
import math
import random
from dataclasses import dataclass
from typing import Iterable, Sequence


@dataclass(frozen=True)
class SwipePoint:
    """A single touch sample. x,y in keyboard-frame pixels;
    timestamp_ms is the touchscreen's reported time."""
    x: float
    y: float
    timestamp_ms: int

    def __repr__(self) -> str:
        return f"({self.x:.0f},{self.y:.0f}@{self.timestamp_ms}ms)"


Path = list[SwipePoint]


# -----------------------------------------------------------------------------
# Generators
# -----------------------------------------------------------------------------

def clean_swipe(
    start: tuple[float, float],
    direction_deg: float,
    length_px: float,
    duration_ms: int,
    sample_hz: float = 120.0,
) -> Path:
    """A perfectly straight swipe from `start` in `direction_deg`
    (0 = east, 90 = north, like math conventions; remember the Y axis
    in keyboard-frame is FLIPPED — y increases DOWNWARD — so
    direction_deg=90 means the path moves up the screen, decreasing y)
    over `duration_ms`, sampled at `sample_hz`.

    Useful for the "user did exactly what they meant" baseline.
    """
    n = max(2, int(duration_ms * sample_hz / 1000) + 1)
    rad = math.radians(direction_deg)
    dx = math.cos(rad) * length_px
    dy = -math.sin(rad) * length_px  # screen-y flipped
    sx, sy = start
    points = []
    for i in range(n):
        t = i / (n - 1)
        points.append(SwipePoint(
            x=sx + dx * t,
            y=sy + dy * t,
            timestamp_ms=int(duration_ms * t),
        ))
    return points


def swipe_with_drift(
    start: tuple[float, float],
    primary_dir_deg: float,
    drift_dir_deg: float,
    primary_length: float,
    drift_length: float,
    duration_ms: int,
    sample_hz: float = 120.0,
) -> Path:
    """A swipe whose primary motion is in `primary_dir_deg` over
    `primary_length` px, but ALSO drifts perpendicular-ish by
    `drift_length` px in `drift_dir_deg`. This models the natural
    finger drift on a touchscreen — a right-handed user swiping up
    from the right side will typically drift toward screen center
    (leftward) as the hand pivots.

    Use this to reproduce the "swipe up to minimize only worked on
    the left" bug — set drift_length to a meaningful fraction of
    primary_length to see when the dominance check rejects.
    """
    n = max(2, int(duration_ms * sample_hz / 1000) + 1)
    p_rad = math.radians(primary_dir_deg)
    d_rad = math.radians(drift_dir_deg)
    p_dx = math.cos(p_rad) * primary_length
    p_dy = -math.sin(p_rad) * primary_length
    d_dx = math.cos(d_rad) * drift_length
    d_dy = -math.sin(d_rad) * drift_length
    sx, sy = start
    points = []
    for i in range(n):
        t = i / (n - 1)
        points.append(SwipePoint(
            x=sx + (p_dx + d_dx) * t,
            y=sy + (p_dy + d_dy) * t,
            timestamp_ms=int(duration_ms * t),
        ))
    return points


def recoil_flick(
    start: tuple[float, float],
    peak_dir_deg: float,
    peak_distance_px: float,
    recoil_fraction: float,
    duration_ms: int,
    sample_hz: float = 120.0,
) -> Path:
    """A flick that travels OUT in `peak_dir_deg` to `peak_distance_px`
    then RECOILS partway back toward the start before lifting. The
    `recoil_fraction` is 0.0..1.0 of the peak distance; 0.0 = no
    recoil (just an out-and-stop), 1.0 = ends exactly at start.

    Models a quick directional flick where the user's finger overshoots
    and recoils on release. This is the classic case where the
    classifier needs the FLICK branch (max_excursion >= 15, end near
    start) to recognize the intent.
    """
    n = max(3, int(duration_ms * sample_hz / 1000) + 1)
    rad = math.radians(peak_dir_deg)
    sx, sy = start
    points = []
    # Peak is at the MIDDLE of the duration roughly; recoil happens
    # in the back half.
    peak_t = 0.5
    for i in range(n):
        t = i / (n - 1)
        if t <= peak_t:
            # Out: 0 -> peak_distance over [0, peak_t]
            extent = (t / peak_t) * peak_distance_px
        else:
            # Recoil: peak_distance -> peak_distance * (1 - recoil_frac)
            # over [peak_t, 1]
            recoil_t = (t - peak_t) / (1 - peak_t)
            extent = peak_distance_px * (1.0 - recoil_fraction * recoil_t)
        points.append(SwipePoint(
            x=sx + math.cos(rad) * extent,
            y=sy - math.sin(rad) * extent,  # screen-y flipped
            timestamp_ms=int(duration_ms * t),
        ))
    return points


def jitter_only(
    center: tuple[float, float],
    radius_px: float,
    duration_ms: int,
    seed: int = 0,
    sample_hz: float = 120.0,
) -> Path:
    """User touched and barely moved — finger jitter within a small
    radius. Should classify as a Tap. Used as a regression guard.
    """
    rng = random.Random(seed)
    n = max(2, int(duration_ms * sample_hz / 1000) + 1)
    cx, cy = center
    points = []
    for i in range(n):
        t = i / (n - 1)
        # Random angle, random radius up to `radius_px`.
        a = rng.uniform(0, math.tau)
        r = rng.uniform(0, radius_px)
        points.append(SwipePoint(
            x=cx + math.cos(a) * r,
            y=cy + math.sin(a) * r,
            timestamp_ms=int(duration_ms * t),
        ))
    return points


def two_point(
    start: tuple[float, float],
    end: tuple[float, float],
    duration_ms: int = 50,
) -> Path:
    """The bare minimum: start sample + end sample, no intermediates.
    Used to test what happens with very-low-sample-rate paths (which
    GTK can produce on slow phones)."""
    return [
        SwipePoint(start[0], start[1], 0),
        SwipePoint(end[0], end[1], duration_ms),
    ]


# -----------------------------------------------------------------------------
# Measurements (mirroring Rust impls)
# -----------------------------------------------------------------------------

def total_length(path: Sequence[SwipePoint]) -> float:
    """Sum of segment euclidean distances. Mirrors
    `SwipeTracker::is_significant_swipe`'s `total` calculation in
    `src/gestures/swipe.rs`."""
    total = 0.0
    for a, b in zip(path, path[1:]):
        total += math.hypot(b.x - a.x, b.y - a.y)
    return total


def is_significant_swipe(path: Sequence[SwipePoint], min_distance: float) -> bool:
    """Mirrors `SwipeTracker::is_significant_swipe` exactly:
    returns false if len < 3 OR total < min_distance.
    The `len < 3` gate is important — short paths with only 2
    samples (low-sample-rate phones) get rejected even if their
    displacement is huge."""
    if len(path) < 3:
        return False
    return total_length(path) >= min_distance


def max_excursion_from_start(path: Sequence[SwipePoint]) -> float:
    """Max euclidean distance from path[0] to any point on the path.
    Used by the cardinal classifier's tap, hairpin, and flick checks.
    """
    if not path:
        return 0.0
    sx, sy = path[0].x, path[0].y
    return max(math.hypot(p.x - sx, p.y - sy) for p in path)


def max_excursion_point(path: Sequence[SwipePoint]) -> SwipePoint:
    """The point on the path farthest from path[0]. Used by the FLICK
    branch in cardinal classify to determine the gesture's direction
    when the end of the path is too close to start."""
    if not path:
        raise ValueError("empty path")
    sx, sy = path[0].x, path[0].y
    return max(path, key=lambda p: math.hypot(p.x - sx, p.y - sy))


def terminal_velocity(
    path: Sequence[SwipePoint],
    window_ms: int = 60,
) -> tuple[float, float]:
    """Average velocity (vx, vy) in px/ms over the trailing `window_ms`.
    Mirrors `terminal_velocity` in `src/ui/keyboard_view.rs`. Used by
    `detect_minimize_fling` to distinguish a fling from a settled-out
    swipe."""
    if len(path) < 2:
        return (0.0, 0.0)
    last = path[-1]
    cutoff = max(0, last.timestamp_ms - window_ms)
    # First point in the window. If the whole path is shorter than
    # window_ms, start_idx=0 — we measure the entire path.
    start_idx = next(
        (i for i, p in enumerate(path) if p.timestamp_ms >= cutoff),
        0,
    )
    start = path[start_idx]
    dt = max(1, last.timestamp_ms - start.timestamp_ms)
    return (
        (last.x - start.x) / dt,
        (last.y - start.y) / dt,
    )


def end_distance(path: Sequence[SwipePoint]) -> float:
    """Euclidean distance from start to end of path. Used by the
    cardinal classifier for the Cardinal/Hairpin discrimination."""
    if len(path) < 2:
        return 0.0
    start = path[0]
    end = path[-1]
    return math.hypot(end.x - start.x, end.y - start.y)


def direction_deg_from_to(
    p_from: SwipePoint,
    p_to: SwipePoint,
) -> float:
    """Angle from p_from to p_to in degrees, math convention
    (0 = east, 90 = north). Screen-y is flipped: a path going UP
    on screen has dy<0, which math-convention says is +90°."""
    dx = p_to.x - p_from.x
    dy = p_to.y - p_from.y
    return math.degrees(math.atan2(-dy, dx))  # flip y for math convention
