"""
Named scenarios for the swipe sim. Each scenario reproduces a known
bug pattern (or a regression-guard for a fixed one) by synthesizing
a path + environment and stating the expected outcome.

Add new ones as bugs come in. The runner reports actual-vs-expected
and flags mismatches.

A scenario is a 0-arg function returning a `Scenario` dataclass.
The runner discovers them by `getattr` over this module's `SCENARIOS`
list.
"""

from __future__ import annotations
from dataclasses import dataclass, field
from typing import Callable, Sequence

from .path import (
    SwipePoint,
    Path,
    clean_swipe,
    swipe_with_drift,
    recoil_flick,
    jitter_only,
    two_point,
)
from .dispatch import DragEndEnv, Behavior, Outcome
from .cardinal import CardinalThresholds, Direction


@dataclass
class Scenario:
    name: str
    description: str
    path: Path
    env: DragEndEnv
    expected_outcome: Outcome
    # Optional finer assertion on the classifier result:
    expected_classify_kind: str | None = None     # "Tap" | "Cardinal" | ...
    expected_direction: Direction | None = None


# ---------------------------------------------------------------------------
# Scenarios
# ---------------------------------------------------------------------------

def cardinal_normal_north_swipe() -> Scenario:
    """A typical cardinal N swipe of ~80px on a steno cell. Should
    classify as Cardinal(N) via the primary `end_dist >= tap_radius`
    branch."""
    return Scenario(
        name="cardinal_normal_north_swipe",
        description="80px north swipe on steno cell; typical chord input",
        path=clean_swipe((100, 200), 90, 80, 180),
        env=DragEndEnv(
            behavior=Behavior.Cardinal,
            was_evicted=True,
            strict_mode=True,
        ),
        expected_outcome=Outcome.CardinalDispatch,
        expected_classify_kind="Cardinal",
        expected_direction=Direction.N,
    )


def steno_short_north_flick_recoil() -> Scenario:
    """The "steno swipes not working" bug: user makes a 15px north
    flick that recoils to ~5px from start. Total path length ~25px
    (cumulative), so significant_swipe(20) passes. But pre-fix, the
    cardinal classifier's Tap branch ate it (total < lenient
    tap_radius=26 AND duration < lenient tap_max=320).
    Post-FLICK + hardened-Tap: classifier returns Flick(N)."""
    return Scenario(
        name="steno_short_north_flick_recoil",
        description="Short N flick with recoil — used to fall through Tap branch",
        path=recoil_flick((150, 250), 90, 15, 0.7, 150),
        env=DragEndEnv(
            behavior=Behavior.Cardinal,
            was_evicted=True,
            strict_mode=True,
            cardinal_thresholds=CardinalThresholds.lenient(),
        ),
        expected_outcome=Outcome.CardinalDispatch,
        expected_classify_kind="Flick",
        expected_direction=Direction.N,
    )


def dead_zone_15px_cardinal() -> Scenario:
    """The dead-zone bug: 15px straight north flick. Total length
    ~15 < swipe_threshold (20), so significant_swipe fails (also
    because len=2 < 3). Pre-dead-zone-recovery, this was silently
    dropped. Post-fix: Cardinal behavior + was_evicted triggers the
    recovery → classifier → Flick(N).

    Note: classifier returns Flick(N), not Cardinal(N), because
    end_dist=15 < tap_radius(20). The FLICK branch catches the
    direction via the max-excursion point. The outer dispatch
    Outcome is DeadZoneRecovery either way."""
    return Scenario(
        name="dead_zone_15px_cardinal",
        description="15px straight N flick — sub-threshold, was silently dropped",
        path=two_point((100, 200), (100, 185), 60),  # 15px north
        env=DragEndEnv(
            behavior=Behavior.Cardinal,
            was_evicted=True,  # 15 > 12px TAP_EVICTION_PX
            strict_mode=False,
        ),
        expected_outcome=Outcome.DeadZoneRecovery,
        expected_classify_kind="Flick",
        expected_direction=Direction.N,
    )


