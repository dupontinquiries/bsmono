"""
Port of `detect_minimize_fling` in `src/ui/keyboard_view.rs`.

The minimize-fling check decides whether an upward swipe is the
"dismiss the keyboard" gesture or a regular swipe. Runs BEFORE the
behavior dispatch in drag_end so it works in all dispatch modes.

Two paths:
    1. Velocity + displacement + dominance — the original tight check
    2. Backup: long unconditional dy + decent terminal velocity — for
       diagonal swipes that fail the dominance check

The backup path was added to fix the "swipe up to minimize only
worked in a certain region" bug. Right-handed users swiping up from
the right side of the keyboard naturally curve toward screen center
(leftward), which can fail the dominance check even when the
upward intent is clear.
"""

from __future__ import annotations
from dataclasses import dataclass
from typing import Sequence

from .path import SwipePoint, terminal_velocity


@dataclass(frozen=True)
class MinimizeVerdict:
    detected: bool
    reason: str  # Which branch fired (or why it didn't)


def detect_minimize_fling(
    path: Sequence[SwipePoint],
    dx: float,
    dy: float,
    strict: bool = False,
) -> MinimizeVerdict:
    """Returns whether the path is a minimize-the-keyboard fling.

    - strict: True for cardinal/stenotype layouts (which need stricter
      thresholds so a cell-swipe doesn't false-positive). False for
      qwerty/dvorak.
    - dx, dy: gross displacement (end - start). Passed in because the
      Rust function takes them as args from the GestureDrag callback,
      where they're already computed.
    """
    # ---- Primary path: velocity + displacement + dominance --------
    #
    # Mirrors the first if-block in detect_minimize_fling. Strict mode
    # has tighter velocity and longer dy, plus higher dominance.
    if strict:
        flick_vel_up = -2.0
        flick_min_dy = -120.0
        flick_dominance = 1.2  # post-fix: was 1.4
    else:
        flick_vel_up = -1.2
        flick_min_dy = -50.0
        flick_dominance = 1.0  # post-fix: was 1.2

    term_vx, term_vy = terminal_velocity(path, 60)

    if (term_vy <= flick_vel_up
            and dy <= flick_min_dy
            and abs(term_vy) > abs(term_vx) * flick_dominance):
        return MinimizeVerdict(
            detected=True,
            reason=f"PRIMARY: vy={term_vy:.2f}<={flick_vel_up}, "
                   f"dy={dy:.0f}<={flick_min_dy}, "
                   f"vy={abs(term_vy):.2f}>{abs(term_vx):.2f}*{flick_dominance}",
        )

    # ---- Backup path: long-dy + decent vy, no dominance check -----
    #
    # Mirrors the second if-block added in the region-fix turn.
    # Catches diagonal flicks where the user clearly meant to dismiss
    # but their hand drifted sideways.
    if strict:
        backup_dy_threshold = -200.0
        backup_vy = -1.5
    else:
        backup_dy_threshold = -150.0
        backup_vy = -1.0

    if dy <= backup_dy_threshold and term_vy <= backup_vy:
        return MinimizeVerdict(
            detected=True,
            reason=f"BACKUP: dy={dy:.0f}<={backup_dy_threshold} "
                   f"AND vy={term_vy:.2f}<={backup_vy}",
        )

    # ---- Rejected — say WHY for diagnostic clarity ----------------
    reasons = []
    if term_vy > flick_vel_up:
        reasons.append(f"vy={term_vy:.2f} not <= {flick_vel_up}")
    if dy > flick_min_dy:
        reasons.append(f"dy={dy:.0f} not <= {flick_min_dy}")
    if abs(term_vy) <= abs(term_vx) * flick_dominance:
        reasons.append(
            f"dominance fail: |vy|={abs(term_vy):.2f} "
            f"<= |vx|={abs(term_vx):.2f} * {flick_dominance}"
        )
    if dy > backup_dy_threshold:
        reasons.append(f"backup-dy={dy:.0f} not <= {backup_dy_threshold}")
    if term_vy > backup_vy:
        reasons.append(f"backup-vy={term_vy:.2f} not <= {backup_vy}")

    return MinimizeVerdict(
        detected=False,
        reason="rejected: " + "; ".join(reasons) if reasons else "rejected",
    )
