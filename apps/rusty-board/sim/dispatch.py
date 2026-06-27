"""
Port of the `drag_end` decision tree in `src/ui/keyboard_view.rs`.

This is the meat of where bugs hide. The flow:

    1. drag_drawing → character-draw branch (skipped here; canvas
       isn't part of the swipe pipeline)
    2. take_swipe_interrupted_by_focus_loss → abort if focus moved
       AWAY from the field during the swipe (Deactivate-only signal,
       NOT every focus event — see prior bug)
    3. is_significant_swipe(20) gate:
        a. If FALSE + was_evicted + behavior=Cardinal → DEAD-ZONE
           RECOVERY: dispatch_cardinal_swipe anyway, classifier
           decides
        b. If FALSE otherwise → cancel + return
    4. If TRUE: detect_minimize_fling first → if minimize, enter
       tray, return
    5. Otherwise behavior dispatch:
        - None        → no source key, ignore
        - Normal      → dispatch_normal_drag (tap-if-in-source-key)
        - Cardinal    → dispatch_cardinal_swipe
        - AltKeySwipe → alt-character
        - SwipeSource → dispatch_swipe_source (recognizer + auto-space)

This module returns a `DragEndOutcome` describing which branch fired
and what would have happened. The actual commit doesn't happen — we
just want to know the decision.
"""

from __future__ import annotations
from dataclasses import dataclass, field
from enum import Enum
from typing import Sequence

from .path import (
    SwipePoint,
    is_significant_swipe,
    total_length,
)
from .cardinal import classify, CardinalThresholds, CardinalGesture
from .minimize import detect_minimize_fling, MinimizeVerdict


# ---------------------------------------------------------------------------
# Inputs
# ---------------------------------------------------------------------------

class Behavior(Enum):
    Normal = "Normal"
    Cardinal = "Cardinal"
    AltKeySwipe = "AltKeySwipe"
    SwipeSource = "SwipeSource"


@dataclass
class DragEndEnv:
    """Mutable environment state that affects the drag_end decision.
    Set up explicitly per scenario."""
    behavior: Behavior | None = None  # captured at drag_begin
    was_evicted: bool = False         # SWIPE_GESTURE_ACTIVE on drag_end entry
    focus_lost: bool = False          # the deactivate-only abort flag
    is_drawing: bool = False          # drag_drawing flag
    strict_mode: bool = False         # cardinal/stenotype layout?
    swipe_threshold: float = 20.0     # cfg.gestures.swipe_threshold
    cardinal_thresholds: CardinalThresholds = field(
        default_factory=CardinalThresholds.default
    )


# ---------------------------------------------------------------------------
# Outcome
# ---------------------------------------------------------------------------

class Outcome(Enum):
    """Top-level branch of the decision tree that fired."""
    Drawing = "drawing"                   # character-draw branch
    FocusLostAbort = "focus_lost_abort"
    Cancel = "cancel"                     # not significant + no recovery
    DeadZoneRecovery = "dead_zone_recovery"  # Cardinal, sub-threshold
    EnterTray = "enter_tray"              # minimize fling fired
    NoSourceKey = "no_source_key"         # behavior is None
    NormalDrag = "normal_drag"            # Normal behavior
    CardinalDispatch = "cardinal_dispatch"
    AltKeyDispatch = "alt_key_dispatch"
    SwipeSourceDispatch = "swipe_source_dispatch"


@dataclass
class DragEndOutcome:
    outcome: Outcome
    # Diagnostic trace — every step the decision tree took, including
    # the values that drove each branch's decision. Useful with --trace.
    trace: list[str] = field(default_factory=list)
    # If a classifier ran, expose its result.
    cardinal_result: CardinalGesture | None = None
    # If minimize check ran, expose its verdict.
    minimize_verdict: MinimizeVerdict | None = None

    def __repr__(self) -> str:
        bits = [self.outcome.value]
        if self.cardinal_result:
            bits.append(f"classify={self.cardinal_result!r}")
        return f"DragEndOutcome({' '.join(bits)})"


# ---------------------------------------------------------------------------
# The decision tree
# ---------------------------------------------------------------------------