def tap_with_jitter_classified_as_tap() -> Scenario:
    """Regression guard: a tap with cumulative finger jitter must
    classify as Tap even when total cumulative path > tap_radius.

    Touch sensors report jitter at every sample, and a 5px-radius
    random walk over 100ms can accumulate 30-40px of total path
    while max_excursion stays small (3-5px). The fix dropped the
    `total < tap_radius` constraint and relies on max_excursion +
    duration alone. Without this fix, jitter-taps on steno cells
    were silently no-op'ing (Tap rejected → Cardinal rejected →
    Flick rejected → Unrecognized).
    """
    return Scenario(
        name="tap_with_jitter_classified_as_tap",
        description="Cumulative jitter > tap_radius but small max_excursion — must be Tap",
        path=jitter_only((100, 200), 5, 100, seed=42),
        env=DragEndEnv(
            behavior=Behavior.Cardinal,
            was_evicted=True,
            strict_mode=True,
        ),
        # significant_swipe passes (total > 20). Then classifier
        # sees max_excursion small → Tap. CardinalDispatch fires
        # with the Tap result — for steno cells, that commits the
        # primary phoneme.
        expected_outcome=Outcome.CardinalDispatch,
        expected_classify_kind="Tap",
    )


def minimize_clean_vertical_left_side() -> Scenario:
    """Baseline: clean vertical swipe up from the LEFT side of the
    keyboard. Should always trigger minimize."""
    return Scenario(
        name="minimize_clean_vertical_left_side",
        description="Clean 100px N swipe from left region — minimize",
        path=clean_swipe((80, 250), 90, 100, 80),
        env=DragEndEnv(
            behavior=Behavior.SwipeSource,
            was_evicted=True,
            strict_mode=False,
            swipe_threshold=20.0,
        ),
        expected_outcome=Outcome.EnterTray,
    )


def minimize_diagonal_right_side_with_drift() -> Scenario:
    """The "swipe up minimize only worked on left" bug: user swipes
    up from the right side, hand naturally curves leftward toward
    screen center. Primary 200px up, drift 180px west — vy/vx ratio
    is ~1.11, failing the pre-fix 1.2 dominance check.

    Post-fix (lower dominance + backup path): should still trigger
    minimize via either the dominance-fix path (vy/vx=1.11 > 1.0) or
    the backup path (dy=-200 <= -150)."""
    return Scenario(
        name="minimize_diagonal_right_side_with_drift",
        description="Right-side swipe-up with natural leftward drift — pre-fix failed",
        # 200px up + 180px left = 270px diagonal over 110ms
        path=swipe_with_drift(
            (400, 300), 90.0, 180.0, 200, 180, 110,
        ),
        env=DragEndEnv(
            behavior=Behavior.SwipeSource,
            was_evicted=True,
            strict_mode=False,
            swipe_threshold=20.0,
        ),
        expected_outcome=Outcome.EnterTray,
    )


def minimize_strict_short_dy_rejected() -> Scenario:
    """In strict mode (cardinal/stenotype), a swipe with dy=-60 is
    BELOW the strict threshold (-120) and should NOT trigger
    minimize. The cardinal cell-swipe needs to remain a Cardinal
    dispatch, not a tray entry."""
    return Scenario(
        name="minimize_strict_short_dy_rejected",
        description="Strict mode: 60px N swipe must classify as Cardinal, not minimize",
        path=clean_swipe((100, 200), 90, 60, 80),
        env=DragEndEnv(
            behavior=Behavior.Cardinal,
            was_evicted=True,
            strict_mode=True,
            cardinal_thresholds=CardinalThresholds.default(),
        ),
        expected_outcome=Outcome.CardinalDispatch,
        expected_classify_kind="Cardinal",
        expected_direction=Direction.N,
    )


def horizontal_swipe_not_minimize() -> Scenario:
    """A horizontal swipe (dx >> dy) must NOT trigger minimize. Belt-
    and-suspenders test that the dominance check actually works."""
    return Scenario(
        name="horizontal_swipe_not_minimize",
        description="200px east swipe — must not minimize",
        path=clean_swipe((100, 200), 0, 200, 100),
        env=DragEndEnv(
            behavior=Behavior.SwipeSource,
            was_evicted=True,
            strict_mode=False,
        ),
        # SwipeSource dispatch with no minimize.
        expected_outcome=Outcome.SwipeSourceDispatch,
    )


def downward_swipe_not_minimize() -> Scenario:
    """A downward swipe must NOT trigger minimize regardless of speed."""
    return Scenario(
        name="downward_swipe_not_minimize",
        description="200px south swipe — must not minimize",
        path=clean_swipe((100, 100), 270, 200, 100),
        env=DragEndEnv(
            behavior=Behavior.SwipeSource,
            was_evicted=True,
            strict_mode=False,
        ),
        expected_outcome=Outcome.SwipeSourceDispatch,
    )


