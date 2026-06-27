# RustyBoard swipe-pipeline sim harness

A pure-Python re-implementation of the swipe decision tree that lives
in `src/ui/keyboard_view.rs::drag_end`, `src/gestures/cardinal.rs::classify`,
and friends. Used to:

1. **Reproduce bug reports** without needing to deploy to the phone. Each
   user-reported swipe glitch lives in `scenarios.py` as a named function
   that synthesizes the matching path and runs the pipeline.
2. **Explain which branch fired** for an arbitrary path. The `run.py`
   `--trace` flag prints every decision step, so it's obvious whether
   a swipe was rejected by `is_significant_swipe`, eaten by the
   dead-zone recovery, classified as Tap when the user meant a flick,
   etc.
3. **Sweep parameter ranges** to find edge cases. Want to know what
   happens at every 1px of total path length from 10 to 30? Generate
   the paths in a loop and feed them through `dispatch_drag_end`.

This harness **does not** invoke any Rust code. The Rust constants and
decision flow are *mirrored* in Python — drift between the two is the
main maintenance risk. Every constant in `cardinal.py`, `minimize.py`,
and `dispatch.py` has a comment pointing at the source file/line in
the Rust codebase that defines it. When the Rust changes, update the
mirror.

## Layout

- `path.py` — `SwipePoint` dataclass + path generators (clean swipe,
  recoil flick, diagonal-with-drift, etc.) + path measurements
  (`total_length`, `max_excursion_from_start`, `terminal_velocity`)
- `cardinal.py` — port of `gestures::cardinal::classify`. Constants
  for default + lenient thresholds; `FLICK_MIN_EXCURSION_PX = 15`
  for the dead-zone-recovery branch; `TAP_MAX_EXCURSION_PX = 13` for
  the hardened tap check
- `minimize.py` — port of `detect_minimize_fling`. Constants for
  strict (cardinal/stenotype) and non-strict (qwerty) thresholds,
  including the dominance check + the backup path for
  long-dy-regardless-of-dominance
- `dispatch.py` — the `drag_end` decision tree: significant-swipe
  gate, dead-zone recovery for Cardinal-behavior keys, focus-loss
  abort, minimize check, then behavior dispatch
- `scenarios.py` — named scenarios (one per known bug report). Each
  returns a `Scenario` with the path, environment (behavior, strict
  mode, was_evicted, etc.), and the EXPECTED outcome. The runner
  prints actual vs expected and flags discrepancies
- `run.py` — CLI. `python sim/run.py` runs all scenarios; `--scenario
  NAME` runs one; `--trace` prints decision steps; `--sweep PARAM`
  sweeps a parameter range and tabulates the verdicts

## Usage

```
$ python3 sim/run.py
[PASS] steno_short_north_flick: cardinal_dispatch (expected cardinal_dispatch)
[PASS] cardinal_normal_swipe:   cardinal_dispatch (expected cardinal_dispatch)
[FAIL] minimize_from_right_with_drift: swipe_source_dispatch (expected enter_tray)
       reason: dominance check rejected (vy.abs() <= vx.abs() * 1.0)
...
```

When a `[FAIL]` shows up, the reason field tells you exactly which
branch the gesture took. Then you either fix the Rust to match the
expected behavior or update the scenario's expected outcome to match
the new design.

## Why Python and not Rust

The harness exists to support REASONING about the swipe pipeline, not
production behavior. Python is faster to iterate in for a few reasons:

- No build step — edit, run.
- The Rust code mixes GTK4 closures, parking_lot Mutexes, RefCells,
  and atomic flags. Mirroring just the decision logic in pure Python
  strips out all that machinery so the FLOW is visible.
- Bug scenarios are easier to write as data (lists of points + named
  parameters) than as integration tests against GTK.

The harness is small — under 1k lines total. The risk of drift is real
but the alternative (a sim that's also Rust) would be much heavier and
no easier to reason about than the actual codebase.