def drag_end(
    path: Sequence[SwipePoint],
    env: DragEndEnv,
) -> DragEndOutcome:
    """Run the full drag_end decision tree on the given path + env.
    Returns a DragEndOutcome with the branch that fired + an optional
    trace explaining each step."""
    trace = []

    # Step 0: character drawing — skip the swipe pipeline entirely.
    if env.is_drawing:
        trace.append("drawing branch: env.is_drawing=True")
        return DragEndOutcome(Outcome.Drawing, trace=trace)
    trace.append("drawing branch: skipped (env.is_drawing=False)")

    # Step 1: focus-loss abort.
    #
    # Set by the deactivate handler when SWIPE_GESTURE_ACTIVE is true.
    # Activate cycles (which Plasma Mobile fires spuriously) do NOT
    # set this — only true focus-AWAY events. The flag is consumed
    # via swap(false) so it's one-shot.
    if env.focus_lost:
        trace.append("focus-loss abort: env.focus_lost=True")
        return DragEndOutcome(Outcome.FocusLostAbort, trace=trace)
    trace.append("focus-loss abort: skipped (env.focus_lost=False)")

    # Step 2: significance gate.
    sig = is_significant_swipe(path, env.swipe_threshold)
    tot = total_length(path)
    trace.append(
        f"significance: total={tot:.1f}, threshold={env.swipe_threshold}, "
        f"len={len(path)}, sig={sig}"
    )
    if not sig:
        # Dead-zone recovery for Cardinal behavior.
        #
        # The path didn't reach `swipe_threshold` (typically 20px
        # total) BUT the user's motion was enough to trigger tap
        # eviction (12px) which means the click handler was
        # suppressed. Without recovery, the gesture is silently
        # dropped. For Cardinal-behavior keys we route through the
        # classifier — its FLICK branch handles exactly this case.
        if env.was_evicted and env.behavior == Behavior.Cardinal:
            trace.append(
                "dead-zone recovery: was_evicted=True, behavior=Cardinal "
                "→ run classifier"
            )
            result = classify(path, env.cardinal_thresholds)
            trace.append(f"  classify result: {result!r} ({result.reason})")
            return DragEndOutcome(
                Outcome.DeadZoneRecovery,
                trace=trace,
                cardinal_result=result,
            )
        trace.append(
            f"cancel: was_evicted={env.was_evicted}, "
            f"behavior={env.behavior.value if env.behavior else None}"
        )
        return DragEndOutcome(Outcome.Cancel, trace=trace)

    # Step 3: minimize-fling check (before behavior dispatch).
    start, end = path[0], path[-1]
    dx = end.x - start.x
    dy = end.y - start.y
    minimize = detect_minimize_fling(path, dx, dy, strict=env.strict_mode)
    trace.append(f"minimize check: {'FIRED' if minimize.detected else 'no'} — {minimize.reason}")
    if minimize.detected:
        return DragEndOutcome(
            Outcome.EnterTray,
            trace=trace,
            minimize_verdict=minimize,
        )

    # Step 4: behavior dispatch.
    if env.behavior is None:
        trace.append("dispatch: behavior=None → ignore")
        return DragEndOutcome(Outcome.NoSourceKey, trace=trace)

    if env.behavior == Behavior.Normal:
        trace.append("dispatch: Normal → dispatch_normal_drag")
        return DragEndOutcome(Outcome.NormalDrag, trace=trace)

    if env.behavior == Behavior.Cardinal:
        result = classify(path, env.cardinal_thresholds)
        trace.append(f"dispatch: Cardinal → classify {result!r} ({result.reason})")
        return DragEndOutcome(
            Outcome.CardinalDispatch,
            trace=trace,
            cardinal_result=result,
        )

    if env.behavior == Behavior.AltKeySwipe:
        trace.append("dispatch: AltKeySwipe")
        return DragEndOutcome(Outcome.AltKeyDispatch, trace=trace)

    if env.behavior == Behavior.SwipeSource:
        trace.append("dispatch: SwipeSource → recognizer")
        return DragEndOutcome(Outcome.SwipeSourceDispatch, trace=trace)

    raise ValueError(f"unhandled behavior: {env.behavior}")