def focus_lost_mid_swipe_aborts() -> Scenario:
    """If focus was lost mid-swipe (the deactivate-only flag), the
    drag_end MUST abort without dispatching anywhere. This protects
    against swipe-then-focus-changes committing to the wrong field."""
    return Scenario(
        name="focus_lost_mid_swipe_aborts",
        description="Long N swipe with focus-loss flag set → abort",
        path=clean_swipe((100, 250), 90, 100, 100),
        env=DragEndEnv(
            behavior=Behavior.SwipeSource,
            was_evicted=True,
            focus_lost=True,  # this is the deactivate-only signal
            strict_mode=False,
        ),
        expected_outcome=Outcome.FocusLostAbort,
    )


def two_point_swipe_falls_through_significance() -> Scenario:
    """A path with only 2 samples (low-sample-rate touchscreen)
    fails `is_significant_swipe` because of the `len < 3` gate,
    even if the displacement is huge. This is by design — but it
    means dead-zone recovery is the ONLY path for Cardinal keys
    in this case. Test it works."""
    return Scenario(
        name="two_point_swipe_falls_through_significance",
        description="2-point huge displacement — needs dead-zone recovery for Cardinal",
        path=two_point((100, 250), (100, 100), 30),  # 150px N in 30ms
        env=DragEndEnv(
            behavior=Behavior.Cardinal,
            was_evicted=True,
            strict_mode=True,
        ),
        expected_outcome=Outcome.DeadZoneRecovery,
        expected_classify_kind="Cardinal",
        expected_direction=Direction.N,
    )


def tap_eviction_dead_zone_no_recovery_for_normal() -> Scenario:
    """A SwipeSource (qwerty letter) sub-threshold gesture should
    NOT trigger dead-zone recovery — the recovery is Cardinal-only.
    A user who started a tap-then-tiny-drift on a qwerty key should
    see a Cancel, not a SwipeSourceDispatch on a non-significant path."""
    return Scenario(
        name="tap_eviction_dead_zone_no_recovery_for_normal",
        description="SwipeSource sub-threshold gesture must Cancel, not recover",
        path=two_point((100, 200), (100, 185), 60),  # 15px N
        env=DragEndEnv(
            behavior=Behavior.SwipeSource,
            was_evicted=True,
            strict_mode=False,
        ),
        expected_outcome=Outcome.Cancel,
    )


def long_press_with_jitter_classified_as_unrecognized() -> Scenario:
    """A LONG-PRESS with jitter (duration > tap_max_ms) should NOT
    classify as Tap. Long-press is a separate gesture (handled by
    a different controller); the cardinal classifier sees only that
    the user didn't move much and held a long time → Unrecognized.

    Belt-and-suspenders against the Tap-check fix: the duration
    gate is still required."""
    return Scenario(
        name="long_press_with_jitter_classified_as_unrecognized",
        description="Long hold (500ms) with small jitter — NOT a Tap",
        path=jitter_only((100, 200), 4, 500, seed=99),
        env=DragEndEnv(
            behavior=Behavior.Cardinal,
            was_evicted=True,
            strict_mode=True,
        ),
        # significant_swipe might pass on jitter. Then classifier
        # sees max_excursion small but duration too long → falls
        # through Tap → Hairpin → Cardinal → Flick → Unrecognized.
        # CardinalDispatch with Unrecognized result.
        expected_outcome=Outcome.CardinalDispatch,
        expected_classify_kind="Unrecognized",
    )


# ---------------------------------------------------------------------------
# Registry — runner iterates this
# ---------------------------------------------------------------------------

SCENARIOS: list[Callable[[], Scenario]] = [
    cardinal_normal_north_swipe,
    steno_short_north_flick_recoil,
    dead_zone_15px_cardinal,
    tap_with_jitter_classified_as_tap,
    long_press_with_jitter_classified_as_unrecognized,
    minimize_clean_vertical_left_side,
    minimize_diagonal_right_side_with_drift,
    minimize_strict_short_dy_rejected,
    horizontal_swipe_not_minimize,
    downward_swipe_not_minimize,
    focus_lost_mid_swipe_aborts,
    two_point_swipe_falls_through_significance,
    tap_eviction_dead_zone_no_recovery_for_normal,
]
